use crate::agent::AgentId;
use std::fmt::{Display, Formatter};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JobId(String);

impl JobId {
    pub fn new(value: impl Into<String>) -> Result<Self, JobError> {
        non_empty(value.into(), "job id").map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskId(String);

impl TaskId {
    pub fn new(value: impl Into<String>) -> Result<Self, JobError> {
        non_empty(value.into(), "task id").map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskNonce(String);

impl TaskNonce {
    pub fn new(value: impl Into<String>) -> Result<Self, JobError> {
        non_empty(value.into(), "task nonce").map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSignature(String);

impl TaskSignature {
    pub fn new(value: impl Into<String>) -> Result<Self, JobError> {
        non_empty(value.into(), "task signature").map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskExpiry(SystemTime);

impl TaskExpiry {
    pub fn new(value: SystemTime) -> Self {
        Self(value)
    }

    pub fn as_system_time(&self) -> SystemTime {
        self.0
    }

    pub fn is_expired_at(&self, now: SystemTime) -> bool {
        now >= self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskRisk {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalRequirement {
    NotRequired,
    AdminConfirmation,
    ManualApproval,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandTask {
    program: String,
    args: Vec<String>,
    timeout: Duration,
    max_output_bytes: usize,
    risk: TaskRisk,
}

impl CommandTask {
    pub fn new(
        program: impl Into<String>,
        args: Vec<String>,
        timeout: Duration,
    ) -> Result<Self, JobError> {
        let program = non_empty(program.into(), "command program")?;
        if timeout.is_zero() {
            return Err(JobError::InvalidTimeout);
        }
        Ok(Self {
            program,
            args,
            timeout,
            max_output_bytes: 1024 * 1024,
            risk: TaskRisk::High,
        })
    }

    pub fn program(&self) -> &str {
        &self.program
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
    }

    pub fn risk(&self) -> TaskRisk {
        self.risk
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftCheckTask {
    policy_document: String,
    timeout: Duration,
    risk: TaskRisk,
}

impl DriftCheckTask {
    pub fn new(policy_document: impl Into<String>, timeout: Duration) -> Result<Self, JobError> {
        let policy_document = non_empty(policy_document.into(), "drift policy document")?;
        if timeout.is_zero() {
            return Err(JobError::InvalidTimeout);
        }
        Ok(Self {
            policy_document,
            timeout,
            risk: TaskRisk::Low,
        })
    }

    pub fn policy_document(&self) -> &str {
        &self.policy_document
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn risk(&self) -> TaskRisk {
        self.risk
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunbookExecutionTask {
    runbook_document: String,
    timeout: Duration,
    risk: TaskRisk,
}

impl RunbookExecutionTask {
    pub fn new(runbook_document: impl Into<String>, timeout: Duration) -> Result<Self, JobError> {
        let runbook_document = non_empty(runbook_document.into(), "runbook document")?;
        if timeout.is_zero() {
            return Err(JobError::InvalidTimeout);
        }
        Ok(Self {
            runbook_document,
            timeout,
            risk: TaskRisk::High,
        })
    }

    pub fn runbook_document(&self) -> &str {
        &self.runbook_document
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn risk(&self) -> TaskRisk {
        self.risk
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskKind {
    Command(CommandTask),
    DriftCheck(DriftCheckTask),
    RunbookExecution(RunbookExecutionTask),
}

impl TaskKind {
    pub fn risk(&self) -> TaskRisk {
        match self {
            Self::Command(task) => task.risk(),
            Self::DriftCheck(task) => task.risk(),
            Self::RunbookExecution(task) => task.risk(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobTarget {
    pub agent_id: AgentId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEnvelope {
    pub job_id: JobId,
    pub task_id: TaskId,
    pub target_agent_id: AgentId,
    pub issued_at: SystemTime,
    pub expires_at: TaskExpiry,
    pub nonce: TaskNonce,
    pub payload_hash: String,
    pub signature: Option<TaskSignature>,
}

impl TaskEnvelope {
    pub fn validate_for_agent(&self, agent_id: &AgentId, now: SystemTime) -> Result<(), JobError> {
        if &self.target_agent_id != agent_id {
            return Err(JobError::TargetAgentMismatch);
        }
        if self.expires_at.is_expired_at(now) {
            return Err(JobError::ExpiredTask);
        }
        if self.signature.is_none() {
            return Err(JobError::UnsignedTask);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Draft,
    PendingApproval,
    Queued,
    Running,
    PartialSuccess,
    Success,
    Failed,
    Canceled,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobResultSummary {
    pub success_count: u32,
    pub failure_count: u32,
    pub changed_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    id: JobId,
    status: JobStatus,
    risk: TaskRisk,
    approval_requirement: ApprovalRequirement,
    timeout: Duration,
}

impl Job {
    pub fn new(
        id: JobId,
        risk: TaskRisk,
        approval_requirement: ApprovalRequirement,
        timeout: Duration,
    ) -> Self {
        let status =
            if risk == TaskRisk::High && approval_requirement != ApprovalRequirement::NotRequired {
                JobStatus::PendingApproval
            } else {
                JobStatus::Draft
            };
        Self {
            id,
            status,
            risk,
            approval_requirement,
            timeout,
        }
    }

    pub fn status(&self) -> JobStatus {
        self.status
    }

    pub fn id(&self) -> &JobId {
        &self.id
    }

    pub fn risk(&self) -> TaskRisk {
        self.risk
    }

    pub fn approval_requirement(&self) -> ApprovalRequirement {
        self.approval_requirement
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn queue(&mut self, confirmed: bool) -> Result<(), JobError> {
        if matches!(
            self.status,
            JobStatus::Success | JobStatus::Failed | JobStatus::Canceled | JobStatus::Expired
        ) {
            return Err(JobError::TerminalState);
        }
        if self.risk == TaskRisk::High
            && self.approval_requirement != ApprovalRequirement::NotRequired
            && !confirmed
        {
            return Err(JobError::HighRiskRequiresApproval);
        }
        self.status = JobStatus::Queued;
        Ok(())
    }

    pub fn start(&mut self) -> Result<(), JobError> {
        if self.status != JobStatus::Queued {
            return Err(JobError::InvalidTransition);
        }
        self.status = JobStatus::Running;
        Ok(())
    }

    pub fn succeed(&mut self) -> Result<(), JobError> {
        self.finish(JobStatus::Success)
    }

    pub fn fail(&mut self) -> Result<(), JobError> {
        self.finish(JobStatus::Failed)
    }

    pub fn cancel(&mut self) -> Result<(), JobError> {
        self.finish(JobStatus::Canceled)
    }

    pub fn expire(&mut self) -> Result<(), JobError> {
        if self.status == JobStatus::Running {
            return Err(JobError::InvalidTransition);
        }
        self.status = JobStatus::Expired;
        Ok(())
    }

    fn finish(&mut self, status: JobStatus) -> Result<(), JobError> {
        if self.status != JobStatus::Running {
            return Err(JobError::InvalidTransition);
        }
        self.status = status;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobError {
    Empty(&'static str),
    InvalidTransition,
    TerminalState,
    HighRiskRequiresApproval,
    ExpiredTask,
    UnsignedTask,
    TargetAgentMismatch,
    InvalidTimeout,
}

impl Display for JobError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty(field) => write!(f, "{field} cannot be empty"),
            Self::InvalidTransition => write!(f, "invalid job state transition"),
            Self::TerminalState => write!(f, "terminal job state cannot transition"),
            Self::HighRiskRequiresApproval => write!(f, "high-risk task requires approval"),
            Self::ExpiredTask => write!(f, "task envelope is expired"),
            Self::UnsignedTask => write!(f, "task envelope is unsigned"),
            Self::TargetAgentMismatch => write!(f, "task envelope target agent mismatch"),
            Self::InvalidTimeout => write!(f, "task timeout must be greater than zero"),
        }
    }
}

impl std::error::Error for JobError {}

fn non_empty(value: String, field: &'static str) -> Result<String, JobError> {
    if value.trim().is_empty() {
        Err(JobError::Empty(field))
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> Job {
        Job::new(
            JobId::new("job-1").unwrap(),
            TaskRisk::Low,
            ApprovalRequirement::NotRequired,
            Duration::from_secs(30),
        )
    }

    #[test]
    fn transitions_queued_to_running() {
        let mut job = job();
        job.queue(false).unwrap();
        job.start().unwrap();
        assert_eq!(job.status(), JobStatus::Running);
    }

    #[test]
    fn job_running_state_represents_coarse_dispatch_or_execution() {
        let mut job = job();
        job.queue(false).unwrap();

        job.start().unwrap();

        assert_eq!(job.status(), JobStatus::Running);
    }

    #[test]
    fn transitions_running_to_success() {
        let mut job = job();
        job.queue(false).unwrap();
        job.start().unwrap();
        job.succeed().unwrap();
        assert_eq!(job.status(), JobStatus::Success);
    }

    #[test]
    fn transitions_running_to_failed() {
        let mut job = job();
        job.queue(false).unwrap();
        job.start().unwrap();
        job.fail().unwrap();
        assert_eq!(job.status(), JobStatus::Failed);
    }

    #[test]
    fn transitions_running_to_canceled() {
        let mut job = job();
        job.queue(false).unwrap();
        job.start().unwrap();
        job.cancel().unwrap();
        assert_eq!(job.status(), JobStatus::Canceled);
    }

    #[test]
    fn rejects_transition_after_success() {
        let mut job = job();
        job.queue(false).unwrap();
        job.start().unwrap();
        job.succeed().unwrap();
        assert_eq!(job.queue(false), Err(JobError::TerminalState));
    }

    #[test]
    fn rejects_expired_job_dispatch() {
        let mut job = job();
        job.expire().unwrap();
        assert_eq!(job.queue(false), Err(JobError::TerminalState));
    }

    #[test]
    fn rejects_high_risk_without_approval() {
        let mut job = Job::new(
            JobId::new("job-1").unwrap(),
            TaskRisk::High,
            ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        assert_eq!(job.queue(false), Err(JobError::HighRiskRequiresApproval));
    }

    #[test]
    fn command_task_defaults_to_high_risk() {
        let task = CommandTask::new("uptime", Vec::new(), Duration::from_secs(30)).unwrap();

        assert_eq!(task.program(), "uptime");
        assert_eq!(task.risk(), TaskRisk::High);
        assert_eq!(TaskKind::Command(task).risk(), TaskRisk::High);
    }

    #[test]
    fn command_task_rejects_empty_program() {
        assert_eq!(
            CommandTask::new("", Vec::new(), Duration::from_secs(30)),
            Err(JobError::Empty("command program"))
        );
    }

    #[test]
    fn command_task_rejects_missing_timeout() {
        assert_eq!(
            CommandTask::new("uptime", Vec::new(), Duration::ZERO),
            Err(JobError::InvalidTimeout)
        );
    }

    #[test]
    fn runbook_execution_task_defaults_to_high_risk() {
        let task = RunbookExecutionTask::new("kind: Runbook", Duration::from_secs(30)).unwrap();

        assert_eq!(task.runbook_document(), "kind: Runbook");
        assert_eq!(task.risk(), TaskRisk::High);
        assert_eq!(TaskKind::RunbookExecution(task).risk(), TaskRisk::High);
    }

    #[test]
    fn runbook_execution_task_rejects_empty_document() {
        assert_eq!(
            RunbookExecutionTask::new("", Duration::from_secs(30)),
            Err(JobError::Empty("runbook document"))
        );
    }

    #[test]
    fn validates_envelope_expiry() {
        let envelope = TaskEnvelope {
            job_id: JobId::new("job-1").unwrap(),
            task_id: TaskId::new("task-1").unwrap(),
            target_agent_id: AgentId::new("agent-1").unwrap(),
            issued_at: SystemTime::UNIX_EPOCH,
            expires_at: TaskExpiry::new(SystemTime::UNIX_EPOCH),
            nonce: TaskNonce::new("nonce-1").unwrap(),
            payload_hash: "hash".to_owned(),
            signature: Some(TaskSignature::new("sig").unwrap()),
        };
        assert_eq!(
            envelope.validate_for_agent(&AgentId::new("agent-1").unwrap(), SystemTime::UNIX_EPOCH),
            Err(JobError::ExpiredTask)
        );
    }

    #[test]
    fn rejects_envelope_target_mismatch() {
        let envelope = TaskEnvelope {
            job_id: JobId::new("job-1").unwrap(),
            task_id: TaskId::new("task-1").unwrap(),
            target_agent_id: AgentId::new("agent-1").unwrap(),
            issued_at: SystemTime::UNIX_EPOCH,
            expires_at: TaskExpiry::new(SystemTime::UNIX_EPOCH + Duration::from_secs(60)),
            nonce: TaskNonce::new("nonce-1").unwrap(),
            payload_hash: "hash".to_owned(),
            signature: Some(TaskSignature::new("sig").unwrap()),
        };
        assert_eq!(
            envelope.validate_for_agent(&AgentId::new("agent-2").unwrap(), SystemTime::UNIX_EPOCH),
            Err(JobError::TargetAgentMismatch)
        );
    }
}
