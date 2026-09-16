//! Cooperative scheduling for foreground consumers of background event streams.
use std::time::{Duration, Instant};

use gpui::BackgroundExecutor;

/// Bounds a burst of ready events. Individual handlers must still remain small.
/// Awaiting a ready channel alone does not let the UI paint or process input.
pub struct UiWorkBudget {
    started: Instant,
    processed: usize,
}

impl Default for UiWorkBudget {
    fn default() -> Self {
        Self { started: Instant::now(), processed: 0 }
    }
}

impl UiWorkBudget {
    pub async fn checkpoint(&mut self, executor: &BackgroundExecutor) {
        self.processed += 1;
        if self.exhausted(self.started.elapsed()) {
            executor.timer(Duration::from_millis(1)).await;
            *self = Self::default();
        }
    }

    fn exhausted(&self, elapsed: Duration) -> bool {
        self.processed >= 32 || elapsed >= Duration::from_millis(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yields_for_either_a_large_burst_or_slow_handlers() {
        let mut budget = UiWorkBudget::default();
        budget.processed = 31;
        assert!(!budget.exhausted(Duration::from_millis(3)));
        budget.processed = 32;
        assert!(budget.exhausted(Duration::ZERO));
        budget.processed = 1;
        assert!(budget.exhausted(Duration::from_millis(4)));
    }
}
