//! How often a visible runtime draws: the one place the editor's viewport
//! frame policy lives.
//!
//! A viewport that draws at the display's rate whatever is happening spends
//! the GPU on frames nobody looks at (a level viewport at 146 Hz held about 70
//! percent of the GPU while the editor sat in the background). The policy
//! below is what Unreal's "use less CPU when in background" and Unity's
//! on-demand editor repaint do, decided once for every runtime the service
//! draws instead of per viewport:
//!
//! | Window | Last frame | Draws |
//! |---|---|---|
//! | minimized, or the tab hidden | | nothing (the runtime sleeps: no frame, no timer) |
//! | not focused | any | at `viewport.unfocused_fps` (default 8; 0 pauses it) |
//! | focused | [`FrameKind::Interactive`] (the user, an edit, content streaming in, the temporal resolve settling) | at the output's pace |
//! | focused | [`FrameKind::Ambient`] (only content that moves on its own: wind, water, particles) | at `viewport.ambient_fps` (default 15; 0 keeps the output's pace: free-run) |
//! | focused | [`FrameKind::Idle`] (nothing changed: the renderer skipped the frame) | a cheap poll for the next change |
//!
//! The renderer says what a frame was ([`FrameKind`]): it already knows, from
//! the scene's revision, the camera, the editor's pending commands and the
//! temporal resolve, whether it has anything new to draw. Every transition
//! wakes the scheduler, so a viewport that gains focus draws on its next
//! pass with its renderer and pipelines as they were: nothing is rebuilt.
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// What the last frame a runtime drew was for (what the renderer's activity
/// detection decided).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// The user or the scene changed something, content is streaming in, or
    /// the temporal resolve is still settling.
    Interactive,
    /// Nothing changed but content that moves on its own.
    Ambient,
    /// Nothing changed: the renderer drew nothing.
    Idle,
}

impl FrameKind {
    pub(super) fn to_bits(self) -> u8 {
        match self {
            Self::Interactive => 0,
            Self::Ambient => 1,
            Self::Idle => 2,
        }
    }

    pub(super) fn from_bits(bits: u8) -> Self {
        match bits {
            1 => Self::Ambient,
            2 => Self::Idle,
            _ => Self::Interactive,
        }
    }
}

/// Where the window showing a runtime's output stands: told by the owner
/// ([`RuntimeHandle::set_presence`](super::RuntimeHandle::set_presence)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Presence {
    /// The window has the operating system's input focus.
    pub focused: bool,
    /// The window is minimized.
    pub minimized: bool,
}

impl Default for Presence {
    fn default() -> Self {
        Self {
            focused: true,
            minimized: false,
        }
    }
}

impl Presence {
    pub(super) fn to_bits(self) -> u8 {
        u8::from(self.focused) | (u8::from(self.minimized) << 1)
    }

    pub(super) fn from_bits(bits: u8) -> Self {
        Self {
            focused: bits & 1 != 0,
            minimized: bits & 2 != 0,
        }
    }
}

/// The settings the policy reads (`editor.viewport`), live.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PacingPolicy {
    /// Frames a second for a visible viewport of a window that is not
    /// focused. 0 pauses it until the window is focused again.
    pub unfocused_fps: f32,
    /// Frames a second when only content that moves on its own changes. 0
    /// keeps the output's pace (the viewport free-runs).
    pub ambient_fps: f32,
}

impl Default for PacingPolicy {
    fn default() -> Self {
        Self {
            unfocused_fps: 8.0,
            ambient_fps: 15.0,
        }
    }
}

/// How often a viewport polls for a change while it has nothing to draw.
const IDLE_POLL: Duration = Duration::from_micros(16_667);

/// What a visible runtime does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    /// Draws nothing until something changes: no frame, no timer.
    Asleep,
    /// Is next due this long after the last frame.
    Every(Duration),
}

impl PacingPolicy {
    /// What a visible runtime does, given its output's own `base` interval,
    /// its window and what its last frame was.
    pub fn pace(&self, base: Duration, presence: Presence, last: FrameKind) -> Pace {
        if presence.minimized {
            return Pace::Asleep;
        }
        let at_most = |fps: f32| {
            if fps <= 0.0 {
                base
            } else {
                base.max(Duration::from_secs_f32(1.0 / fps))
            }
        };
        if !presence.focused {
            return if self.unfocused_fps <= 0.0 {
                Pace::Asleep
            } else {
                Pace::Every(at_most(self.unfocused_fps))
            };
        }
        match last {
            FrameKind::Interactive => Pace::Every(base),
            FrameKind::Ambient => Pace::Every(at_most(self.ambient_fps)),
            FrameKind::Idle => Pace::Every(base.max(IDLE_POLL)),
        }
    }

    /// The policy from the editor's settings.
    pub fn from_settings() -> Self {
        let config = engine_state::settings::global_config();
        let read = |key: &str, default: f32| {
            config
                .get(engine_state::settings::NS_EDITOR, "viewport", key)
                .ok()
                .and_then(|value| value.as_float().ok())
                .map_or(default, |value| value.max(0.0) as f32)
        };
        let defaults = Self::default();
        Self {
            unfocused_fps: read("unfocused_fps", defaults.unfocused_fps),
            ambient_fps: read("ambient_fps", defaults.ambient_fps),
        }
    }
}

/// The policy now, from the settings, read at most four times a second.
pub(super) fn current() -> PacingPolicy {
    static CACHE: Mutex<Option<(Instant, PacingPolicy)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    match *cache {
        Some((at, policy)) if at.elapsed() < Duration::from_millis(250) => policy,
        _ => {
            let policy = PacingPolicy::from_settings();
            *cache = Some((Instant::now(), policy));
            policy
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISPLAY: Duration = Duration::from_micros(6_944); // 144 Hz
    const FOCUSED: Presence = Presence {
        focused: true,
        minimized: false,
    };
    const BACKGROUND: Presence = Presence {
        focused: false,
        minimized: false,
    };
    const MINIMIZED: Presence = Presence {
        focused: false,
        minimized: true,
    };

    fn fps(pace: Pace) -> f32 {
        match pace {
            Pace::Every(interval) => 1.0 / interval.as_secs_f32(),
            Pace::Asleep => 0.0,
        }
    }

    #[test]
    fn a_minimized_window_draws_nothing_whatever_it_shows() {
        let policy = PacingPolicy::default();
        for kind in [FrameKind::Interactive, FrameKind::Ambient, FrameKind::Idle] {
            assert_eq!(policy.pace(DISPLAY, MINIMIZED, kind), Pace::Asleep);
        }
    }

    #[test]
    fn a_visible_viewport_of_an_unfocused_window_draws_at_the_low_rate() {
        let policy = PacingPolicy::default();
        for kind in [FrameKind::Interactive, FrameKind::Ambient, FrameKind::Idle] {
            let rate = fps(policy.pace(DISPLAY, BACKGROUND, kind));
            assert!((rate - 8.0).abs() < 0.1, "{kind:?}: {rate}");
        }
    }

    #[test]
    fn zero_unfocused_fps_pauses_an_unfocused_viewport() {
        let policy = PacingPolicy {
            unfocused_fps: 0.0,
            ..PacingPolicy::default()
        };
        assert_eq!(
            policy.pace(DISPLAY, BACKGROUND, FrameKind::Interactive),
            Pace::Asleep
        );
        // Focused again, it draws at once at the output's pace.
        assert_eq!(
            policy.pace(DISPLAY, FOCUSED, FrameKind::Interactive),
            Pace::Every(DISPLAY)
        );
    }

    #[test]
    fn a_focused_viewport_that_is_being_used_draws_at_the_outputs_pace() {
        assert_eq!(
            PacingPolicy::default().pace(DISPLAY, FOCUSED, FrameKind::Interactive),
            Pace::Every(DISPLAY)
        );
    }

    #[test]
    fn content_that_only_moves_on_its_own_is_capped_unless_free_run_is_asked_for() {
        let policy = PacingPolicy::default();
        let rate = fps(policy.pace(DISPLAY, FOCUSED, FrameKind::Ambient));
        assert!((rate - 15.0).abs() < 0.1, "{rate}");
        let free = PacingPolicy {
            ambient_fps: 0.0,
            ..policy
        };
        assert_eq!(
            free.pace(DISPLAY, FOCUSED, FrameKind::Ambient),
            Pace::Every(DISPLAY)
        );
    }

    #[test]
    fn an_idle_viewport_only_polls_for_a_change() {
        let rate = fps(PacingPolicy::default().pace(DISPLAY, FOCUSED, FrameKind::Idle));
        assert!((rate - 60.0).abs() < 0.5, "{rate}");
    }

    #[test]
    fn a_rate_is_never_faster_than_the_outputs_own() {
        // A 30 Hz display is not driven at the unfocused 8 Hz's complement.
        let slow = Duration::from_millis(100);
        let policy = PacingPolicy {
            unfocused_fps: 60.0,
            ambient_fps: 60.0,
        };
        assert_eq!(
            policy.pace(slow, BACKGROUND, FrameKind::Interactive),
            Pace::Every(slow)
        );
        assert_eq!(
            policy.pace(slow, FOCUSED, FrameKind::Ambient),
            Pace::Every(slow)
        );
    }

    #[test]
    fn presence_and_kind_round_trip_through_their_bits() {
        for presence in [FOCUSED, BACKGROUND, MINIMIZED] {
            assert_eq!(Presence::from_bits(presence.to_bits()), presence);
        }
        for kind in [FrameKind::Interactive, FrameKind::Ambient, FrameKind::Idle] {
            assert_eq!(FrameKind::from_bits(kind.to_bits()), kind);
        }
    }
}
