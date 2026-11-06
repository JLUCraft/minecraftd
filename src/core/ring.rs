use std::collections::VecDeque;
use std::sync::Arc;

/// A thread-safe ring buffer that captures output lines from a process.
///
/// Stores up to `capacity` lines of output. When full, oldest lines are dropped.
/// Inspired by `MCSManager`'s terminal output buffer and SJMCL's log capture.
#[derive(Debug, Clone)]
pub struct OutputRingBuffer {
    inner: Arc<parking_lot::Mutex<OutputRingState>>,
    capacity: usize,
}

#[derive(Debug)]
struct OutputRingState {
    lines: VecDeque<String>,
    written: u64,
}

impl OutputRingBuffer {
    /// Creates a new ring buffer with the given capacity.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(parking_lot::Mutex::new(OutputRingState {
                lines: VecDeque::with_capacity(capacity),
                written: 0,
            })),
            capacity,
        }
    }

    /// Push a raw byte slice into the buffer, splitting on newlines.
    pub fn push_bytes(&self, data: &[u8]) {
        if let Ok(text) = std::str::from_utf8(data) {
            self.push_str(text);
        }
    }

    /// Push a string into the buffer, splitting on newlines.
    pub fn push_str(&self, text: &str) {
        let mut guard = self.inner.lock();
        for line in text.lines() {
            guard.written += 1;
            if self.capacity == 0 {
                continue;
            }
            if guard.lines.len() >= self.capacity {
                guard.lines.pop_front();
            }
            guard.lines.push_back(line.to_string());
        }
    }

    /// Returns the most recent `n` lines of output.
    #[must_use]
    pub fn recent_lines(&self, n: usize) -> Vec<String> {
        self.recent_with_offset(n).1
    }

    /// Returns the zero-based global index of the first returned line and up to `n` lines.
    /// An empty window starts at the next write index. Clearing does not reset the index.
    #[must_use]
    pub fn recent_with_offset(&self, n: usize) -> (u64, Vec<String>) {
        let guard = self.inner.lock();
        let count = n.min(guard.lines.len());
        let start = guard.written - count as u64;
        let lines = guard
            .lines
            .iter()
            .skip(guard.lines.len() - count)
            .cloned()
            .collect();
        drop(guard);
        (start, lines)
    }

    /// Returns all lines in the buffer.
    #[must_use]
    pub fn all_lines(&self) -> Vec<String> {
        let guard = self.inner.lock();
        guard.lines.iter().cloned().collect()
    }

    /// Returns the number of lines stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().lines.len()
    }

    /// Returns true if the buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clears all lines.
    pub fn clear(&self) {
        self.inner.lock().lines.clear();
    }

    /// Returns the capacity.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Default for OutputRingBuffer {
    fn default() -> Self {
        Self::new(1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_with_offset_tracks_window_rollover() {
        const CAPACITY: usize = 3;
        let ring = OutputRingBuffer::new(CAPACITY);
        assert_eq!(ring.recent_with_offset(CAPACITY), (0, vec![]));
        ring.push_str("a\nb\n");
        assert_eq!(
            ring.recent_with_offset(CAPACITY),
            (0, vec!["a".into(), "b".into()])
        );
        assert_eq!(ring.recent_with_offset(1), (1, vec!["b".into()]));
        ring.push_str("c\n");
        assert_eq!(
            ring.recent_with_offset(CAPACITY),
            (0, vec!["a".into(), "b".into(), "c".into()])
        );
        ring.push_str("d\ne\n");
        assert_eq!(
            ring.recent_with_offset(CAPACITY),
            (2, vec!["c".into(), "d".into(), "e".into()])
        );
        ring.push_str("f\n");
        assert_eq!(
            ring.recent_with_offset(CAPACITY),
            (3, vec!["d".into(), "e".into(), "f".into()])
        );
        assert_eq!(ring.recent_with_offset(0), (6, vec![]));
        assert_eq!(
            ring.recent_lines(CAPACITY),
            ring.recent_with_offset(CAPACITY).1
        );
    }

    #[test]
    fn recent_with_offset_preserves_counter_after_clear() {
        let ring = OutputRingBuffer::new(2);
        ring.push_str("a\nb\nc\n");
        ring.clear();
        assert_eq!(ring.recent_with_offset(2), (3, vec![]));
        ring.push_str("d\n");
        assert_eq!(ring.recent_with_offset(2), (3, vec!["d".into()]));
    }

    #[test]
    fn recent_with_offset_counts_writes_without_capacity() {
        let ring = OutputRingBuffer::new(0);
        ring.push_bytes(b"a\nb\n");
        ring.push_bytes(&[0xff]);
        assert!(ring.is_empty());
        assert_eq!(ring.recent_with_offset(1), (2, vec![]));
    }

    #[test]
    fn test_output_ring_basic() {
        let ring = OutputRingBuffer::new(3);
        ring.push_str("line1\nline2\n");
        assert_eq!(ring.len(), 2);
        assert_eq!(ring.recent_lines(2), vec!["line1", "line2"]);
    }

    #[test]
    fn test_output_ring_overflow() {
        let ring = OutputRingBuffer::new(2);
        ring.push_str("a\nb\nc\n");
        assert_eq!(ring.len(), 2);
        assert_eq!(ring.recent_lines(2), vec!["b", "c"]);
    }

    #[test]
    fn test_output_ring_bytes() {
        let ring = OutputRingBuffer::new(10);
        ring.push_bytes(b"hello\nworld\n");
        assert_eq!(ring.recent_lines(2), vec!["hello", "world"]);
    }

    #[test]
    fn test_output_ring_recent_less_than_total() {
        let ring = OutputRingBuffer::new(100);
        ring.push_str("a\nb\nc\n");
        assert_eq!(ring.recent_lines(2), vec!["b", "c"]);
    }

    #[test]
    fn test_output_ring_clear() {
        let ring = OutputRingBuffer::new(10);
        ring.push_str("a\n");
        ring.clear();
        assert!(ring.is_empty());
    }

    #[test]
    fn test_output_ring_default_capacity() {
        let ring = OutputRingBuffer::default();
        assert_eq!(ring.capacity(), 1000);
    }

    #[test]
    fn test_output_ring_multiline_push() {
        let ring = OutputRingBuffer::new(5);
        ring.push_str("line1\nline2\nline3");
        assert_eq!(ring.len(), 3);
        assert_eq!(ring.all_lines(), vec!["line1", "line2", "line3"]);
    }
}
