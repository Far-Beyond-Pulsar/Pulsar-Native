use std::sync::Arc;
use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Shared state container for data accessed by multiple UI components.
///
/// # Policy
/// - Use `Entity<T>` for GPUI-owned UI component state.
/// - Use `SharedState<T>` **only** for backend data shared across multiple UI components.
/// - Never use raw `Arc<Mutex<T>>` or `Arc<RwLock<T>>` directly in UI components.
///
/// # Example
/// ```ignore
/// let metrics: SharedState<PerformanceMetrics> = SharedState::new(PerformanceMetrics::new());
/// let m = metrics.clone(); // cheap Arc clone
/// let cpu = m.with_read(|pm| pm.current_cpu);
/// m.with_write(|pm| pm.update_system_metrics());
/// ```
pub struct SharedState<T> {
    inner: Arc<RwLock<T>>,
}

impl<T> Clone for SharedState<T> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

impl<T> SharedState<T> {
    pub fn new(value: T) -> Self {
        Self { inner: Arc::new(RwLock::new(value)) }
    }

    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        self.inner.read()
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        self.inner.write()
    }

    pub fn with_read<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.inner.read())
    }

    pub fn with_write<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        f(&mut self.inner.write())
    }

    /// Replace the entire contained value.
    pub fn set(&self, value: T) {
        *self.inner.write() = value;
    }
}

impl<T: Default> Default for SharedState<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}
