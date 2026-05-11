use std::sync::{Arc, Mutex};

/// A typed event channel.
///
/// `EventWriter<T>` and `EventReader<T>` share the same internal buffer.
/// Events are held until `drain()` is called, which clears the buffer and
/// returns all queued events since the last drain.
pub struct EventBuffer<T> {
    inner: Arc<Mutex<Vec<T>>>,
}

impl<T> Clone for EventBuffer<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T: Send + 'static> EventBuffer<T> {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Push an event into the buffer.
    pub fn send(&self, event: T) {
        self.inner.lock().unwrap().push(event);
    }

    /// Drain all queued events.  Clears the internal buffer.
    pub fn drain(&self) -> Vec<T> {
        std::mem::take(&mut self.inner.lock().unwrap())
    }

    /// Peek at queued events without consuming them.
    pub fn peek(&self) -> Vec<T>
    where
        T: Clone,
    {
        self.inner.lock().unwrap().clone()
    }
}

impl<T: Send + 'static> Default for EventBuffer<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Write end of an event channel (alias for `EventBuffer`).
pub type EventWriter<T> = EventBuffer<T>;
/// Read end of an event channel (alias for `EventBuffer`).
pub type EventReader<T> = EventBuffer<T>;
