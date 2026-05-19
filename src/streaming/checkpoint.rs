use std::collections::VecDeque;

pub struct Checkpoint<E> {
    buf: VecDeque<E>,
    capacity: usize,
    pub client_visible_bytes: u64,
}

impl<E> Checkpoint<E> {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(capacity),
            capacity,
            client_visible_bytes: 0,
        }
    }

    pub fn record(&mut self, event: E) {
        if self.buf.len() == self.capacity {
            self.buf.pop_front();
        }
        self.buf.push_back(event);
    }

    pub fn can_failover(&self) -> bool {
        self.client_visible_bytes == 0
    }
}
