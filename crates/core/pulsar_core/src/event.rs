use std::sync::{Arc, Mutex, MutexGuard};

/// Lock policy: recover from poisoning. The buffer is a plain queue that a
/// panicking holder cannot leave half-updated, so later users keep working.
fn lock<T>(m: &Mutex<Vec<T>>) -> MutexGuard<'_, Vec<T>> {
    m.lock().unwrap_or_else(|poisoned| {
        tracing::error!("EventBuffer mutex was poisoned by a panic; recovering");
        poisoned.into_inner()
    })
}

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
        lock(&self.inner).push(event);
    }

    pub fn drain(&self) -> Vec<T> {
        std::mem::take(&mut *lock(&self.inner))
    }

    pub fn peek(&self) -> Vec<T>
    where
        T: Clone,
    {
        lock(&self.inner).clone()
    }
}

impl<T: Send + 'static> Default for EventBuffer<T> {
    fn default() -> Self {
        Self::new()
    }
}

pub type EventWriter<T> = EventBuffer<T>;
pub type EventReader<T> = EventBuffer<T>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_survives_a_panic_while_the_lock_is_held() {
        let buffer = EventBuffer::<u32>::new();
        buffer.send(1);
        let poisoner = buffer.clone();
        let result = std::thread::spawn(move || {
            let _guard = poisoner.inner.lock().unwrap();
            panic!("poison the buffer");
        })
        .join();
        assert!(result.is_err());
        assert!(buffer.inner.is_poisoned());

        buffer.send(2);
        assert_eq!(buffer.peek(), vec![1, 2]);
        assert_eq!(buffer.drain(), vec![1, 2]);
        assert!(buffer.drain().is_empty());
    }
}
