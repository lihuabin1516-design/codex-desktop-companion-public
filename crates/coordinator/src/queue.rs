use std::collections::VecDeque;

use coordination_core::{CoordinationError, ErrorCode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedQueue<T> {
    capacity: usize,
    items: VecDeque<T>,
}

impl<T> BoundedQueue<T> {
    pub fn new(capacity: usize) -> Result<Self, CoordinationError> {
        if capacity == 0 {
            return Err(CoordinationError::new(
                ErrorCode::CapacityExhausted,
                "bounded queue capacity must be greater than zero",
            ));
        }
        Ok(Self {
            capacity,
            items: VecDeque::new(),
        })
    }

    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn available(&self) -> usize {
        self.capacity.saturating_sub(self.items.len())
    }

    pub fn try_push(&mut self, item: T) -> Result<(), CoordinationError> {
        if self.items.len() >= self.capacity {
            Err(CoordinationError::new(
                ErrorCode::CapacityExhausted,
                "coordinator work queue capacity is exhausted",
            )
            .with_retry_after_ms(1_000))
        } else {
            self.items.push_back(item);
            Ok(())
        }
    }

    pub fn pop_front(&mut self) -> Option<T> {
        self.items.pop_front()
    }
}
