//! Latent primitives beyond `Wait { seconds }`: wait for frames, until a
//! condition holds or until an event; timers that can be cancelled or
//! retriggered.
//!
//! None of these is an instruction. The VM has one way to suspend a call
//! (the `Wait` instruction, which parks the frames in a
//! [`Continuation`](crate::Continuation)) and one way a native can ask for
//! it: write a [`Wake`] into the [`Latent`] state the host attached, which
//! makes the VM suspend the call right after the native returns. What the
//! call is waiting *for*, and when it resumes, is the runtime's business:
//! it owns the clock, the frame counter, the instance and its events. A
//! host that attaches no `Latent` (the exported-Rust actors, tests) makes
//! these natives fail the call with a clear error instead.
//!
//! Scheduled calls ("timers") are per instance and live here, as plain data the runtime
//! advances ([`Latent::advance`]); a timer that fires names an exported
//! function of its class and the runtime calls it, so a timer callback can
//! itself wait, set timers, and so on.
//!
//! They are distinct from the event hub's `timer::set` (see `pulsar_game`),
//! which publishes a `TimerFired` event to subscribers: `schedule::call`
//! names the function to run and needs no handler or subscription.

use crate::error::ScriptError;
use crate::native::{Host, NativeFn, NativeRegistry};

/// What a suspended call is waiting for. Written by a latent native; the
/// runtime parks the call under it and resumes it when it is satisfied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wake {
    /// This many further ticks (at least one: a call never resumes on the
    /// tick it suspended in).
    Frames(u32),
    /// Until the instance's exported, parameterless `bool` function
    /// `predicate` returns true, tested once per tick before `tick` runs.
    Until { predicate: String },
    /// Until an event with this name is delivered to the instance.
    Event { name: String },
}

/// A timer: call `function` after `remaining` seconds, then every
/// `repeat` seconds if that is set.
#[derive(Clone, Debug, PartialEq)]
pub struct Timer {
    pub handle: i64,
    /// Retriggerable timers are named; setting the same key again restarts
    /// the countdown instead of adding a timer.
    pub key: Option<String>,
    pub function: String,
    pub remaining: f64,
    pub repeat: Option<f64>,
}

/// The most timers one instance may have pending. A script that sets one
/// per frame without ever clearing them hits this instead of growing
/// without bound.
pub const MAX_TIMERS: usize = 1024;

/// The latent state of one script instance: the pending wake request of
/// the call that is running, and the instance's timers. Owned by the
/// runtime and attached to the [`Host`] of each call.
#[derive(Debug, Default)]
pub struct Latent {
    suspend: Option<Wake>,
    timers: Vec<Timer>,
    next_handle: i64,
}

impl Latent {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask for the running call to suspend until `wake`. The VM suspends it
    /// when the native returns; a native can ask once per call.
    pub fn request(&mut self, wake: Wake) {
        self.suspend = Some(wake);
    }

    /// Whether a native asked the running call to suspend.
    pub fn suspend_requested(&self) -> bool {
        self.suspend.is_some()
    }

    /// The request a suspended call left, if it suspended for one (`None`
    /// for a plain `Wait`).
    pub fn take_request(&mut self) -> Option<Wake> {
        self.suspend.take()
    }

    pub fn timers(&self) -> &[Timer] {
        &self.timers
    }

    fn allocate(&mut self) -> Result<i64, ScriptError> {
        if self.timers.len() >= MAX_TIMERS {
            return Err(ScriptError::native(format!("too many pending timers (the limit is {MAX_TIMERS})")));
        }
        self.next_handle += 1;
        Ok(self.next_handle)
    }

    /// Add a timer. `seconds` and `repeat` are clamped to be non-negative;
    /// a repeat of zero would fire every tick, so it is at least a
    /// millisecond. Returns its handle.
    pub fn set(&mut self, function: &str, seconds: f64, repeat: Option<f64>) -> Result<i64, ScriptError> {
        let handle = self.allocate()?;
        self.timers.push(Timer {
            handle,
            key: None,
            function: function.to_owned(),
            remaining: finite(seconds),
            repeat: repeat.map(|r| finite(r).max(0.001)),
        });
        Ok(handle)
    }

    /// Start, or restart, the timer named `key`: the countdown begins again
    /// from `seconds` and the function is the new one. Returns its handle
    /// (the same each time).
    pub fn restart(&mut self, key: &str, function: &str, seconds: f64) -> Result<i64, ScriptError> {
        if let Some(timer) = self.timers.iter_mut().find(|t| t.key.as_deref() == Some(key)) {
            timer.function = function.to_owned();
            timer.remaining = finite(seconds);
            timer.repeat = None;
            return Ok(timer.handle);
        }
        let handle = self.allocate()?;
        self.timers.push(Timer {
            handle,
            key: Some(key.to_owned()),
            function: function.to_owned(),
            remaining: finite(seconds),
            repeat: None,
        });
        Ok(handle)
    }

    /// Cancel a timer. `false` if it had already fired (one-shot) or been
    /// cleared.
    pub fn clear(&mut self, handle: i64) -> bool {
        let before = self.timers.len();
        self.timers.retain(|t| t.handle != handle);
        self.timers.len() != before
    }

    /// Cancel the timer named `key`.
    pub fn clear_key(&mut self, key: &str) -> bool {
        let before = self.timers.len();
        self.timers.retain(|t| t.key.as_deref() != Some(key));
        self.timers.len() != before
    }

    pub fn is_pending(&self, handle: i64) -> bool {
        self.timers.iter().any(|t| t.handle == handle)
    }

    /// Seconds until a timer fires.
    pub fn remaining(&self, handle: i64) -> Option<f64> {
        self.timers.iter().find(|t| t.handle == handle).map(|t| t.remaining)
    }

    /// Advance every timer by `delta` seconds and return the functions that
    /// are due, in the order they were due (ties: the order set). A
    /// repeating timer fires once per elapsed interval, up to a cap, so a
    /// long hitch cannot queue unbounded calls; one-shot timers are
    /// removed.
    pub fn advance(&mut self, delta: f64) -> Vec<Fired> {
        /// A repeating timer fires at most this often per tick.
        const MAX_CATCH_UP: u32 = 8;
        let delta = finite(delta);
        let mut fired = Vec::new();
        for timer in &mut self.timers {
            timer.remaining -= delta;
            if timer.remaining > 0.0 {
                continue;
            }
            match timer.repeat {
                Some(interval) => {
                    let mut due_at = timer.remaining;
                    let mut count = 0;
                    while due_at <= 0.0 && count < MAX_CATCH_UP {
                        fired.push(Fired { handle: timer.handle, function: timer.function.clone(), overdue: -due_at });
                        due_at += interval;
                        count += 1;
                    }
                    // Past the cap, skip the rest rather than owe them.
                    timer.remaining = if due_at <= 0.0 { interval } else { due_at };
                }
                None => {
                    fired.push(Fired { handle: timer.handle, function: timer.function.clone(), overdue: -timer.remaining });
                    timer.remaining = f64::NEG_INFINITY;
                }
            }
        }
        self.timers.retain(|t| t.remaining != f64::NEG_INFINITY);
        fired.sort_by(|a, b| b.overdue.total_cmp(&a.overdue));
        fired
    }

    /// Drop every timer whose function `exists` says is gone (after a class
    /// reload). Returns the dropped timers' functions.
    pub fn retain_functions(&mut self, mut exists: impl FnMut(&str) -> bool) -> Vec<String> {
        let mut dropped = Vec::new();
        self.timers.retain(|t| {
            let keep = exists(&t.function);
            if !keep {
                dropped.push(t.function.clone());
            }
            keep
        });
        dropped
    }
}

/// A timer that came due.
#[derive(Clone, Debug, PartialEq)]
pub struct Fired {
    pub handle: i64,
    pub function: String,
    /// How long ago, in seconds, it was due.
    pub overdue: f64,
}

fn finite(seconds: f64) -> f64 {
    if seconds.is_finite() && seconds > 0.0 {
        seconds
    } else {
        0.0
    }
}

fn latent<'a, 'h>(host: &'a mut Host<'h>, what: &str) -> Result<&'a mut Latent, ScriptError> {
    host.latent
        .as_deref_mut()
        .ok_or_else(|| ScriptError::native(format!("{what} is not available here: this host has no latent actions")))
}

pub(crate) fn register(registry: &mut NativeRegistry) {
    let mut add = |native: NativeFn| {
        if let Err(err) = registry.register(native) {
            tracing::error!("script latent natives: {err}");
        }
    };

    add(NativeFn::builder("wait::frames")
        .doc("Suspend this call for `count` ticks (at least one).")
        .attr("category", "Flow")
        .attr("access", "read")
        .params(["count"])
        .build(|host: &mut Host<'_>, count: i64| -> Result<(), ScriptError> {
            let frames = u32::try_from(count.clamp(1, i64::from(u32::MAX))).unwrap_or(1);
            latent(host, "wait::frames")?.request(Wake::Frames(frames));
            Ok(())
        }));
    add(NativeFn::builder("wait::next_tick")
        .doc("Suspend this call until the next tick.")
        .attr("category", "Flow")
        .attr("access", "read")
        .build(|host: &mut Host<'_>| -> Result<(), ScriptError> {
            latent(host, "wait::next_tick")?.request(Wake::Frames(1));
            Ok(())
        }));
    add(NativeFn::builder("wait::until")
        .doc(
            "Suspend this call until the exported function `predicate` (no parameters, returns bool) \
             returns true. It is tested once per tick, before `tick` runs, and should have no side effects.",
        )
        .attr("category", "Flow")
        .attr("access", "read")
        .params(["predicate"])
        .build(|host: &mut Host<'_>, predicate: String| -> Result<(), ScriptError> {
            latent(host, "wait::until")?.request(Wake::Until { predicate });
            Ok(())
        }));
    add(NativeFn::builder("wait::event")
        .doc("Suspend this call until an event with this name is delivered to this instance.")
        .attr("category", "Flow")
        .attr("access", "read")
        .params(["name"])
        .build(|host: &mut Host<'_>, name: String| -> Result<(), ScriptError> {
            latent(host, "wait::event")?.request(Wake::Event { name });
            Ok(())
        }));

    add(NativeFn::builder("schedule::call")
        .doc("Call the exported function `function` once after `seconds`. Returns a handle for `schedule::clear`.")
        .attr("category", "Schedule")
        .attr("access", "read")
        .params(["function", "seconds"])
        .build(|host: &mut Host<'_>, function: String, seconds: f64| -> Result<i64, ScriptError> {
            latent(host, "schedule::call")?.set(&function, seconds, None)
        }));
    add(NativeFn::builder("schedule::repeat")
        .doc("Call the exported function `function` every `interval` seconds, the first time after `interval`.")
        .attr("category", "Schedule")
        .attr("access", "read")
        .params(["function", "interval"])
        .build(|host: &mut Host<'_>, function: String, interval: f64| -> Result<i64, ScriptError> {
            latent(host, "schedule::repeat")?.set(&function, interval, Some(interval))
        }));
    add(NativeFn::builder("schedule::restart")
        .doc(
            "A retriggerable delay: call `function` `seconds` after the *last* call with this key. \
             Calling it again before then restarts the countdown instead of adding a timer.",
        )
        .attr("category", "Schedule")
        .attr("access", "read")
        .params(["key", "function", "seconds"])
        .build(|host: &mut Host<'_>, key: String, function: String, seconds: f64| -> Result<i64, ScriptError> {
            latent(host, "schedule::restart")?.restart(&key, &function, seconds)
        }));
    add(NativeFn::builder("schedule::clear")
        .doc("Cancel a timer by handle. False if it had already fired or been cleared.")
        .attr("category", "Schedule")
        .attr("access", "read")
        .params(["handle"])
        .build(|host: &mut Host<'_>, handle: i64| -> Result<bool, ScriptError> {
            Ok(latent(host, "schedule::clear")?.clear(handle))
        }));
    add(NativeFn::builder("schedule::clear_key")
        .doc("Cancel the retriggerable timer with this key. False if there is none.")
        .attr("category", "Schedule")
        .attr("access", "read")
        .params(["key"])
        .build(|host: &mut Host<'_>, key: String| -> Result<bool, ScriptError> {
            Ok(latent(host, "schedule::clear_key")?.clear_key(&key))
        }));
    add(NativeFn::builder("schedule::pending")
        .doc("Whether the timer is still waiting to fire.")
        .attr("category", "Schedule")
        .attr("access", "read")
        .params(["handle"])
        .build(|host: &mut Host<'_>, handle: i64| -> Result<bool, ScriptError> {
            Ok(latent(host, "schedule::pending")?.is_pending(handle))
        }));
    add(NativeFn::builder("schedule::remaining")
        .doc("Seconds until the timer fires; -1 if it is not pending.")
        .attr("category", "Schedule")
        .attr("access", "read")
        .params(["handle"])
        .build(|host: &mut Host<'_>, handle: i64| -> Result<f64, ScriptError> {
            Ok(latent(host, "schedule::remaining")?.remaining(handle).unwrap_or(-1.0))
        }));
}
