use std::collections::VecDeque;

#[derive(Debug)]
pub(crate) struct Scrollback {
    capacity: usize,
    chunks: VecDeque<Vec<u8>>,
    held: usize,
    dropped: usize,
}

impl Scrollback {
    #[must_use]
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            chunks: VecDeque::new(),
            held: 0,
            dropped: 0,
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) {
        if chunk.len() >= self.capacity {
            let tail: Vec<u8> = chunk
                .iter()
                .skip(chunk.len() - self.capacity)
                .copied()
                .collect();
            self.chunks.clear();
            self.dropped += self.held + chunk.len() - self.capacity;
            self.held = self.capacity;
            self.chunks.push_back(tail);
            return;
        }
        self.chunks.push_back(chunk.to_vec());
        self.held += chunk.len();
        while self.held > self.capacity {
            let Some(front) = self.chunks.front() else {
                break;
            };
            if front.len() <= self.held - self.capacity {
                self.held -= front.len();
                self.dropped += front.len();
                self.chunks.pop_front();
            } else {
                let keep = front.len() - (self.held - self.capacity);
                self.dropped += self.held - self.capacity;
                self.held = self.capacity;
                if let Some(front) = self.chunks.front_mut() {
                    front.drain(..front.len() - keep);
                }
                break;
            }
        }
    }

    #[must_use]
    pub(crate) fn snapshot(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.held);
        for chunk in &self.chunks {
            out.extend_from_slice(chunk);
        }
        out
    }

    #[cfg(test)]
    fn held(&self) -> usize {
        self.held
    }

    #[cfg(test)]
    fn dropped(&self) -> usize {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::Scrollback;

    #[test]
    fn snapshot_preserves_recorded_bytes() {
        let mut ring = Scrollback::new(16);
        ring.push(b"execvp(3) failed.: No such file or directory\r\n");
        assert_eq!(ring.snapshot(), b"e or directory\r\n".as_slice());
        assert_eq!(ring.dropped(), 30);
    }

    #[test]
    fn chunk_larger_than_capacity_keeps_the_tail() {
        let mut ring = Scrollback::new(4);
        ring.push(b"abcdef");
        assert_eq!(ring.snapshot(), b"cdef".as_slice());
        assert_eq!(ring.held(), 4);
    }

    #[test]
    fn under_capacity_keeps_everything() {
        let mut ring = Scrollback::new(64);
        ring.push(b"bye 3\r\n");
        assert_eq!(ring.snapshot(), b"bye 3\r\n".as_slice());
        assert_eq!(ring.dropped(), 0);
    }
}
