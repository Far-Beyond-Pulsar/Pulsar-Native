//! A plugin's editor state: its tool's settings and selection (the sculpt
//! brush, the spline tool's selection, the foliage palette), kept in the
//! level editor's state by type, as a domain of its own.

use std::any::{Any, TypeId};
use std::collections::HashMap;

/// A plugin's editor state. `Default` is its state in a new editor.
pub trait PluginState: Any + Send + Sync {
    /// A copy for another holder of the editor state.
    fn clone_state(&self) -> Box<dyn PluginState>;
    /// The copy read-only tool queries get (the toolbar asking a mode for
    /// its controls). `None` leaves it out: a state holding large history
    /// (an in-progress stroke) is not copied for that.
    fn query_copy(&self) -> Option<Box<dyn PluginState>> {
        Some(self.clone_state())
    }
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
}

/// Implements [`PluginState`] for `Clone` types (copied for tool queries).
#[macro_export]
macro_rules! plugin_state {
    ($($ty:ty),* $(,)?) => {$(
        impl $crate::extend::PluginState for $ty {
            fn clone_state(&self) -> Box<dyn $crate::extend::PluginState> {
                Box::new(self.clone())
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }
            fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
                self
            }
        }
    )*};
}

/// The plugins' editor states, one per type.
#[derive(Default)]
pub struct PluginStates {
    states: HashMap<TypeId, Box<dyn PluginState>>,
}

impl Clone for PluginStates {
    fn clone(&self) -> Self {
        Self {
            states: self
                .states
                .iter()
                .map(|(id, state)| (*id, state.clone_state()))
                .collect(),
        }
    }
}

impl PluginStates {
    /// `T`, if a plugin made one.
    pub fn get<T: PluginState>(&self) -> Option<&T> {
        self.states
            .get(&TypeId::of::<T>())
            .and_then(|state| state.as_any().downcast_ref())
    }

    /// `T`, or its default when none was made yet.
    pub fn get_or_default<T: PluginState + Default + Clone>(&self) -> T {
        self.get::<T>().cloned().unwrap_or_default()
    }

    /// `T`, made from its default the first time.
    pub fn get_mut<T: PluginState + Default>(&mut self) -> &mut T {
        self.states
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(T::default()))
            .as_any_mut()
            .downcast_mut()
            .expect("plugin state keyed by its type")
    }

    /// Replace `T`.
    pub fn set<T: PluginState>(&mut self, state: T) {
        self.states.insert(TypeId::of::<T>(), Box::new(state));
    }

    /// Take `T` out (`None`: none).
    pub fn take<T: PluginState>(&mut self) -> Option<T> {
        let state = self.states.remove(&TypeId::of::<T>())?;
        state.into_any().downcast::<T>().ok().map(|state| *state)
    }

    /// The copies tool queries get ([`PluginState::query_copy`]).
    pub fn clone_for_tool_query(&self) -> Self {
        Self {
            states: self
                .states
                .iter()
                .filter_map(|(id, state)| Some((*id, state.query_copy()?)))
                .collect(),
        }
    }
}
