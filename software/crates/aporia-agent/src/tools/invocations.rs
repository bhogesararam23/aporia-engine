//! Invocation identity and idempotency.
//!
//! Every model call gets one invocation, and an invocation runs at most once. The table is built
//! from the journal on resume: a call whose completion was recorded returns that recorded result
//! instead of running again, and a call journalled as started but never completed is reported as
//! interrupted -- the previous process may have run it halfway, so the honest answer is "this did
//! not finish", never a silent re-execution.

use super::result::ToolResult;
use std::collections::{BTreeMap, BTreeSet};

/// What `begin` decided about a call id.
#[derive(Clone, Debug, PartialEq)]
pub enum Begin {
    /// Never seen; the caller may run it.
    Fresh,
    /// Already completed in this session or a previous process; here is the recorded result.
    Completed(ToolResult),
    /// Started before an interruption with no recorded completion. It must not run again.
    Interrupted,
}

/// The invocation ledger of one session.
#[derive(Debug, Default)]
pub struct InvocationTable {
    started: BTreeSet<String>,
    completed: BTreeMap<String, ToolResult>,
}

impl InvocationTable {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Decide whether a call may run. A fresh id is marked started immediately, so a crash
    /// between here and the completion record leaves an interrupted invocation, not a gap.
    pub fn begin(&mut self, call_id: &str) -> Begin {
        if let Some(result) = self.completed.get(call_id) {
            return Begin::Completed(result.clone());
        }
        if self.started.contains(call_id) {
            return Begin::Interrupted;
        }
        self.started.insert(call_id.to_string());
        Begin::Fresh
    }

    /// Record the outcome. The completion replaces the started mark.
    pub fn complete(&mut self, call_id: &str, result: ToolResult) {
        self.started.remove(call_id);
        self.completed.insert(call_id.to_string(), result);
    }

    /// Replay a `tool.started` journal event during recovery.
    pub fn replay_started(&mut self, call_id: &str) {
        if !self.completed.contains_key(call_id) {
            self.started.insert(call_id.to_string());
        }
    }

    /// Replay a `tool.completed` journal event during recovery.
    pub fn replay_completed(&mut self, call_id: &str, result: ToolResult) {
        self.complete(call_id, result);
    }

    /// The ids that were started but never completed -- what a resume reports as interrupted.
    #[must_use]
    pub fn interrupted(&self) -> Vec<String> {
        self.started.iter().cloned().collect()
    }

    #[must_use]
    pub fn completed_count(&self) -> usize {
        self.completed.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::result::{ErrorClass, ToolFailure, ToolOutput};

    #[test]
    fn a_call_runs_once_and_a_repeat_returns_the_recorded_result() {
        let mut table = InvocationTable::new();
        assert_eq!(table.begin("call-1"), Begin::Fresh);
        table.complete("call-1", Ok(ToolOutput::text("the real output")));
        match table.begin("call-1") {
            Begin::Completed(Ok(output)) => assert_eq!(output.text, "the real output"),
            other => panic!("expected the recorded result, got {other:?}"),
        }
        assert_eq!(table.completed_count(), 1);
    }

    #[test]
    fn a_started_call_without_completion_is_interrupted_not_rerun() {
        let mut table = InvocationTable::new();
        assert_eq!(table.begin("call-2"), Begin::Fresh);
        // The process dies here. Recovery rebuilds from the journal:
        let mut recovered = InvocationTable::new();
        recovered.replay_started("call-2");
        assert_eq!(recovered.begin("call-2"), Begin::Interrupted);
        assert_eq!(recovered.interrupted(), vec!["call-2".to_string()]);
    }

    #[test]
    fn recovery_prefers_the_completion_over_the_start() {
        let mut recovered = InvocationTable::new();
        recovered.replay_started("call-3");
        recovered.replay_completed(
            "call-3",
            Err(ToolFailure::new(ErrorClass::Timeout, "it ran too long")),
        );
        match recovered.begin("call-3") {
            Begin::Completed(Err(failure)) => assert_eq!(failure.class, ErrorClass::Timeout),
            other => panic!("expected the recorded failure, got {other:?}"),
        }
        assert!(recovered.interrupted().is_empty());
    }

    #[test]
    fn a_failure_is_also_a_recorded_completion() {
        let mut table = InvocationTable::new();
        assert_eq!(table.begin("call-4"), Begin::Fresh);
        table.complete(
            "call-4",
            Err(ToolFailure::new(ErrorClass::Denied, "not approved")),
        );
        assert!(matches!(table.begin("call-4"), Begin::Completed(Err(_))));
    }
}
