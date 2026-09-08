use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Backpressure-aware bounded queue for capture callbacks.
/// Capturers never block or wait on disk or UI; if queue is saturated,
/// oldest frames are dropped and the drop counter is incremented.
pub struct BoundedQueue<T> {
    inner: Mutex<VecDeque<T>>,
    capacity: usize,
    dropped_count: AtomicU64,
}

impl<T> BoundedQueue<T> {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "BoundedQueue capacity must be > 0");
        Self {
            inner: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
            dropped_count: AtomicU64::new(0),
        }
    }

    /// Enqueues an item. If full, drops the oldest item and increments drop counter.
    /// Returns true if an item was dropped to make room.
    pub fn push(&self, item: T) -> bool {
        let mut q = self.inner.lock().unwrap();
        let mut dropped = false;
        if q.len() >= self.capacity {
            q.pop_front();
            self.dropped_count.fetch_add(1, Ordering::Relaxed);
            dropped = true;
        }
        q.push_back(item);
        dropped
    }

    /// Dequeues the next item, or None if empty.
    pub fn pop(&self) -> Option<T> {
        let mut q = self.inner.lock().unwrap();
        q.pop_front()
    }

    /// Number of items currently queued.
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Cumulative count of dropped items due to backpressure saturation.
    pub fn dropped_count(&self) -> u64 {
        self.dropped_count.load(Ordering::Relaxed)
    }

    /// Clears the queue and resets drop counter.
    pub fn clear(&self) {
        let mut q = self.inner.lock().unwrap();
        q.clear();
        self.dropped_count.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bounded_queue_drop_behavior() {
        let queue = BoundedQueue::<i32>::new(3);
        assert!(!queue.push(1));
        assert!(!queue.push(2));
        assert!(!queue.push(3));
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.dropped_count(), 0);

        // 4th push should drop the oldest (1)
        assert!(queue.push(4));
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.dropped_count(), 1);

        // Remaining elements should be 2, 3, 4
        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), Some(3));
        assert_eq!(queue.pop(), Some(4));
        assert_eq!(queue.pop(), None);
    }
}
