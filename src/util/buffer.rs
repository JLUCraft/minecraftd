use std::collections::VecDeque;

/// A circular buffer for storing output lines/bytes with a fixed capacity.
///
/// When the buffer is full, new items overwrite the oldest ones.
/// This is inspired by `MCSManager`'s `CircularBuffer` used for terminal output.
#[derive(Debug, Clone)]
pub struct CircularBuffer<T> {
    buffer: VecDeque<T>,
    capacity: usize,
    overflow_count: usize,
}

impl<T> CircularBuffer<T> {
    /// Creates a new circular buffer with the given capacity.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(capacity),
            capacity,
            overflow_count: 0,
        }
    }

    /// Pushes an item into the buffer.
    ///
    /// If the buffer is at capacity, the oldest item is removed.
    pub fn push(&mut self, item: T) {
        if self.buffer.len() >= self.capacity {
            self.buffer.pop_front();
            self.overflow_count += 1;
        }
        self.buffer.push_back(item);
    }

    /// Returns all items in the buffer without removing them.
    #[must_use]
    pub const fn items(&self) -> &VecDeque<T> {
        &self.buffer
    }

    /// Drains all items from the buffer.
    pub fn drain(&mut self) -> Vec<T> {
        let items: Vec<T> = self.buffer.drain(..).collect();
        items
    }

    /// Returns the number of items in the buffer.
    #[must_use]
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Returns true if the buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Clears the buffer.
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.overflow_count = 0;
    }

    /// Returns the buffer capacity.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the number of items that were dropped due to overflow.
    #[must_use]
    pub const fn overflow_count(&self) -> usize {
        self.overflow_count
    }

    /// Returns true if items have been dropped due to overflow.
    #[must_use]
    pub const fn was_overflowed(&self) -> bool {
        self.overflow_count > 0
    }
}

impl CircularBuffer<u8> {
    /// Pushes a byte slice into the buffer.
    pub fn push_bytes(&mut self, data: &[u8]) {
        for &byte in data {
            self.push(byte);
        }
    }

    /// Returns the buffer contents as a string, if valid UTF-8.
    #[must_use]
    pub fn as_string(&self) -> Option<String> {
        String::from_utf8(self.buffer.iter().copied().collect()).ok()
    }
}

impl CircularBuffer<String> {
    /// Joins all lines into a single string.
    #[must_use]
    pub fn join(&self, separator: &str) -> String {
        self.buffer
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(separator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circular_buffer_basic() {
        let mut buf = CircularBuffer::new(3);
        buf.push(1);
        buf.push(2);
        buf.push(3);
        assert_eq!(buf.len(), 3);
        assert!(!buf.was_overflowed());

        buf.push(4);
        assert_eq!(buf.len(), 3);
        assert!(buf.was_overflowed());
        assert_eq!(buf.overflow_count(), 1);

        let items: Vec<i32> = buf.drain();
        assert_eq!(items, vec![2, 3, 4]);
    }

    #[test]
    fn test_circular_buffer_bytes() {
        let mut buf = CircularBuffer::new(10);
        buf.push_bytes(b"hello");
        assert_eq!(buf.as_string(), Some("hello".to_string()));
    }

    #[test]
    fn test_circular_buffer_lines() {
        let mut buf = CircularBuffer::new(5);
        buf.push("line1".to_string());
        buf.push("line2".to_string());
        assert_eq!(buf.join("\n"), "line1\nline2");
    }

    #[test]
    fn test_circular_buffer_clear_and_is_empty() {
        let mut buf = CircularBuffer::new(3);
        assert!(buf.is_empty());
        buf.push(1);
        assert!(!buf.is_empty());
        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.overflow_count(), 0);
    }

    #[test]
    fn test_circular_buffer_capacity_and_len() {
        let mut buf = CircularBuffer::new(3);
        assert_eq!(buf.capacity(), 3);
        buf.push(1);
        buf.push(2);
        assert_eq!(buf.len(), 2);
    }

    #[test]
    fn test_circular_buffer_items() {
        let mut buf = CircularBuffer::new(3);
        buf.push(1);
        buf.push(2);
        assert_eq!(buf.items().len(), 2);
    }

    #[test]
    fn test_circular_buffer_bytes_invalid_utf8() {
        let mut buf = CircularBuffer::new(10);
        buf.push_bytes(&[0x80, 0x81, 0x82]);
        assert!(buf.as_string().is_none());
    }
}
