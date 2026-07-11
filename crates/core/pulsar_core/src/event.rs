use std::sync::{Arc, Mutex};

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

    pub fn send(&self, event: T) {
        self.inner.lock().unwrap().push(event);
    }

    pub fn drain(&self) -> Vec<T> {
        std::mem::take(&mut self.inner.lock().unwrap())
    }

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

pub type EventWriter<T> = EventBuffer<T>;
pub type EventReader<T> = EventBuffer<T>;
