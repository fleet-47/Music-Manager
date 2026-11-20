//! Cooperative stopping (F9, F10, F11).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A flag a running task checks between files. Convert and Sync device finish the file in hand,
/// then stop when it is set; the runner sets it on `s`. Clones share the one flag.
#[derive(Debug, Clone, Default)]
pub struct StopFlag(Arc<AtomicBool>);

impl StopFlag {
    /// A new flag, not set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the task to stop after the file in hand.
    pub fn request(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether a stop was asked for.
    #[must_use]
    pub fn is_requested(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_the_flag() {
        let flag = StopFlag::new();
        let seen_by_task = flag.clone();
        assert!(!seen_by_task.is_requested());
        flag.request();
        assert!(seen_by_task.is_requested());
    }
}
