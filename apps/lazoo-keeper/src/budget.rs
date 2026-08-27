use std::time::Duration;

const WINDOW_MILLIS: u64 = 5 * 60 * 1_000;
const HEALTHY_RESET_MILLIS: u64 = 10 * 60 * 1_000;
const BACKOFF_SECONDS: [u64; 3] = [1, 2, 5];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestartAction {
    RestartAfter(Duration),
    Exhausted,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RestartBudget {
    exits: Vec<u64>,
}

impl RestartBudget {
    #[must_use]
    pub fn from_exit_times(exits: Vec<u64>) -> Self {
        Self { exits }
    }

    pub fn record_exit(&mut self, now_millis: u64) -> RestartAction {
        if self
            .exits
            .last()
            .is_some_and(|last| now_millis.saturating_sub(*last) >= HEALTHY_RESET_MILLIS)
        {
            self.exits.clear();
        }
        self.exits
            .retain(|exit| now_millis.saturating_sub(*exit) <= WINDOW_MILLIS);
        self.exits.push(now_millis);
        let restart_index = self.exits.len().saturating_sub(1);
        BACKOFF_SECONDS
            .get(restart_index)
            .map_or(RestartAction::Exhausted, |seconds| {
                RestartAction::RestartAfter(Duration::from_secs(*seconds))
            })
    }

    #[must_use]
    pub fn exit_times(&self) -> &[u64] {
        &self.exits
    }
}
