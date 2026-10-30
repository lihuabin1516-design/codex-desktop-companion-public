use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeSemaphoreKind {
    Dispatch,
    Reconcile,
    Control,
}

#[derive(Debug, Clone)]
pub struct RuntimeSemaphore {
    limit: usize,
    in_use: Arc<AtomicUsize>,
}

impl RuntimeSemaphore {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            in_use: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub const fn limit(&self) -> usize {
        self.limit
    }

    pub fn in_use(&self) -> usize {
        self.in_use.load(Ordering::SeqCst)
    }

    pub fn available(&self) -> usize {
        self.limit.saturating_sub(self.in_use())
    }

    pub fn try_acquire(&self) -> Option<RuntimePermit> {
        let mut current = self.in_use.load(Ordering::SeqCst);
        loop {
            if current >= self.limit {
                return None;
            }
            match self.in_use.compare_exchange(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    return Some(RuntimePermit {
                        in_use: Arc::clone(&self.in_use),
                    });
                }
                Err(actual) => current = actual,
            }
        }
    }
}

#[derive(Debug)]
pub struct RuntimePermit {
    in_use: Arc<AtomicUsize>,
}

impl Drop for RuntimePermit {
    fn drop(&mut self) {
        self.in_use.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone)]
pub struct CoordinatorSemaphores {
    pub dispatch: RuntimeSemaphore,
    pub reconcile: RuntimeSemaphore,
    pub control: RuntimeSemaphore,
}

impl CoordinatorSemaphores {
    pub fn new(dispatch: usize, reconcile: usize, control: usize) -> Self {
        Self {
            dispatch: RuntimeSemaphore::new(dispatch),
            reconcile: RuntimeSemaphore::new(reconcile),
            control: RuntimeSemaphore::new(control),
        }
    }
}
