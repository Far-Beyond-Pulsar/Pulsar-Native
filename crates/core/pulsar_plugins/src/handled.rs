//! Startup work that must run once per registration, however late it arrives.
//!
//! A registry's startup (installing an asset kind's loader with SceneDB,
//! binding a component to its texture asset) used to run under a `Once` the
//! first time something needed it. A plugin library's registrations arrive
//! when the library is imported, which can be after that first time, and
//! every copy of a crate linked into a library had a `Once` of its own. A
//! [`Handled`] instead remembers which registrations it has handed out, by
//! identity (origin and id, [`Contribution`]), and hands out only the new
//! ones each time the registrations changed ([`crate::generation`]): the
//! work is idempotent, keyed by identity, and a registration arriving later
//! gets its turn exactly once per copy that asks.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::{all_registered, registered, Registration};

/// The registrations already handed out, by (category, origin, id).
pub struct Handled {
    /// The [`crate::generation`] the last call saw (`u64::MAX`: none yet).
    generation: AtomicU64,
    seen: Mutex<Option<HashSet<(&'static str, &'static str, String)>>>,
}

impl Handled {
    pub const fn new() -> Self {
        Self {
            generation: AtomicU64::new(u64::MAX),
            seen: Mutex::new(None),
        }
    }

    /// The registrations of `T` that count ([`registered`]: their plugin is
    /// enabled) not handed out before.
    pub fn take_new<T: Registration + 'static>(&self) -> Vec<&'static T> {
        self.take(registered::<T>)
    }

    /// As [`Self::take_new`] over every registration, whatever its plugin's
    /// state ([`all_registered`]), for a registry that gates at use.
    pub fn take_new_ungated<T: Registration + 'static>(&self) -> Vec<&'static T> {
        self.take(all_registered::<T>)
    }

    fn take<T, I>(&self, list: impl FnOnce() -> I) -> Vec<&'static T>
    where
        T: Registration + 'static,
        I: Iterator<Item = &'static T>,
    {
        // Read first: a change after this is seen by the next call.
        let generation = crate::generation();
        if self.generation.load(Ordering::Acquire) == generation {
            return Vec::new();
        }
        let mut seen = self.seen.lock().unwrap_or_else(|poison| poison.into_inner());
        let seen = seen.get_or_insert_with(HashSet::new);
        let fresh = list()
            .filter(|registration| {
                seen.insert((T::CATEGORY, registration.origin(), registration.id()))
            })
            .collect();
        self.generation.store(generation, Ordering::Release);
        fresh
    }
}

impl Default for Handled {
    fn default() -> Self {
        Self::new()
    }
}
