//! Agent-side bootstrap contracts.
//!
//! Product execution is exposed through the single product binary. This crate
//! owns agent-specific, process-local state as the runtime grows.

use std::collections::VecDeque;

pub const AGENT_ROLE: &str = "agent";

/// The connection lifecycle between an agent and its Controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentSessionConnection {
    Disconnected,
    Authenticating,
    Connected,
}

/// A rejected session-state transition or bounded outbox operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentSessionSupervisorError {
    InvalidConnectionTransition,
    TaskAlreadyActive,
    NoMatchingActiveTask,
    PendingReportsFull,
}

/// Process-local ownership of one agent's connection, active task, and report outbox.
///
/// A lost connection intentionally preserves the active task and pending reports so a
/// socket adapter can reconnect and replay reports in the same process. It never
/// persists reports or performs network I/O.
#[derive(Debug)]
pub struct AgentSessionSupervisor<T> {
    connection: AgentSessionConnection,
    active_task_id: Option<String>,
    pending_reports: VecDeque<T>,
    pending_report_limit: usize,
}

impl<T> AgentSessionSupervisor<T> {
    /// Creates a disconnected supervisor with a bounded report outbox.
    pub fn new(pending_report_limit: usize) -> Self {
        Self {
            connection: AgentSessionConnection::Disconnected,
            active_task_id: None,
            pending_reports: VecDeque::new(),
            pending_report_limit,
        }
    }

    /// Starts a new connection attempt from the disconnected state.
    pub fn begin_connect(&mut self) -> Result<(), AgentSessionSupervisorError> {
        if self.connection != AgentSessionConnection::Disconnected {
            return Err(AgentSessionSupervisorError::InvalidConnectionTransition);
        }
        self.connection = AgentSessionConnection::Authenticating;
        Ok(())
    }

    /// Marks the current connection as authenticated.
    pub fn mark_authenticated(&mut self) -> Result<(), AgentSessionSupervisorError> {
        if self.connection != AgentSessionConnection::Authenticating {
            return Err(AgentSessionSupervisorError::InvalidConnectionTransition);
        }
        self.connection = AgentSessionConnection::Connected;
        Ok(())
    }

    /// Records a socket loss without discarding task or report state.
    pub fn connection_lost(&mut self) {
        self.connection = AgentSessionConnection::Disconnected;
    }

    /// Starts the only task that may be active in this agent process.
    pub fn start_task(
        &mut self,
        task_id: impl Into<String>,
    ) -> Result<(), AgentSessionSupervisorError> {
        if self.active_task_id.is_some() {
            return Err(AgentSessionSupervisorError::TaskAlreadyActive);
        }
        self.active_task_id = Some(task_id.into());
        Ok(())
    }

    /// Completes the active task when its identifier matches the current task.
    pub fn finish_task(&mut self, task_id: &str) -> Result<(), AgentSessionSupervisorError> {
        if self.active_task_id.as_deref() != Some(task_id) {
            return Err(AgentSessionSupervisorError::NoMatchingActiveTask);
        }
        self.active_task_id = None;
        Ok(())
    }

    /// Adds a report to the bounded process-local outbox.
    pub fn enqueue_report(&mut self, report: T) -> Result<(), AgentSessionSupervisorError> {
        if self.pending_reports.len() == self.pending_report_limit {
            return Err(AgentSessionSupervisorError::PendingReportsFull);
        }
        self.pending_reports.push_back(report);
        Ok(())
    }

    /// Returns the next report without acknowledging its handoff to the socket adapter.
    pub fn pending_report(&self) -> Option<&T> {
        self.pending_reports.front()
    }

    /// Removes the front report only after the socket adapter has handed it off.
    pub fn remove_pending_report(&mut self) -> Option<T> {
        self.pending_reports.pop_front()
    }

    /// Returns the present connection state.
    pub fn connection(&self) -> AgentSessionConnection {
        self.connection
    }

    /// Returns the active task identifier, if a task is still running.
    pub fn active_task_id(&self) -> Option<&str> {
        self.active_task_id.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentSessionConnection, AgentSessionSupervisor};

    #[test]
    fn disconnect_keeps_active_task_and_outbox_for_reconnect() {
        let mut supervisor = AgentSessionSupervisor::new(1);
        supervisor.begin_connect().unwrap();
        supervisor.mark_authenticated().unwrap();
        supervisor.start_task("task-42").unwrap();
        supervisor.enqueue_report("completed").unwrap();

        supervisor.connection_lost();

        assert_eq!(
            supervisor.connection(),
            AgentSessionConnection::Disconnected
        );
        assert_eq!(supervisor.active_task_id(), Some("task-42"));
        assert_eq!(supervisor.pending_report(), Some(&"completed"));

        supervisor.begin_connect().unwrap();
        supervisor.mark_authenticated().unwrap();
        assert_eq!(supervisor.remove_pending_report(), Some("completed"));
        supervisor.finish_task("task-42").unwrap();
    }

    #[test]
    fn pending_reports_are_bounded() {
        let mut supervisor = AgentSessionSupervisor::new(1);
        supervisor.enqueue_report("first").unwrap();

        assert_eq!(
            supervisor.enqueue_report("second"),
            Err(super::AgentSessionSupervisorError::PendingReportsFull)
        );
    }
}
