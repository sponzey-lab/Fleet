use fleet_domain::{
    Agent, AgentError, AgentId, AgentLabel, AgentStatus, ApprovalId, ApprovalRequest,
    ApprovalStatus, AuditActor, AuditCategory, AuditEvent, AuditTarget, AuditValue, CommandTask,
    DriftCheckTask, DriftReport, Job, JobError, JobId, JobStatus, JobTarget, Policy,
    RunbookExecutionTask, Selector, TaskEnvelope, TaskExpiry, TaskId, TaskKind, TaskNonce,
    TaskSignature, approval_requirement_for_task,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};
use std::time::{Duration, SystemTime};

pub trait AgentRepository {
    type Error;

    fn save(&mut self, agent: Agent) -> Result<(), Self::Error>;
    fn find_by_id(&self, id: &AgentId) -> Result<Option<Agent>, Self::Error>;
    fn list(&self) -> Result<Vec<Agent>, Self::Error>;
}

pub trait AgentInventoryRepository {
    type Error;

    fn list_agents(&self) -> Result<Vec<Agent>, Self::Error>;
    fn find_agent_by_id(&self, id: &AgentId) -> Result<Option<Agent>, Self::Error>;
    fn revoke_agent_key(&mut self, id: &AgentId) -> Result<bool, Self::Error>;
    fn update_agent_labels(
        &mut self,
        id: &AgentId,
        labels: &[AgentLabel],
    ) -> Result<bool, Self::Error>;
}

pub trait AdminTokenRepository {
    type Error;

    fn admin_token_exists(&self) -> Result<bool, Self::Error>;
    fn insert_admin_token_hash(&mut self, token_hash: &str) -> Result<(), Self::Error>;
    fn verify_admin_token_hash(&self, token_hash: &str) -> Result<bool, Self::Error>;
    fn find_admin_token_record(
        &self,
        token_hash: &str,
    ) -> Result<Option<AdminTokenRecord>, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminTokenRecord {
    pub actor_id: String,
    pub role: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentIdentityRecord {
    pub public_key: String,
    pub fingerprint: String,
}

pub trait AgentIdentityRepository {
    type Error;

    fn find_agent_identity(
        &self,
        agent_id: &str,
    ) -> Result<Option<AgentIdentityRecord>, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerIdentityMetadata {
    pub public_key: String,
    pub public_fingerprint: String,
    pub private_key_path: String,
    pub created_at: SystemTime,
}

pub trait ControllerIdentityRepository {
    type Error;

    fn save_controller_identity_metadata(
        &mut self,
        metadata: ControllerIdentityMetadata,
    ) -> Result<(), Self::Error>;
    fn controller_identity_metadata(
        &self,
    ) -> Result<Option<ControllerIdentityMetadata>, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollmentTokenRecord {
    pub id: String,
    pub default_labels: String,
    pub expires_at: SystemTime,
    pub max_uses: u32,
    pub used_count: u32,
    pub revoked: bool,
}

pub trait EnrollmentTokenRepository {
    type Error;

    fn insert_enrollment_token_hash(
        &mut self,
        id: &str,
        token_hash: &str,
        default_labels: &str,
        expires_at: SystemTime,
        max_uses: u32,
    ) -> Result<(), Self::Error>;
    fn list_enrollment_tokens(&self) -> Result<Vec<EnrollmentTokenRecord>, Self::Error>;
    fn revoke_enrollment_token(&mut self, id: &str) -> Result<bool, Self::Error>;
    fn consume_enrollment_token_hash(
        &mut self,
        token_hash: &str,
        now: SystemTime,
    ) -> Result<EnrollmentTokenRecord, Self::Error>;
}

pub trait AuditWriter {
    type Error;

    fn write(&mut self, event: AuditEvent) -> Result<(), Self::Error>;
}

pub trait AuditRepository: AuditWriter {
    fn list(&self, limit: usize) -> Result<Vec<AuditEvent>, Self::Error>;
    fn list_by_category(
        &self,
        category: fleet_domain::AuditCategory,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequestRecord {
    pub id: String,
    pub job_id: String,
    pub requester: String,
    pub approver: Option<String>,
    pub reason: String,
    pub status: String,
    pub expires_at: SystemTime,
    pub created_at: SystemTime,
    pub decided_at: Option<SystemTime>,
}

pub trait ApprovalRepository {
    type Error;

    fn insert_approval_request(
        &mut self,
        request: ApprovalRequestRecord,
    ) -> Result<(), Self::Error>;
    fn find_approval_request(
        &self,
        approval_id: &str,
    ) -> Result<Option<ApprovalRequestRecord>, Self::Error>;
    fn find_pending_approval_for_job(
        &self,
        job_id: &str,
    ) -> Result<Option<ApprovalRequestRecord>, Self::Error>;
    fn list_approval_requests(
        &self,
        status: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ApprovalRequestRecord>, Self::Error>;
    fn update_approval_request(
        &mut self,
        request: ApprovalRequestRecord,
    ) -> Result<bool, Self::Error>;
    fn update_job_status_for_approval(
        &mut self,
        job_id: &str,
        status: JobStatus,
    ) -> Result<bool, Self::Error>;
}

pub trait JobRepository {
    type Error;

    fn save(&mut self, job: Job) -> Result<(), Self::Error>;
}

pub trait CommandJobRepository:
    TaskAssignmentRepository + ApprovalRepository<Error = <Self as TaskAssignmentRepository>::Error>
{
    fn save_command_job(
        &mut self,
        job: Job,
        task: &CommandTask,
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error>;
}

pub trait DriftCheckJobRepository:
    TaskAssignmentRepository + ApprovalRepository<Error = <Self as TaskAssignmentRepository>::Error>
{
    fn save_drift_check_job(
        &mut self,
        job: Job,
        task: &DriftCheckTask,
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error>;
}

pub trait RunbookJobRepository:
    TaskAssignmentRepository + ApprovalRepository<Error = <Self as TaskAssignmentRepository>::Error>
{
    fn save_runbook_job(
        &mut self,
        job: Job,
        task: &RunbookExecutionTask,
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error>;
}

pub trait TaskAssignmentRepository {
    type Error;

    fn save_assignment(&mut self, envelope: TaskEnvelope) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobOutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobOutputChunk {
    pub job_id: String,
    pub agent_id: String,
    pub stream: JobOutputStream,
    pub sequence: u64,
    pub body: String,
}

pub trait JobOutputRepository {
    type Error;

    fn append_output_chunk(&mut self, chunk: JobOutputChunk) -> Result<(), Self::Error>;
    fn list_output_chunks(
        &self,
        job_id: &str,
        agent_id: &str,
    ) -> Result<Vec<JobOutputChunk>, Self::Error>;
    fn list_output_chunks_for_job(&self, job_id: &str) -> Result<Vec<JobOutputChunk>, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobSummaryRecord {
    pub id: String,
    pub status: String,
    pub risk: String,
    pub command_program: Option<String>,
    pub command_args: Vec<String>,
    pub selector_kind: String,
    pub selector_source: String,
    pub strategy_concurrency: u32,
    pub strategy_max_failures: Option<u32>,
    pub target_count: usize,
    pub target_agents: Vec<JobTargetSummaryRecord>,
    pub created_at: SystemTime,
    pub expires_at: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobTargetSummaryRecord {
    pub agent_id: String,
    pub agent_name: String,
    pub status: String,
    pub labels: Vec<(String, String)>,
    pub task_id: Option<String>,
    pub assignment_status: Option<String>,
    pub last_error: String,
}

pub trait JobQueryRepository {
    type Error;

    fn list_job_summaries(&self, limit: usize) -> Result<Vec<JobSummaryRecord>, Self::Error>;
    fn find_job_summary(&self, job_id: &str) -> Result<Option<JobSummaryRecord>, Self::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotPageCursor {
    pub occurred_at: SystemTime,
    pub row_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactsSnapshotRecord {
    pub agent_id: String,
    pub body: String,
    pub collected_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactsSnapshotPageRecord {
    pub agent_id: String,
    pub body: String,
    pub collected_at: SystemTime,
    pub cursor: SnapshotPageCursor,
}

pub trait FactsRepository {
    type Error;

    fn insert_facts_snapshot(
        &mut self,
        agent_id: &str,
        body: &str,
        collected_at: SystemTime,
    ) -> Result<(), Self::Error>;
    fn latest_facts_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<FactsSnapshotRecord>, Self::Error>;
    fn list_facts_snapshots(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<FactsSnapshotPageRecord>, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsSnapshotRecord {
    pub agent_id: String,
    pub body: String,
    pub collected_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsSnapshotPageRecord {
    pub agent_id: String,
    pub body: String,
    pub collected_at: SystemTime,
    pub cursor: SnapshotPageCursor,
}

pub trait MetricsRepository {
    type Error;

    fn insert_metrics_snapshot(
        &mut self,
        agent_id: &str,
        body: &str,
        collected_at: SystemTime,
    ) -> Result<(), Self::Error>;
    fn latest_metrics_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<MetricsSnapshotRecord>, Self::Error>;
    fn list_metrics_snapshots(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<MetricsSnapshotPageRecord>, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLogChunkPageRecord {
    pub agent_id: String,
    pub line: String,
    pub collected_at: SystemTime,
    pub cursor: SnapshotPageCursor,
}

pub trait AgentLogRepository {
    type Error;

    fn list_agent_log_chunks(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AgentLogChunkPageRecord>, Self::Error>;
}

pub struct ListAgentLogChunks;

impl ListAgentLogChunks {
    pub fn execute<R>(
        repo: &R,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AgentLogChunkPageRecord>, R::Error>
    where
        R: AgentLogRepository,
    {
        repo.list_agent_log_chunks(agent_id, limit, before)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftReportRecord {
    pub agent_id: String,
    pub report: DriftReport,
    pub checked_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftReportPageRecord {
    pub agent_id: String,
    pub report: DriftReport,
    pub checked_at: SystemTime,
    pub cursor: SnapshotPageCursor,
}

pub trait DriftRepository {
    type Error;

    fn insert_drift_report(
        &mut self,
        agent_id: &str,
        report: &DriftReport,
        checked_at: SystemTime,
    ) -> Result<(), Self::Error>;
    fn latest_drift_report(&self, agent_id: &str)
    -> Result<Option<DriftReportRecord>, Self::Error>;
    fn list_drift_reports(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<DriftReportPageRecord>, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRecord {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub source: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyAssignmentRecord {
    pub policy_id: String,
    pub agent_id: String,
    pub assigned_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledDriftRecord {
    pub policy_id: String,
    pub agent_id: String,
    pub interval_seconds: u64,
    pub next_due_at: SystemTime,
    pub last_checked_at: Option<SystemTime>,
}

pub trait PolicyRepository {
    type Error;

    fn save_policy_source(
        &mut self,
        policy_id: &str,
        name: &str,
        version: u32,
        source: &str,
    ) -> Result<(), Self::Error>;
    fn list_policies(&self) -> Result<Vec<PolicyRecord>, Self::Error>;
    fn find_policy(&self, policy_id: &str) -> Result<Option<PolicyRecord>, Self::Error>;
    fn assign_policy_to_agent(
        &mut self,
        policy_id: &str,
        agent_id: &str,
        assigned_at: SystemTime,
    ) -> Result<(), Self::Error>;
    fn policies_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<PolicyAssignmentRecord>, Self::Error>;
    fn upsert_policy_schedule(
        &mut self,
        policy_id: &str,
        agent_id: &str,
        interval: Duration,
        next_due_at: SystemTime,
    ) -> Result<(), Self::Error>;
    fn due_scheduled_drift_checks(
        &self,
        now: SystemTime,
        limit: usize,
    ) -> Result<Vec<ScheduledDriftRecord>, Self::Error>;
    fn record_scheduled_drift_check(
        &mut self,
        policy_id: &str,
        agent_id: &str,
        checked_at: SystemTime,
    ) -> Result<(), Self::Error>;
    fn acknowledge_latest_drift_report(
        &mut self,
        agent_id: &str,
        policy_name: &str,
        actor: &str,
        acknowledged_at: SystemTime,
    ) -> Result<bool, Self::Error>;
    fn mark_latest_drift_resolved(
        &mut self,
        agent_id: &str,
        policy_name: &str,
        job_id: &str,
        resolved_at: SystemTime,
    ) -> Result<bool, Self::Error>;
}

pub trait TaskEnvelopeSigner {
    type Error;

    fn sign(&mut self, payload: &str) -> Result<String, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavePolicyInput {
    pub source: String,
    pub actor: String,
    pub now: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignPolicyToAgentInput {
    pub policy_id: String,
    pub agent_id: String,
    pub actor: String,
    pub now: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulePolicyDriftInput {
    pub policy_id: String,
    pub agent_id: String,
    pub interval: Duration,
    pub next_due_at: SystemTime,
    pub actor: String,
    pub now: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordScheduledDriftCheckInput {
    pub policy_id: String,
    pub agent_id: String,
    pub actor: String,
    pub checked_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemediationApprovalInput {
    pub approval_id: String,
    pub job_id: String,
    pub policy_id: String,
    pub agent_id: String,
    pub requester: String,
    pub reason: String,
    pub expires_at: SystemTime,
    pub now: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyUseCaseError<RepoError, AuditError> {
    Domain(String),
    NotFound(String),
    Repository(RepoError),
    Audit(AuditError),
}

impl<RepoError, AuditError> Display for PolicyUseCaseError<RepoError, AuditError>
where
    RepoError: Display,
    AuditError: Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Domain(error) => write!(formatter, "{error}"),
            Self::NotFound(target) => write!(formatter, "not found: {target}"),
            Self::Repository(error) => write!(formatter, "repository error: {error}"),
            Self::Audit(error) => write!(formatter, "audit error: {error}"),
        }
    }
}

pub struct SavePolicy;

impl SavePolicy {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: SavePolicyInput,
    ) -> Result<Policy, PolicyUseCaseError<R::Error, A::Error>>
    where
        R: PolicyRepository,
        A: AuditWriter,
    {
        let policy = fleet_domain::parse_policy_document(&input.source)
            .map_err(|error| PolicyUseCaseError::Domain(error.to_string()))?;
        repo.save_policy_source(&policy.id, &policy.name, policy.version, &policy.source)
            .map_err(PolicyUseCaseError::Repository)?;
        audit
            .write(AuditEvent {
                category: AuditCategory::Policy,
                action: "policy_saved".to_owned(),
                actor: AuditActor::new(input.actor),
                target: AuditTarget::new(policy.id.clone()),
                value: AuditValue::Plain(format!(
                    "name={},version={}",
                    policy.name, policy.version
                )),
                occurred_at: input.now,
            })
            .map_err(PolicyUseCaseError::Audit)?;
        Ok(policy)
    }
}

pub struct AssignPolicyToAgent;

impl AssignPolicyToAgent {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: AssignPolicyToAgentInput,
    ) -> Result<PolicyAssignmentRecord, PolicyUseCaseError<R::Error, A::Error>>
    where
        R: PolicyRepository,
        A: AuditWriter,
    {
        if repo
            .find_policy(&input.policy_id)
            .map_err(PolicyUseCaseError::Repository)?
            .is_none()
        {
            return Err(PolicyUseCaseError::NotFound(input.policy_id));
        }
        repo.assign_policy_to_agent(&input.policy_id, &input.agent_id, input.now)
            .map_err(PolicyUseCaseError::Repository)?;
        audit
            .write(AuditEvent {
                category: AuditCategory::Policy,
                action: "policy_assigned".to_owned(),
                actor: AuditActor::new(input.actor),
                target: AuditTarget::new(input.agent_id.clone()),
                value: AuditValue::Plain(format!("policy_id={}", input.policy_id)),
                occurred_at: input.now,
            })
            .map_err(PolicyUseCaseError::Audit)?;
        Ok(PolicyAssignmentRecord {
            policy_id: input.policy_id,
            agent_id: input.agent_id,
            assigned_at: input.now,
        })
    }
}

pub struct SchedulePolicyDrift;

impl SchedulePolicyDrift {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: SchedulePolicyDriftInput,
    ) -> Result<(), PolicyUseCaseError<R::Error, A::Error>>
    where
        R: PolicyRepository,
        A: AuditWriter,
    {
        if repo
            .find_policy(&input.policy_id)
            .map_err(PolicyUseCaseError::Repository)?
            .is_none()
        {
            return Err(PolicyUseCaseError::NotFound(input.policy_id));
        }
        repo.upsert_policy_schedule(
            &input.policy_id,
            &input.agent_id,
            input.interval,
            input.next_due_at,
        )
        .map_err(PolicyUseCaseError::Repository)?;
        audit
            .write(AuditEvent {
                category: AuditCategory::Drift,
                action: "scheduled_drift_configured".to_owned(),
                actor: AuditActor::new(input.actor),
                target: AuditTarget::new(input.agent_id),
                value: AuditValue::Plain(format!(
                    "policy_id={},interval_seconds={}",
                    input.policy_id,
                    input.interval.as_secs()
                )),
                occurred_at: input.now,
            })
            .map_err(PolicyUseCaseError::Audit)?;
        Ok(())
    }
}

pub struct ListDueScheduledDrift;

impl ListDueScheduledDrift {
    pub fn execute<R>(
        repo: &R,
        now: SystemTime,
        limit: usize,
    ) -> Result<Vec<ScheduledDriftRecord>, R::Error>
    where
        R: PolicyRepository,
    {
        repo.due_scheduled_drift_checks(now, limit)
    }
}

pub struct RecordScheduledDriftCheck;

impl RecordScheduledDriftCheck {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: RecordScheduledDriftCheckInput,
    ) -> Result<(), PolicyUseCaseError<R::Error, A::Error>>
    where
        R: PolicyRepository,
        A: AuditWriter,
    {
        repo.record_scheduled_drift_check(&input.policy_id, &input.agent_id, input.checked_at)
            .map_err(PolicyUseCaseError::Repository)?;
        audit
            .write(AuditEvent {
                category: AuditCategory::Drift,
                action: "scheduled_drift_checked".to_owned(),
                actor: AuditActor::new(input.actor),
                target: AuditTarget::new(input.agent_id),
                value: AuditValue::Plain(format!("policy_id={}", input.policy_id)),
                occurred_at: input.checked_at,
            })
            .map_err(PolicyUseCaseError::Audit)?;
        Ok(())
    }
}

pub struct CreateRemediationApproval;

impl CreateRemediationApproval {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: RemediationApprovalInput,
    ) -> Result<ApprovalRequestRecord, PolicyUseCaseError<R::Error, A::Error>>
    where
        R: ApprovalRepository,
        A: AuditWriter,
    {
        let request = ApprovalRequestRecord {
            id: input.approval_id,
            job_id: input.job_id,
            requester: input.requester.clone(),
            approver: None,
            reason: input.reason,
            status: "pending".to_owned(),
            expires_at: input.expires_at,
            created_at: input.now,
            decided_at: None,
        };
        repo.insert_approval_request(request.clone())
            .map_err(PolicyUseCaseError::Repository)?;
        audit
            .write(AuditEvent {
                category: AuditCategory::Policy,
                action: "remediation_approval_requested".to_owned(),
                actor: AuditActor::new(input.requester),
                target: AuditTarget::new(input.agent_id),
                value: AuditValue::Plain(format!("policy_id={}", input.policy_id)),
                occurred_at: input.now,
            })
            .map_err(PolicyUseCaseError::Audit)?;
        Ok(request)
    }
}

pub struct MarkRemediationResolved;

impl MarkRemediationResolved {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        agent_id: &str,
        policy_name: &str,
        job_id: &str,
        actor: &str,
        resolved_at: SystemTime,
    ) -> Result<bool, PolicyUseCaseError<R::Error, A::Error>>
    where
        R: PolicyRepository,
        A: AuditWriter,
    {
        let changed = repo
            .mark_latest_drift_resolved(agent_id, policy_name, job_id, resolved_at)
            .map_err(PolicyUseCaseError::Repository)?;
        if changed {
            audit
                .write(AuditEvent {
                    category: AuditCategory::Drift,
                    action: "drift_resolved_by_remediation".to_owned(),
                    actor: AuditActor::new(actor.to_owned()),
                    target: AuditTarget::new(agent_id.to_owned()),
                    value: AuditValue::Plain(format!("policy_name={policy_name},job_id={job_id}")),
                    occurred_at: resolved_at,
                })
                .map_err(PolicyUseCaseError::Audit)?;
        }
        Ok(changed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateCommandJobInput {
    pub job_id: String,
    pub target_agent_ids: Vec<String>,
    pub program: String,
    pub args: Vec<String>,
    pub timeout: Duration,
    pub confirmed_high_risk: bool,
    pub confirmed_by: String,
    pub issued_at: SystemTime,
    pub expires_at: SystemTime,
    pub nonce_prefix: String,
    pub approval_request_id: String,
    pub approval_expires_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateCommandJobOutput {
    pub task: CommandTask,
    pub targets: Vec<JobTarget>,
    pub envelopes: Vec<TaskEnvelope>,
    pub approval_request: Option<ApprovalRequestRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateCommandJobError<RepoError, AuditError, SignError> {
    Domain(JobError),
    Agent(AgentError),
    NoTargets,
    Repository(RepoError),
    Audit(AuditError),
    Sign(SignError),
}

pub type CreateCommandJobResult<R, A, S> = Result<
    CreateCommandJobOutput,
    CreateCommandJobError<
        <R as TaskAssignmentRepository>::Error,
        <A as AuditWriter>::Error,
        <S as TaskEnvelopeSigner>::Error,
    >,
>;

impl<RepoError, AuditError, SignError> Display
    for CreateCommandJobError<RepoError, AuditError, SignError>
where
    RepoError: Display,
    AuditError: Display,
    SignError: Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Domain(error) => write!(formatter, "{error}"),
            Self::Agent(error) => write!(formatter, "{error}"),
            Self::NoTargets => write!(formatter, "command job requires at least one target"),
            Self::Repository(error) => write!(formatter, "repository error: {error}"),
            Self::Audit(error) => write!(formatter, "audit error: {error}"),
            Self::Sign(error) => write!(formatter, "sign error: {error}"),
        }
    }
}

pub struct CreateCommandJob;

impl CreateCommandJob {
    pub fn execute<R, A, S>(
        repo: &mut R,
        audit: &mut A,
        signer: &mut S,
        input: CreateCommandJobInput,
    ) -> CreateCommandJobResult<R, A, S>
    where
        R: CommandJobRepository,
        A: AuditWriter,
        S: TaskEnvelopeSigner,
    {
        if input.target_agent_ids.is_empty() {
            return Err(CreateCommandJobError::NoTargets);
        }

        let task = CommandTask::new(input.program, input.args, input.timeout)
            .map_err(CreateCommandJobError::Domain)?;
        let task_kind = TaskKind::Command(task.clone());
        let approval_requirement =
            approval_requirement_for_task(&task_kind, input.target_agent_ids.len());
        let mut job = Job::new(
            JobId::new(input.job_id.clone()).map_err(CreateCommandJobError::Domain)?,
            task.risk(),
            approval_requirement,
            input.timeout,
        );
        if approval_requirement == fleet_domain::ApprovalRequirement::NotRequired {
            job.queue(false).map_err(CreateCommandJobError::Domain)?;
        }

        let targets = input
            .target_agent_ids
            .iter()
            .map(|id| {
                AgentId::new(id.clone())
                    .map(|agent_id| JobTarget { agent_id })
                    .map_err(CreateCommandJobError::Agent)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut envelopes = Vec::with_capacity(targets.len());
        for (index, target) in targets.iter().enumerate() {
            let payload_hash = command_payload_hash(&task, target, index);
            let signature = signer
                .sign(&payload_hash)
                .map_err(CreateCommandJobError::Sign)?;
            envelopes.push(TaskEnvelope {
                job_id: JobId::new(input.job_id.clone()).map_err(CreateCommandJobError::Domain)?,
                task_id: TaskId::new(format!("{}-task-{index}", input.job_id))
                    .map_err(CreateCommandJobError::Domain)?,
                target_agent_id: target.agent_id.clone(),
                issued_at: input.issued_at,
                expires_at: TaskExpiry::new(input.expires_at),
                nonce: TaskNonce::new(format!("{}-{index}", input.nonce_prefix))
                    .map_err(CreateCommandJobError::Domain)?,
                payload_hash,
                signature: Some(
                    TaskSignature::new(signature).map_err(CreateCommandJobError::Domain)?,
                ),
            });
        }

        repo.save_command_job(job, &task)
            .map_err(CreateCommandJobError::Repository)?;
        for envelope in envelopes.iter().cloned() {
            repo.save_assignment(envelope)
                .map_err(CreateCommandJobError::Repository)?;
        }
        let approval_request =
            if approval_requirement != fleet_domain::ApprovalRequirement::NotRequired {
                let approval = ApprovalRequest::new(
                    ApprovalId::new(input.approval_request_id.clone())
                        .map_err(CreateCommandJobError::Domain)?,
                    JobId::new(input.job_id.clone()).map_err(CreateCommandJobError::Domain)?,
                    input.confirmed_by.clone(),
                    "high-risk command requires manual approval",
                    input.approval_expires_at,
                    input.issued_at,
                )
                .map_err(CreateCommandJobError::Domain)?;
                let record = approval_request_to_record(&approval);
                repo.insert_approval_request(record.clone())
                    .map_err(CreateCommandJobError::Repository)?;
                audit
                    .write(AuditEvent {
                        category: AuditCategory::Approval,
                        action: "approval_requested".to_owned(),
                        actor: AuditActor::new(input.confirmed_by.clone()),
                        target: AuditTarget::new(input.job_id.clone()),
                        value: AuditValue::Plain(format!(
                            "approval_id={},reason={},confirmed_high_risk={},target_count={}",
                            record.id,
                            record.reason,
                            input.confirmed_high_risk,
                            targets.len()
                        )),
                        occurred_at: input.issued_at,
                    })
                    .map_err(CreateCommandJobError::Audit)?;
                Some(record)
            } else {
                None
            };
        audit
            .write(AuditEvent {
                category: AuditCategory::Job,
                action: "job_created".to_owned(),
                actor: AuditActor::new(input.confirmed_by.clone()),
                target: AuditTarget::new(input.job_id),
                value: AuditValue::Plain(format!(
                    "confirmed_high_risk={},confirmed_by={},target_count={}",
                    input.confirmed_high_risk,
                    input.confirmed_by,
                    targets.len()
                )),
                occurred_at: input.issued_at,
            })
            .map_err(CreateCommandJobError::Audit)?;

        Ok(CreateCommandJobOutput {
            task,
            targets,
            envelopes,
            approval_request,
        })
    }
}

fn command_payload_hash(task: &CommandTask, target: &JobTarget, index: usize) -> String {
    format!(
        "command:{index}:{}:{}:{}",
        target.agent_id.as_str(),
        task.program(),
        task.args().join("\u{1f}")
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateDriftCheckJobInput {
    pub job_id: String,
    pub target_agent_ids: Vec<String>,
    pub policy_document: String,
    pub timeout: Duration,
    pub created_by: String,
    pub issued_at: SystemTime,
    pub expires_at: SystemTime,
    pub nonce_prefix: String,
    pub approval_request_id: String,
    pub approval_expires_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateDriftCheckJobOutput {
    pub task: DriftCheckTask,
    pub targets: Vec<JobTarget>,
    pub envelopes: Vec<TaskEnvelope>,
    pub approval_request: Option<ApprovalRequestRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateDriftCheckJobError<RepoError, AuditError, SignError> {
    Domain(JobError),
    Agent(AgentError),
    NoTargets,
    Repository(RepoError),
    Audit(AuditError),
    Sign(SignError),
}

pub type CreateDriftCheckJobResult<R, A, S> = Result<
    CreateDriftCheckJobOutput,
    CreateDriftCheckJobError<
        <R as TaskAssignmentRepository>::Error,
        <A as AuditWriter>::Error,
        <S as TaskEnvelopeSigner>::Error,
    >,
>;

impl<RepoError, AuditError, SignError> Display
    for CreateDriftCheckJobError<RepoError, AuditError, SignError>
where
    RepoError: Display,
    AuditError: Display,
    SignError: Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Domain(error) => write!(formatter, "{error}"),
            Self::Agent(error) => write!(formatter, "{error}"),
            Self::NoTargets => write!(formatter, "drift check job requires at least one target"),
            Self::Repository(error) => write!(formatter, "repository error: {error}"),
            Self::Audit(error) => write!(formatter, "audit error: {error}"),
            Self::Sign(error) => write!(formatter, "sign error: {error}"),
        }
    }
}

pub struct CreateDriftCheckJob;

impl CreateDriftCheckJob {
    pub fn execute<R, A, S>(
        repo: &mut R,
        audit: &mut A,
        signer: &mut S,
        input: CreateDriftCheckJobInput,
    ) -> CreateDriftCheckJobResult<R, A, S>
    where
        R: DriftCheckJobRepository,
        A: AuditWriter,
        S: TaskEnvelopeSigner,
    {
        if input.target_agent_ids.is_empty() {
            return Err(CreateDriftCheckJobError::NoTargets);
        }

        let task = DriftCheckTask::new(input.policy_document, input.timeout)
            .map_err(CreateDriftCheckJobError::Domain)?;
        let task_kind = TaskKind::DriftCheck(task.clone());
        let approval_requirement =
            approval_requirement_for_task(&task_kind, input.target_agent_ids.len());
        let mut job = Job::new(
            JobId::new(input.job_id.clone()).map_err(CreateDriftCheckJobError::Domain)?,
            task.risk(),
            approval_requirement,
            input.timeout,
        );
        if approval_requirement == fleet_domain::ApprovalRequirement::NotRequired {
            job.queue(false).map_err(CreateDriftCheckJobError::Domain)?;
        }

        let targets = input
            .target_agent_ids
            .iter()
            .map(|id| {
                AgentId::new(id.clone())
                    .map(|agent_id| JobTarget { agent_id })
                    .map_err(CreateDriftCheckJobError::Agent)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut envelopes = Vec::with_capacity(targets.len());
        for (index, target) in targets.iter().enumerate() {
            let payload_hash = drift_check_payload_hash(&task, target, index);
            let signature = signer
                .sign(&payload_hash)
                .map_err(CreateDriftCheckJobError::Sign)?;
            envelopes.push(TaskEnvelope {
                job_id: JobId::new(input.job_id.clone())
                    .map_err(CreateDriftCheckJobError::Domain)?,
                task_id: TaskId::new(format!("{}-task-{index}", input.job_id))
                    .map_err(CreateDriftCheckJobError::Domain)?,
                target_agent_id: target.agent_id.clone(),
                issued_at: input.issued_at,
                expires_at: TaskExpiry::new(input.expires_at),
                nonce: TaskNonce::new(format!("{}-{index}", input.nonce_prefix))
                    .map_err(CreateDriftCheckJobError::Domain)?,
                payload_hash,
                signature: Some(
                    TaskSignature::new(signature).map_err(CreateDriftCheckJobError::Domain)?,
                ),
            });
        }

        repo.save_drift_check_job(job, &task)
            .map_err(CreateDriftCheckJobError::Repository)?;
        for envelope in envelopes.iter().cloned() {
            repo.save_assignment(envelope)
                .map_err(CreateDriftCheckJobError::Repository)?;
        }
        let approval_request =
            if approval_requirement != fleet_domain::ApprovalRequirement::NotRequired {
                let approval = ApprovalRequest::new(
                    ApprovalId::new(input.approval_request_id.clone())
                        .map_err(CreateDriftCheckJobError::Domain)?,
                    JobId::new(input.job_id.clone()).map_err(CreateDriftCheckJobError::Domain)?,
                    input.created_by.clone(),
                    "broad drift check requires manual approval",
                    input.approval_expires_at,
                    input.issued_at,
                )
                .map_err(CreateDriftCheckJobError::Domain)?;
                let record = approval_request_to_record(&approval);
                repo.insert_approval_request(record.clone())
                    .map_err(CreateDriftCheckJobError::Repository)?;
                audit
                    .write(AuditEvent {
                        category: AuditCategory::Approval,
                        action: "approval_requested".to_owned(),
                        actor: AuditActor::new(input.created_by.clone()),
                        target: AuditTarget::new(input.job_id.clone()),
                        value: AuditValue::Plain(format!(
                            "approval_id={},reason={},target_count={}",
                            record.id,
                            record.reason,
                            targets.len()
                        )),
                        occurred_at: input.issued_at,
                    })
                    .map_err(CreateDriftCheckJobError::Audit)?;
                Some(record)
            } else {
                None
            };
        audit
            .write(AuditEvent {
                category: AuditCategory::Drift,
                action: "drift_check_job_created".to_owned(),
                actor: AuditActor::new(input.created_by.clone()),
                target: AuditTarget::new(input.job_id),
                value: AuditValue::Plain(format!(
                    "created_by={},target_count={}",
                    input.created_by,
                    targets.len()
                )),
                occurred_at: input.issued_at,
            })
            .map_err(CreateDriftCheckJobError::Audit)?;

        Ok(CreateDriftCheckJobOutput {
            task,
            targets,
            envelopes,
            approval_request,
        })
    }
}

fn drift_check_payload_hash(task: &DriftCheckTask, target: &JobTarget, index: usize) -> String {
    format!(
        "drift_check:{index}:{}:{}",
        target.agent_id.as_str(),
        task.policy_document()
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRunbookJobInput {
    pub job_id: String,
    pub target_agent_ids: Vec<String>,
    pub runbook_document: String,
    pub timeout: Duration,
    pub confirmed_high_risk: bool,
    pub confirmed_by: String,
    pub issued_at: SystemTime,
    pub expires_at: SystemTime,
    pub nonce_prefix: String,
    pub approval_request_id: String,
    pub approval_expires_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRunbookJobOutput {
    pub task: RunbookExecutionTask,
    pub targets: Vec<JobTarget>,
    pub envelopes: Vec<TaskEnvelope>,
    pub approval_request: Option<ApprovalRequestRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateRunbookJobError<RepoError, AuditError, SignError> {
    Domain(JobError),
    Agent(AgentError),
    InvalidRunbook(String),
    NoTargets,
    Repository(RepoError),
    Audit(AuditError),
    Sign(SignError),
}

pub type CreateRunbookJobResult<R, A, S> = Result<
    CreateRunbookJobOutput,
    CreateRunbookJobError<
        <R as TaskAssignmentRepository>::Error,
        <A as AuditWriter>::Error,
        <S as TaskEnvelopeSigner>::Error,
    >,
>;

impl<RepoError, AuditError, SignError> Display
    for CreateRunbookJobError<RepoError, AuditError, SignError>
where
    RepoError: Display,
    AuditError: Display,
    SignError: Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Domain(error) => write!(formatter, "{error}"),
            Self::Agent(error) => write!(formatter, "{error}"),
            Self::InvalidRunbook(error) => write!(formatter, "invalid runbook: {error}"),
            Self::NoTargets => write!(formatter, "runbook job requires at least one target"),
            Self::Repository(error) => write!(formatter, "repository error: {error}"),
            Self::Audit(error) => write!(formatter, "audit error: {error}"),
            Self::Sign(error) => write!(formatter, "sign error: {error}"),
        }
    }
}

pub struct CreateRunbookJob;

impl CreateRunbookJob {
    pub fn execute<R, A, S>(
        repo: &mut R,
        audit: &mut A,
        signer: &mut S,
        input: CreateRunbookJobInput,
    ) -> CreateRunbookJobResult<R, A, S>
    where
        R: RunbookJobRepository,
        A: AuditWriter,
        S: TaskEnvelopeSigner,
    {
        if input.target_agent_ids.is_empty() {
            return Err(CreateRunbookJobError::NoTargets);
        }

        fleet_domain::parse_runbook_document(&input.runbook_document)
            .map_err(|error| CreateRunbookJobError::InvalidRunbook(error.to_string()))?;
        let task = RunbookExecutionTask::new(input.runbook_document, input.timeout)
            .map_err(CreateRunbookJobError::Domain)?;
        let task_kind = TaskKind::RunbookExecution(task.clone());
        let approval_requirement =
            approval_requirement_for_task(&task_kind, input.target_agent_ids.len());
        let mut job = Job::new(
            JobId::new(input.job_id.clone()).map_err(CreateRunbookJobError::Domain)?,
            task.risk(),
            approval_requirement,
            input.timeout,
        );
        if approval_requirement == fleet_domain::ApprovalRequirement::NotRequired {
            job.queue(false).map_err(CreateRunbookJobError::Domain)?;
        }

        let targets = input
            .target_agent_ids
            .iter()
            .map(|id| {
                AgentId::new(id.clone())
                    .map(|agent_id| JobTarget { agent_id })
                    .map_err(CreateRunbookJobError::Agent)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut envelopes = Vec::with_capacity(targets.len());
        for (index, target) in targets.iter().enumerate() {
            let payload_hash = runbook_payload_hash(&task, target, index);
            let signature = signer
                .sign(&payload_hash)
                .map_err(CreateRunbookJobError::Sign)?;
            envelopes.push(TaskEnvelope {
                job_id: JobId::new(input.job_id.clone()).map_err(CreateRunbookJobError::Domain)?,
                task_id: TaskId::new(format!("{}-task-{index}", input.job_id))
                    .map_err(CreateRunbookJobError::Domain)?,
                target_agent_id: target.agent_id.clone(),
                issued_at: input.issued_at,
                expires_at: TaskExpiry::new(input.expires_at),
                nonce: TaskNonce::new(format!("{}-{index}", input.nonce_prefix))
                    .map_err(CreateRunbookJobError::Domain)?,
                payload_hash,
                signature: Some(
                    TaskSignature::new(signature).map_err(CreateRunbookJobError::Domain)?,
                ),
            });
        }

        repo.save_runbook_job(job, &task)
            .map_err(CreateRunbookJobError::Repository)?;
        for envelope in envelopes.iter().cloned() {
            repo.save_assignment(envelope)
                .map_err(CreateRunbookJobError::Repository)?;
        }
        let approval_request =
            if approval_requirement != fleet_domain::ApprovalRequirement::NotRequired {
                let approval = ApprovalRequest::new(
                    ApprovalId::new(input.approval_request_id.clone())
                        .map_err(CreateRunbookJobError::Domain)?,
                    JobId::new(input.job_id.clone()).map_err(CreateRunbookJobError::Domain)?,
                    input.confirmed_by.clone(),
                    "runbook execution requires manual approval",
                    input.approval_expires_at,
                    input.issued_at,
                )
                .map_err(CreateRunbookJobError::Domain)?;
                let record = approval_request_to_record(&approval);
                repo.insert_approval_request(record.clone())
                    .map_err(CreateRunbookJobError::Repository)?;
                audit
                    .write(AuditEvent {
                        category: AuditCategory::Approval,
                        action: "approval_requested".to_owned(),
                        actor: AuditActor::new(input.confirmed_by.clone()),
                        target: AuditTarget::new(input.job_id.clone()),
                        value: AuditValue::Plain(format!(
                            "approval_id={},reason={},confirmed_high_risk={},target_count={}",
                            record.id,
                            record.reason,
                            input.confirmed_high_risk,
                            targets.len()
                        )),
                        occurred_at: input.issued_at,
                    })
                    .map_err(CreateRunbookJobError::Audit)?;
                Some(record)
            } else {
                None
            };
        audit
            .write(AuditEvent {
                category: AuditCategory::Job,
                action: "runbook_job_created".to_owned(),
                actor: AuditActor::new(input.confirmed_by.clone()),
                target: AuditTarget::new(input.job_id),
                value: AuditValue::Plain(format!(
                    "confirmed_high_risk={},confirmed_by={},target_count={}",
                    input.confirmed_high_risk,
                    input.confirmed_by,
                    targets.len()
                )),
                occurred_at: input.issued_at,
            })
            .map_err(CreateRunbookJobError::Audit)?;

        Ok(CreateRunbookJobOutput {
            task,
            targets,
            envelopes,
            approval_request,
        })
    }
}

fn runbook_payload_hash(task: &RunbookExecutionTask, target: &JobTarget, index: usize) -> String {
    format!(
        "runbook:{index}:{}:{}",
        target.agent_id.as_str(),
        task.runbook_document()
    )
}

fn approval_request_to_record(approval: &ApprovalRequest) -> ApprovalRequestRecord {
    ApprovalRequestRecord {
        id: approval.id().as_str().to_owned(),
        job_id: approval.job_id().as_str().to_owned(),
        requester: approval.requester().to_owned(),
        approver: approval.approver().map(str::to_owned),
        reason: approval.reason().to_owned(),
        status: approval.status().as_str().to_owned(),
        expires_at: approval.expires_at(),
        created_at: approval.created_at(),
        decided_at: approval.decided_at(),
    }
}

fn approval_record_to_request(record: &ApprovalRequestRecord) -> Result<ApprovalRequest, JobError> {
    let mut approval = ApprovalRequest::new(
        ApprovalId::new(record.id.clone())?,
        JobId::new(record.job_id.clone())?,
        record.requester.clone(),
        record.reason.clone(),
        record.expires_at,
        record.created_at,
    )?;
    match ApprovalStatus::parse(&record.status).ok_or(JobError::InvalidApprovalStatus)? {
        ApprovalStatus::Pending => {}
        ApprovalStatus::Approved => approval.approve(
            record
                .approver
                .clone()
                .unwrap_or_else(|| "unknown".to_owned()),
            record.reason.clone(),
            record.decided_at.unwrap_or(record.created_at),
        )?,
        ApprovalStatus::Rejected => approval.reject(
            record
                .approver
                .clone()
                .unwrap_or_else(|| "unknown".to_owned()),
            record.reason.clone(),
            record.decided_at.unwrap_or(record.created_at),
        )?,
        ApprovalStatus::Expired => {
            approval.expire(record.decided_at.unwrap_or(record.expires_at))?
        }
        ApprovalStatus::Canceled => {
            approval.cancel(record.decided_at.unwrap_or(record.created_at))?
        }
    }
    Ok(approval)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApproveApprovalInput {
    pub approval_id: String,
    pub approver: String,
    pub reason: String,
    pub occurred_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectApprovalInput {
    pub approval_id: String,
    pub approver: String,
    pub reason: String,
    pub occurred_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpireApprovalsInput {
    pub occurred_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalWorkflowOutput {
    pub approval: ApprovalRequestRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpireApprovalsOutput {
    pub expired: Vec<ApprovalRequestRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalUseCaseError<RepoError, AuditError> {
    Domain(JobError),
    NotFound,
    Repository(RepoError),
    Audit(AuditError),
}

impl<RepoError, AuditError> Display for ApprovalUseCaseError<RepoError, AuditError>
where
    RepoError: Display,
    AuditError: Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Domain(error) => write!(formatter, "{error}"),
            Self::NotFound => write!(formatter, "approval request not found"),
            Self::Repository(error) => write!(formatter, "repository error: {error}"),
            Self::Audit(error) => write!(formatter, "audit error: {error}"),
        }
    }
}

pub struct ListApprovalRequests;

impl ListApprovalRequests {
    pub fn execute<R>(
        repo: &R,
        status: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ApprovalRequestRecord>, R::Error>
    where
        R: ApprovalRepository,
    {
        repo.list_approval_requests(status, limit)
    }
}

pub struct ApproveApprovalRequest;

impl ApproveApprovalRequest {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: ApproveApprovalInput,
    ) -> Result<ApprovalWorkflowOutput, ApprovalUseCaseError<R::Error, A::Error>>
    where
        R: ApprovalRepository,
        A: AuditWriter,
    {
        let Some(record) = repo
            .find_approval_request(&input.approval_id)
            .map_err(ApprovalUseCaseError::Repository)?
        else {
            return Err(ApprovalUseCaseError::NotFound);
        };
        let mut approval =
            approval_record_to_request(&record).map_err(ApprovalUseCaseError::Domain)?;
        approval
            .approve(
                input.approver.clone(),
                input.reason.clone(),
                input.occurred_at,
            )
            .map_err(ApprovalUseCaseError::Domain)?;
        let updated = approval_request_to_record(&approval);
        repo.update_approval_request(updated.clone())
            .map_err(ApprovalUseCaseError::Repository)?;
        repo.update_job_status_for_approval(&updated.job_id, JobStatus::Queued)
            .map_err(ApprovalUseCaseError::Repository)?;
        audit
            .write(AuditEvent {
                category: AuditCategory::Approval,
                action: "approval_approved".to_owned(),
                actor: AuditActor::new(input.approver),
                target: AuditTarget::new(updated.job_id.clone()),
                value: AuditValue::Plain(format!(
                    "approval_id={},reason={}",
                    updated.id, updated.reason
                )),
                occurred_at: input.occurred_at,
            })
            .map_err(ApprovalUseCaseError::Audit)?;
        Ok(ApprovalWorkflowOutput { approval: updated })
    }
}

pub struct RejectApprovalRequest;

impl RejectApprovalRequest {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: RejectApprovalInput,
    ) -> Result<ApprovalWorkflowOutput, ApprovalUseCaseError<R::Error, A::Error>>
    where
        R: ApprovalRepository,
        A: AuditWriter,
    {
        let Some(record) = repo
            .find_approval_request(&input.approval_id)
            .map_err(ApprovalUseCaseError::Repository)?
        else {
            return Err(ApprovalUseCaseError::NotFound);
        };
        let mut approval =
            approval_record_to_request(&record).map_err(ApprovalUseCaseError::Domain)?;
        approval
            .reject(
                input.approver.clone(),
                input.reason.clone(),
                input.occurred_at,
            )
            .map_err(ApprovalUseCaseError::Domain)?;
        let updated = approval_request_to_record(&approval);
        repo.update_approval_request(updated.clone())
            .map_err(ApprovalUseCaseError::Repository)?;
        repo.update_job_status_for_approval(&updated.job_id, JobStatus::Failed)
            .map_err(ApprovalUseCaseError::Repository)?;
        audit
            .write(AuditEvent {
                category: AuditCategory::Approval,
                action: "approval_rejected".to_owned(),
                actor: AuditActor::new(input.approver),
                target: AuditTarget::new(updated.job_id.clone()),
                value: AuditValue::Plain(format!(
                    "approval_id={},reason={}",
                    updated.id, updated.reason
                )),
                occurred_at: input.occurred_at,
            })
            .map_err(ApprovalUseCaseError::Audit)?;
        Ok(ApprovalWorkflowOutput { approval: updated })
    }
}

pub struct ExpireApprovalRequests;

impl ExpireApprovalRequests {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: ExpireApprovalsInput,
    ) -> Result<ExpireApprovalsOutput, ApprovalUseCaseError<R::Error, A::Error>>
    where
        R: ApprovalRepository,
        A: AuditWriter,
    {
        let pending = repo
            .list_approval_requests(Some("pending"), 500)
            .map_err(ApprovalUseCaseError::Repository)?;
        let mut expired = Vec::new();
        for record in pending
            .into_iter()
            .filter(|record| record.expires_at <= input.occurred_at)
        {
            let mut approval =
                approval_record_to_request(&record).map_err(ApprovalUseCaseError::Domain)?;
            approval
                .expire(input.occurred_at)
                .map_err(ApprovalUseCaseError::Domain)?;
            let updated = approval_request_to_record(&approval);
            repo.update_approval_request(updated.clone())
                .map_err(ApprovalUseCaseError::Repository)?;
            repo.update_job_status_for_approval(&updated.job_id, JobStatus::Expired)
                .map_err(ApprovalUseCaseError::Repository)?;
            audit
                .write(AuditEvent {
                    category: AuditCategory::Approval,
                    action: "approval_expired".to_owned(),
                    actor: AuditActor::new("controller"),
                    target: AuditTarget::new(updated.job_id.clone()),
                    value: AuditValue::Plain(format!("approval_id={}", updated.id)),
                    occurred_at: input.occurred_at,
                })
                .map_err(ApprovalUseCaseError::Audit)?;
            expired.push(updated);
        }
        Ok(ExpireApprovalsOutput { expired })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateEnrollmentTokenInput {
    pub id: String,
    pub token_hash: String,
    pub default_labels: String,
    pub expires_at: SystemTime,
    pub max_uses: u32,
    pub actor: String,
    pub occurred_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateEnrollmentTokenOutput {
    pub id: String,
    pub expires_at: SystemTime,
    pub max_uses: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnrollmentTokenUseCaseError<RepoError, AuditError> {
    Repository(RepoError),
    Audit(AuditError),
}

impl<RepoError, AuditError> Display for EnrollmentTokenUseCaseError<RepoError, AuditError>
where
    RepoError: Display,
    AuditError: Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Repository(error) => write!(formatter, "repository error: {error}"),
            Self::Audit(error) => write!(formatter, "audit error: {error}"),
        }
    }
}

pub type CreateEnrollmentTokenResult<R, A> = Result<
    CreateEnrollmentTokenOutput,
    EnrollmentTokenUseCaseError<<R as EnrollmentTokenRepository>::Error, <A as AuditWriter>::Error>,
>;

pub struct CreateEnrollmentToken;

impl CreateEnrollmentToken {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: CreateEnrollmentTokenInput,
    ) -> CreateEnrollmentTokenResult<R, A>
    where
        R: EnrollmentTokenRepository,
        A: AuditWriter,
    {
        repo.insert_enrollment_token_hash(
            &input.id,
            &input.token_hash,
            &input.default_labels,
            input.expires_at,
            input.max_uses,
        )
        .map_err(EnrollmentTokenUseCaseError::Repository)?;

        audit
            .write(AuditEvent {
                category: AuditCategory::Enrollment,
                action: "enrollment_token_created".to_owned(),
                actor: AuditActor::new(input.actor),
                target: AuditTarget::new(input.id.clone()),
                value: AuditValue::SecretRef(input.id.clone()),
                occurred_at: input.occurred_at,
            })
            .map_err(EnrollmentTokenUseCaseError::Audit)?;

        Ok(CreateEnrollmentTokenOutput {
            id: input.id,
            expires_at: input.expires_at,
            max_uses: input.max_uses,
        })
    }
}

pub struct ListEnrollmentTokens;

impl ListEnrollmentTokens {
    pub fn execute<R>(repo: &R) -> Result<Vec<EnrollmentTokenRecord>, R::Error>
    where
        R: EnrollmentTokenRepository,
    {
        repo.list_enrollment_tokens()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokeEnrollmentTokenInput {
    pub id: String,
    pub actor: String,
    pub occurred_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokeEnrollmentTokenOutput {
    pub revoked: bool,
}

pub type RevokeEnrollmentTokenResult<R, A> = Result<
    RevokeEnrollmentTokenOutput,
    EnrollmentTokenUseCaseError<<R as EnrollmentTokenRepository>::Error, <A as AuditWriter>::Error>,
>;

pub struct RevokeEnrollmentToken;

impl RevokeEnrollmentToken {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: RevokeEnrollmentTokenInput,
    ) -> RevokeEnrollmentTokenResult<R, A>
    where
        R: EnrollmentTokenRepository,
        A: AuditWriter,
    {
        let revoked = repo
            .revoke_enrollment_token(&input.id)
            .map_err(EnrollmentTokenUseCaseError::Repository)?;
        if revoked {
            audit
                .write(AuditEvent {
                    category: AuditCategory::Enrollment,
                    action: "enrollment_token_revoked".to_owned(),
                    actor: AuditActor::new(input.actor),
                    target: AuditTarget::new(input.id.clone()),
                    value: AuditValue::SecretRef(input.id),
                    occurred_at: input.occurred_at,
                })
                .map_err(EnrollmentTokenUseCaseError::Audit)?;
        }
        Ok(RevokeEnrollmentTokenOutput { revoked })
    }
}

pub fn select_agents(agents: &[Agent], selector: &Selector) -> Vec<Agent> {
    agents
        .iter()
        .filter(|agent| selector.matches(agent))
        .cloned()
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchTargetSelection {
    pub targets: Vec<Agent>,
    pub matched_count: usize,
    pub disabled_count: usize,
    pub offline_count: usize,
}

pub fn select_dispatch_targets(agents: &[Agent], selector: &Selector) -> DispatchTargetSelection {
    let matched = select_agents(agents, selector);
    let disabled_count = matched
        .iter()
        .filter(|agent| agent.status() == AgentStatus::Disabled)
        .count();
    let offline_count = matched
        .iter()
        .filter(|agent| agent.status() == AgentStatus::Offline)
        .count();
    let targets = matched
        .iter()
        .filter(|agent| agent.status() != AgentStatus::Disabled)
        .cloned()
        .collect();

    DispatchTargetSelection {
        targets,
        matched_count: matched.len(),
        disabled_count,
        offline_count,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectorPreviewInput {
    pub selector: Selector,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectorPreviewAgentRecord {
    pub agent_id: String,
    pub name: String,
    pub status: String,
    pub labels: Vec<(String, String)>,
    pub selected_for_dispatch: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectorPreviewWarningRecord {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectorPreviewOutput {
    pub matched_count: usize,
    pub selected_count: usize,
    pub disabled_count: usize,
    pub offline_count: usize,
    pub agents: Vec<SelectorPreviewAgentRecord>,
    pub warnings: Vec<SelectorPreviewWarningRecord>,
}

pub struct PreviewSelector;

impl PreviewSelector {
    pub fn execute<R>(
        repo: &R,
        input: SelectorPreviewInput,
    ) -> Result<SelectorPreviewOutput, R::Error>
    where
        R: AgentInventoryRepository,
    {
        let agents = ListInventoryAgents::execute(repo)?;
        let matched = select_agents(&agents, &input.selector);
        let selection = select_dispatch_targets(&agents, &input.selector);
        let selected_agent_ids = selection
            .targets
            .iter()
            .map(|agent| agent.id().as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let agents = matched
            .into_iter()
            .map(|agent| SelectorPreviewAgentRecord {
                agent_id: agent.id().as_str().to_owned(),
                name: agent.name().as_str().to_owned(),
                status: agent_status_to_str(agent.status()).to_owned(),
                labels: agent
                    .labels()
                    .iter()
                    .map(|label| (label.key().to_owned(), label.value().to_owned()))
                    .collect(),
                selected_for_dispatch: selected_agent_ids.contains(agent.id().as_str()),
            })
            .collect::<Vec<_>>();
        let mut warnings = Vec::new();
        if selection.disabled_count > 0 {
            warnings.push(SelectorPreviewWarningRecord {
                code: "disabled_agents_excluded".to_owned(),
                message: format!(
                    "{} disabled or revoked agent(s) match the selector but will be excluded",
                    selection.disabled_count
                ),
            });
        }
        if selection.offline_count > 0 {
            warnings.push(SelectorPreviewWarningRecord {
                code: "offline_agents_will_queue".to_owned(),
                message: format!(
                    "{} offline agent(s) match the selector and will remain queued until connected",
                    selection.offline_count
                ),
            });
        }
        if selection.targets.is_empty() {
            warnings.push(SelectorPreviewWarningRecord {
                code: "no_dispatch_targets".to_owned(),
                message: "selector did not produce any dispatchable agents".to_owned(),
            });
        }

        Ok(SelectorPreviewOutput {
            matched_count: selection.matched_count,
            selected_count: selection.targets.len(),
            disabled_count: selection.disabled_count,
            offline_count: selection.offline_count,
            agents,
            warnings,
        })
    }
}

fn agent_status_to_str(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Pending => "pending",
        AgentStatus::Online => "online",
        AgentStatus::Busy => "busy",
        AgentStatus::Degraded => "degraded",
        AgentStatus::Offline => "offline",
        AgentStatus::Disabled => "disabled",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingTaskAssignment {
    pub envelope: TaskEnvelope,
    pub task: TaskKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobDispatchGate {
    pub concurrency: usize,
    pub max_failures: Option<u32>,
    pub active_count: usize,
    pub failure_count: usize,
}

impl JobDispatchGate {
    pub fn allows_dispatch(self, local_dispatched_count: usize) -> bool {
        self.active_count + local_dispatched_count < self.concurrency.max(1)
    }

    pub fn max_failures_reached(self) -> bool {
        matches!(self.max_failures, Some(limit) if limit > 0 && self.failure_count >= limit as usize)
    }
}

pub trait DispatchAssignmentRepository {
    type Error;

    fn list_pending_assignments(
        &self,
        agent_id: Option<&AgentId>,
        job_id: Option<&JobId>,
        limit: usize,
    ) -> Result<Vec<PendingTaskAssignment>, Self::Error>;
    fn find_dispatch_agent(&self, agent_id: &AgentId) -> Result<Option<Agent>, Self::Error>;
    fn dispatch_gate(&self, job_id: &JobId) -> Result<JobDispatchGate, Self::Error>;
    fn mark_assignment_dispatched(
        &mut self,
        task_id: &TaskId,
        now: SystemTime,
    ) -> Result<(), Self::Error>;
    fn mark_job_running(&mut self, job_id: &JobId, now: SystemTime) -> Result<(), Self::Error>;
    fn mark_job_expired(&mut self, job_id: &JobId, now: SystemTime) -> Result<(), Self::Error>;
}

pub trait PendingAssignmentDispatcher {
    type Error;

    fn has_active_session(&self, agent_id: &AgentId) -> bool;
    fn dispatch(&mut self, assignment: &PendingTaskAssignment) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchPendingAssignmentsInput {
    pub agent_id: Option<AgentId>,
    pub job_id: Option<JobId>,
    pub now: SystemTime,
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DispatchPendingAssignmentsOutput {
    pub dispatched_count: usize,
    pub queued_count: usize,
    pub skipped_expired_count: usize,
    pub skipped_disabled_count: usize,
    pub skipped_concurrency_count: usize,
    pub skipped_max_failures_count: usize,
    pub failed_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchPendingAssignmentsError<RepoError, AuditError> {
    Repository(RepoError),
    Audit(AuditError),
}

impl<RepoError, AuditError> Display for DispatchPendingAssignmentsError<RepoError, AuditError>
where
    RepoError: Display,
    AuditError: Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Repository(error) => write!(formatter, "repository error: {error}"),
            Self::Audit(error) => write!(formatter, "audit error: {error}"),
        }
    }
}

pub struct DispatchPendingAssignments;

impl DispatchPendingAssignments {
    pub fn execute<R, D, A>(
        repo: &mut R,
        dispatcher: &mut D,
        audit: &mut A,
        input: DispatchPendingAssignmentsInput,
    ) -> Result<DispatchPendingAssignmentsOutput, DispatchPendingAssignmentsError<R::Error, A::Error>>
    where
        R: DispatchAssignmentRepository,
        D: PendingAssignmentDispatcher,
        D::Error: Display,
        A: AuditWriter,
    {
        let assignments = repo
            .list_pending_assignments(input.agent_id.as_ref(), input.job_id.as_ref(), input.limit)
            .map_err(DispatchPendingAssignmentsError::Repository)?;
        let mut output = DispatchPendingAssignmentsOutput::default();
        let mut local_dispatched_by_job = BTreeMap::<String, usize>::new();
        let mut gate_by_job = BTreeMap::<String, JobDispatchGate>::new();

        for assignment in assignments {
            if assignment.envelope.expires_at.is_expired_at(input.now) {
                repo.mark_job_expired(&assignment.envelope.job_id, input.now)
                    .map_err(DispatchPendingAssignmentsError::Repository)?;
                output.skipped_expired_count += 1;
                continue;
            }

            let gate = if let Some(gate) = gate_by_job.get(assignment.envelope.job_id.as_str()) {
                *gate
            } else {
                let gate = repo
                    .dispatch_gate(&assignment.envelope.job_id)
                    .map_err(DispatchPendingAssignmentsError::Repository)?;
                gate_by_job.insert(assignment.envelope.job_id.as_str().to_owned(), gate);
                gate
            };
            if gate.max_failures_reached() {
                output.skipped_max_failures_count += 1;
                output.queued_count += 1;
                continue;
            }
            let local_dispatched_count = local_dispatched_by_job
                .get(assignment.envelope.job_id.as_str())
                .copied()
                .unwrap_or_default();
            if !gate.allows_dispatch(local_dispatched_count) {
                output.skipped_concurrency_count += 1;
                output.queued_count += 1;
                continue;
            }

            let Some(agent) = repo
                .find_dispatch_agent(&assignment.envelope.target_agent_id)
                .map_err(DispatchPendingAssignmentsError::Repository)?
            else {
                output.skipped_disabled_count += 1;
                continue;
            };

            if agent.status() == AgentStatus::Disabled {
                output.skipped_disabled_count += 1;
                continue;
            }

            if !dispatcher.has_active_session(&assignment.envelope.target_agent_id) {
                output.queued_count += 1;
                continue;
            }

            match dispatcher.dispatch(&assignment) {
                Ok(()) => {
                    repo.mark_assignment_dispatched(&assignment.envelope.task_id, input.now)
                        .map_err(DispatchPendingAssignmentsError::Repository)?;
                    repo.mark_job_running(&assignment.envelope.job_id, input.now)
                        .map_err(DispatchPendingAssignmentsError::Repository)?;
                    *local_dispatched_by_job
                        .entry(assignment.envelope.job_id.as_str().to_owned())
                        .or_default() += 1;
                    let dispatch_latency_ms = input
                        .now
                        .duration_since(assignment.envelope.issued_at)
                        .unwrap_or_default()
                        .as_millis();
                    audit
                        .write(dispatch_audit_event(
                            "task_dispatched",
                            &assignment,
                            AuditValue::Plain(format!(
                                "agent_id={},task_id={},assignment_status=dispatched,dispatch_state=delivered,dispatch_latency_ms={},active_session=true",
                                assignment.envelope.target_agent_id.as_str(),
                                assignment.envelope.task_id.as_str(),
                                dispatch_latency_ms
                            )),
                            input.now,
                        ))
                        .map_err(DispatchPendingAssignmentsError::Audit)?;
                    output.dispatched_count += 1;
                }
                Err(error) => {
                    let dispatch_latency_ms = input
                        .now
                        .duration_since(assignment.envelope.issued_at)
                        .unwrap_or_default()
                        .as_millis();
                    audit
                        .write(dispatch_audit_event(
                            "task_dispatch_failed",
                            &assignment,
                            AuditValue::Plain(format!(
                                "agent_id={},task_id={},dispatch_state=queued,dispatch_latency_ms={},active_session=true,failure_reason={}",
                                assignment.envelope.target_agent_id.as_str(),
                                assignment.envelope.task_id.as_str(),
                                dispatch_latency_ms,
                                error
                            )),
                            input.now,
                        ))
                        .map_err(DispatchPendingAssignmentsError::Audit)?;
                    output.failed_count += 1;
                }
            }
        }

        Ok(output)
    }
}

fn dispatch_audit_event(
    action: &str,
    assignment: &PendingTaskAssignment,
    value: AuditValue,
    occurred_at: SystemTime,
) -> AuditEvent {
    AuditEvent {
        category: AuditCategory::Job,
        action: action.to_owned(),
        actor: AuditActor::new("controller"),
        target: AuditTarget::new(assignment.envelope.job_id.as_str().to_owned()),
        value,
        occurred_at,
    }
}

pub struct EnrollAgentInput {
    pub agent: Agent,
}

pub struct EnrollAgent;

impl EnrollAgent {
    pub fn execute<R>(repo: &mut R, input: EnrollAgentInput) -> Result<(), R::Error>
    where
        R: AgentRepository,
    {
        repo.save(input.agent)
    }
}

pub struct ListInventoryAgents;

impl ListInventoryAgents {
    pub fn execute<R>(repo: &R) -> Result<Vec<Agent>, R::Error>
    where
        R: AgentInventoryRepository,
    {
        repo.list_agents()
    }
}

pub struct GetInventoryAgent;

impl GetInventoryAgent {
    pub fn execute<R>(repo: &R, agent_id: AgentId) -> Result<Option<Agent>, R::Error>
    where
        R: AgentInventoryRepository,
    {
        repo.find_agent_by_id(&agent_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateAgentLabelsInput {
    pub agent_id: String,
    pub labels: Vec<AgentLabel>,
    pub actor: String,
    pub occurred_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateAgentLabelsError<RepoError, AuditError> {
    Agent(AgentError),
    Repository(RepoError),
    Audit(AuditError),
}

impl<RepoError, AuditError> Display for UpdateAgentLabelsError<RepoError, AuditError>
where
    RepoError: Display,
    AuditError: Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Agent(error) => write!(formatter, "{error}"),
            Self::Repository(error) => write!(formatter, "repository error: {error}"),
            Self::Audit(error) => write!(formatter, "audit error: {error}"),
        }
    }
}

pub type UpdateAgentLabelsResult<R, A> = Result<
    Option<Agent>,
    UpdateAgentLabelsError<<R as AgentInventoryRepository>::Error, <A as AuditWriter>::Error>,
>;

pub struct UpdateAgentLabels;

impl UpdateAgentLabels {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: UpdateAgentLabelsInput,
    ) -> UpdateAgentLabelsResult<R, A>
    where
        R: AgentInventoryRepository,
        A: AuditWriter,
    {
        let agent_id =
            AgentId::new(input.agent_id.clone()).map_err(UpdateAgentLabelsError::Agent)?;
        let changed = repo
            .update_agent_labels(&agent_id, &input.labels)
            .map_err(UpdateAgentLabelsError::Repository)?;
        if !changed {
            return Ok(None);
        }

        audit
            .write(AuditEvent {
                category: AuditCategory::Agent,
                action: "agent_labels_updated".to_owned(),
                actor: AuditActor::new(input.actor),
                target: AuditTarget::new(input.agent_id),
                value: AuditValue::Plain(format!("label_count={}", input.labels.len())),
                occurred_at: input.occurred_at,
            })
            .map_err(UpdateAgentLabelsError::Audit)?;

        repo.find_agent_by_id(&agent_id)
            .map_err(UpdateAgentLabelsError::Repository)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokeAgentKeyInput {
    pub agent_id: String,
    pub actor: String,
    pub occurred_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevokeAgentKeyError<RepoError, AuditError> {
    Agent(AgentError),
    Repository(RepoError),
    Audit(AuditError),
}

impl<RepoError, AuditError> Display for RevokeAgentKeyError<RepoError, AuditError>
where
    RepoError: Display,
    AuditError: Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Agent(error) => write!(formatter, "{error}"),
            Self::Repository(error) => write!(formatter, "repository error: {error}"),
            Self::Audit(error) => write!(formatter, "audit error: {error}"),
        }
    }
}

pub type RevokeAgentKeyResult<R, A> = Result<
    Option<Agent>,
    RevokeAgentKeyError<<R as AgentInventoryRepository>::Error, <A as AuditWriter>::Error>,
>;

pub struct RevokeAgentKey;

impl RevokeAgentKey {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: RevokeAgentKeyInput,
    ) -> RevokeAgentKeyResult<R, A>
    where
        R: AgentInventoryRepository,
        A: AuditWriter,
    {
        let agent_id = AgentId::new(input.agent_id.clone()).map_err(RevokeAgentKeyError::Agent)?;
        let Some(agent) = repo
            .find_agent_by_id(&agent_id)
            .map_err(RevokeAgentKeyError::Repository)?
        else {
            return Ok(None);
        };

        if agent.status() == AgentStatus::Disabled {
            return Ok(Some(agent));
        }

        repo.revoke_agent_key(&agent_id)
            .map_err(RevokeAgentKeyError::Repository)?;
        audit
            .write(AuditEvent {
                category: AuditCategory::Agent,
                action: "agent_key_revoked".to_owned(),
                actor: AuditActor::new(input.actor),
                target: AuditTarget::new(input.agent_id),
                value: AuditValue::Plain("status=revoked".to_owned()),
                occurred_at: input.occurred_at,
            })
            .map_err(RevokeAgentKeyError::Audit)?;

        repo.find_agent_by_id(&agent_id)
            .map_err(RevokeAgentKeyError::Repository)
    }
}

pub struct EnsureAdminToken;

impl EnsureAdminToken {
    pub fn execute<R>(repo: &mut R, token_hash: &str) -> Result<bool, R::Error>
    where
        R: AdminTokenRepository,
    {
        if repo.admin_token_exists()? {
            return Ok(false);
        }
        repo.insert_admin_token_hash(token_hash)?;
        Ok(true)
    }
}

pub struct VerifyAdminToken;

impl VerifyAdminToken {
    pub fn execute<R>(repo: &R, token_hash: &str) -> Result<bool, R::Error>
    where
        R: AdminTokenRepository,
    {
        repo.verify_admin_token_hash(token_hash)
    }
}

pub struct AuthenticateAdminToken;

impl AuthenticateAdminToken {
    pub fn execute<R>(repo: &R, token_hash: &str) -> Result<Option<AdminTokenRecord>, R::Error>
    where
        R: AdminTokenRepository,
    {
        repo.find_admin_token_record(token_hash)
    }
}

pub struct ListJobSummaries;

impl ListJobSummaries {
    pub fn execute<R>(repo: &R, limit: usize) -> Result<Vec<JobSummaryRecord>, R::Error>
    where
        R: JobQueryRepository,
    {
        repo.list_job_summaries(limit)
    }
}

pub struct GetJobSummary;

impl GetJobSummary {
    pub fn execute<R>(repo: &R, job_id: &str) -> Result<Option<JobSummaryRecord>, R::Error>
    where
        R: JobQueryRepository,
    {
        repo.find_job_summary(job_id)
    }
}

pub struct ListJobOutputForJob;

impl ListJobOutputForJob {
    pub fn execute<R>(repo: &R, job_id: &str) -> Result<Vec<JobOutputChunk>, R::Error>
    where
        R: JobOutputRepository,
    {
        repo.list_output_chunks_for_job(job_id)
    }
}

pub struct GetLatestFacts;

impl GetLatestFacts {
    pub fn execute<R>(repo: &R, agent_id: &str) -> Result<Option<FactsSnapshotRecord>, R::Error>
    where
        R: FactsRepository,
    {
        repo.latest_facts_snapshot(agent_id)
    }
}

pub struct ListFactsSnapshots;

impl ListFactsSnapshots {
    pub fn execute<R>(
        repo: &R,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<FactsSnapshotPageRecord>, R::Error>
    where
        R: FactsRepository,
    {
        repo.list_facts_snapshots(agent_id, limit, before)
    }
}

pub struct GetLatestMetrics;

impl GetLatestMetrics {
    pub fn execute<R>(repo: &R, agent_id: &str) -> Result<Option<MetricsSnapshotRecord>, R::Error>
    where
        R: MetricsRepository,
    {
        repo.latest_metrics_snapshot(agent_id)
    }
}

pub struct ListMetricsSnapshots;

impl ListMetricsSnapshots {
    pub fn execute<R>(
        repo: &R,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<MetricsSnapshotPageRecord>, R::Error>
    where
        R: MetricsRepository,
    {
        repo.list_metrics_snapshots(agent_id, limit, before)
    }
}

pub struct GetLatestDrift;

impl GetLatestDrift {
    pub fn execute<R>(repo: &R, agent_id: &str) -> Result<Option<DriftReportRecord>, R::Error>
    where
        R: DriftRepository,
    {
        repo.latest_drift_report(agent_id)
    }
}

pub struct ListDriftReports;

impl ListDriftReports {
    pub fn execute<R>(
        repo: &R,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<DriftReportPageRecord>, R::Error>
    where
        R: DriftRepository,
    {
        repo.list_drift_reports(agent_id, limit, before)
    }
}

pub struct ListAuditEvents;

impl ListAuditEvents {
    pub fn execute<R>(repo: &R, limit: usize) -> Result<Vec<AuditEvent>, R::Error>
    where
        R: AuditRepository,
    {
        repo.list(limit)
    }
}

pub fn application_layer_name() -> &'static str {
    fleet_domain::DOMAIN_LAYER
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_domain::{AgentFingerprint, AgentIdentity, AgentLabel, AgentName, AgentPublicKey};
    use std::convert::Infallible;

    fn agent(id: &str, role: &str) -> Agent {
        let mut agent = Agent::new(
            AgentId::new(id).unwrap(),
            AgentName::new(id).unwrap(),
            AgentIdentity {
                public_key: AgentPublicKey::new("pk").unwrap(),
                fingerprint: AgentFingerprint::new("0123456789abcdef").unwrap(),
            },
        );
        agent.set_labels(vec![AgentLabel::new("role", role).unwrap()]);
        agent
    }

    #[test]
    fn selector_filters_agents() {
        let agents = vec![agent("web-01", "web"), agent("db-01", "db")];
        let selected = select_agents(&agents, &Selector::parse("role=web").unwrap());
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name().as_str(), "web-01");
    }

    #[test]
    fn dispatch_selector_excludes_disabled_agents() {
        let mut disabled = agent("web-02", "web");
        disabled.disable();
        let agents = vec![agent("web-01", "web"), disabled];

        let selected = select_dispatch_targets(&agents, &Selector::parse("role=web").unwrap());

        assert_eq!(selected.matched_count, 2);
        assert_eq!(selected.disabled_count, 1);
        assert_eq!(selected.targets.len(), 1);
        assert_eq!(selected.targets[0].id().as_str(), "web-01");
    }

    #[test]
    fn dispatch_selector_allows_offline_agents_to_remain_queued() {
        let mut offline = agent("web-02", "web");
        offline.mark_online(SystemTime::UNIX_EPOCH).unwrap();
        offline.mark_offline();

        let selected = select_dispatch_targets(&[offline], &Selector::parse("role=web").unwrap());

        assert_eq!(selected.matched_count, 1);
        assert_eq!(selected.offline_count, 1);
        assert_eq!(selected.targets.len(), 1);
        assert_eq!(selected.targets[0].status(), AgentStatus::Offline);
    }

    #[test]
    fn dispatch_selector_matches_multiple_labels() {
        let mut web_prod = agent("web-01", "web");
        web_prod.set_labels(vec![
            AgentLabel::new("role", "web").unwrap(),
            AgentLabel::new("env", "prod").unwrap(),
        ]);
        let mut web_dev = agent("web-02", "web");
        web_dev.set_labels(vec![
            AgentLabel::new("role", "web").unwrap(),
            AgentLabel::new("env", "dev").unwrap(),
        ]);

        let selected = select_dispatch_targets(
            &[web_prod, web_dev],
            &Selector::parse("role=web,env=prod").unwrap(),
        );

        assert_eq!(selected.targets.len(), 1);
        assert_eq!(selected.targets[0].id().as_str(), "web-01");
    }

    #[test]
    fn selector_preview_reports_selected_disabled_and_offline_agents() {
        let online = agent("web-01", "web");
        let mut offline = agent("web-02", "web");
        offline.mark_online(SystemTime::UNIX_EPOCH).unwrap();
        offline.mark_offline();
        let mut disabled = agent("web-03", "web");
        disabled.disable();
        let repo = FakeAgentInventoryRepository {
            agents: vec![online, offline, disabled],
        };

        let preview = PreviewSelector::execute(
            &repo,
            SelectorPreviewInput {
                selector: Selector::parse("label:role=web").unwrap(),
            },
        )
        .unwrap();

        assert_eq!(preview.matched_count, 3);
        assert_eq!(preview.selected_count, 2);
        assert_eq!(preview.disabled_count, 1);
        assert_eq!(preview.offline_count, 1);
        assert_eq!(preview.agents.len(), 3);
        assert!(preview.agents[0].selected_for_dispatch);
        assert!(preview.agents[1].selected_for_dispatch);
        assert!(!preview.agents[2].selected_for_dispatch);
        assert_eq!(
            preview
                .warnings
                .iter()
                .map(|warning| warning.code.as_str())
                .collect::<Vec<_>>(),
            vec!["disabled_agents_excluded", "offline_agents_will_queue"]
        );
    }

    #[test]
    fn dispatch_pending_assignments_sends_connected_agent_and_marks_running() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut repo = FakeDispatchAssignmentRepository {
            agents: vec![agent("web-01", "web")],
            assignments: vec![pending_assignment(
                "job-1",
                "task-1",
                "web-01",
                now + Duration::from_secs(60),
            )],
            ..Default::default()
        };
        let mut dispatcher = FakePendingAssignmentDispatcher {
            active_agent_ids: vec!["web-01".to_owned()],
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();

        let output = DispatchPendingAssignments::execute(
            &mut repo,
            &mut dispatcher,
            &mut audit,
            DispatchPendingAssignmentsInput {
                agent_id: Some(AgentId::new("web-01").unwrap()),
                job_id: None,
                now,
                limit: 10,
            },
        )
        .unwrap();

        assert_eq!(
            output,
            DispatchPendingAssignmentsOutput {
                dispatched_count: 1,
                ..Default::default()
            }
        );
        assert_eq!(dispatcher.sent_task_ids, vec!["task-1"]);
        assert_eq!(repo.dispatched_assignments, vec!["task-1"]);
        assert_eq!(repo.running_jobs, vec!["job-1"]);
        assert_eq!(audit.events[0].action, "task_dispatched");
        assert!(matches!(
            &audit.events[0].value,
            AuditValue::Plain(value)
                if value.contains("assignment_status=dispatched")
                    && value.contains("dispatch_state=delivered")
                    && value.contains("dispatch_latency_ms=")
                    && value.contains("active_session=true")
        ));
    }

    #[test]
    fn dispatch_pending_assignments_keeps_disconnected_agent_queued() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut repo = FakeDispatchAssignmentRepository {
            agents: vec![agent("web-01", "web")],
            assignments: vec![pending_assignment(
                "job-1",
                "task-1",
                "web-01",
                now + Duration::from_secs(60),
            )],
            ..Default::default()
        };
        let mut dispatcher = FakePendingAssignmentDispatcher::default();
        let mut audit = FakeAuditWriter::default();

        let output = DispatchPendingAssignments::execute(
            &mut repo,
            &mut dispatcher,
            &mut audit,
            DispatchPendingAssignmentsInput {
                agent_id: Some(AgentId::new("web-01").unwrap()),
                job_id: None,
                now,
                limit: 10,
            },
        )
        .unwrap();

        assert_eq!(
            output,
            DispatchPendingAssignmentsOutput {
                queued_count: 1,
                ..Default::default()
            }
        );
        assert!(dispatcher.sent_task_ids.is_empty());
        assert!(repo.running_jobs.is_empty());
        assert!(audit.events.is_empty());
    }

    #[test]
    fn dispatch_pending_assignments_skips_disabled_agents() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut disabled = agent("web-01", "web");
        disabled.disable();
        let mut repo = FakeDispatchAssignmentRepository {
            agents: vec![disabled],
            assignments: vec![pending_assignment(
                "job-1",
                "task-1",
                "web-01",
                now + Duration::from_secs(60),
            )],
            ..Default::default()
        };
        let mut dispatcher = FakePendingAssignmentDispatcher {
            active_agent_ids: vec!["web-01".to_owned()],
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();

        let output = DispatchPendingAssignments::execute(
            &mut repo,
            &mut dispatcher,
            &mut audit,
            DispatchPendingAssignmentsInput {
                agent_id: Some(AgentId::new("web-01").unwrap()),
                job_id: None,
                now,
                limit: 10,
            },
        )
        .unwrap();

        assert_eq!(
            output,
            DispatchPendingAssignmentsOutput {
                skipped_disabled_count: 1,
                ..Default::default()
            }
        );
        assert!(dispatcher.sent_task_ids.is_empty());
        assert!(repo.running_jobs.is_empty());
    }

    #[test]
    fn dispatch_pending_assignments_skips_expired_assignments() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut repo = FakeDispatchAssignmentRepository {
            agents: vec![agent("web-01", "web")],
            assignments: vec![pending_assignment(
                "job-1",
                "task-1",
                "web-01",
                now - Duration::from_secs(1),
            )],
            ..Default::default()
        };
        let mut dispatcher = FakePendingAssignmentDispatcher {
            active_agent_ids: vec!["web-01".to_owned()],
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();

        let output = DispatchPendingAssignments::execute(
            &mut repo,
            &mut dispatcher,
            &mut audit,
            DispatchPendingAssignmentsInput {
                agent_id: Some(AgentId::new("web-01").unwrap()),
                job_id: None,
                now,
                limit: 10,
            },
        )
        .unwrap();

        assert_eq!(
            output,
            DispatchPendingAssignmentsOutput {
                skipped_expired_count: 1,
                ..Default::default()
            }
        );
        assert_eq!(repo.expired_jobs, vec!["job-1"]);
        assert!(dispatcher.sent_task_ids.is_empty());
    }

    #[test]
    fn dispatch_pending_assignments_keeps_queued_after_send_failure_and_audits() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut repo = FakeDispatchAssignmentRepository {
            agents: vec![agent("web-01", "web")],
            assignments: vec![pending_assignment(
                "job-1",
                "task-1",
                "web-01",
                now + Duration::from_secs(60),
            )],
            ..Default::default()
        };
        let mut dispatcher = FakePendingAssignmentDispatcher {
            active_agent_ids: vec!["web-01".to_owned()],
            failed_task_ids: vec!["task-1".to_owned()],
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();

        let output = DispatchPendingAssignments::execute(
            &mut repo,
            &mut dispatcher,
            &mut audit,
            DispatchPendingAssignmentsInput {
                agent_id: Some(AgentId::new("web-01").unwrap()),
                job_id: None,
                now,
                limit: 10,
            },
        )
        .unwrap();

        assert_eq!(
            output,
            DispatchPendingAssignmentsOutput {
                failed_count: 1,
                ..Default::default()
            }
        );
        assert!(repo.running_jobs.is_empty());
        assert_eq!(audit.events[0].action, "task_dispatch_failed");
        assert!(matches!(
            &audit.events[0].value,
            AuditValue::Plain(value)
                if value.contains("dispatch_state=queued")
                    && value.contains("failure_reason=")
        ));
    }

    #[test]
    fn dispatch_pending_assignments_respects_concurrency_one() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut repo = FakeDispatchAssignmentRepository {
            agents: vec![agent("web-01", "web"), agent("web-02", "web")],
            assignments: vec![
                pending_assignment("job-1", "task-1", "web-01", now + Duration::from_secs(60)),
                pending_assignment("job-1", "task-2", "web-02", now + Duration::from_secs(60)),
            ],
            gates: BTreeMap::from([(
                "job-1".to_owned(),
                JobDispatchGate {
                    concurrency: 1,
                    max_failures: None,
                    active_count: 0,
                    failure_count: 0,
                },
            )]),
            ..Default::default()
        };
        let mut dispatcher = FakePendingAssignmentDispatcher {
            active_agent_ids: vec!["web-01".to_owned(), "web-02".to_owned()],
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();

        let output = DispatchPendingAssignments::execute(
            &mut repo,
            &mut dispatcher,
            &mut audit,
            DispatchPendingAssignmentsInput {
                agent_id: None,
                job_id: Some(JobId::new("job-1").unwrap()),
                now,
                limit: 10,
            },
        )
        .unwrap();

        assert_eq!(output.dispatched_count, 1);
        assert_eq!(output.skipped_concurrency_count, 1);
        assert_eq!(output.queued_count, 1);
        assert_eq!(dispatcher.sent_task_ids, vec!["task-1"]);
    }

    #[test]
    fn dispatch_pending_assignments_stops_when_max_failures_reached() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut repo = FakeDispatchAssignmentRepository {
            agents: vec![agent("web-01", "web")],
            assignments: vec![pending_assignment(
                "job-1",
                "task-1",
                "web-01",
                now + Duration::from_secs(60),
            )],
            gates: BTreeMap::from([(
                "job-1".to_owned(),
                JobDispatchGate {
                    concurrency: 10,
                    max_failures: Some(1),
                    active_count: 0,
                    failure_count: 1,
                },
            )]),
            ..Default::default()
        };
        let mut dispatcher = FakePendingAssignmentDispatcher {
            active_agent_ids: vec!["web-01".to_owned()],
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();

        let output = DispatchPendingAssignments::execute(
            &mut repo,
            &mut dispatcher,
            &mut audit,
            DispatchPendingAssignmentsInput {
                agent_id: None,
                job_id: Some(JobId::new("job-1").unwrap()),
                now,
                limit: 10,
            },
        )
        .unwrap();

        assert_eq!(output.dispatched_count, 0);
        assert_eq!(output.skipped_max_failures_count, 1);
        assert_eq!(output.queued_count, 1);
        assert!(dispatcher.sent_task_ids.is_empty());
        assert!(audit.events.is_empty());
    }

    #[test]
    fn command_job_without_timeout_is_rejected() {
        let mut repo = FakeCommandJobRepository::default();
        let mut audit = FakeAuditWriter::default();
        let mut signer = FakeSigner;

        let result = CreateCommandJob::execute(
            &mut repo,
            &mut audit,
            &mut signer,
            command_input(Duration::ZERO, true),
        );

        assert!(matches!(
            result,
            Err(CreateCommandJobError::Domain(JobError::InvalidTimeout))
        ));
    }

    #[test]
    fn high_risk_command_without_confirmation_creates_pending_approval() {
        let mut repo = FakeCommandJobRepository::default();
        let mut audit = FakeAuditWriter::default();
        let mut signer = FakeSigner;
        let mut input = command_input(Duration::from_secs(30), false);
        input.program = "bash".to_owned();

        let output = CreateCommandJob::execute(&mut repo, &mut audit, &mut signer, input).unwrap();

        assert_eq!(repo.approval_requests.len(), 1);
        assert_eq!(repo.approval_requests[0].status, "pending");
        assert_eq!(output.approval_request.unwrap().id, "approval-command");
        assert!(
            audit
                .events
                .iter()
                .any(|event| event.category == AuditCategory::Approval
                    && event.action == "approval_requested")
        );
    }

    #[test]
    fn approve_approval_request_queues_job_and_audits_approver() {
        let mut repo = FakeCommandJobRepository::default();
        repo.approval_requests.push(approval_request_record(
            "approval-1",
            "job-1",
            SystemTime::UNIX_EPOCH + Duration::from_secs(60),
        ));
        let mut audit = FakeAuditWriter::default();

        let output = ApproveApprovalRequest::execute(
            &mut repo,
            &mut audit,
            ApproveApprovalInput {
                approval_id: "approval-1".to_owned(),
                approver: "manager-1".to_owned(),
                reason: "approved maintenance window".to_owned(),
                occurred_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            },
        )
        .unwrap();

        assert_eq!(output.approval.status, "approved");
        assert_eq!(output.approval.approver.as_deref(), Some("manager-1"));
        assert_eq!(
            repo.approval_status_updates,
            vec![("job-1".to_owned(), JobStatus::Queued)]
        );
        assert_eq!(audit.events.len(), 1);
        assert_eq!(audit.events[0].category, AuditCategory::Approval);
        assert_eq!(audit.events[0].action, "approval_approved");
        assert_eq!(audit.events[0].actor.as_str(), "manager-1");
    }

    #[test]
    fn reject_approval_request_fails_job_and_audits_reason() {
        let mut repo = FakeCommandJobRepository::default();
        repo.approval_requests.push(approval_request_record(
            "approval-1",
            "job-1",
            SystemTime::UNIX_EPOCH + Duration::from_secs(60),
        ));
        let mut audit = FakeAuditWriter::default();

        let output = RejectApprovalRequest::execute(
            &mut repo,
            &mut audit,
            RejectApprovalInput {
                approval_id: "approval-1".to_owned(),
                approver: "manager-1".to_owned(),
                reason: "outside maintenance window".to_owned(),
                occurred_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            },
        )
        .unwrap();

        assert_eq!(output.approval.status, "rejected");
        assert_eq!(output.approval.reason, "outside maintenance window");
        assert_eq!(
            repo.approval_status_updates,
            vec![("job-1".to_owned(), JobStatus::Failed)]
        );
        assert_eq!(audit.events.len(), 1);
        assert_eq!(audit.events[0].category, AuditCategory::Approval);
        assert_eq!(audit.events[0].action, "approval_rejected");
        assert!(matches!(
            &audit.events[0].value,
            AuditValue::Plain(value) if value.contains("approval_id=approval-1")
        ));
    }

    #[test]
    fn expire_approval_requests_expires_due_jobs_only() {
        let mut repo = FakeCommandJobRepository::default();
        repo.approval_requests.push(approval_request_record(
            "approval-due",
            "job-due",
            SystemTime::UNIX_EPOCH + Duration::from_secs(30),
        ));
        repo.approval_requests.push(approval_request_record(
            "approval-future",
            "job-future",
            SystemTime::UNIX_EPOCH + Duration::from_secs(90),
        ));
        let mut audit = FakeAuditWriter::default();

        let output = ExpireApprovalRequests::execute(
            &mut repo,
            &mut audit,
            ExpireApprovalsInput {
                occurred_at: SystemTime::UNIX_EPOCH + Duration::from_secs(30),
            },
        )
        .unwrap();

        assert_eq!(output.expired.len(), 1);
        assert_eq!(output.expired[0].id, "approval-due");
        assert_eq!(output.expired[0].status, "expired");
        assert_eq!(
            repo.approval_status_updates,
            vec![("job-due".to_owned(), JobStatus::Expired)]
        );
        assert_eq!(audit.events.len(), 1);
        assert_eq!(audit.events[0].category, AuditCategory::Approval);
        assert_eq!(audit.events[0].action, "approval_expired");
    }

    #[test]
    fn expired_approval_request_cannot_be_approved() {
        let mut repo = FakeCommandJobRepository::default();
        repo.approval_requests.push(approval_request_record(
            "approval-1",
            "job-1",
            SystemTime::UNIX_EPOCH + Duration::from_secs(1),
        ));
        let mut audit = FakeAuditWriter::default();

        let result = ApproveApprovalRequest::execute(
            &mut repo,
            &mut audit,
            ApproveApprovalInput {
                approval_id: "approval-1".to_owned(),
                approver: "manager-1".to_owned(),
                reason: "too late".to_owned(),
                occurred_at: SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            },
        );

        assert!(matches!(
            result,
            Err(ApprovalUseCaseError::Domain(JobError::ExpiredApproval))
        ));
        assert!(repo.approval_status_updates.is_empty());
        assert!(audit.events.is_empty());
    }

    #[test]
    fn command_job_without_targets_is_rejected() {
        let mut repo = FakeCommandJobRepository::default();
        let mut audit = FakeAuditWriter::default();
        let mut signer = FakeSigner;
        let mut input = command_input(Duration::from_secs(30), true);
        input.target_agent_ids.clear();

        let result = CreateCommandJob::execute(&mut repo, &mut audit, &mut signer, input);

        assert!(matches!(result, Err(CreateCommandJobError::NoTargets)));
    }

    #[test]
    fn confirmed_command_job_creates_signed_envelope_and_audit() {
        let mut repo = FakeCommandJobRepository::default();
        let mut audit = FakeAuditWriter::default();
        let mut signer = FakeSigner;

        let output = CreateCommandJob::execute(
            &mut repo,
            &mut audit,
            &mut signer,
            command_input(Duration::from_secs(30), true),
        )
        .unwrap();

        assert_eq!(repo.saved_count, 1);
        assert_eq!(repo.saved_assignments.len(), 1);
        assert_eq!(audit.events.len(), 1);
        assert_eq!(audit.events[0].category, AuditCategory::Job);
        assert_eq!(audit.events[0].actor.as_str(), "admin");
        assert_eq!(
            audit.events[0].value,
            AuditValue::Plain(
                "confirmed_high_risk=true,confirmed_by=admin,target_count=1".to_owned()
            )
        );
        assert_eq!(output.targets.len(), 1);
        assert_eq!(output.envelopes.len(), 1);
        assert!(output.envelopes[0].signature.is_some());
        assert_eq!(output.task.program(), "uptime");
    }

    #[test]
    fn drift_check_job_creates_signed_envelope_and_audit() {
        let mut repo = FakeCommandJobRepository::default();
        let mut audit = FakeAuditWriter::default();
        let mut signer = FakeSigner;

        let output = CreateDriftCheckJob::execute(
            &mut repo,
            &mut audit,
            &mut signer,
            CreateDriftCheckJobInput {
                job_id: "drift-job-1".to_owned(),
                target_agent_ids: vec!["web-01".to_owned()],
                policy_document: "apiVersion: fleet.sponzey.dev/v1alpha1".to_owned(),
                timeout: Duration::from_secs(30),
                created_by: "admin".to_owned(),
                issued_at: SystemTime::UNIX_EPOCH,
                expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(60),
                nonce_prefix: "nonce-drift".to_owned(),
                approval_request_id: "approval-drift".to_owned(),
                approval_expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(60),
            },
        )
        .unwrap();

        assert_eq!(repo.saved_count, 1);
        assert_eq!(
            repo.saved_drift_policy.as_deref(),
            Some("apiVersion: fleet.sponzey.dev/v1alpha1")
        );
        assert_eq!(repo.saved_assignments.len(), 1);
        assert_eq!(audit.events.len(), 1);
        assert_eq!(audit.events[0].category, AuditCategory::Drift);
        assert_eq!(audit.events[0].action, "drift_check_job_created");
        assert_eq!(output.targets.len(), 1);
        assert_eq!(output.envelopes.len(), 1);
        assert!(output.envelopes[0].payload_hash.starts_with("drift_check:"));
        assert!(output.envelopes[0].signature.is_some());
    }

    #[test]
    fn high_risk_runbook_without_confirmation_creates_pending_approval() {
        let mut repo = FakeCommandJobRepository::default();
        let mut audit = FakeAuditWriter::default();
        let mut signer = FakeSigner;

        let output =
            CreateRunbookJob::execute(&mut repo, &mut audit, &mut signer, runbook_input(false))
                .unwrap();

        assert_eq!(repo.approval_requests.len(), 1);
        assert_eq!(repo.approval_requests[0].status, "pending");
        assert_eq!(output.approval_request.unwrap().id, "approval-runbook");
        assert!(
            audit
                .events
                .iter()
                .any(|event| event.category == AuditCategory::Approval
                    && event.action == "approval_requested")
        );
    }

    #[test]
    fn confirmed_runbook_job_creates_signed_envelope_and_audit() {
        let mut repo = FakeCommandJobRepository::default();
        let mut audit = FakeAuditWriter::default();
        let mut signer = FakeSigner;

        let output =
            CreateRunbookJob::execute(&mut repo, &mut audit, &mut signer, runbook_input(true))
                .unwrap();

        assert_eq!(repo.saved_count, 1);
        assert!(
            repo.saved_runbook_document
                .as_deref()
                .unwrap()
                .contains("kind: Runbook")
        );
        assert_eq!(repo.saved_assignments.len(), 1);
        assert_eq!(audit.events.len(), 2);
        assert!(
            audit
                .events
                .iter()
                .any(|event| event.category == AuditCategory::Approval
                    && event.action == "approval_requested")
        );
        assert!(
            audit
                .events
                .iter()
                .any(|event| event.category == AuditCategory::Job
                    && event.action == "runbook_job_created")
        );
        assert_eq!(output.targets.len(), 1);
        assert_eq!(output.envelopes.len(), 1);
        assert!(output.envelopes[0].payload_hash.starts_with("runbook:"));
        assert!(output.envelopes[0].signature.is_some());
    }

    #[test]
    fn create_enrollment_token_persists_hash_and_audit_secret_ref() {
        let mut repo = FakeEnrollmentTokenRepository::default();
        let mut audit = FakeAuditWriter::default();

        let output = CreateEnrollmentToken::execute(
            &mut repo,
            &mut audit,
            CreateEnrollmentTokenInput {
                id: "et-1".to_owned(),
                token_hash: "hash-only".to_owned(),
                default_labels: "role=web".to_owned(),
                expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(60),
                max_uses: 1,
                actor: "bootstrap-admin".to_owned(),
                occurred_at: SystemTime::UNIX_EPOCH,
            },
        )
        .unwrap();

        assert_eq!(output.id, "et-1");
        assert_eq!(repo.records.len(), 1);
        assert_eq!(repo.token_hashes, vec!["hash-only"]);
        assert_eq!(audit.events.len(), 1);
        assert_eq!(audit.events[0].category, AuditCategory::Enrollment);
        assert_eq!(audit.events[0].action, "enrollment_token_created");
        assert_eq!(audit.events[0].actor.as_str(), "bootstrap-admin");
        assert_eq!(
            audit.events[0].value,
            AuditValue::SecretRef("et-1".to_owned())
        );
    }

    #[test]
    fn list_enrollment_tokens_returns_repository_records() {
        let mut repo = FakeEnrollmentTokenRepository::default();
        repo.records.push(enrollment_record("et-1", false));

        let records = ListEnrollmentTokens::execute(&repo).unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "et-1");
    }

    #[test]
    fn revoke_enrollment_token_audits_only_when_record_changed() {
        let mut repo = FakeEnrollmentTokenRepository::default();
        repo.records.push(enrollment_record("et-1", false));
        let mut audit = FakeAuditWriter::default();

        let output = RevokeEnrollmentToken::execute(
            &mut repo,
            &mut audit,
            RevokeEnrollmentTokenInput {
                id: "et-1".to_owned(),
                actor: "bootstrap-admin".to_owned(),
                occurred_at: SystemTime::UNIX_EPOCH,
            },
        )
        .unwrap();

        assert!(output.revoked);
        assert!(repo.records[0].revoked);
        assert_eq!(audit.events.len(), 1);
        assert_eq!(audit.events[0].action, "enrollment_token_revoked");
        assert_eq!(audit.events[0].actor.as_str(), "bootstrap-admin");

        let output = RevokeEnrollmentToken::execute(
            &mut repo,
            &mut audit,
            RevokeEnrollmentTokenInput {
                id: "missing".to_owned(),
                actor: "bootstrap-admin".to_owned(),
                occurred_at: SystemTime::UNIX_EPOCH,
            },
        )
        .unwrap();

        assert!(!output.revoked);
        assert_eq!(audit.events.len(), 1);
    }

    #[test]
    fn inventory_use_cases_list_and_get_agents_through_repository() {
        let mut repo = FakeAgentInventoryRepository::default();
        repo.agents.push(agent("web-01", "web"));

        let agents = ListInventoryAgents::execute(&repo).unwrap();
        let found = GetInventoryAgent::execute(&repo, AgentId::new("web-01").unwrap())
            .unwrap()
            .unwrap();

        assert_eq!(agents.len(), 1);
        assert_eq!(found.name().as_str(), "web-01");
    }

    #[test]
    fn update_agent_labels_audits_changed_agent_and_returns_updated_agent() {
        let mut repo = FakeAgentInventoryRepository::default();
        repo.agents.push(agent("web-01", "web"));
        let mut audit = FakeAuditWriter::default();

        let updated = UpdateAgentLabels::execute(
            &mut repo,
            &mut audit,
            UpdateAgentLabelsInput {
                agent_id: "web-01".to_owned(),
                labels: vec![AgentLabel::new("role", "api").unwrap()],
                actor: "admin".to_owned(),
                occurred_at: SystemTime::UNIX_EPOCH,
            },
        )
        .unwrap()
        .unwrap();

        assert_eq!(updated.labels()[0].value(), "api");
        assert_eq!(audit.events.len(), 1);
        assert_eq!(audit.events[0].category, AuditCategory::Agent);
        assert_eq!(audit.events[0].action, "agent_labels_updated");
        assert_eq!(
            audit.events[0].value,
            AuditValue::Plain("label_count=1".to_owned())
        );
    }

    #[test]
    fn update_agent_labels_returns_none_without_audit_for_missing_agent() {
        let mut repo = FakeAgentInventoryRepository::default();
        let mut audit = FakeAuditWriter::default();

        let updated = UpdateAgentLabels::execute(
            &mut repo,
            &mut audit,
            UpdateAgentLabelsInput {
                agent_id: "missing".to_owned(),
                labels: vec![AgentLabel::new("role", "api").unwrap()],
                actor: "admin".to_owned(),
                occurred_at: SystemTime::UNIX_EPOCH,
            },
        )
        .unwrap();

        assert!(updated.is_none());
        assert!(audit.events.is_empty());
    }

    #[test]
    fn revoke_agent_key_disables_agent_and_writes_audit() {
        let mut repo = FakeAgentInventoryRepository::default();
        repo.agents.push(agent("web-01", "web"));
        let mut audit = FakeAuditWriter::default();

        let revoked = RevokeAgentKey::execute(
            &mut repo,
            &mut audit,
            RevokeAgentKeyInput {
                agent_id: "web-01".to_owned(),
                actor: "admin".to_owned(),
                occurred_at: SystemTime::UNIX_EPOCH,
            },
        )
        .unwrap()
        .unwrap();

        assert_eq!(revoked.status(), AgentStatus::Disabled);
        assert_eq!(audit.events.len(), 1);
        assert_eq!(audit.events[0].category, AuditCategory::Agent);
        assert_eq!(audit.events[0].action, "agent_key_revoked");
        assert_eq!(
            audit.events[0].value,
            AuditValue::Plain("status=revoked".to_owned())
        );
    }

    #[test]
    fn revoke_agent_key_is_idempotent_without_duplicate_audit() {
        let mut repo = FakeAgentInventoryRepository::default();
        let mut agent = agent("web-01", "web");
        agent.disable();
        repo.agents.push(agent);
        let mut audit = FakeAuditWriter::default();

        let revoked = RevokeAgentKey::execute(
            &mut repo,
            &mut audit,
            RevokeAgentKeyInput {
                agent_id: "web-01".to_owned(),
                actor: "admin".to_owned(),
                occurred_at: SystemTime::UNIX_EPOCH,
            },
        )
        .unwrap()
        .unwrap();

        assert_eq!(revoked.status(), AgentStatus::Disabled);
        assert!(audit.events.is_empty());
    }

    #[test]
    fn revoke_agent_key_returns_none_without_audit_for_missing_agent() {
        let mut repo = FakeAgentInventoryRepository::default();
        let mut audit = FakeAuditWriter::default();

        let revoked = RevokeAgentKey::execute(
            &mut repo,
            &mut audit,
            RevokeAgentKeyInput {
                agent_id: "missing".to_owned(),
                actor: "admin".to_owned(),
                occurred_at: SystemTime::UNIX_EPOCH,
            },
        )
        .unwrap();

        assert!(revoked.is_none());
        assert!(audit.events.is_empty());
    }

    #[test]
    fn admin_token_use_cases_create_once_and_verify_hash() {
        let mut repo = FakeAdminTokenRepository::default();

        assert!(EnsureAdminToken::execute(&mut repo, "hash-1").unwrap());
        assert!(!EnsureAdminToken::execute(&mut repo, "hash-2").unwrap());
        assert!(VerifyAdminToken::execute(&repo, "hash-1").unwrap());
        assert!(!VerifyAdminToken::execute(&repo, "hash-2").unwrap());
        assert_eq!(
            AuthenticateAdminToken::execute(&repo, "hash-1").unwrap(),
            Some(AdminTokenRecord {
                actor_id: "bootstrap-admin".to_owned(),
                role: "owner".to_owned(),
            })
        );
        assert!(
            AuthenticateAdminToken::execute(&repo, "hash-2")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn query_use_cases_read_jobs_output_metrics_drift_and_audit() {
        let mut repo = FakeQueryRepository::default();
        repo.jobs.push(JobSummaryRecord {
            id: "job-1".to_owned(),
            status: "success".to_owned(),
            risk: "high".to_owned(),
            command_program: Some("uptime".to_owned()),
            command_args: vec!["-a".to_owned()],
            selector_kind: "explicit_ids".to_owned(),
            selector_source: "[\"agent-1\"]".to_owned(),
            strategy_concurrency: 1,
            strategy_max_failures: Some(1),
            target_count: 1,
            target_agents: vec![JobTargetSummaryRecord {
                agent_id: "agent-1".to_owned(),
                agent_name: "agent-1".to_owned(),
                status: "online".to_owned(),
                labels: vec![("role".to_owned(), "web".to_owned())],
                task_id: Some("task-1".to_owned()),
                assignment_status: Some("succeeded".to_owned()),
                last_error: String::new(),
            }],
            created_at: SystemTime::UNIX_EPOCH,
            expires_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(60)),
        });
        repo.output.push(JobOutputChunk {
            job_id: "job-1".to_owned(),
            agent_id: "agent-1".to_owned(),
            stream: JobOutputStream::Stdout,
            sequence: 0,
            body: "ok".to_owned(),
        });
        repo.facts.push(FactsSnapshotPageRecord {
            agent_id: "agent-1".to_owned(),
            body: "{\"os\":\"linux\"}".to_owned(),
            collected_at: SystemTime::UNIX_EPOCH,
            cursor: SnapshotPageCursor {
                occurred_at: SystemTime::UNIX_EPOCH,
                row_id: 1,
            },
        });
        repo.metrics = Some(MetricsSnapshotRecord {
            agent_id: "agent-1".to_owned(),
            body: "{\"cpu\":1}".to_owned(),
            collected_at: SystemTime::UNIX_EPOCH,
        });
        repo.metrics_pages.push(MetricsSnapshotPageRecord {
            agent_id: "agent-1".to_owned(),
            body: "{\"cpu\":1}".to_owned(),
            collected_at: SystemTime::UNIX_EPOCH,
            cursor: SnapshotPageCursor {
                occurred_at: SystemTime::UNIX_EPOCH,
                row_id: 1,
            },
        });
        repo.drift = Some(DriftReportRecord {
            agent_id: "agent-1".to_owned(),
            report: DriftReport {
                policy_name: "nginx-running".to_owned(),
                status: fleet_domain::DriftStatus::Compliant,
                severity: fleet_domain::DriftSeverity::None,
                acknowledgement: fleet_domain::DriftAcknowledgement::Open,
                expected: "service nginx running".to_owned(),
                actual: "service nginx running".to_owned(),
            },
            checked_at: SystemTime::UNIX_EPOCH,
        });
        repo.drift_pages.push(DriftReportPageRecord {
            agent_id: "agent-1".to_owned(),
            report: DriftReport {
                policy_name: "nginx-running".to_owned(),
                status: fleet_domain::DriftStatus::Compliant,
                severity: fleet_domain::DriftSeverity::None,
                acknowledgement: fleet_domain::DriftAcknowledgement::Open,
                expected: "service nginx running".to_owned(),
                actual: "service nginx running".to_owned(),
            },
            checked_at: SystemTime::UNIX_EPOCH,
            cursor: SnapshotPageCursor {
                occurred_at: SystemTime::UNIX_EPOCH,
                row_id: 1,
            },
        });
        repo.audit
            .push(AuditEvent::security("invalid_signature", "agent-1"));

        assert_eq!(ListJobSummaries::execute(&repo, 50).unwrap().len(), 1);
        assert_eq!(
            ListJobOutputForJob::execute(&repo, "job-1").unwrap().len(),
            1
        );
        assert_eq!(
            ListFactsSnapshots::execute(&repo, "agent-1", 50, None)
                .unwrap()
                .len(),
            1
        );
        assert!(
            GetLatestMetrics::execute(&repo, "agent-1")
                .unwrap()
                .is_some()
        );
        assert_eq!(
            ListMetricsSnapshots::execute(&repo, "agent-1", 50, None)
                .unwrap()
                .len(),
            1
        );
        assert!(GetLatestDrift::execute(&repo, "agent-1").unwrap().is_some());
        assert_eq!(
            ListDriftReports::execute(&repo, "agent-1", 50, None)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(ListAuditEvents::execute(&repo, 50).unwrap().len(), 1);
    }

    #[test]
    fn policy_use_cases_save_assign_and_schedule_drift() {
        let mut repo = FakePolicyRepository::default();
        let mut audit = FakeAuditWriter::default();
        let source = policy_document("nginx-running");

        let policy = SavePolicy::execute(
            &mut repo,
            &mut audit,
            SavePolicyInput {
                source,
                actor: "admin".to_owned(),
                now: SystemTime::UNIX_EPOCH,
            },
        )
        .unwrap();

        assert_eq!(policy.id, "nginx-running");
        assert_eq!(repo.policies[0].id, "nginx-running");
        assert_eq!(audit.events[0].action, "policy_saved");

        let assignment = AssignPolicyToAgent::execute(
            &mut repo,
            &mut audit,
            AssignPolicyToAgentInput {
                policy_id: "nginx-running".to_owned(),
                agent_id: "agent-1".to_owned(),
                actor: "admin".to_owned(),
                now: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            },
        )
        .unwrap();

        assert_eq!(assignment.agent_id, "agent-1");
        assert_eq!(repo.assignments[0].policy_id, "nginx-running");
        assert_eq!(audit.events[1].action, "policy_assigned");

        SchedulePolicyDrift::execute(
            &mut repo,
            &mut audit,
            SchedulePolicyDriftInput {
                policy_id: "nginx-running".to_owned(),
                agent_id: "agent-1".to_owned(),
                interval: Duration::from_secs(300),
                next_due_at: SystemTime::UNIX_EPOCH + Duration::from_secs(300),
                actor: "admin".to_owned(),
                now: SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            },
        )
        .unwrap();

        assert_eq!(audit.events[2].action, "scheduled_drift_configured");
        assert!(
            ListDueScheduledDrift::execute(
                &repo,
                SystemTime::UNIX_EPOCH + Duration::from_secs(299),
                10
            )
            .unwrap()
            .is_empty()
        );
        assert_eq!(
            ListDueScheduledDrift::execute(
                &repo,
                SystemTime::UNIX_EPOCH + Duration::from_secs(300),
                10
            )
            .unwrap()
            .len(),
            1
        );

        RecordScheduledDriftCheck::execute(
            &mut repo,
            &mut audit,
            RecordScheduledDriftCheckInput {
                policy_id: "nginx-running".to_owned(),
                agent_id: "agent-1".to_owned(),
                actor: "scheduler".to_owned(),
                checked_at: SystemTime::UNIX_EPOCH + Duration::from_secs(300),
            },
        )
        .unwrap();

        assert_eq!(audit.events[3].action, "scheduled_drift_checked");
        assert!(
            ListDueScheduledDrift::execute(
                &repo,
                SystemTime::UNIX_EPOCH + Duration::from_secs(599),
                10
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn remediation_approval_is_required_and_resolution_updates_drift_state() {
        let mut approvals = FakeCommandJobRepository::default();
        let mut audit = FakeAuditWriter::default();

        let approval = CreateRemediationApproval::execute(
            &mut approvals,
            &mut audit,
            RemediationApprovalInput {
                approval_id: "approval-remediate".to_owned(),
                job_id: "job-remediate".to_owned(),
                policy_id: "nginx-running".to_owned(),
                agent_id: "agent-1".to_owned(),
                requester: "admin".to_owned(),
                reason: "drift remediation requires approval".to_owned(),
                expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(600),
                now: SystemTime::UNIX_EPOCH,
            },
        )
        .unwrap();

        assert_eq!(approval.status, "pending");
        assert_eq!(audit.events[0].action, "remediation_approval_requested");

        let mut repo = FakePolicyRepository {
            resolve_latest_result: true,
            ..Default::default()
        };
        let changed = MarkRemediationResolved::execute(
            &mut repo,
            &mut audit,
            "agent-1",
            "nginx-running",
            "job-remediate",
            "admin",
            SystemTime::UNIX_EPOCH + Duration::from_secs(10),
        )
        .unwrap();

        assert!(changed);
        assert_eq!(
            repo.resolved_reports,
            vec![(
                "agent-1".to_owned(),
                "nginx-running".to_owned(),
                "job-remediate".to_owned()
            )]
        );
        assert_eq!(audit.events[1].action, "drift_resolved_by_remediation");
    }

    fn command_input(timeout: Duration, confirmed_high_risk: bool) -> CreateCommandJobInput {
        CreateCommandJobInput {
            job_id: "job-1".to_owned(),
            target_agent_ids: vec!["agent-1".to_owned()],
            program: "uptime".to_owned(),
            args: Vec::new(),
            timeout,
            confirmed_high_risk,
            confirmed_by: "admin".to_owned(),
            issued_at: SystemTime::UNIX_EPOCH,
            expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(60),
            nonce_prefix: "nonce".to_owned(),
            approval_request_id: "approval-command".to_owned(),
            approval_expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(60),
        }
    }

    fn approval_request_record(
        id: &str,
        job_id: &str,
        expires_at: SystemTime,
    ) -> ApprovalRequestRecord {
        ApprovalRequestRecord {
            id: id.to_owned(),
            job_id: job_id.to_owned(),
            requester: "admin".to_owned(),
            approver: None,
            reason: "approval required".to_owned(),
            status: "pending".to_owned(),
            expires_at,
            created_at: SystemTime::UNIX_EPOCH,
            decided_at: None,
        }
    }

    fn runbook_input(confirmed_high_risk: bool) -> CreateRunbookJobInput {
        CreateRunbookJobInput {
            job_id: "runbook-job-1".to_owned(),
            target_agent_ids: vec!["agent-1".to_owned()],
            runbook_document: r#"
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Runbook
metadata:
  name: nginx-basic
spec:
  targets:
    selector: role=web
  tasks:
    - id: nginx-package
      package:
        name: nginx
        state: present
"#
            .to_owned(),
            timeout: Duration::from_secs(30),
            confirmed_high_risk,
            confirmed_by: "admin".to_owned(),
            issued_at: SystemTime::UNIX_EPOCH,
            expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(60),
            nonce_prefix: "nonce-runbook".to_owned(),
            approval_request_id: "approval-runbook".to_owned(),
            approval_expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(60),
        }
    }

    #[derive(Default)]
    struct FakeCommandJobRepository {
        saved_count: usize,
        saved_program: Option<String>,
        saved_drift_policy: Option<String>,
        saved_runbook_document: Option<String>,
        saved_assignments: Vec<TaskEnvelope>,
        approval_requests: Vec<ApprovalRequestRecord>,
        approval_status_updates: Vec<(String, JobStatus)>,
    }

    impl TaskAssignmentRepository for FakeCommandJobRepository {
        type Error = Infallible;

        fn save_assignment(&mut self, envelope: TaskEnvelope) -> Result<(), Self::Error> {
            self.saved_assignments.push(envelope);
            Ok(())
        }
    }

    impl CommandJobRepository for FakeCommandJobRepository {
        fn save_command_job(
            &mut self,
            _job: Job,
            task: &CommandTask,
        ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
            self.saved_count += 1;
            self.saved_program = Some(task.program().to_owned());
            Ok(())
        }
    }

    impl DriftCheckJobRepository for FakeCommandJobRepository {
        fn save_drift_check_job(
            &mut self,
            _job: Job,
            task: &DriftCheckTask,
        ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
            self.saved_count += 1;
            self.saved_drift_policy = Some(task.policy_document().to_owned());
            Ok(())
        }
    }

    impl RunbookJobRepository for FakeCommandJobRepository {
        fn save_runbook_job(
            &mut self,
            _job: Job,
            task: &RunbookExecutionTask,
        ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
            self.saved_count += 1;
            self.saved_runbook_document = Some(task.runbook_document().to_owned());
            Ok(())
        }
    }

    impl ApprovalRepository for FakeCommandJobRepository {
        type Error = Infallible;

        fn insert_approval_request(
            &mut self,
            request: ApprovalRequestRecord,
        ) -> Result<(), Self::Error> {
            self.approval_requests.push(request);
            Ok(())
        }

        fn find_approval_request(
            &self,
            approval_id: &str,
        ) -> Result<Option<ApprovalRequestRecord>, Self::Error> {
            Ok(self
                .approval_requests
                .iter()
                .find(|request| request.id == approval_id)
                .cloned())
        }

        fn find_pending_approval_for_job(
            &self,
            job_id: &str,
        ) -> Result<Option<ApprovalRequestRecord>, Self::Error> {
            Ok(self
                .approval_requests
                .iter()
                .find(|request| request.job_id == job_id && request.status == "pending")
                .cloned())
        }

        fn list_approval_requests(
            &self,
            status: Option<&str>,
            limit: usize,
        ) -> Result<Vec<ApprovalRequestRecord>, Self::Error> {
            Ok(self
                .approval_requests
                .iter()
                .filter(|request| status.is_none_or(|status| request.status == status))
                .take(limit)
                .cloned()
                .collect())
        }

        fn update_approval_request(
            &mut self,
            request: ApprovalRequestRecord,
        ) -> Result<bool, Self::Error> {
            if let Some(existing) = self
                .approval_requests
                .iter_mut()
                .find(|existing| existing.id == request.id)
            {
                *existing = request;
                Ok(true)
            } else {
                Ok(false)
            }
        }

        fn update_job_status_for_approval(
            &mut self,
            job_id: &str,
            status: JobStatus,
        ) -> Result<bool, Self::Error> {
            self.approval_status_updates
                .push((job_id.to_owned(), status));
            Ok(true)
        }
    }

    #[derive(Default)]
    struct FakeDispatchAssignmentRepository {
        agents: Vec<Agent>,
        assignments: Vec<PendingTaskAssignment>,
        gates: BTreeMap<String, JobDispatchGate>,
        dispatched_assignments: Vec<String>,
        running_jobs: Vec<String>,
        expired_jobs: Vec<String>,
    }

    impl DispatchAssignmentRepository for FakeDispatchAssignmentRepository {
        type Error = Infallible;

        fn list_pending_assignments(
            &self,
            agent_id: Option<&AgentId>,
            job_id: Option<&JobId>,
            limit: usize,
        ) -> Result<Vec<PendingTaskAssignment>, Self::Error> {
            Ok(self
                .assignments
                .iter()
                .filter(|assignment| {
                    agent_id
                        .map(|id| &assignment.envelope.target_agent_id == id)
                        .unwrap_or(true)
                        && job_id
                            .map(|id| &assignment.envelope.job_id == id)
                            .unwrap_or(true)
                })
                .take(limit)
                .cloned()
                .collect())
        }

        fn find_dispatch_agent(&self, agent_id: &AgentId) -> Result<Option<Agent>, Self::Error> {
            Ok(self
                .agents
                .iter()
                .find(|agent| agent.id() == agent_id)
                .cloned())
        }

        fn dispatch_gate(&self, job_id: &JobId) -> Result<JobDispatchGate, Self::Error> {
            Ok(self
                .gates
                .get(job_id.as_str())
                .copied()
                .unwrap_or(JobDispatchGate {
                    concurrency: usize::MAX,
                    max_failures: None,
                    active_count: 0,
                    failure_count: 0,
                }))
        }

        fn mark_assignment_dispatched(
            &mut self,
            task_id: &TaskId,
            _now: SystemTime,
        ) -> Result<(), Self::Error> {
            self.dispatched_assignments
                .push(task_id.as_str().to_owned());
            Ok(())
        }

        fn mark_job_running(
            &mut self,
            job_id: &JobId,
            _now: SystemTime,
        ) -> Result<(), Self::Error> {
            self.running_jobs.push(job_id.as_str().to_owned());
            Ok(())
        }

        fn mark_job_expired(
            &mut self,
            job_id: &JobId,
            _now: SystemTime,
        ) -> Result<(), Self::Error> {
            self.expired_jobs.push(job_id.as_str().to_owned());
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakePendingAssignmentDispatcher {
        active_agent_ids: Vec<String>,
        failed_task_ids: Vec<String>,
        sent_task_ids: Vec<String>,
    }

    impl PendingAssignmentDispatcher for FakePendingAssignmentDispatcher {
        type Error = String;

        fn has_active_session(&self, agent_id: &AgentId) -> bool {
            self.active_agent_ids
                .iter()
                .any(|active_agent_id| active_agent_id == agent_id.as_str())
        }

        fn dispatch(&mut self, assignment: &PendingTaskAssignment) -> Result<(), Self::Error> {
            let task_id = assignment.envelope.task_id.as_str().to_owned();
            if self.failed_task_ids.iter().any(|id| id == &task_id) {
                return Err("send failed".to_owned());
            }
            self.sent_task_ids.push(task_id);
            Ok(())
        }
    }

    fn pending_assignment(
        job_id: &str,
        task_id: &str,
        agent_id: &str,
        expires_at: SystemTime,
    ) -> PendingTaskAssignment {
        PendingTaskAssignment {
            envelope: TaskEnvelope {
                job_id: JobId::new(job_id).unwrap(),
                task_id: TaskId::new(task_id).unwrap(),
                target_agent_id: AgentId::new(agent_id).unwrap(),
                issued_at: SystemTime::UNIX_EPOCH,
                expires_at: TaskExpiry::new(expires_at),
                nonce: TaskNonce::new(format!("{task_id}-nonce")).unwrap(),
                payload_hash: "hash".to_owned(),
                signature: Some(TaskSignature::new("sig").unwrap()),
            },
            task: TaskKind::Command(
                CommandTask::new("uptime", Vec::new(), Duration::from_secs(30)).unwrap(),
            ),
        }
    }

    #[derive(Default)]
    struct FakeAgentInventoryRepository {
        agents: Vec<Agent>,
    }

    impl AgentInventoryRepository for FakeAgentInventoryRepository {
        type Error = Infallible;

        fn list_agents(&self) -> Result<Vec<Agent>, Self::Error> {
            Ok(self.agents.clone())
        }

        fn find_agent_by_id(&self, id: &AgentId) -> Result<Option<Agent>, Self::Error> {
            Ok(self.agents.iter().find(|agent| agent.id() == id).cloned())
        }

        fn revoke_agent_key(&mut self, id: &AgentId) -> Result<bool, Self::Error> {
            let Some(agent) = self.agents.iter_mut().find(|agent| agent.id() == id) else {
                return Ok(false);
            };
            agent.disable();
            Ok(true)
        }

        fn update_agent_labels(
            &mut self,
            id: &AgentId,
            labels: &[AgentLabel],
        ) -> Result<bool, Self::Error> {
            let Some(agent) = self.agents.iter_mut().find(|agent| agent.id() == id) else {
                return Ok(false);
            };
            agent.set_labels(labels.to_vec());
            Ok(true)
        }
    }

    #[derive(Default)]
    struct FakeAdminTokenRepository {
        token_hash: Option<String>,
        actor_id: Option<String>,
        role: Option<String>,
    }

    impl AdminTokenRepository for FakeAdminTokenRepository {
        type Error = Infallible;

        fn admin_token_exists(&self) -> Result<bool, Self::Error> {
            Ok(self.token_hash.is_some())
        }

        fn insert_admin_token_hash(&mut self, token_hash: &str) -> Result<(), Self::Error> {
            if self.token_hash.is_none() {
                self.token_hash = Some(token_hash.to_owned());
                self.actor_id = Some("bootstrap-admin".to_owned());
                self.role = Some("owner".to_owned());
            }
            Ok(())
        }

        fn verify_admin_token_hash(&self, token_hash: &str) -> Result<bool, Self::Error> {
            Ok(self.token_hash.as_deref() == Some(token_hash))
        }

        fn find_admin_token_record(
            &self,
            token_hash: &str,
        ) -> Result<Option<AdminTokenRecord>, Self::Error> {
            if self.token_hash.as_deref() != Some(token_hash) {
                return Ok(None);
            }
            Ok(Some(AdminTokenRecord {
                actor_id: self
                    .actor_id
                    .clone()
                    .unwrap_or_else(|| "bootstrap-admin".to_owned()),
                role: self.role.clone().unwrap_or_else(|| "owner".to_owned()),
            }))
        }
    }

    #[derive(Default)]
    struct FakeQueryRepository {
        jobs: Vec<JobSummaryRecord>,
        output: Vec<JobOutputChunk>,
        facts: Vec<FactsSnapshotPageRecord>,
        metrics: Option<MetricsSnapshotRecord>,
        metrics_pages: Vec<MetricsSnapshotPageRecord>,
        drift: Option<DriftReportRecord>,
        drift_pages: Vec<DriftReportPageRecord>,
        audit: Vec<AuditEvent>,
    }

    impl JobQueryRepository for FakeQueryRepository {
        type Error = Infallible;

        fn list_job_summaries(&self, limit: usize) -> Result<Vec<JobSummaryRecord>, Self::Error> {
            Ok(self.jobs.iter().take(limit).cloned().collect())
        }

        fn find_job_summary(&self, job_id: &str) -> Result<Option<JobSummaryRecord>, Self::Error> {
            Ok(self.jobs.iter().find(|job| job.id == job_id).cloned())
        }
    }

    impl JobOutputRepository for FakeQueryRepository {
        type Error = Infallible;

        fn append_output_chunk(&mut self, chunk: JobOutputChunk) -> Result<(), Self::Error> {
            self.output.push(chunk);
            Ok(())
        }

        fn list_output_chunks(
            &self,
            job_id: &str,
            agent_id: &str,
        ) -> Result<Vec<JobOutputChunk>, Self::Error> {
            Ok(self
                .output
                .iter()
                .filter(|chunk| chunk.job_id == job_id && chunk.agent_id == agent_id)
                .cloned()
                .collect())
        }

        fn list_output_chunks_for_job(
            &self,
            job_id: &str,
        ) -> Result<Vec<JobOutputChunk>, Self::Error> {
            Ok(self
                .output
                .iter()
                .filter(|chunk| chunk.job_id == job_id)
                .cloned()
                .collect())
        }
    }

    impl MetricsRepository for FakeQueryRepository {
        type Error = Infallible;

        fn insert_metrics_snapshot(
            &mut self,
            agent_id: &str,
            body: &str,
            collected_at: SystemTime,
        ) -> Result<(), Self::Error> {
            self.metrics = Some(MetricsSnapshotRecord {
                agent_id: agent_id.to_owned(),
                body: body.to_owned(),
                collected_at,
            });
            Ok(())
        }

        fn latest_metrics_snapshot(
            &self,
            _agent_id: &str,
        ) -> Result<Option<MetricsSnapshotRecord>, Self::Error> {
            Ok(self.metrics.clone())
        }

        fn list_metrics_snapshots(
            &self,
            _agent_id: &str,
            limit: usize,
            before: Option<SnapshotPageCursor>,
        ) -> Result<Vec<MetricsSnapshotPageRecord>, Self::Error> {
            Ok(self
                .metrics_pages
                .iter()
                .filter(|record| before.is_none_or(|cursor| record.cursor.row_id < cursor.row_id))
                .take(limit)
                .cloned()
                .collect())
        }
    }

    impl FactsRepository for FakeQueryRepository {
        type Error = Infallible;

        fn insert_facts_snapshot(
            &mut self,
            agent_id: &str,
            body: &str,
            collected_at: SystemTime,
        ) -> Result<(), Self::Error> {
            self.facts.push(FactsSnapshotPageRecord {
                agent_id: agent_id.to_owned(),
                body: body.to_owned(),
                collected_at,
                cursor: SnapshotPageCursor {
                    occurred_at: collected_at,
                    row_id: self.facts.len() as i64 + 1,
                },
            });
            Ok(())
        }

        fn latest_facts_snapshot(
            &self,
            _agent_id: &str,
        ) -> Result<Option<FactsSnapshotRecord>, Self::Error> {
            Ok(self.facts.last().map(|record| FactsSnapshotRecord {
                agent_id: record.agent_id.clone(),
                body: record.body.clone(),
                collected_at: record.collected_at,
            }))
        }

        fn list_facts_snapshots(
            &self,
            _agent_id: &str,
            limit: usize,
            before: Option<SnapshotPageCursor>,
        ) -> Result<Vec<FactsSnapshotPageRecord>, Self::Error> {
            Ok(self
                .facts
                .iter()
                .filter(|record| before.is_none_or(|cursor| record.cursor.row_id < cursor.row_id))
                .take(limit)
                .cloned()
                .collect())
        }
    }

    impl DriftRepository for FakeQueryRepository {
        type Error = Infallible;

        fn insert_drift_report(
            &mut self,
            agent_id: &str,
            report: &DriftReport,
            checked_at: SystemTime,
        ) -> Result<(), Self::Error> {
            self.drift = Some(DriftReportRecord {
                agent_id: agent_id.to_owned(),
                report: report.clone(),
                checked_at,
            });
            Ok(())
        }

        fn latest_drift_report(
            &self,
            _agent_id: &str,
        ) -> Result<Option<DriftReportRecord>, Self::Error> {
            Ok(self.drift.clone())
        }

        fn list_drift_reports(
            &self,
            _agent_id: &str,
            limit: usize,
            before: Option<SnapshotPageCursor>,
        ) -> Result<Vec<DriftReportPageRecord>, Self::Error> {
            Ok(self
                .drift_pages
                .iter()
                .filter(|record| before.is_none_or(|cursor| record.cursor.row_id < cursor.row_id))
                .take(limit)
                .cloned()
                .collect())
        }
    }

    impl AuditWriter for FakeQueryRepository {
        type Error = Infallible;

        fn write(&mut self, event: AuditEvent) -> Result<(), Self::Error> {
            self.audit.push(event);
            Ok(())
        }
    }

    impl AuditRepository for FakeQueryRepository {
        fn list(&self, limit: usize) -> Result<Vec<AuditEvent>, Self::Error> {
            Ok(self.audit.iter().take(limit).cloned().collect())
        }

        fn list_by_category(
            &self,
            category: fleet_domain::AuditCategory,
            limit: usize,
        ) -> Result<Vec<AuditEvent>, Self::Error> {
            Ok(self
                .audit
                .iter()
                .filter(|event| event.category == category)
                .take(limit)
                .cloned()
                .collect())
        }
    }

    #[derive(Default)]
    struct FakeEnrollmentTokenRepository {
        records: Vec<EnrollmentTokenRecord>,
        token_hashes: Vec<String>,
    }

    impl EnrollmentTokenRepository for FakeEnrollmentTokenRepository {
        type Error = Infallible;

        fn insert_enrollment_token_hash(
            &mut self,
            id: &str,
            token_hash: &str,
            default_labels: &str,
            expires_at: SystemTime,
            max_uses: u32,
        ) -> Result<(), Self::Error> {
            self.token_hashes.push(token_hash.to_owned());
            self.records.push(EnrollmentTokenRecord {
                id: id.to_owned(),
                default_labels: default_labels.to_owned(),
                expires_at,
                max_uses,
                used_count: 0,
                revoked: false,
            });
            Ok(())
        }

        fn list_enrollment_tokens(&self) -> Result<Vec<EnrollmentTokenRecord>, Self::Error> {
            Ok(self.records.clone())
        }

        fn revoke_enrollment_token(&mut self, id: &str) -> Result<bool, Self::Error> {
            let Some(record) = self.records.iter_mut().find(|record| record.id == id) else {
                return Ok(false);
            };
            record.revoked = true;
            Ok(true)
        }

        fn consume_enrollment_token_hash(
            &mut self,
            _token_hash: &str,
            _now: SystemTime,
        ) -> Result<EnrollmentTokenRecord, Self::Error> {
            unreachable!("consume is covered by enrollment flow tests")
        }
    }

    fn enrollment_record(id: &str, revoked: bool) -> EnrollmentTokenRecord {
        EnrollmentTokenRecord {
            id: id.to_owned(),
            default_labels: String::new(),
            expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(60),
            max_uses: 1,
            used_count: 0,
            revoked,
        }
    }

    fn policy_document(name: &str) -> String {
        format!(
            r#"
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Policy
metadata:
  name: {name}
spec:
  selector:
    matchLabels:
      role: web
  checks:
    - id: nginx-service
      service:
        name: nginx
        state: running
"#
        )
    }

    #[derive(Default)]
    struct FakePolicyRepository {
        policies: Vec<PolicyRecord>,
        assignments: Vec<PolicyAssignmentRecord>,
        schedules: Vec<ScheduledDriftRecord>,
        acknowledged_reports: Vec<(String, String, String)>,
        resolved_reports: Vec<(String, String, String)>,
        resolve_latest_result: bool,
    }

    impl PolicyRepository for FakePolicyRepository {
        type Error = Infallible;

        fn save_policy_source(
            &mut self,
            policy_id: &str,
            name: &str,
            version: u32,
            source: &str,
        ) -> Result<(), Self::Error> {
            self.policies.push(PolicyRecord {
                id: policy_id.to_owned(),
                name: name.to_owned(),
                version,
                source: source.to_owned(),
                created_at: SystemTime::UNIX_EPOCH,
                updated_at: SystemTime::UNIX_EPOCH,
            });
            Ok(())
        }

        fn list_policies(&self) -> Result<Vec<PolicyRecord>, Self::Error> {
            Ok(self.policies.clone())
        }

        fn find_policy(&self, policy_id: &str) -> Result<Option<PolicyRecord>, Self::Error> {
            Ok(self
                .policies
                .iter()
                .find(|policy| policy.id == policy_id)
                .cloned())
        }

        fn assign_policy_to_agent(
            &mut self,
            policy_id: &str,
            agent_id: &str,
            assigned_at: SystemTime,
        ) -> Result<(), Self::Error> {
            self.assignments.push(PolicyAssignmentRecord {
                policy_id: policy_id.to_owned(),
                agent_id: agent_id.to_owned(),
                assigned_at,
            });
            Ok(())
        }

        fn policies_for_agent(
            &self,
            agent_id: &str,
        ) -> Result<Vec<PolicyAssignmentRecord>, Self::Error> {
            Ok(self
                .assignments
                .iter()
                .filter(|assignment| assignment.agent_id == agent_id)
                .cloned()
                .collect())
        }

        fn upsert_policy_schedule(
            &mut self,
            policy_id: &str,
            agent_id: &str,
            interval: Duration,
            next_due_at: SystemTime,
        ) -> Result<(), Self::Error> {
            self.schedules.push(ScheduledDriftRecord {
                policy_id: policy_id.to_owned(),
                agent_id: agent_id.to_owned(),
                interval_seconds: interval.as_secs(),
                next_due_at,
                last_checked_at: None,
            });
            Ok(())
        }

        fn due_scheduled_drift_checks(
            &self,
            now: SystemTime,
            limit: usize,
        ) -> Result<Vec<ScheduledDriftRecord>, Self::Error> {
            Ok(self
                .schedules
                .iter()
                .filter(|schedule| schedule.next_due_at <= now)
                .take(limit)
                .cloned()
                .collect())
        }

        fn record_scheduled_drift_check(
            &mut self,
            policy_id: &str,
            agent_id: &str,
            checked_at: SystemTime,
        ) -> Result<(), Self::Error> {
            if let Some(schedule) = self
                .schedules
                .iter_mut()
                .find(|schedule| schedule.policy_id == policy_id && schedule.agent_id == agent_id)
            {
                schedule.last_checked_at = Some(checked_at);
                schedule.next_due_at = checked_at + Duration::from_secs(schedule.interval_seconds);
            }
            Ok(())
        }

        fn acknowledge_latest_drift_report(
            &mut self,
            agent_id: &str,
            policy_name: &str,
            actor: &str,
            _acknowledged_at: SystemTime,
        ) -> Result<bool, Self::Error> {
            self.acknowledged_reports.push((
                agent_id.to_owned(),
                policy_name.to_owned(),
                actor.to_owned(),
            ));
            Ok(true)
        }

        fn mark_latest_drift_resolved(
            &mut self,
            agent_id: &str,
            policy_name: &str,
            job_id: &str,
            _resolved_at: SystemTime,
        ) -> Result<bool, Self::Error> {
            self.resolved_reports.push((
                agent_id.to_owned(),
                policy_name.to_owned(),
                job_id.to_owned(),
            ));
            Ok(self.resolve_latest_result)
        }
    }

    #[derive(Default)]
    struct FakeAuditWriter {
        events: Vec<AuditEvent>,
    }

    impl AuditWriter for FakeAuditWriter {
        type Error = Infallible;

        fn write(&mut self, event: AuditEvent) -> Result<(), Self::Error> {
            self.events.push(event);
            Ok(())
        }
    }

    struct FakeSigner;

    impl TaskEnvelopeSigner for FakeSigner {
        type Error = Infallible;

        fn sign(&mut self, payload: &str) -> Result<String, Self::Error> {
            Ok(format!("sig:{payload}"))
        }
    }
}
