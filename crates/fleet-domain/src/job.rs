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
pub struct ApprovalId(String);

impl ApprovalId {
    pub fn new(value: impl Into<String>) -> Result<Self, JobError> {
        non_empty(value.into(), "approval id").map(Self)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
    Expired,
    Canceled,
}

impl ApprovalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
            Self::Canceled => "canceled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "approved" => Some(Self::Approved),
            "rejected" => Some(Self::Rejected),
            "expired" => Some(Self::Expired),
            "canceled" => Some(Self::Canceled),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    id: ApprovalId,
    job_id: JobId,
    requester: String,
    approver: Option<String>,
    reason: String,
    status: ApprovalStatus,
    expires_at: TaskExpiry,
    created_at: SystemTime,
    decided_at: Option<SystemTime>,
}

impl ApprovalRequest {
    pub fn new(
        id: ApprovalId,
        job_id: JobId,
        requester: impl Into<String>,
        reason: impl Into<String>,
        expires_at: SystemTime,
        created_at: SystemTime,
    ) -> Result<Self, JobError> {
        let requester = non_empty(requester.into(), "approval requester")?;
        if expires_at <= created_at {
            return Err(JobError::ApprovalExpiryMustBeFuture);
        }
        Ok(Self {
            id,
            job_id,
            requester,
            approver: None,
            reason: reason.into(),
            status: ApprovalStatus::Pending,
            expires_at: TaskExpiry::new(expires_at),
            created_at,
            decided_at: None,
        })
    }

    pub fn id(&self) -> &ApprovalId {
        &self.id
    }

    pub fn job_id(&self) -> &JobId {
        &self.job_id
    }

    pub fn requester(&self) -> &str {
        &self.requester
    }

    pub fn approver(&self) -> Option<&str> {
        self.approver.as_deref()
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub fn status(&self) -> ApprovalStatus {
        self.status
    }

    pub fn expires_at(&self) -> SystemTime {
        self.expires_at.as_system_time()
    }

    pub fn created_at(&self) -> SystemTime {
        self.created_at
    }

    pub fn decided_at(&self) -> Option<SystemTime> {
        self.decided_at
    }

    pub fn approve(
        &mut self,
        approver: impl Into<String>,
        reason: impl Into<String>,
        now: SystemTime,
    ) -> Result<(), JobError> {
        self.ensure_pending(now)?;
        self.status = ApprovalStatus::Approved;
        self.approver = Some(non_empty(approver.into(), "approval approver")?);
        self.reason = reason.into();
        self.decided_at = Some(now);
        Ok(())
    }

    pub fn reject(
        &mut self,
        approver: impl Into<String>,
        reason: impl Into<String>,
        now: SystemTime,
    ) -> Result<(), JobError> {
        self.ensure_pending(now)?;
        self.status = ApprovalStatus::Rejected;
        self.approver = Some(non_empty(approver.into(), "approval approver")?);
        self.reason = reason.into();
        self.decided_at = Some(now);
        Ok(())
    }

    pub fn expire(&mut self, now: SystemTime) -> Result<(), JobError> {
        if self.status.is_terminal() {
            return Err(JobError::TerminalApprovalState);
        }
        if !self.expires_at.is_expired_at(now) {
            return Err(JobError::ApprovalNotExpired);
        }
        self.status = ApprovalStatus::Expired;
        self.decided_at = Some(now);
        Ok(())
    }

    pub fn cancel(&mut self, now: SystemTime) -> Result<(), JobError> {
        if self.status.is_terminal() {
            return Err(JobError::TerminalApprovalState);
        }
        self.status = ApprovalStatus::Canceled;
        self.decided_at = Some(now);
        Ok(())
    }

    fn ensure_pending(&self, now: SystemTime) -> Result<(), JobError> {
        if self.status.is_terminal() {
            return Err(JobError::TerminalApprovalState);
        }
        if self.expires_at.is_expired_at(now) {
            return Err(JobError::ExpiredApproval);
        }
        Ok(())
    }
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
        let risk = classify_command_risk(program.as_str(), &args);
        Ok(Self {
            program,
            args,
            timeout,
            max_output_bytes: 1024 * 1024,
            risk,
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

pub fn classify_command_risk(program: &str, args: &[String]) -> TaskRisk {
    let program = program.trim();
    let lower_program = program
        .rsplit('/')
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    let high_risk_programs = [
        "sh",
        "bash",
        "zsh",
        "fish",
        "dash",
        "cmd",
        "powershell",
        "pwsh",
        "sudo",
        "su",
        "reboot",
        "shutdown",
        "halt",
        "poweroff",
        "useradd",
        "usermod",
        "userdel",
        "groupadd",
        "groupmod",
        "groupdel",
        "passwd",
        "chown",
        "chmod",
        "rm",
        "mv",
        "cp",
        "systemctl",
        "service",
        "apt",
        "apt-get",
        "dnf",
        "yum",
        "pacman",
    ];
    if high_risk_programs.contains(&lower_program.as_str()) {
        return TaskRisk::High;
    }
    if args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "reboot" | "shutdown" | "halt" | "poweroff" | "useradd" | "usermod" | "userdel"
        )
    }) {
        return TaskRisk::High;
    }
    let low_risk_programs = [
        "true", "false", "echo", "uptime", "hostname", "whoami", "id", "date", "uname",
    ];
    if low_risk_programs.contains(&lower_program.as_str()) {
        TaskRisk::Low
    } else {
        TaskRisk::High
    }
}

pub fn approval_requirement_for_task(task: &TaskKind, target_count: usize) -> ApprovalRequirement {
    if target_count > 1 {
        return ApprovalRequirement::ManualApproval;
    }
    match task.risk() {
        TaskRisk::High => ApprovalRequirement::ManualApproval,
        TaskRisk::Medium => ApprovalRequirement::AdminConfirmation,
        TaskRisk::Low => ApprovalRequirement::NotRequired,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentStatus {
    Queued,
    Dispatched,
    Accepted,
    Started,
    Succeeded,
    Failed,
    Rejected,
    Canceled,
    Expired,
}

impl AssignmentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Dispatched => "dispatched",
            Self::Accepted => "accepted",
            Self::Started => "started",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Rejected => "rejected",
            Self::Canceled => "canceled",
            Self::Expired => "expired",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Rejected | Self::Canceled | Self::Expired
        )
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "dispatched" => Some(Self::Dispatched),
            "accepted" => Some(Self::Accepted),
            "started" => Some(Self::Started),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "rejected" => Some(Self::Rejected),
            "canceled" => Some(Self::Canceled),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }

    pub fn can_cancel(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Dispatched | Self::Accepted | Self::Started
        )
    }

    fn is_failure_for_aggregate(self) -> bool {
        matches!(self, Self::Failed | Self::Rejected | Self::Expired)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    job_id: JobId,
    task_id: TaskId,
    agent_id: AgentId,
    status: AssignmentStatus,
}

impl Assignment {
    pub fn new(job_id: JobId, task_id: TaskId, agent_id: AgentId) -> Self {
        Self {
            job_id,
            task_id,
            agent_id,
            status: AssignmentStatus::Queued,
        }
    }

    pub fn job_id(&self) -> &JobId {
        &self.job_id
    }

    pub fn task_id(&self) -> &TaskId {
        &self.task_id
    }

    pub fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }

    pub fn status(&self) -> AssignmentStatus {
        self.status
    }

    pub fn dispatch(&mut self) -> Result<(), JobError> {
        self.transition(AssignmentStatus::Dispatched, &[AssignmentStatus::Queued])
    }

    pub fn accept(&mut self) -> Result<(), JobError> {
        self.transition(AssignmentStatus::Accepted, &[AssignmentStatus::Dispatched])
    }

    pub fn start(&mut self) -> Result<(), JobError> {
        self.transition(AssignmentStatus::Started, &[AssignmentStatus::Accepted])
    }

    pub fn record_output_received(&self) -> Result<(), JobError> {
        if self.status == AssignmentStatus::Started {
            Ok(())
        } else if self.status.is_terminal() {
            Err(JobError::TerminalState)
        } else {
            Err(JobError::InvalidAssignmentTransition)
        }
    }

    pub fn succeed(&mut self) -> Result<(), JobError> {
        self.transition(AssignmentStatus::Succeeded, &[AssignmentStatus::Started])
    }

    pub fn fail(&mut self) -> Result<(), JobError> {
        self.transition(AssignmentStatus::Failed, &[AssignmentStatus::Started])
    }

    pub fn reject(&mut self) -> Result<(), JobError> {
        self.transition(AssignmentStatus::Rejected, &[AssignmentStatus::Dispatched])
    }

    pub fn cancel(&mut self) -> Result<(), JobError> {
        self.transition(
            AssignmentStatus::Canceled,
            &[
                AssignmentStatus::Queued,
                AssignmentStatus::Dispatched,
                AssignmentStatus::Accepted,
                AssignmentStatus::Started,
            ],
        )
    }

    pub fn expire(&mut self) -> Result<(), JobError> {
        self.transition(
            AssignmentStatus::Expired,
            &[
                AssignmentStatus::Queued,
                AssignmentStatus::Dispatched,
                AssignmentStatus::Accepted,
                AssignmentStatus::Started,
            ],
        )
    }

    fn transition(
        &mut self,
        next: AssignmentStatus,
        allowed_from: &[AssignmentStatus],
    ) -> Result<(), JobError> {
        if self.status.is_terminal() {
            return Err(JobError::TerminalState);
        }
        if !allowed_from.contains(&self.status) {
            return Err(JobError::InvalidAssignmentTransition);
        }
        self.status = next;
        Ok(())
    }
}

pub fn aggregate_job_status(
    assignment_statuses: &[AssignmentStatus],
    max_failures: Option<u32>,
) -> JobStatus {
    if assignment_statuses.is_empty() {
        return JobStatus::Draft;
    }

    let success_count = assignment_statuses
        .iter()
        .filter(|status| **status == AssignmentStatus::Succeeded)
        .count();
    let failure_count = assignment_statuses
        .iter()
        .filter(|status| status.is_failure_for_aggregate())
        .count();

    if matches!(max_failures, Some(limit) if limit > 0 && failure_count >= limit as usize) {
        return if success_count > 0 {
            JobStatus::PartialSuccess
        } else {
            JobStatus::Failed
        };
    }

    let terminal_count = assignment_statuses
        .iter()
        .filter(|status| status.is_terminal())
        .count();
    if terminal_count < assignment_statuses.len() {
        return if assignment_statuses
            .iter()
            .all(|status| *status == AssignmentStatus::Queued)
        {
            JobStatus::Queued
        } else {
            JobStatus::Running
        };
    }

    if success_count == assignment_statuses.len() {
        return JobStatus::Success;
    }
    if success_count > 0 {
        return JobStatus::PartialSuccess;
    }
    if assignment_statuses
        .iter()
        .all(|status| *status == AssignmentStatus::Canceled)
    {
        return JobStatus::Canceled;
    }
    if assignment_statuses
        .iter()
        .all(|status| *status == AssignmentStatus::Expired)
    {
        return JobStatus::Expired;
    }
    JobStatus::Failed
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
        let status = if approval_requirement != ApprovalRequirement::NotRequired {
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
        if self.approval_requirement != ApprovalRequirement::NotRequired && !confirmed {
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

    pub fn mark_partial_success(&mut self) -> Result<(), JobError> {
        self.finish(JobStatus::PartialSuccess)
    }

    pub fn cancel(&mut self) -> Result<(), JobError> {
        if matches!(
            self.status,
            JobStatus::Success | JobStatus::Failed | JobStatus::Canceled | JobStatus::Expired
        ) {
            return Err(JobError::TerminalState);
        }
        if !matches!(self.status, JobStatus::Queued | JobStatus::Running) {
            return Err(JobError::InvalidTransition);
        }
        self.status = JobStatus::Canceled;
        Ok(())
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
    InvalidAssignmentTransition,
    TerminalState,
    HighRiskRequiresApproval,
    ExpiredTask,
    UnsignedTask,
    TargetAgentMismatch,
    InvalidTimeout,
    ApprovalExpiryMustBeFuture,
    ExpiredApproval,
    ApprovalNotExpired,
    TerminalApprovalState,
    InvalidApprovalStatus,
}

impl Display for JobError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty(field) => write!(f, "{field} cannot be empty"),
            Self::InvalidTransition => write!(f, "invalid job state transition"),
            Self::InvalidAssignmentTransition => {
                write!(f, "invalid assignment state transition")
            }
            Self::TerminalState => write!(f, "terminal job state cannot transition"),
            Self::HighRiskRequiresApproval => write!(f, "high-risk task requires approval"),
            Self::ExpiredTask => write!(f, "task envelope is expired"),
            Self::UnsignedTask => write!(f, "task envelope is unsigned"),
            Self::TargetAgentMismatch => write!(f, "task envelope target agent mismatch"),
            Self::InvalidTimeout => write!(f, "task timeout must be greater than zero"),
            Self::ApprovalExpiryMustBeFuture => write!(f, "approval expiry must be in the future"),
            Self::ExpiredApproval => write!(f, "approval request is expired"),
            Self::ApprovalNotExpired => write!(f, "approval request is not expired"),
            Self::TerminalApprovalState => write!(f, "terminal approval state cannot transition"),
            Self::InvalidApprovalStatus => write!(f, "invalid approval status"),
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
    fn transitions_queued_to_canceled() {
        let mut job = job();
        job.queue(false).unwrap();
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
    fn job_creation_initial_state_reflects_approval_requirement() {
        let low_risk = Job::new(
            JobId::new("job-low").unwrap(),
            TaskRisk::Low,
            ApprovalRequirement::NotRequired,
            Duration::from_secs(30),
        );
        let high_risk = Job::new(
            JobId::new("job-high").unwrap(),
            TaskRisk::High,
            ApprovalRequirement::ManualApproval,
            Duration::from_secs(30),
        );

        assert_eq!(low_risk.status(), JobStatus::Draft);
        assert_eq!(high_risk.status(), JobStatus::PendingApproval);
    }

    #[test]
    fn approval_pending_job_queues_after_confirmation() {
        let mut job = Job::new(
            JobId::new("job-approval").unwrap(),
            TaskRisk::High,
            ApprovalRequirement::ManualApproval,
            Duration::from_secs(30),
        );

        job.queue(true).unwrap();

        assert_eq!(job.status(), JobStatus::Queued);
    }

    #[test]
    fn transitions_running_to_partial_success() {
        let mut job = job();
        job.queue(false).unwrap();
        job.start().unwrap();
        job.mark_partial_success().unwrap();

        assert_eq!(job.status(), JobStatus::PartialSuccess);
    }

    #[test]
    fn assignment_transitions_from_queued_to_dispatched() {
        let mut assignment = assignment();

        assignment.dispatch().unwrap();

        assert_eq!(assignment.status(), AssignmentStatus::Dispatched);
    }

    #[test]
    fn assignment_transitions_from_dispatched_to_accepted() {
        let mut assignment = assignment();

        assignment.dispatch().unwrap();
        assignment.accept().unwrap();

        assert_eq!(assignment.status(), AssignmentStatus::Accepted);
    }

    #[test]
    fn assignment_transitions_from_accepted_to_started() {
        let mut assignment = assignment();

        assignment.dispatch().unwrap();
        assignment.accept().unwrap();
        assignment.start().unwrap();

        assert_eq!(assignment.status(), AssignmentStatus::Started);
    }

    #[test]
    fn assignment_transitions_from_started_to_succeeded() {
        let mut assignment = started_assignment();

        assignment.succeed().unwrap();

        assert_eq!(assignment.status(), AssignmentStatus::Succeeded);
    }

    #[test]
    fn assignment_transitions_from_started_to_failed() {
        let mut assignment = started_assignment();

        assignment.fail().unwrap();

        assert_eq!(assignment.status(), AssignmentStatus::Failed);
    }

    #[test]
    fn assignment_transitions_from_dispatched_to_rejected() {
        let mut assignment = assignment();

        assignment.dispatch().unwrap();
        assignment.reject().unwrap();

        assert_eq!(assignment.status(), AssignmentStatus::Rejected);
    }

    #[test]
    fn assignment_status_can_cancel_only_active_states() {
        for status in [
            AssignmentStatus::Queued,
            AssignmentStatus::Dispatched,
            AssignmentStatus::Accepted,
            AssignmentStatus::Started,
        ] {
            assert!(status.can_cancel());
        }
        for status in [
            AssignmentStatus::Succeeded,
            AssignmentStatus::Failed,
            AssignmentStatus::Rejected,
            AssignmentStatus::Canceled,
            AssignmentStatus::Expired,
        ] {
            assert!(!status.can_cancel());
            assert!(status.is_terminal());
        }
    }

    #[test]
    fn output_received_is_an_event_not_an_assignment_status() {
        let assignment = started_assignment();

        assignment.record_output_received().unwrap();

        assert_eq!(assignment.status(), AssignmentStatus::Started);
    }

    #[test]
    fn assignment_rejects_invalid_transition() {
        let mut assignment = assignment();

        assert_eq!(
            assignment.accept(),
            Err(JobError::InvalidAssignmentTransition)
        );
        assignment.dispatch().unwrap();
        assignment.accept().unwrap();
        assignment.start().unwrap();
        assignment.succeed().unwrap();
        assert_eq!(assignment.dispatch(), Err(JobError::TerminalState));
    }

    #[test]
    fn aggregate_status_success_when_all_assignments_succeeded() {
        assert_eq!(
            aggregate_job_status(&[AssignmentStatus::Succeeded], None),
            JobStatus::Success
        );
    }

    #[test]
    fn aggregate_status_partial_success_when_success_and_failure_mix() {
        assert_eq!(
            aggregate_job_status(
                &[AssignmentStatus::Succeeded, AssignmentStatus::Failed],
                None
            ),
            JobStatus::PartialSuccess
        );
    }

    #[test]
    fn aggregate_status_distinguishes_all_failed_from_all_rejected_at_assignment_level() {
        assert_eq!(
            aggregate_job_status(&[AssignmentStatus::Failed], None),
            JobStatus::Failed
        );
        assert_eq!(
            aggregate_job_status(&[AssignmentStatus::Rejected], None),
            JobStatus::Failed
        );
    }

    #[test]
    fn aggregate_status_handles_partial_cancel() {
        assert_eq!(
            aggregate_job_status(
                &[AssignmentStatus::Succeeded, AssignmentStatus::Canceled],
                None
            ),
            JobStatus::PartialSuccess
        );
        assert_eq!(
            aggregate_job_status(
                &[AssignmentStatus::Canceled, AssignmentStatus::Canceled],
                None
            ),
            JobStatus::Canceled
        );
    }

    #[test]
    fn aggregate_status_respects_max_failures() {
        assert_eq!(
            aggregate_job_status(
                &[AssignmentStatus::Failed, AssignmentStatus::Queued],
                Some(1)
            ),
            JobStatus::Failed
        );
        assert_eq!(
            aggregate_job_status(
                &[
                    AssignmentStatus::Succeeded,
                    AssignmentStatus::Failed,
                    AssignmentStatus::Queued
                ],
                Some(1)
            ),
            JobStatus::PartialSuccess
        );
    }

    #[test]
    fn command_task_classifies_safe_commands_as_low_risk() {
        let task = CommandTask::new("uptime", Vec::new(), Duration::from_secs(30)).unwrap();

        assert_eq!(task.program(), "uptime");
        assert_eq!(task.risk(), TaskRisk::Low);
        assert_eq!(TaskKind::Command(task).risk(), TaskRisk::Low);
    }

    #[test]
    fn command_task_classifies_shell_reboot_and_user_commands_as_high_risk() {
        for program in ["sh", "bash", "reboot", "useradd", "groupadd", "sudo"] {
            let task = CommandTask::new(program, Vec::new(), Duration::from_secs(30)).unwrap();
            assert_eq!(task.risk(), TaskRisk::High, "{program} must be high risk");
        }
    }

    #[test]
    fn broad_target_requires_manual_approval_even_for_safe_command() {
        let task = CommandTask::new("uptime", Vec::new(), Duration::from_secs(30)).unwrap();

        assert_eq!(
            approval_requirement_for_task(&TaskKind::Command(task), 2),
            ApprovalRequirement::ManualApproval
        );
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
    fn approval_request_lifecycle_transitions() {
        let now = SystemTime::UNIX_EPOCH;
        let mut approval = ApprovalRequest::new(
            ApprovalId::new("approval-1").unwrap(),
            JobId::new("job-1").unwrap(),
            "operator",
            "needs approval",
            now + Duration::from_secs(60),
            now,
        )
        .unwrap();

        assert_eq!(approval.status(), ApprovalStatus::Pending);
        approval
            .approve("approver", "looks safe", now + Duration::from_secs(1))
            .unwrap();

        assert_eq!(approval.status(), ApprovalStatus::Approved);
        assert_eq!(approval.approver(), Some("approver"));
        assert_eq!(
            approval.reject("other", "too late", now + Duration::from_secs(2)),
            Err(JobError::TerminalApprovalState)
        );
    }

    #[test]
    fn approval_request_expires_only_after_deadline() {
        let now = SystemTime::UNIX_EPOCH;
        let mut approval = ApprovalRequest::new(
            ApprovalId::new("approval-expire").unwrap(),
            JobId::new("job-1").unwrap(),
            "operator",
            "",
            now + Duration::from_secs(60),
            now,
        )
        .unwrap();

        assert_eq!(
            approval.expire(now + Duration::from_secs(30)),
            Err(JobError::ApprovalNotExpired)
        );
        approval.expire(now + Duration::from_secs(60)).unwrap();
        assert_eq!(approval.status(), ApprovalStatus::Expired);
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

    fn assignment() -> Assignment {
        Assignment::new(
            JobId::new("job-1").unwrap(),
            TaskId::new("task-1").unwrap(),
            AgentId::new("agent-1").unwrap(),
        )
    }

    fn started_assignment() -> Assignment {
        let mut assignment = assignment();
        assignment.dispatch().unwrap();
        assignment.accept().unwrap();
        assignment.start().unwrap();
        assignment
    }
}
