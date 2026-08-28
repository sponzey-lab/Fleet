use fleet_domain::{
    Agent, AgentCapabilitySnapshot, AgentError, AgentId, AgentLabel, AgentStatus, ApprovalId,
    ApprovalRequest, ApprovalStatus, ArtifactChecksum, ArtifactId, ArtifactRetentionClass,
    AuditActor, AuditCategory, AuditEvent, AuditTarget, AuditValue, CapabilitySnapshotStatus,
    CommandTask, DriftCheckTask, DriftJobProvenance, DriftReport, DriftReportId,
    DriftReportProvenance, DriftStatus, Job, JobError, JobId, JobStatus, JobTarget, Policy,
    RemediationRequest, RemediationStatus, RenderedArtifactMetadata, RunbookExecutionTask,
    RuntimePrimitive, SecretRef, Selector, TaskEnvelope, TaskExpiry, TaskId, TaskKind, TaskNonce,
    TaskSignature, TemplateRenderError, TemplateSecretResolutionFailure, TemplateVariableValue,
    VerifiedDriftEvidence, approval_requirement_for_task, scheduled_drift_due,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Debug, Display, Formatter};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

pub trait AgentCapabilityRepository {
    type Error;

    fn save_agent_capability_snapshot(
        &mut self,
        agent_id: &AgentId,
        snapshot: AgentCapabilitySnapshot,
    ) -> Result<(), Self::Error>;
    fn latest_agent_capability_snapshot(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<AgentCapabilitySnapshot>, Self::Error>;
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

pub trait SecretProvider {
    type Error;

    fn resolve_secret(
        &self,
        reference: &SecretRef,
    ) -> Result<ResolvedSecret, SecretProviderError<Self::Error>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DisabledSecretProvider;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisabledSecretProviderError;

impl Display for DisabledSecretProviderError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("secret provider is disabled")
    }
}

impl std::error::Error for DisabledSecretProviderError {}

impl SecretProvider for DisabledSecretProvider {
    type Error = DisabledSecretProviderError;

    fn resolve_secret(
        &self,
        reference: &SecretRef,
    ) -> Result<ResolvedSecret, SecretProviderError<Self::Error>> {
        Err(SecretProviderError::Denied {
            reference: reference.clone(),
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedSecret {
    value: String,
}

impl ResolvedSecret {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
        }
    }

    pub fn expose_secret_for_rendering(&self) -> &str {
        &self.value
    }
}

impl Debug for ResolvedSecret {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ResolvedSecret([REDACTED])")
    }
}

impl Display for ResolvedSecret {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum SecretProviderError<E> {
    NotFound { reference: SecretRef },
    Denied { reference: SecretRef },
    Provider { reference: SecretRef, source: E },
}

impl<E> SecretProviderError<E> {
    pub fn reference(&self) -> &SecretRef {
        match self {
            Self::NotFound { reference }
            | Self::Denied { reference }
            | Self::Provider { reference, .. } => reference,
        }
    }
}

impl<E> Debug for SecretProviderError<E> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self, formatter)
    }
}

impl<E> Display for SecretProviderError<E> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { .. } => write!(formatter, "secret reference was not found"),
            Self::Denied { .. } => write!(formatter, "secret reference access was denied"),
            Self::Provider { .. } => write!(formatter, "secret provider failed"),
        }
    }
}

impl<E: Debug> std::error::Error for SecretProviderError<E> {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticSecretProviderError;

impl Display for StaticSecretProviderError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("static secret provider error")
    }
}

impl std::error::Error for StaticSecretProviderError {}

#[derive(Debug, Clone, Default)]
pub struct StaticSecretProvider {
    secrets: BTreeMap<SecretRef, ResolvedSecret>,
    denied: BTreeSet<SecretRef>,
}

impl StaticSecretProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_secret(mut self, reference: SecretRef, value: impl Into<String>) -> Self {
        self.secrets.insert(reference, ResolvedSecret::new(value));
        self
    }

    pub fn with_denied(mut self, reference: SecretRef) -> Self {
        self.denied.insert(reference);
        self
    }
}

impl SecretProvider for StaticSecretProvider {
    type Error = StaticSecretProviderError;

    fn resolve_secret(
        &self,
        reference: &SecretRef,
    ) -> Result<ResolvedSecret, SecretProviderError<Self::Error>> {
        if self.denied.contains(reference) {
            return Err(SecretProviderError::Denied {
                reference: reference.clone(),
            });
        }
        self.secrets
            .get(reference)
            .cloned()
            .ok_or_else(|| SecretProviderError::NotFound {
                reference: reference.clone(),
            })
    }
}

pub fn render_template_content_with_provider<P>(
    template: &str,
    variables: &BTreeMap<String, TemplateVariableValue>,
    provider: &P,
) -> Result<String, TemplateRenderError>
where
    P: SecretProvider,
{
    fleet_domain::render_template_content_with_secret_resolver(template, variables, |reference| {
        provider
            .resolve_secret(reference)
            .map(|secret| secret.expose_secret_for_rendering().to_owned())
            .map_err(|error| match error {
                SecretProviderError::NotFound { .. } => TemplateSecretResolutionFailure::NotFound,
                SecretProviderError::Denied { .. } => TemplateSecretResolutionFailure::Denied,
                SecretProviderError::Provider { .. } => TemplateSecretResolutionFailure::Provider,
            })
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEventPageRecord {
    pub event: AuditEvent,
    pub cursor: SnapshotPageCursor,
}

pub trait AuditRepository: AuditWriter {
    fn list(&self, limit: usize) -> Result<Vec<AuditEvent>, Self::Error>;
    fn list_by_category(
        &self,
        category: fleet_domain::AuditCategory,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, Self::Error>;
    fn export_page(
        &self,
        category: Option<fleet_domain::AuditCategory>,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AuditEventPageRecord>, Self::Error>;
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

    fn save_command_job_with_assignments(
        &mut self,
        job: Job,
        task: &CommandTask,
        assignments: &[TaskEnvelope],
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.save_command_job(job, task)?;
        for assignment in assignments {
            self.save_assignment(assignment.clone())?;
        }
        Ok(())
    }
}

pub trait DriftCheckJobRepository:
    TaskAssignmentRepository + ApprovalRepository<Error = <Self as TaskAssignmentRepository>::Error>
{
    fn save_drift_check_job(
        &mut self,
        job: Job,
        task: &DriftCheckTask,
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error>;

    fn save_drift_check_job_with_assignments(
        &mut self,
        job: Job,
        task: &DriftCheckTask,
        assignments: &[TaskEnvelope],
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.save_drift_check_job(job, task)?;
        for assignment in assignments {
            self.save_assignment(assignment.clone())?;
        }
        Ok(())
    }

    fn save_drift_check_job_with_assignments_and_provenance(
        &mut self,
        job: Job,
        task: &DriftCheckTask,
        assignments: &[TaskEnvelope],
        provenance: Option<&DriftJobProvenance>,
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        let _ = provenance;
        self.save_drift_check_job_with_assignments(job, task, assignments)
    }
}

pub trait RunbookJobRepository:
    TaskAssignmentRepository + ApprovalRepository<Error = <Self as TaskAssignmentRepository>::Error>
{
    fn save_runbook_job(
        &mut self,
        job: Job,
        task: &RunbookExecutionTask,
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error>;

    fn save_runbook_job_with_assignments(
        &mut self,
        job: Job,
        task: &RunbookExecutionTask,
        assignments: &[TaskEnvelope],
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.save_runbook_job(job, task)?;
        for assignment in assignments {
            self.save_assignment(assignment.clone())?;
        }
        Ok(())
    }
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

pub trait ArtifactMetadataRepository {
    type Error;

    fn save_rendered_artifact_metadata(
        &mut self,
        metadata: RenderedArtifactMetadata,
    ) -> Result<(), Self::Error>;
    fn list_rendered_artifacts_for_job(
        &self,
        job_id: &JobId,
    ) -> Result<Vec<RenderedArtifactMetadata>, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactStorePut {
    pub id: ArtifactId,
    pub retention_class: ArtifactRetentionClass,
    pub expected_checksum: ArtifactChecksum,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactStoreRecord {
    pub id: ArtifactId,
    pub retention_class: ArtifactRetentionClass,
    pub checksum: ArtifactChecksum,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactVerification {
    Verified(ArtifactStoreRecord),
    Missing,
    Corrupt {
        expected: ArtifactChecksum,
        actual: ArtifactChecksum,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactDeleteOutcome {
    Deleted,
    Missing,
}

pub trait ArtifactStore {
    type Error;

    fn put(&mut self, input: ArtifactStorePut) -> Result<ArtifactStoreRecord, Self::Error>;
    fn get(
        &self,
        id: &ArtifactId,
        retention_class: ArtifactRetentionClass,
    ) -> Result<Option<Vec<u8>>, Self::Error>;
    fn verify(
        &self,
        id: &ArtifactId,
        retention_class: ArtifactRetentionClass,
        expected: &ArtifactChecksum,
    ) -> Result<ArtifactVerification, Self::Error>;
    fn delete(
        &mut self,
        id: &ArtifactId,
        retention_class: ArtifactRetentionClass,
    ) -> Result<ArtifactDeleteOutcome, Self::Error>;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub job_output: Duration,
    pub facts: Duration,
    pub metrics: Duration,
    pub agent_logs: Duration,
}

impl RetentionPolicy {
    pub fn mvp_defaults() -> Self {
        Self {
            job_output: Duration::from_secs(14 * 86_400),
            facts: Duration::from_secs(30 * 86_400),
            metrics: Duration::from_secs(7 * 86_400),
            agent_logs: Duration::from_secs(86_400),
        }
    }

    pub fn uniform(duration: Duration) -> Self {
        Self {
            job_output: duration,
            facts: duration,
            metrics: duration,
            agent_logs: duration,
        }
    }

    pub fn validate(self) -> Result<(), RetentionPolicyError> {
        if self.job_output.is_zero() {
            return Err(RetentionPolicyError::ZeroDuration("job_output"));
        }
        if self.facts.is_zero() {
            return Err(RetentionPolicyError::ZeroDuration("facts"));
        }
        if self.metrics.is_zero() {
            return Err(RetentionPolicyError::ZeroDuration("metrics"));
        }
        if self.agent_logs.is_zero() {
            return Err(RetentionPolicyError::ZeroDuration("agent_logs"));
        }
        Ok(())
    }

    pub fn cutoffs(self, now: SystemTime) -> RetentionCutoffs {
        RetentionCutoffs {
            job_output: retention_cutoff(now, self.job_output),
            facts: retention_cutoff(now, self.facts),
            metrics: retention_cutoff(now, self.metrics),
            agent_logs: retention_cutoff(now, self.agent_logs),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionPolicyError {
    ZeroDuration(&'static str),
}

impl Display for RetentionPolicyError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroDuration(field) => {
                write!(formatter, "retention duration must be positive: {field}")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionCutoffs {
    pub job_output: SystemTime,
    pub facts: SystemTime,
    pub metrics: SystemTime,
    pub agent_logs: SystemTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RetentionCleanupSummary {
    pub job_output_chunks: usize,
    pub facts_snapshots: usize,
    pub metrics_snapshots: usize,
    pub agent_log_chunks: usize,
}

impl RetentionCleanupSummary {
    pub fn total(self) -> usize {
        self.job_output_chunks
            + self.facts_snapshots
            + self.metrics_snapshots
            + self.agent_log_chunks
    }
}

pub trait RetentionRepository {
    type Error;

    fn cleanup_retention(
        &mut self,
        cutoffs: RetentionCutoffs,
        dry_run: bool,
    ) -> Result<RetentionCleanupSummary, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRetentionCleanupInput {
    pub now: SystemTime,
    pub policy: RetentionPolicy,
    pub dry_run: bool,
    pub actor: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRetentionCleanupOutput {
    pub state: RetentionRunState,
    pub cutoffs: RetentionCutoffs,
    pub summary: RetentionCleanupSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionRunState {
    Planned,
    DryRun,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunRetentionCleanupError<RepoError, AuditError> {
    Domain(String),
    Repository(RepoError),
    Audit(AuditError),
}

pub type RunRetentionCleanupResult<RepoError, AuditError> =
    Result<RunRetentionCleanupOutput, RunRetentionCleanupError<RepoError, AuditError>>;

impl<RepoError, AuditError> Display for RunRetentionCleanupError<RepoError, AuditError>
where
    RepoError: Display,
    AuditError: Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Domain(error) => write!(formatter, "{error}"),
            Self::Repository(error) => write!(formatter, "repository error: {error}"),
            Self::Audit(error) => write!(formatter, "audit error: {error}"),
        }
    }
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

pub struct RunRetentionCleanup;

impl RunRetentionCleanup {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: RunRetentionCleanupInput,
    ) -> RunRetentionCleanupResult<R::Error, A::Error>
    where
        R: RetentionRepository,
        A: AuditWriter,
    {
        input
            .policy
            .validate()
            .map_err(|error| RunRetentionCleanupError::Domain(error.to_string()))?;
        let cutoffs = input.policy.cutoffs(input.now);
        let summary = repo
            .cleanup_retention(cutoffs, input.dry_run)
            .map_err(RunRetentionCleanupError::Repository)?;

        if input.dry_run {
            return Ok(RunRetentionCleanupOutput {
                state: RetentionRunState::DryRun,
                cutoffs,
                summary,
            });
        }

        audit
            .write(AuditEvent {
                category: AuditCategory::Security,
                action: "retention_cleanup".to_owned(),
                actor: AuditActor::new(input.actor),
                target: AuditTarget::new(input.target),
                value: AuditValue::Plain(retention_summary_value(summary)),
                occurred_at: input.now,
            })
            .map_err(RunRetentionCleanupError::Audit)?;

        Ok(RunRetentionCleanupOutput {
            state: RetentionRunState::Completed,
            cutoffs,
            summary,
        })
    }
}

fn retention_cutoff(now: SystemTime, duration: Duration) -> SystemTime {
    now.checked_sub(duration).unwrap_or(UNIX_EPOCH)
}

fn retention_summary_value(summary: RetentionCleanupSummary) -> String {
    format!(
        "job_output_chunks={},facts_snapshots={},metrics_snapshots={},agent_log_chunks={},total={}",
        summary.job_output_chunks,
        summary.facts_snapshots,
        summary.metrics_snapshots,
        summary.agent_log_chunks,
        summary.total()
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftReportRecord {
    pub id: DriftReportId,
    pub agent_id: String,
    pub report: DriftReport,
    /// Only controller-verified provenance may make this report automation evidence.
    pub provenance: DriftReportProvenance,
    pub checked_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftReportPageRecord {
    pub id: DriftReportId,
    pub agent_id: String,
    pub report: DriftReport,
    /// Carries the same authority boundary as the non-paged report record.
    pub provenance: DriftReportProvenance,
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

    /// Persists provenance only when the caller has independently verified it.
    /// Implementations must not infer authority from report payload fields.
    fn insert_drift_report_with_provenance(
        &mut self,
        agent_id: &str,
        report: &DriftReport,
        provenance: &DriftReportProvenance,
        checked_at: SystemTime,
    ) -> Result<(), Self::Error> {
        let _ = provenance;
        self.insert_drift_report(agent_id, report, checked_at)
    }
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

pub trait ScheduledDriftRepository:
    PolicyRepository
    + AgentRepository<Error = <Self as PolicyRepository>::Error>
    + TaskAssignmentRepository<Error = <Self as PolicyRepository>::Error>
    + ApprovalRepository<Error = <Self as PolicyRepository>::Error>
    + DriftCheckJobRepository
{
}

impl<T> ScheduledDriftRepository for T where
    T: PolicyRepository
        + AgentRepository<Error = <T as PolicyRepository>::Error>
        + TaskAssignmentRepository<Error = <T as PolicyRepository>::Error>
        + ApprovalRepository<Error = <T as PolicyRepository>::Error>
        + DriftCheckJobRepository
{
}

pub trait TaskEnvelopeSigner {
    type Error;

    fn sign(&mut self, payload: &str) -> Result<String, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningKeyRotationRecord {
    pub controller_id: String,
    pub rotation: fleet_domain::ControllerSigningKeyRotation,
    pub updated_at: SystemTime,
}

pub trait SigningKeyRotationRepository {
    type Error;

    fn save_signing_key_rotation(
        &mut self,
        record: SigningKeyRotationRecord,
    ) -> Result<(), Self::Error>;
    fn load_signing_key_rotation(
        &self,
        controller_id: &str,
    ) -> Result<Option<SigningKeyRotationRecord>, Self::Error>;
}

pub trait SigningKeyRotationReader {
    type Error;

    fn load_signing_key_rotation(
        &self,
        controller_id: &str,
    ) -> Result<Option<SigningKeyRotationRecord>, Self::Error>;
}

impl<T> SigningKeyRotationReader for T
where
    T: SigningKeyRotationRepository,
{
    type Error = T::Error;

    fn load_signing_key_rotation(
        &self,
        controller_id: &str,
    ) -> Result<Option<SigningKeyRotationRecord>, Self::Error> {
        SigningKeyRotationRepository::load_signing_key_rotation(self, controller_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSigningStagedRolloutRecord {
    pub controller_id: String,
    pub current_fingerprint: String,
    pub previous_fingerprint: Option<String>,
    pub rollout: fleet_domain::ControllerSigningStagedRollout,
    pub updated_at: SystemTime,
}

pub trait ControllerSigningStagedRolloutRepository {
    type Error;

    fn save_controller_signing_staged_rollout(
        &mut self,
        record: ControllerSigningStagedRolloutRecord,
    ) -> Result<(), Self::Error>;

    fn load_controller_signing_staged_rollout(
        &self,
        controller_id: &str,
    ) -> Result<Option<ControllerSigningStagedRolloutRecord>, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCertificateLifecycleRecord {
    pub agent_id: AgentId,
    pub lifecycle: fleet_domain::AgentCertificateLifecycleSnapshot,
    pub updated_at: SystemTime,
}

pub trait AgentCertificateLifecycleRepository {
    type Error;

    fn save_agent_certificate_lifecycle(
        &mut self,
        record: AgentCertificateLifecycleRecord,
    ) -> Result<(), Self::Error>;

    fn load_agent_certificate_lifecycle(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<AgentCertificateLifecycleRecord>, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCertificateLifecycleOperationOutput {
    pub record: AgentCertificateLifecycleRecord,
    pub audit_event: AuditEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestAgentCertificateIssuanceInput {
    pub agent_id: AgentId,
    pub actor: String,
    pub requested_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueAgentCertificateInput {
    pub agent_id: AgentId,
    pub actor: String,
    pub certificate: fleet_domain::AgentCertificate,
    pub issued_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestAgentCertificateRenewalInput {
    pub agent_id: AgentId,
    pub actor: String,
    pub requested_at: SystemTime,
    pub policy: fleet_domain::AgentCertificateRenewalPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivateAgentCertificateRenewalInput {
    pub agent_id: AgentId,
    pub actor: String,
    pub certificate: fleet_domain::AgentCertificate,
    pub activated_at: SystemTime,
    pub policy: fleet_domain::AgentCertificateRenewalPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteAgentCertificateRotationInput {
    pub agent_id: AgentId,
    pub actor: String,
    pub completed_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentCertificateLifecycleUseCaseError<RepositoryError, AuditError> {
    Repository(RepositoryError),
    Audit(AuditError),
    Domain(fleet_domain::AgentCertificateLifecycleError),
    NotFound,
}

impl<RepositoryError, AuditError> Display
    for AgentCertificateLifecycleUseCaseError<RepositoryError, AuditError>
where
    RepositoryError: Display,
    AuditError: Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Repository(error) => write!(formatter, "repository error: {error}"),
            Self::Audit(error) => write!(formatter, "audit error: {error}"),
            Self::Domain(error) => write!(formatter, "{error}"),
            Self::NotFound => formatter.write_str("agent certificate lifecycle record not found"),
        }
    }
}

pub struct RequestAgentCertificateIssuance;

impl RequestAgentCertificateIssuance {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: RequestAgentCertificateIssuanceInput,
    ) -> Result<
        AgentCertificateLifecycleOperationOutput,
        AgentCertificateLifecycleUseCaseError<R::Error, A::Error>,
    >
    where
        R: AgentCertificateLifecycleRepository,
        A: AuditWriter,
    {
        let mut lifecycle = load_optional_agent_certificate_lifecycle(repo, &input.agent_id)?
            .unwrap_or_else(|| {
                fleet_domain::AgentCertificateLifecycle::new(input.agent_id.clone())
            });
        lifecycle
            .request_issuance(input.requested_at)
            .map_err(AgentCertificateLifecycleUseCaseError::Domain)?;
        save_and_audit_agent_certificate_lifecycle(
            repo,
            audit,
            input.agent_id,
            input.actor,
            input.requested_at,
            "agent_certificate_issuance_requested",
            lifecycle,
        )
    }
}

pub struct IssueAgentCertificate;

impl IssueAgentCertificate {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: IssueAgentCertificateInput,
    ) -> Result<
        AgentCertificateLifecycleOperationOutput,
        AgentCertificateLifecycleUseCaseError<R::Error, A::Error>,
    >
    where
        R: AgentCertificateLifecycleRepository,
        A: AuditWriter,
    {
        let mut lifecycle = load_agent_certificate_lifecycle_for_update(repo, &input.agent_id)?;
        lifecycle
            .issue(input.certificate, input.issued_at)
            .map_err(AgentCertificateLifecycleUseCaseError::Domain)?;
        save_and_audit_agent_certificate_lifecycle(
            repo,
            audit,
            input.agent_id,
            input.actor,
            input.issued_at,
            "agent_certificate_issued",
            lifecycle,
        )
    }
}

pub struct RequestAgentCertificateRenewal;

impl RequestAgentCertificateRenewal {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: RequestAgentCertificateRenewalInput,
    ) -> Result<
        AgentCertificateLifecycleOperationOutput,
        AgentCertificateLifecycleUseCaseError<R::Error, A::Error>,
    >
    where
        R: AgentCertificateLifecycleRepository,
        A: AuditWriter,
    {
        let mut lifecycle = load_agent_certificate_lifecycle_for_update(repo, &input.agent_id)?;
        lifecycle
            .request_renewal(input.requested_at, &input.policy)
            .map_err(AgentCertificateLifecycleUseCaseError::Domain)?;
        save_and_audit_agent_certificate_lifecycle(
            repo,
            audit,
            input.agent_id,
            input.actor,
            input.requested_at,
            "agent_certificate_renewal_requested",
            lifecycle,
        )
    }
}

pub struct ActivateAgentCertificateRenewal;

impl ActivateAgentCertificateRenewal {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: ActivateAgentCertificateRenewalInput,
    ) -> Result<
        AgentCertificateLifecycleOperationOutput,
        AgentCertificateLifecycleUseCaseError<R::Error, A::Error>,
    >
    where
        R: AgentCertificateLifecycleRepository,
        A: AuditWriter,
    {
        let mut lifecycle = load_agent_certificate_lifecycle_for_update(repo, &input.agent_id)?;
        lifecycle
            .activate_renewal(input.certificate, input.activated_at, &input.policy)
            .map_err(AgentCertificateLifecycleUseCaseError::Domain)?;
        save_and_audit_agent_certificate_lifecycle(
            repo,
            audit,
            input.agent_id,
            input.actor,
            input.activated_at,
            "agent_certificate_renewal_activated",
            lifecycle,
        )
    }
}

pub struct CompleteAgentCertificateRotation;

impl CompleteAgentCertificateRotation {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: CompleteAgentCertificateRotationInput,
    ) -> Result<
        AgentCertificateLifecycleOperationOutput,
        AgentCertificateLifecycleUseCaseError<R::Error, A::Error>,
    >
    where
        R: AgentCertificateLifecycleRepository,
        A: AuditWriter,
    {
        let mut lifecycle = load_agent_certificate_lifecycle_for_update(repo, &input.agent_id)?;
        lifecycle
            .complete_rotation(input.completed_at)
            .map_err(AgentCertificateLifecycleUseCaseError::Domain)?;
        save_and_audit_agent_certificate_lifecycle(
            repo,
            audit,
            input.agent_id,
            input.actor,
            input.completed_at,
            "agent_certificate_rotation_completed",
            lifecycle,
        )
    }
}

fn load_optional_agent_certificate_lifecycle<R, AuditError>(
    repo: &R,
    agent_id: &AgentId,
) -> Result<
    Option<fleet_domain::AgentCertificateLifecycle>,
    AgentCertificateLifecycleUseCaseError<R::Error, AuditError>,
>
where
    R: AgentCertificateLifecycleRepository,
{
    repo.load_agent_certificate_lifecycle(agent_id)
        .map_err(AgentCertificateLifecycleUseCaseError::Repository)?
        .map(|record| {
            fleet_domain::AgentCertificateLifecycle::from_snapshot(record.lifecycle)
                .map_err(AgentCertificateLifecycleUseCaseError::Domain)
        })
        .transpose()
}

fn load_agent_certificate_lifecycle_for_update<R, AuditError>(
    repo: &R,
    agent_id: &AgentId,
) -> Result<
    fleet_domain::AgentCertificateLifecycle,
    AgentCertificateLifecycleUseCaseError<R::Error, AuditError>,
>
where
    R: AgentCertificateLifecycleRepository,
{
    load_optional_agent_certificate_lifecycle(repo, agent_id)?
        .ok_or(AgentCertificateLifecycleUseCaseError::NotFound)
}

fn save_and_audit_agent_certificate_lifecycle<R, A>(
    repo: &mut R,
    audit: &mut A,
    agent_id: AgentId,
    actor: String,
    occurred_at: SystemTime,
    action: &str,
    lifecycle: fleet_domain::AgentCertificateLifecycle,
) -> Result<
    AgentCertificateLifecycleOperationOutput,
    AgentCertificateLifecycleUseCaseError<R::Error, A::Error>,
>
where
    R: AgentCertificateLifecycleRepository,
    A: AuditWriter,
{
    let record = AgentCertificateLifecycleRecord {
        agent_id: agent_id.clone(),
        lifecycle: lifecycle.snapshot(),
        updated_at: occurred_at,
    };
    repo.save_agent_certificate_lifecycle(record.clone())
        .map_err(AgentCertificateLifecycleUseCaseError::Repository)?;
    let audit_event = AuditEvent {
        category: AuditCategory::Security,
        action: action.to_owned(),
        actor: AuditActor::new(actor),
        target: AuditTarget::new(agent_id.as_str()),
        value: AuditValue::Plain(format!("state={}", record.lifecycle.state.as_str())),
        occurred_at,
    };
    audit
        .write(audit_event.clone())
        .map_err(AgentCertificateLifecycleUseCaseError::Audit)?;
    Ok(AgentCertificateLifecycleOperationOutput {
        record,
        audit_event,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveSigningKeyRotationInput {
    pub controller_id: String,
    pub rotation: fleet_domain::ControllerSigningKeyRotation,
    pub actor: String,
    pub now: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveSigningKeyRotationOutput {
    pub record: SigningKeyRotationRecord,
    pub audit_event: AuditEvent,
}

pub struct SaveSigningKeyRotation;

impl SaveSigningKeyRotation {
    pub fn execute<R>(
        repo: &mut R,
        input: SaveSigningKeyRotationInput,
    ) -> Result<SaveSigningKeyRotationOutput, R::Error>
    where
        R: SigningKeyRotationRepository,
    {
        let record = SigningKeyRotationRecord {
            controller_id: input.controller_id.clone(),
            rotation: input.rotation,
            updated_at: input.now,
        };
        repo.save_signing_key_rotation(record.clone())?;
        let audit_event = AuditEvent {
            category: AuditCategory::Security,
            action: "controller_signing_key_rotation_state_saved".to_owned(),
            actor: AuditActor::new(input.actor),
            target: AuditTarget::new(input.controller_id),
            value: AuditValue::Plain(record.rotation.state().as_str().to_owned()),
            occurred_at: input.now,
        };
        Ok(SaveSigningKeyRotationOutput {
            record,
            audit_event,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningKeyRotationOperationOutput {
    pub record: SigningKeyRotationRecord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerSigningRotationReadiness {
    SteadyReady,
    RotationRequestedNotValidated,
    NewMaterialValidatedWaitingActivation,
    DualTrustActiveAgentsMigrating,
    OldKeyRetirementAvailable,
    TerminalFailed,
    TerminalCanceled,
    TerminalRetired,
}

impl ControllerSigningRotationReadiness {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SteadyReady => "steady_ready",
            Self::RotationRequestedNotValidated => "rotation_requested_not_validated",
            Self::NewMaterialValidatedWaitingActivation => {
                "new_material_validated_waiting_activation"
            }
            Self::DualTrustActiveAgentsMigrating => "dual_trust_active_agents_migrating",
            Self::OldKeyRetirementAvailable => "old_key_retirement_available",
            Self::TerminalFailed => "terminal_failed",
            Self::TerminalCanceled => "terminal_canceled",
            Self::TerminalRetired => "terminal_retired",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSigningRotationStatusInput {
    pub controller_id: String,
    pub active_fingerprint: fleet_domain::SigningKeyFingerprint,
    pub now: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSigningRotationStatus {
    pub controller_id: String,
    pub persisted_record_present: bool,
    pub persisted_state: String,
    pub readiness: ControllerSigningRotationReadiness,
    pub active_signing_fingerprint_prefix: String,
    pub selected_signing_fingerprint_prefix: String,
    pub old_fingerprint_prefix: String,
    pub new_fingerprint_prefix: Option<String>,
    pub requested_at_ms: Option<u64>,
    pub validated_at_ms: Option<u64>,
    pub activated_at_ms: Option<u64>,
    pub old_key_verifies_until_ms: Option<u64>,
    pub retired_at_ms: Option<u64>,
    pub failed_at_ms: Option<u64>,
    pub bootstrap_guard: String,
    pub agent_trust_rollout: String,
}

pub struct QueryControllerSigningRotationStatus;

impl QueryControllerSigningRotationStatus {
    pub fn execute<R>(
        repo: &R,
        input: ControllerSigningRotationStatusInput,
    ) -> Result<ControllerSigningRotationStatus, R::Error>
    where
        R: SigningKeyRotationReader,
    {
        let persisted = repo.load_signing_key_rotation(&input.controller_id)?;
        let persisted_record_present = persisted.is_some();
        let rotation = persisted.map(|record| record.rotation).unwrap_or_else(|| {
            fleet_domain::ControllerSigningKeyRotation::steady(input.active_fingerprint.clone())
        });
        Ok(controller_signing_rotation_status_from_rotation(
            input.controller_id,
            input.active_fingerprint,
            input.now,
            persisted_record_present,
            &rotation,
        ))
    }
}

fn controller_signing_rotation_status_from_rotation(
    controller_id: String,
    active_fingerprint: fleet_domain::SigningKeyFingerprint,
    now: SystemTime,
    persisted_record_present: bool,
    rotation: &fleet_domain::ControllerSigningKeyRotation,
) -> ControllerSigningRotationStatus {
    let snapshot = rotation.snapshot();
    let selected = select_controller_signing_fingerprint(rotation, now);
    let readiness = controller_signing_rotation_readiness(rotation, now);
    let selected_matches_active = selected.fingerprint == active_fingerprint;
    ControllerSigningRotationStatus {
        controller_id,
        persisted_record_present,
        persisted_state: snapshot.state.as_str().to_owned(),
        readiness,
        active_signing_fingerprint_prefix: fingerprint_prefix(active_fingerprint.as_str())
            .to_owned(),
        selected_signing_fingerprint_prefix: fingerprint_prefix(selected.fingerprint.as_str())
            .to_owned(),
        old_fingerprint_prefix: fingerprint_prefix(snapshot.old_fingerprint.as_str()).to_owned(),
        new_fingerprint_prefix: snapshot
            .new_fingerprint
            .as_ref()
            .map(|fingerprint| fingerprint_prefix(fingerprint.as_str()).to_owned()),
        requested_at_ms: snapshot.requested_at.map(system_time_millis_u64),
        validated_at_ms: snapshot.validated_at.map(system_time_millis_u64),
        activated_at_ms: snapshot.activated_at.map(system_time_millis_u64),
        old_key_verifies_until_ms: snapshot.old_key_verifies_until.map(system_time_millis_u64),
        retired_at_ms: snapshot.retired_at.map(system_time_millis_u64),
        failed_at_ms: snapshot.failed_at.map(system_time_millis_u64),
        bootstrap_guard: if selected_matches_active {
            "active_matches_selected".to_owned()
        } else {
            "active_mismatch_selected".to_owned()
        },
        agent_trust_rollout: controller_signing_agent_trust_rollout(readiness).to_owned(),
    }
}

fn controller_signing_rotation_readiness(
    rotation: &fleet_domain::ControllerSigningKeyRotation,
    now: SystemTime,
) -> ControllerSigningRotationReadiness {
    match rotation.state() {
        fleet_domain::SigningKeyRotationState::Steady => {
            ControllerSigningRotationReadiness::SteadyReady
        }
        fleet_domain::SigningKeyRotationState::RotationRequested => {
            ControllerSigningRotationReadiness::RotationRequestedNotValidated
        }
        fleet_domain::SigningKeyRotationState::NewMaterialValidated => {
            ControllerSigningRotationReadiness::NewMaterialValidatedWaitingActivation
        }
        fleet_domain::SigningKeyRotationState::DualTrustActive => {
            if rotation
                .old_key_verifies_until()
                .is_some_and(|old_key_verifies_until| now >= old_key_verifies_until)
            {
                ControllerSigningRotationReadiness::OldKeyRetirementAvailable
            } else {
                ControllerSigningRotationReadiness::DualTrustActiveAgentsMigrating
            }
        }
        fleet_domain::SigningKeyRotationState::OldKeyRetired => {
            ControllerSigningRotationReadiness::TerminalRetired
        }
        fleet_domain::SigningKeyRotationState::RotationFailed => {
            ControllerSigningRotationReadiness::TerminalFailed
        }
        fleet_domain::SigningKeyRotationState::CanceledBeforeActivation => {
            ControllerSigningRotationReadiness::TerminalCanceled
        }
    }
}

fn controller_signing_agent_trust_rollout(
    readiness: ControllerSigningRotationReadiness,
) -> &'static str {
    match readiness {
        ControllerSigningRotationReadiness::SteadyReady => "not_required",
        ControllerSigningRotationReadiness::RotationRequestedNotValidated => "not_ready",
        ControllerSigningRotationReadiness::NewMaterialValidatedWaitingActivation => {
            "ready_for_rollout"
        }
        ControllerSigningRotationReadiness::DualTrustActiveAgentsMigrating => "agents_migrating",
        ControllerSigningRotationReadiness::OldKeyRetirementAvailable => "retirement_available",
        ControllerSigningRotationReadiness::TerminalFailed => "failed",
        ControllerSigningRotationReadiness::TerminalCanceled => "canceled",
        ControllerSigningRotationReadiness::TerminalRetired => "completed",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigningKeyRotationUseCaseError<RepoError, AuditError> {
    Repository(RepoError),
    Audit(AuditError),
    Domain(fleet_domain::SigningKeyRotationError),
    FingerprintMismatch,
    NotFound,
}

impl<RepoError, AuditError> Display for SigningKeyRotationUseCaseError<RepoError, AuditError>
where
    RepoError: Display,
    AuditError: Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Repository(error) => write!(formatter, "repository error: {error}"),
            Self::Audit(error) => write!(formatter, "audit error: {error}"),
            Self::Domain(error) => write!(formatter, "{error}"),
            Self::FingerprintMismatch => formatter.write_str(
                "validated signing fingerprint does not match requested rotation fingerprint",
            ),
            Self::NotFound => formatter.write_str("signing key rotation record not found"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestSigningKeyRotationInput {
    pub controller_id: String,
    pub actor: String,
    pub old_fingerprint: fleet_domain::SigningKeyFingerprint,
    pub new_fingerprint: fleet_domain::SigningKeyFingerprint,
    pub requested_at: SystemTime,
    pub old_key_verifies_until: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidateSigningKeyRotationInput {
    pub controller_id: String,
    pub actor: String,
    pub validated_new_fingerprint: fleet_domain::SigningKeyFingerprint,
    pub validated_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivateSigningKeyRotationInput {
    pub controller_id: String,
    pub actor: String,
    pub activated_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetireSigningKeyRotationInput {
    pub controller_id: String,
    pub actor: String,
    pub retired_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailSigningKeyRotationInput {
    pub controller_id: String,
    pub actor: String,
    pub failed_at: SystemTime,
    pub failure_summary: String,
}

pub struct RequestSigningKeyRotation;

impl RequestSigningKeyRotation {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: RequestSigningKeyRotationInput,
    ) -> Result<SigningKeyRotationOperationOutput, SigningKeyRotationUseCaseError<R::Error, A::Error>>
    where
        R: SigningKeyRotationRepository,
        A: AuditWriter,
    {
        let mut rotation =
            fleet_domain::ControllerSigningKeyRotation::steady(input.old_fingerprint);
        rotation
            .request_rotation(
                input.new_fingerprint,
                input.requested_at,
                input.old_key_verifies_until,
            )
            .map_err(SigningKeyRotationUseCaseError::Domain)?;
        save_and_audit_signing_key_rotation(
            repo,
            audit,
            input.controller_id,
            input.actor,
            input.requested_at,
            "controller_signing_key_rotation_requested",
            rotation,
        )
    }
}

pub struct ValidateSigningKeyRotation;

impl ValidateSigningKeyRotation {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: ValidateSigningKeyRotationInput,
    ) -> Result<SigningKeyRotationOperationOutput, SigningKeyRotationUseCaseError<R::Error, A::Error>>
    where
        R: SigningKeyRotationRepository,
        A: AuditWriter,
    {
        let mut record = load_signing_key_rotation_for_update(repo, &input.controller_id)?;
        let requested_new_fingerprint = record
            .rotation
            .snapshot()
            .new_fingerprint
            .ok_or(SigningKeyRotationUseCaseError::FingerprintMismatch)?;
        if requested_new_fingerprint != input.validated_new_fingerprint {
            return Err(SigningKeyRotationUseCaseError::FingerprintMismatch);
        }
        record
            .rotation
            .validate_new_material(input.validated_at)
            .map_err(SigningKeyRotationUseCaseError::Domain)?;
        save_and_audit_signing_key_rotation(
            repo,
            audit,
            input.controller_id,
            input.actor,
            input.validated_at,
            "controller_signing_key_rotation_validated",
            record.rotation,
        )
    }
}

pub struct ActivateSigningKeyRotation;

impl ActivateSigningKeyRotation {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: ActivateSigningKeyRotationInput,
    ) -> Result<SigningKeyRotationOperationOutput, SigningKeyRotationUseCaseError<R::Error, A::Error>>
    where
        R: SigningKeyRotationRepository,
        A: AuditWriter,
    {
        let mut record = load_signing_key_rotation_for_update(repo, &input.controller_id)?;
        record
            .rotation
            .activate_dual_trust(input.activated_at)
            .map_err(SigningKeyRotationUseCaseError::Domain)?;
        save_and_audit_signing_key_rotation(
            repo,
            audit,
            input.controller_id,
            input.actor,
            input.activated_at,
            "controller_signing_key_rotation_activated",
            record.rotation,
        )
    }
}

pub struct RetireSigningKeyRotation;

impl RetireSigningKeyRotation {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: RetireSigningKeyRotationInput,
    ) -> Result<SigningKeyRotationOperationOutput, SigningKeyRotationUseCaseError<R::Error, A::Error>>
    where
        R: SigningKeyRotationRepository,
        A: AuditWriter,
    {
        let mut record = load_signing_key_rotation_for_update(repo, &input.controller_id)?;
        record
            .rotation
            .retire_old_key(input.retired_at)
            .map_err(SigningKeyRotationUseCaseError::Domain)?;
        save_and_audit_signing_key_rotation(
            repo,
            audit,
            input.controller_id,
            input.actor,
            input.retired_at,
            "controller_signing_key_rotation_retired",
            record.rotation,
        )
    }
}

pub struct FailSigningKeyRotation;

impl FailSigningKeyRotation {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: FailSigningKeyRotationInput,
    ) -> Result<SigningKeyRotationOperationOutput, SigningKeyRotationUseCaseError<R::Error, A::Error>>
    where
        R: SigningKeyRotationRepository,
        A: AuditWriter,
    {
        let mut record = load_signing_key_rotation_for_update(repo, &input.controller_id)?;
        let _redacted_failure_summary = input.failure_summary;
        record
            .rotation
            .fail_rotation(input.failed_at)
            .map_err(SigningKeyRotationUseCaseError::Domain)?;
        save_and_audit_signing_key_rotation(
            repo,
            audit,
            input.controller_id,
            input.actor,
            input.failed_at,
            "controller_signing_key_rotation_failed",
            record.rotation,
        )
    }
}

fn load_signing_key_rotation_for_update<R, AuditError>(
    repo: &R,
    controller_id: &str,
) -> Result<SigningKeyRotationRecord, SigningKeyRotationUseCaseError<R::Error, AuditError>>
where
    R: SigningKeyRotationRepository,
{
    repo.load_signing_key_rotation(controller_id)
        .map_err(SigningKeyRotationUseCaseError::Repository)?
        .ok_or(SigningKeyRotationUseCaseError::NotFound)
}

fn save_and_audit_signing_key_rotation<R, A>(
    repo: &mut R,
    audit: &mut A,
    controller_id: String,
    actor: String,
    occurred_at: SystemTime,
    action: &str,
    rotation: fleet_domain::ControllerSigningKeyRotation,
) -> Result<SigningKeyRotationOperationOutput, SigningKeyRotationUseCaseError<R::Error, A::Error>>
where
    R: SigningKeyRotationRepository,
    A: AuditWriter,
{
    let record = SigningKeyRotationRecord {
        controller_id: controller_id.clone(),
        rotation,
        updated_at: occurred_at,
    };
    repo.save_signing_key_rotation(record.clone())
        .map_err(SigningKeyRotationUseCaseError::Repository)?;
    audit
        .write(AuditEvent {
            category: AuditCategory::Security,
            action: action.to_owned(),
            actor: AuditActor::new(actor),
            target: AuditTarget::new(controller_id),
            value: AuditValue::Plain(signing_key_rotation_audit_value(&record.rotation)),
            occurred_at,
        })
        .map_err(SigningKeyRotationUseCaseError::Audit)?;
    Ok(SigningKeyRotationOperationOutput { record })
}

fn signing_key_rotation_audit_value(
    rotation: &fleet_domain::ControllerSigningKeyRotation,
) -> String {
    let snapshot = rotation.snapshot();
    format!(
        "state={},old_fingerprint_prefix={},new_fingerprint_prefix={}",
        snapshot.state.as_str(),
        fingerprint_prefix(snapshot.old_fingerprint.as_str()),
        snapshot
            .new_fingerprint
            .as_ref()
            .map(|fingerprint| fingerprint_prefix(fingerprint.as_str()))
            .unwrap_or("none")
    )
}

fn fingerprint_prefix(fingerprint: &str) -> &str {
    fingerprint
        .char_indices()
        .nth(12)
        .map(|(index, _)| &fingerprint[..index])
        .unwrap_or(fingerprint)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSigningSelection {
    pub fingerprint: fleet_domain::SigningKeyFingerprint,
}

pub fn select_controller_signing_fingerprint(
    rotation: &fleet_domain::ControllerSigningKeyRotation,
    now: SystemTime,
) -> ControllerSigningSelection {
    ControllerSigningSelection {
        fingerprint: rotation.current_signing_fingerprint(now).clone(),
    }
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
pub struct RunDueScheduledDriftInput {
    pub now: SystemTime,
    pub grace_duration: Duration,
    pub limit: usize,
    pub job_timeout: Duration,
    pub job_expires_in: Duration,
    pub actor: String,
    pub job_id_prefix: String,
    pub nonce_prefix: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunDueScheduledDriftOutput {
    pub created_count: usize,
    pub missed_count: usize,
    pub skipped_disabled_count: usize,
    pub skipped_missing_policy_count: usize,
    pub skipped_missing_agent_count: usize,
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
pub struct CreateRemediationRequestInput {
    pub remediation_id: String,
    pub policy: Policy,
    pub origin: VerifiedDriftEvidence,
    pub actor: String,
    pub requested_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemediationRequestRecord {
    pub id: String,
    pub policy_id: String,
    pub policy_name: String,
    pub agent_id: String,
    pub runbook_ref: String,
    pub status: String,
    pub approval_required: bool,
    pub risk_summary: String,
    pub job_id: Option<String>,
    pub origin_drift_report_id: Option<DriftReportId>,
    pub policy_version: Option<u32>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemediationProposalSave {
    pub remediation: RemediationRequestRecord,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistVerifiedDriftProposalInput {
    pub agent_id: String,
    pub report: DriftReport,
    pub provenance: DriftReportProvenance,
    pub remediation: RemediationRequestRecord,
    pub drift_audit: AuditEvent,
    pub proposal_audit: AuditEvent,
    pub checked_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistVerifiedDriftProposalUseCaseInput {
    pub remediation_id: String,
    pub policy: Policy,
    pub agent_id: String,
    pub report: DriftReport,
    pub provenance: DriftReportProvenance,
    pub actor: String,
    pub requested_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistVerifiedDriftProposalOutput {
    pub report: DriftReportRecord,
    pub proposal: RemediationProposalSave,
}

/// Persists a verified drift report and its first remediation proposal as one durable operation.
pub trait VerifiedDriftProposalRepository {
    type Error;

    fn save_verified_drift_proposal(
        &mut self,
        input: PersistVerifiedDriftProposalInput,
    ) -> Result<PersistVerifiedDriftProposalOutput, Self::Error>;
}

/// Owns the narrow transaction that writes a proposal and its corresponding audit event.
pub trait RemediationProposalRepository {
    type Error;

    fn save_remediation_proposal(
        &mut self,
        remediation: RemediationRequestRecord,
        audit: AuditEvent,
    ) -> Result<RemediationProposalSave, Self::Error>;
}

pub trait RemediationRequestRepository {
    type Error;

    fn save_remediation_request(
        &mut self,
        request: RemediationRequestRecord,
    ) -> Result<(), Self::Error>;

    fn find_remediation_request(
        &self,
        request_id: &str,
    ) -> Result<Option<RemediationRequestRecord>, Self::Error>;

    fn find_remediation_request_by_job_id(
        &self,
        job_id: &str,
    ) -> Result<Option<RemediationRequestRecord>, Self::Error>;

    fn list_remediation_requests(
        &self,
        agent_id: Option<&str>,
        policy_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RemediationRequestRecord>, Self::Error>;

    fn update_remediation_request_status(
        &mut self,
        request_id: &str,
        status: &str,
        job_id: Option<&str>,
        updated_at: SystemTime,
    ) -> Result<(), Self::Error>;
}

/// Persists one task-assignment transition and its optional remediation lifecycle transition.
/// Implementations commit every supplied record and audit event together or roll back all of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemediationExecutionPersistenceInput {
    pub task_id: String,
    pub assignment_status: String,
    pub assignment_last_error: Option<String>,
    pub occurred_at: SystemTime,
    pub remediation: Option<RemediationRequestRecord>,
    pub remediation_audit: Option<AuditEvent>,
}

pub trait RemediationExecutionPersistenceRepository {
    type Error;

    fn persist_remediation_execution_transition(
        &mut self,
        input: RemediationExecutionPersistenceInput,
    ) -> Result<bool, Self::Error>;
}

/// The all-or-nothing persistence boundary for a remediation's post-execution verification.
///
/// The returned job identity is authoritative: a duplicate success event must return the
/// pre-existing correlation rather than create another signed assignment or audit event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemediationVerificationJobPersistenceInput {
    pub remediation_id: String,
    pub job: Job,
    pub task: DriftCheckTask,
    pub assignment: TaskEnvelope,
    pub provenance: DriftJobProvenance,
    pub audit: AuditEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemediationVerificationJobSave {
    pub job_id: String,
    pub created: bool,
}

/// Owns the durable one-to-one verification job correlation for a remediation request.
///
/// Implementations must commit the Job, its sole assignment, the v19 correlation, and audit
/// together, or leave none of those writes behind.
pub trait RemediationVerificationJobRepository:
    RemediationRequestRepository
    + PolicyRepository<Error = <Self as RemediationRequestRepository>::Error>
{
    fn find_remediation_verification_job(
        &self,
        remediation_id: &str,
    ) -> Result<Option<String>, <Self as RemediationRequestRepository>::Error>;

    fn save_remediation_verification_job(
        &mut self,
        input: RemediationVerificationJobPersistenceInput,
    ) -> Result<RemediationVerificationJobSave, <Self as RemediationRequestRepository>::Error>;
}

/// Lists the restart-recovery candidates that still need a remediation verification job.
///
/// The returned records intentionally include legacy rows with an absent execution job or policy
/// version. The create use case remains the invariant gate and the caller records a redacted
/// recovery-skip audit instead of dispatching those rows.
pub trait RemediationVerificationRecoveryRepository {
    type Error;

    fn list_pending_remediation_verification_recovery(
        &self,
        limit: usize,
    ) -> Result<Vec<RemediationRequestRecord>, Self::Error>;
}

/// The hard upper bound for one controller-start recovery pass.
pub const MAX_REMEDIATION_VERIFICATION_RECOVERY_BATCH: usize = 100;

/// Selects the bounded durable backlog that a controller may reconcile before listener readiness.
pub struct ListPendingRemediationVerificationRecovery;

impl ListPendingRemediationVerificationRecovery {
    pub fn execute<R>(repo: &R, limit: usize) -> Result<Vec<RemediationRequestRecord>, R::Error>
    where
        R: RemediationVerificationRecoveryRepository,
    {
        repo.list_pending_remediation_verification_recovery(
            limit.clamp(1, MAX_REMEDIATION_VERIFICATION_RECOVERY_BATCH),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRemediationVerificationJobInput {
    pub remediation_id: String,
    pub verification_job_id: String,
    pub timeout: Duration,
    pub actor: String,
    pub issued_at: SystemTime,
    pub expires_at: SystemTime,
    pub nonce_prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRemediationVerificationJobOutput {
    pub job_id: String,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateRemediationVerificationJobError<RepoError, SignError> {
    NotFound(&'static str),
    InvalidRemediation(String),
    PolicyVersionMismatch { expected: u32, actual: u32 },
    Domain(JobError),
    Agent(AgentError),
    Repository(RepoError),
    Sign(SignError),
}

impl<RepoError, SignError> Display for CreateRemediationVerificationJobError<RepoError, SignError>
where
    RepoError: Display,
    SignError: Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(resource) => write!(formatter, "{resource} was not found"),
            Self::InvalidRemediation(message) => formatter.write_str(message),
            Self::PolicyVersionMismatch { expected, actual } => write!(
                formatter,
                "remediation policy version mismatch: expected {expected}, found {actual}"
            ),
            Self::Domain(error) => Display::fmt(error, formatter),
            Self::Agent(error) => Display::fmt(error, formatter),
            Self::Repository(error) => write!(formatter, "repository error: {error}"),
            Self::Sign(error) => write!(formatter, "sign error: {error}"),
        }
    }
}

impl<RepoError, SignError> std::error::Error
    for CreateRemediationVerificationJobError<RepoError, SignError>
where
    RepoError: std::error::Error + 'static,
    SignError: std::error::Error + 'static,
{
}

pub type CreateRemediationVerificationJobResult<R, S> = Result<
    CreateRemediationVerificationJobOutput,
    CreateRemediationVerificationJobError<
        <R as RemediationRequestRepository>::Error,
        <S as TaskEnvelopeSigner>::Error,
    >,
>;

/// Creates the single signed drift check that verifies a successful remediation.
pub struct CreateRemediationVerificationJob;

impl CreateRemediationVerificationJob {
    pub fn execute<R, S>(
        repo: &mut R,
        signer: &mut S,
        input: CreateRemediationVerificationJobInput,
    ) -> CreateRemediationVerificationJobResult<R, S>
    where
        R: RemediationVerificationJobRepository,
        S: TaskEnvelopeSigner,
    {
        if let Some(job_id) = repo
            .find_remediation_verification_job(&input.remediation_id)
            .map_err(CreateRemediationVerificationJobError::Repository)?
        {
            return Ok(CreateRemediationVerificationJobOutput {
                job_id,
                created: false,
            });
        }

        let Some(record) = repo
            .find_remediation_request(&input.remediation_id)
            .map_err(CreateRemediationVerificationJobError::Repository)?
        else {
            return Err(CreateRemediationVerificationJobError::NotFound(
                "remediation",
            ));
        };
        let request = remediation_record_to_request(&record).map_err(|error| {
            CreateRemediationVerificationJobError::InvalidRemediation(error.to_string())
        })?;
        if request.status != RemediationStatus::SucceededPendingVerify || record.job_id.is_none() {
            return Err(CreateRemediationVerificationJobError::InvalidRemediation(
                "verification requires a successful persisted remediation execution".to_owned(),
            ));
        }

        let Some(policy) = repo
            .find_policy(&record.policy_id)
            .map_err(CreateRemediationVerificationJobError::Repository)?
        else {
            return Err(CreateRemediationVerificationJobError::NotFound("policy"));
        };
        let Some(expected_policy_version) = record.policy_version else {
            return Err(CreateRemediationVerificationJobError::InvalidRemediation(
                "verification requires a persisted remediation policy version".to_owned(),
            ));
        };
        if policy.version != expected_policy_version {
            return Err(
                CreateRemediationVerificationJobError::PolicyVersionMismatch {
                    expected: expected_policy_version,
                    actual: policy.version,
                },
            );
        }

        let task = DriftCheckTask::new(policy.source, input.timeout)
            .map_err(CreateRemediationVerificationJobError::Domain)?;
        let mut job = Job::new(
            JobId::new(input.verification_job_id.clone())
                .map_err(CreateRemediationVerificationJobError::Domain)?,
            task.risk(),
            fleet_domain::ApprovalRequirement::NotRequired,
            input.timeout,
        );
        job.queue(false)
            .map_err(CreateRemediationVerificationJobError::Domain)?;
        let target = JobTarget {
            agent_id: AgentId::new(record.agent_id.clone())
                .map_err(CreateRemediationVerificationJobError::Agent)?,
        };
        let payload_hash = drift_check_payload_hash(&task, &target, 0);
        let signature = signer
            .sign(&payload_hash)
            .map_err(CreateRemediationVerificationJobError::Sign)?;
        let assignment = TaskEnvelope {
            job_id: JobId::new(input.verification_job_id.clone())
                .map_err(CreateRemediationVerificationJobError::Domain)?,
            task_id: TaskId::new(format!("{}-task-0", input.verification_job_id))
                .map_err(CreateRemediationVerificationJobError::Domain)?,
            target_agent_id: target.agent_id.clone(),
            issued_at: input.issued_at,
            expires_at: TaskExpiry::new(input.expires_at),
            nonce: TaskNonce::new(format!("{}-0", input.nonce_prefix))
                .map_err(CreateRemediationVerificationJobError::Domain)?,
            payload_hash,
            signature: Some(
                TaskSignature::new(signature)
                    .map_err(CreateRemediationVerificationJobError::Domain)?,
            ),
        };
        let provenance = DriftJobProvenance::remediation_verification(
            record.policy_id.clone(),
            expected_policy_version,
        );
        let audit = AuditEvent {
            category: AuditCategory::Policy,
            action: "remediation_verification_created".to_owned(),
            actor: AuditActor::new(input.actor),
            target: AuditTarget::new(record.agent_id.clone()),
            value: AuditValue::Plain(format!(
                "remediation_id={},policy_id={},agent_id={},job_id={},purpose={}",
                record.id,
                record.policy_id,
                record.agent_id,
                input.verification_job_id,
                provenance.purpose.as_str(),
            )),
            occurred_at: input.issued_at,
        };
        let saved = repo
            .save_remediation_verification_job(RemediationVerificationJobPersistenceInput {
                remediation_id: record.id,
                job,
                task,
                assignment: assignment.clone(),
                provenance,
                audit,
            })
            .map_err(CreateRemediationVerificationJobError::Repository)?;

        Ok(CreateRemediationVerificationJobOutput {
            job_id: saved.job_id,
            created: saved.created,
        })
    }
}

pub trait RemediationApprovalRepository:
    ApprovalRepository + RemediationRequestRepository<Error = <Self as ApprovalRepository>::Error>
{
}

impl<T> RemediationApprovalRepository for T where
    T: ApprovalRepository + RemediationRequestRepository<Error = <T as ApprovalRepository>::Error>
{
}

pub trait RemediationJobRepository:
    RunbookJobRepository
    + RemediationRequestRepository<Error = <Self as TaskAssignmentRepository>::Error>
{
}

impl<T> RemediationJobRepository for T where
    T: RunbookJobRepository
        + RemediationRequestRepository<Error = <T as TaskAssignmentRepository>::Error>
{
}

pub trait RemediationResultRepository:
    PolicyRepository + RemediationRequestRepository<Error = <Self as PolicyRepository>::Error>
{
}

impl<T> RemediationResultRepository for T where
    T: PolicyRepository + RemediationRequestRepository<Error = <T as PolicyRepository>::Error>
{
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestRemediationApprovalInput {
    pub remediation_id: String,
    pub approval_id: String,
    pub job_id: String,
    pub requester: String,
    pub reason: String,
    pub expires_at: SystemTime,
    pub now: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestRemediationApprovalOutput {
    pub remediation: RemediationRequestRecord,
    pub approval: ApprovalRequestRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemediationApprovalRequestError<RepoError, AuditError> {
    Domain(String),
    NotFound(&'static str),
    Repository(RepoError),
    Audit(AuditError),
}

pub type RequestRemediationApprovalResult<R, A> = Result<
    RequestRemediationApprovalOutput,
    RemediationApprovalRequestError<<R as ApprovalRepository>::Error, <A as AuditWriter>::Error>,
>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApproveRemediationRunbookJobInput {
    pub remediation_id: String,
    pub approval_id: String,
    pub job_id: String,
    pub runbook_document: String,
    pub timeout: Duration,
    pub approver: String,
    pub approval_reason: String,
    pub issued_at: SystemTime,
    pub expires_at: SystemTime,
    pub nonce_prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApproveRemediationRunbookJobOutput {
    pub remediation: RemediationRequestRecord,
    pub approval: ApprovalRequestRecord,
    pub task: RunbookExecutionTask,
    pub targets: Vec<JobTarget>,
    pub envelopes: Vec<TaskEnvelope>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApproveRemediationRunbookJobError<RepoError, AuditError, SignError> {
    Domain(String),
    InvalidRunbook(String),
    NoTargets,
    NotFound(&'static str),
    Repository(RepoError),
    Audit(AuditError),
    Sign(SignError),
}

pub type ApproveRemediationRunbookJobResult<R, A, S> = Result<
    ApproveRemediationRunbookJobOutput,
    ApproveRemediationRunbookJobError<
        <R as TaskAssignmentRepository>::Error,
        <A as AuditWriter>::Error,
        <S as TaskEnvelopeSigner>::Error,
    >,
>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemediationJobResultStatus {
    Succeeded,
    Failed,
    Canceled,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemediationExecutionTransition {
    Started,
    Result(RemediationJobResultStatus),
}

pub struct PrepareRemediationExecutionTransition;

impl PrepareRemediationExecutionTransition {
    /// Produces the durable remediation update and its audit event without performing I/O.
    pub fn execute(
        record: RemediationRequestRecord,
        job_id: &str,
        transition: RemediationExecutionTransition,
        actor: &str,
        occurred_at: SystemTime,
    ) -> Result<Option<(RemediationRequestRecord, AuditEvent)>, String> {
        let mut request =
            remediation_record_to_request(&record).map_err(|error| format!("{error:?}"))?;
        if request.job_id.as_deref() != Some(job_id) {
            return Err("remediation execution job mismatch".to_owned());
        }
        let action = match transition {
            RemediationExecutionTransition::Started
                if request.status == RemediationStatus::JobCreated =>
            {
                request
                    .mark_running()
                    .map_err(|error| format!("{error:?}"))?;
                "remediation_job_running"
            }
            RemediationExecutionTransition::Result(RemediationJobResultStatus::Succeeded)
                if request.status == RemediationStatus::Running =>
            {
                request
                    .job_succeeded()
                    .map_err(|error| format!("{error:?}"))?;
                "remediation_job_succeeded_pending_verify"
            }
            RemediationExecutionTransition::Result(RemediationJobResultStatus::Failed)
                if !request.status.is_terminal() =>
            {
                request
                    .mark_failed()
                    .map_err(|error| format!("{error:?}"))?;
                "remediation_job_failed"
            }
            RemediationExecutionTransition::Result(RemediationJobResultStatus::Canceled)
                if !request.status.is_terminal() =>
            {
                request.cancel().map_err(|error| format!("{error:?}"))?;
                "remediation_job_canceled"
            }
            RemediationExecutionTransition::Result(RemediationJobResultStatus::Expired)
                if !request.status.is_terminal() =>
            {
                request.expire().map_err(|error| format!("{error:?}"))?;
                "remediation_job_expired"
            }
            _ => return Ok(None),
        };
        let updated = RemediationRequestRecord {
            status: request.status.as_str().to_owned(),
            job_id: request.job_id.clone(),
            updated_at: occurred_at,
            ..record
        };
        let audit = AuditEvent {
            category: AuditCategory::Policy,
            action: action.to_owned(),
            actor: AuditActor::new(actor.to_owned()),
            target: AuditTarget::new(request.agent_id.clone()),
            value: AuditValue::Plain(format!(
                "remediation_id={},policy_id={},policy_name={},agent_id={},job_id={},status={}",
                request.id,
                request.policy_id,
                request.policy_name,
                request.agent_id,
                job_id,
                request.status.as_str()
            )),
            occurred_at,
        };
        Ok(Some((updated, audit)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkRemediationJobRunningInput {
    pub remediation_id: String,
    pub job_id: String,
    pub actor: String,
    pub occurred_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordRemediationJobResultInput {
    pub remediation_id: String,
    pub job_id: String,
    pub status: RemediationJobResultStatus,
    pub actor: String,
    pub occurred_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyRemediationResolutionInput {
    pub remediation_id: String,
    pub agent_id: String,
    pub policy_id: String,
    pub policy_name: String,
    pub job_id: String,
    pub actor: String,
    pub verified_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemediationLifecycleOutput {
    pub remediation: RemediationRequestRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemediationResultUseCaseError<RepoError, AuditError> {
    Domain(String),
    NotFound(&'static str),
    Mismatch(&'static str),
    Repository(RepoError),
    Audit(AuditError),
}

pub type RemediationResultUseCaseResult<R, A> = Result<
    RemediationLifecycleOutput,
    RemediationResultUseCaseError<<R as PolicyRepository>::Error, <A as AuditWriter>::Error>,
>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyUseCaseError<RepoError, AuditError> {
    Domain(String),
    NotFound(String),
    Repository(RepoError),
    Audit(AuditError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunDueScheduledDriftError<RepoError, AuditError, SignError> {
    Domain(String),
    Repository(RepoError),
    Audit(AuditError),
    Sign(SignError),
}

pub type RunDueScheduledDriftResult<RepoError, AuditError, SignError> =
    Result<RunDueScheduledDriftOutput, RunDueScheduledDriftError<RepoError, AuditError, SignError>>;

impl<RepoError, AuditError, SignError> Display
    for RunDueScheduledDriftError<RepoError, AuditError, SignError>
where
    RepoError: Display,
    AuditError: Display,
    SignError: Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Domain(error) => write!(formatter, "{error}"),
            Self::Repository(error) => write!(formatter, "repository error: {error}"),
            Self::Audit(error) => write!(formatter, "audit error: {error}"),
            Self::Sign(error) => write!(formatter, "sign error: {error}"),
        }
    }
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

pub struct RunDueScheduledDrift;

impl RunDueScheduledDrift {
    pub fn execute<R, A, S>(
        repo: &mut R,
        audit: &mut A,
        signer: &mut S,
        input: RunDueScheduledDriftInput,
    ) -> RunDueScheduledDriftResult<<R as PolicyRepository>::Error, A::Error, S::Error>
    where
        R: ScheduledDriftRepository,
        A: AuditWriter,
        S: TaskEnvelopeSigner,
    {
        let schedules = repo
            .due_scheduled_drift_checks(input.now, input.limit)
            .map_err(RunDueScheduledDriftError::Repository)?;
        let mut output = RunDueScheduledDriftOutput::default();

        for schedule in schedules {
            let Some(due) = scheduled_drift_due(
                schedule.policy_id.clone(),
                schedule.agent_id.clone(),
                schedule.next_due_at,
                input.now,
                input.grace_duration,
            ) else {
                continue;
            };
            let Some(policy) = repo
                .find_policy(&schedule.policy_id)
                .map_err(RunDueScheduledDriftError::Repository)?
            else {
                output.skipped_missing_policy_count += 1;
                write_scheduled_drift_audit(
                    audit,
                    "scheduled_drift_skipped_missing_policy",
                    &input.actor,
                    &schedule,
                    input.now,
                )?;
                continue;
            };
            let agent_id = AgentId::new(schedule.agent_id.clone())
                .map_err(|error| RunDueScheduledDriftError::Domain(error.to_string()))?;
            let Some(agent) = repo
                .find_by_id(&agent_id)
                .map_err(RunDueScheduledDriftError::Repository)?
            else {
                output.skipped_missing_agent_count += 1;
                write_scheduled_drift_audit(
                    audit,
                    "scheduled_drift_skipped_missing_agent",
                    &input.actor,
                    &schedule,
                    input.now,
                )?;
                continue;
            };
            if agent.status() == AgentStatus::Disabled {
                output.skipped_disabled_count += 1;
                write_scheduled_drift_audit(
                    audit,
                    "scheduled_drift_skipped_disabled_agent",
                    &input.actor,
                    &schedule,
                    input.now,
                )?;
                repo.record_scheduled_drift_check(
                    &schedule.policy_id,
                    &schedule.agent_id,
                    input.now,
                )
                .map_err(RunDueScheduledDriftError::Repository)?;
                continue;
            }
            if due.missed {
                output.missed_count += 1;
                write_scheduled_drift_audit(
                    audit,
                    "scheduled_drift_missed",
                    &input.actor,
                    &schedule,
                    input.now,
                )?;
            }

            let job_id = scheduled_drift_job_id(&input.job_id_prefix, &schedule, input.now);
            CreateDriftCheckJob::execute(
                repo,
                audit,
                signer,
                CreateDriftCheckJobInput {
                    job_id: job_id.clone(),
                    target_agent_ids: vec![schedule.agent_id.clone()],
                    policy_document: policy.source,
                    provenance: Some(DriftJobProvenance::scheduled(policy.id, policy.version)),
                    timeout: input.job_timeout,
                    created_by: input.actor.clone(),
                    issued_at: input.now,
                    expires_at: input.now + input.job_expires_in,
                    nonce_prefix: format!("{}-{}", input.nonce_prefix, job_id),
                    approval_request_id: format!("approval-{job_id}"),
                    approval_expires_at: input.now + input.job_expires_in,
                },
            )
            .map_err(map_scheduled_drift_job_error)?;
            repo.record_scheduled_drift_check(&schedule.policy_id, &schedule.agent_id, input.now)
                .map_err(RunDueScheduledDriftError::Repository)?;
            write_scheduled_drift_audit(
                audit,
                "scheduled_drift_job_created",
                &input.actor,
                &schedule,
                input.now,
            )?;
            output.created_count += 1;
        }

        Ok(output)
    }
}

fn map_scheduled_drift_job_error<RepoError, AuditError, SignError>(
    error: CreateDriftCheckJobError<RepoError, AuditError, SignError>,
) -> RunDueScheduledDriftError<RepoError, AuditError, SignError> {
    match error {
        CreateDriftCheckJobError::Domain(error) => {
            RunDueScheduledDriftError::Domain(error.to_string())
        }
        CreateDriftCheckJobError::Agent(error) => {
            RunDueScheduledDriftError::Domain(error.to_string())
        }
        CreateDriftCheckJobError::NoTargets => {
            RunDueScheduledDriftError::Domain("scheduled drift job requires a target".to_owned())
        }
        CreateDriftCheckJobError::Repository(error) => RunDueScheduledDriftError::Repository(error),
        CreateDriftCheckJobError::Audit(error) => RunDueScheduledDriftError::Audit(error),
        CreateDriftCheckJobError::Sign(error) => RunDueScheduledDriftError::Sign(error),
    }
}

fn write_scheduled_drift_audit<A, RepoError, SignError>(
    audit: &mut A,
    action: &str,
    actor: &str,
    schedule: &ScheduledDriftRecord,
    occurred_at: SystemTime,
) -> Result<(), RunDueScheduledDriftError<RepoError, A::Error, SignError>>
where
    A: AuditWriter,
{
    audit
        .write(AuditEvent {
            category: AuditCategory::Drift,
            action: action.to_owned(),
            actor: AuditActor::new(actor),
            target: AuditTarget::new(schedule.agent_id.clone()),
            value: AuditValue::Plain(format!("policy_id={}", schedule.policy_id)),
            occurred_at,
        })
        .map_err(RunDueScheduledDriftError::Audit)
}

fn scheduled_drift_job_id(
    prefix: &str,
    schedule: &ScheduledDriftRecord,
    now: SystemTime,
) -> String {
    format!(
        "{}-{}-{}-{}",
        prefix,
        schedule.policy_id,
        schedule.agent_id,
        system_time_to_millis(now)
    )
}

fn system_time_to_millis(time: SystemTime) -> u128 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn system_time_millis_u64(time: SystemTime) -> u64 {
    system_time_to_millis(time).try_into().unwrap_or(u64::MAX)
}

pub struct CreateRemediationApproval;

pub struct CreateRemediationRequestProposal;

pub struct PersistVerifiedDriftProposal;

impl PersistVerifiedDriftProposal {
    pub fn execute<R>(
        repo: &mut R,
        input: PersistVerifiedDriftProposalUseCaseInput,
    ) -> Result<PersistVerifiedDriftProposalOutput, PolicyUseCaseError<R::Error, R::Error>>
    where
        R: VerifiedDriftProposalRepository,
    {
        VerifiedDriftEvidence::validate_remediation_candidate(
            &input.report,
            &input.provenance,
            &input.policy,
        )
        .map_err(|error| PolicyUseCaseError::Domain(format!("{error:?}")))?;
        let request = RemediationRequest::propose_from_drift(
            input.remediation_id,
            &input.policy,
            input.agent_id.clone(),
            &input.report,
        )
        .map_err(|error| PolicyUseCaseError::Domain(format!("{error:?}")))?;
        let remediation = RemediationRequestRecord {
            id: request.id.clone(),
            policy_id: request.policy_id.clone(),
            policy_name: request.policy_name.clone(),
            agent_id: request.agent_id.clone(),
            runbook_ref: request.runbook_ref.clone(),
            status: request.status.as_str().to_owned(),
            approval_required: request.approval_required,
            risk_summary: request.risk_summary.clone(),
            job_id: request.job_id.clone(),
            origin_drift_report_id: None,
            policy_version: input.provenance.policy_version,
            created_at: input.requested_at,
            updated_at: input.requested_at,
        };
        let drift_audit = AuditEvent {
            category: AuditCategory::Drift,
            action: "drift_report_received".to_owned(),
            actor: AuditActor::new("agent"),
            target: AuditTarget::new(input.agent_id.clone()),
            value: AuditValue::Plain(format!(
                "policy_name={},status=drifted",
                input.report.policy_name
            )),
            occurred_at: input.requested_at,
        };
        let proposal_audit = AuditEvent {
            category: AuditCategory::Policy,
            action: "remediation_requested".to_owned(),
            actor: AuditActor::new(input.actor),
            target: AuditTarget::new(remediation.agent_id.clone()),
            value: AuditValue::Plain(format!(
                "remediation_id={},policy_id={},agent_id={},runbook_ref={},status={}",
                remediation.id,
                remediation.policy_id,
                remediation.agent_id,
                remediation.runbook_ref,
                remediation.status
            )),
            occurred_at: input.requested_at,
        };
        repo.save_verified_drift_proposal(PersistVerifiedDriftProposalInput {
            agent_id: input.agent_id,
            report: input.report,
            provenance: input.provenance,
            remediation,
            drift_audit,
            proposal_audit,
            checked_at: input.requested_at,
        })
        .map_err(PolicyUseCaseError::Repository)
    }
}

impl CreateRemediationRequestProposal {
    pub fn execute<R>(
        repo: &mut R,
        input: CreateRemediationRequestInput,
    ) -> Result<RemediationProposalSave, PolicyUseCaseError<R::Error, R::Error>>
    where
        R: RemediationProposalRepository,
    {
        let request = RemediationRequest::propose_from_verified_drift(
            input.remediation_id,
            &input.policy,
            &input.origin,
        )
        .map_err(|error| PolicyUseCaseError::Domain(format!("{error:?}")))?;
        let record = RemediationRequestRecord {
            id: request.id.clone(),
            policy_id: request.policy_id.clone(),
            policy_name: request.policy_name.clone(),
            agent_id: request.agent_id.clone(),
            runbook_ref: request.runbook_ref.clone(),
            status: request.status.as_str().to_owned(),
            approval_required: request.approval_required,
            risk_summary: request.risk_summary.clone(),
            job_id: request.job_id.clone(),
            origin_drift_report_id: Some(input.origin.report_id),
            policy_version: input.origin.provenance.policy_version,
            created_at: input.requested_at,
            updated_at: input.requested_at,
        };
        let audit = AuditEvent {
            category: AuditCategory::Policy,
            action: "remediation_requested".to_owned(),
            actor: AuditActor::new(input.actor),
            target: AuditTarget::new(record.agent_id.clone()),
            value: AuditValue::Plain(format!(
                "remediation_id={},policy_id={},agent_id={},runbook_ref={},status={}",
                record.id, record.policy_id, record.agent_id, record.runbook_ref, record.status
            )),
            occurred_at: input.requested_at,
        };
        repo.save_remediation_proposal(record, audit)
            .map_err(PolicyUseCaseError::Repository)
    }
}

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

pub struct RequestRemediationApproval;

impl RequestRemediationApproval {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: RequestRemediationApprovalInput,
    ) -> RequestRemediationApprovalResult<R, A>
    where
        R: RemediationApprovalRepository,
        A: AuditWriter,
    {
        let Some(record) = repo
            .find_remediation_request(&input.remediation_id)
            .map_err(RemediationApprovalRequestError::Repository)?
        else {
            return Err(RemediationApprovalRequestError::NotFound("remediation"));
        };
        let mut request = remediation_record_to_request(&record)
            .map_err(RemediationApprovalRequestError::Domain)?;
        request
            .request_approval()
            .map_err(|error| RemediationApprovalRequestError::Domain(format!("{error:?}")))?;

        let approval = ApprovalRequest::new(
            ApprovalId::new(input.approval_id.clone())
                .map_err(|error| RemediationApprovalRequestError::Domain(error.to_string()))?,
            JobId::new(input.job_id.clone())
                .map_err(|error| RemediationApprovalRequestError::Domain(error.to_string()))?,
            input.requester.clone(),
            input.reason,
            input.expires_at,
            input.now,
        )
        .map_err(|error| RemediationApprovalRequestError::Domain(error.to_string()))?;
        let approval_record = approval_request_to_record(&approval);
        repo.insert_approval_request(approval_record.clone())
            .map_err(RemediationApprovalRequestError::Repository)?;
        repo.update_remediation_request_status(
            &request.id,
            request.status.as_str(),
            request.job_id.as_deref(),
            input.now,
        )
        .map_err(RemediationApprovalRequestError::Repository)?;

        let remediation_record = RemediationRequestRecord {
            status: request.status.as_str().to_owned(),
            job_id: request.job_id.clone(),
            updated_at: input.now,
            ..record
        };
        audit
            .write(AuditEvent {
                category: AuditCategory::Policy,
                action: "remediation_approval_requested".to_owned(),
                actor: AuditActor::new(input.requester),
                target: AuditTarget::new(request.agent_id.clone()),
                value: AuditValue::Plain(format!(
                    "remediation_id={},approval_id={},policy_id={},agent_id={},job_id={},status={}",
                    request.id,
                    approval_record.id,
                    request.policy_id,
                    request.agent_id,
                    approval_record.job_id,
                    request.status.as_str()
                )),
                occurred_at: input.now,
            })
            .map_err(RemediationApprovalRequestError::Audit)?;

        Ok(RequestRemediationApprovalOutput {
            remediation: remediation_record,
            approval: approval_record,
        })
    }
}

pub struct ApproveRemediationRunbookJob;

impl ApproveRemediationRunbookJob {
    pub fn execute<R, A, S>(
        repo: &mut R,
        audit: &mut A,
        signer: &mut S,
        input: ApproveRemediationRunbookJobInput,
    ) -> ApproveRemediationRunbookJobResult<R, A, S>
    where
        R: RemediationJobRepository,
        A: AuditWriter,
        S: TaskEnvelopeSigner,
    {
        let Some(record) = repo
            .find_remediation_request(&input.remediation_id)
            .map_err(ApproveRemediationRunbookJobError::Repository)?
        else {
            return Err(ApproveRemediationRunbookJobError::NotFound("remediation"));
        };
        let mut request = remediation_record_to_request(&record)
            .map_err(ApproveRemediationRunbookJobError::Domain)?;
        request
            .approve()
            .map_err(|error| ApproveRemediationRunbookJobError::Domain(format!("{error:?}")))?;

        let Some(approval_record) = repo
            .find_approval_request(&input.approval_id)
            .map_err(ApproveRemediationRunbookJobError::Repository)?
        else {
            return Err(ApproveRemediationRunbookJobError::NotFound("approval"));
        };
        if approval_record.job_id != input.job_id {
            return Err(ApproveRemediationRunbookJobError::Domain(
                "approval job id does not match remediation job id".to_owned(),
            ));
        }
        let mut approval = approval_record_to_request(&approval_record)
            .map_err(|error| ApproveRemediationRunbookJobError::Domain(error.to_string()))?;
        approval
            .approve(
                input.approver.clone(),
                input.approval_reason.clone(),
                input.issued_at,
            )
            .map_err(|error| ApproveRemediationRunbookJobError::Domain(error.to_string()))?;
        let updated_approval = approval_request_to_record(&approval);

        let mut signed = build_signed_runbook_job(
            signer,
            BuildSignedRunbookJobInput {
                job_id: input.job_id.clone(),
                target_agent_ids: vec![request.agent_id.clone()],
                runbook_document: input.runbook_document,
                timeout: input.timeout,
                issued_at: input.issued_at,
                expires_at: input.expires_at,
                nonce_prefix: input.nonce_prefix,
            },
        )
        .map_err(map_remediation_runbook_job_error)?;
        signed
            .job
            .queue(true)
            .map_err(|error| ApproveRemediationRunbookJobError::Domain(error.to_string()))?;

        repo.save_runbook_job_with_assignments(signed.job, &signed.task, &signed.envelopes)
            .map_err(ApproveRemediationRunbookJobError::Repository)?;

        request
            .create_job(input.job_id.clone())
            .map_err(|error| ApproveRemediationRunbookJobError::Domain(format!("{error:?}")))?;
        repo.update_approval_request(updated_approval.clone())
            .map_err(ApproveRemediationRunbookJobError::Repository)?;
        repo.update_remediation_request_status(
            &request.id,
            request.status.as_str(),
            request.job_id.as_deref(),
            input.issued_at,
        )
        .map_err(ApproveRemediationRunbookJobError::Repository)?;

        let remediation_record = RemediationRequestRecord {
            status: request.status.as_str().to_owned(),
            job_id: request.job_id.clone(),
            updated_at: input.issued_at,
            ..record
        };
        audit
            .write(AuditEvent {
                category: AuditCategory::Approval,
                action: "approval_approved".to_owned(),
                actor: AuditActor::new(input.approver.clone()),
                target: AuditTarget::new(input.job_id.clone()),
                value: AuditValue::Plain(format!(
                    "approval_id={},reason={}",
                    updated_approval.id, updated_approval.reason
                )),
                occurred_at: input.issued_at,
            })
            .map_err(ApproveRemediationRunbookJobError::Audit)?;
        audit
            .write(AuditEvent {
                category: AuditCategory::Job,
                action: "runbook_job_created".to_owned(),
                actor: AuditActor::new(input.approver.clone()),
                target: AuditTarget::new(input.job_id.clone()),
                value: AuditValue::Plain(format!(
                    "confirmed_high_risk=true,confirmed_by={},target_count={}",
                    input.approver,
                    signed.targets.len()
                )),
                occurred_at: input.issued_at,
            })
            .map_err(ApproveRemediationRunbookJobError::Audit)?;
        audit
            .write(AuditEvent {
                category: AuditCategory::Policy,
                action: "remediation_job_created".to_owned(),
                actor: AuditActor::new(updated_approval.approver.clone().unwrap_or_default()),
                target: AuditTarget::new(request.agent_id.clone()),
                value: AuditValue::Plain(format!(
                    "remediation_id={},approval_id={},policy_id={},agent_id={},job_id={},status={}",
                    request.id,
                    updated_approval.id,
                    request.policy_id,
                    request.agent_id,
                    input.job_id,
                    request.status.as_str()
                )),
                occurred_at: input.issued_at,
            })
            .map_err(ApproveRemediationRunbookJobError::Audit)?;

        Ok(ApproveRemediationRunbookJobOutput {
            remediation: remediation_record,
            approval: updated_approval,
            task: signed.task,
            targets: signed.targets,
            envelopes: signed.envelopes,
        })
    }
}

fn remediation_record_to_request(
    record: &RemediationRequestRecord,
) -> Result<RemediationRequest, String> {
    let status = RemediationStatus::parse(&record.status)
        .ok_or_else(|| format!("invalid remediation status: {}", record.status))?;
    Ok(RemediationRequest {
        id: record.id.clone(),
        policy_id: record.policy_id.clone(),
        policy_name: record.policy_name.clone(),
        agent_id: record.agent_id.clone(),
        runbook_ref: record.runbook_ref.clone(),
        approval_required: record.approval_required,
        status,
        risk_summary: record.risk_summary.clone(),
        job_id: record.job_id.clone(),
    })
}

fn map_remediation_runbook_job_error<RepoError, AuditError, SignError>(
    error: BuildSignedRunbookJobError<SignError>,
) -> ApproveRemediationRunbookJobError<RepoError, AuditError, SignError> {
    match error {
        BuildSignedRunbookJobError::Domain(error) => {
            ApproveRemediationRunbookJobError::Domain(error.to_string())
        }
        BuildSignedRunbookJobError::Agent(error) => {
            ApproveRemediationRunbookJobError::Domain(error.to_string())
        }
        BuildSignedRunbookJobError::InvalidRunbook(error) => {
            ApproveRemediationRunbookJobError::InvalidRunbook(error)
        }
        BuildSignedRunbookJobError::NoTargets => ApproveRemediationRunbookJobError::NoTargets,
        BuildSignedRunbookJobError::Sign(error) => ApproveRemediationRunbookJobError::Sign(error),
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

pub struct MarkRemediationJobRunning;

impl MarkRemediationJobRunning {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: MarkRemediationJobRunningInput,
    ) -> RemediationResultUseCaseResult<R, A>
    where
        R: RemediationResultRepository,
        A: AuditWriter,
    {
        let Some(record) = repo
            .find_remediation_request(&input.remediation_id)
            .map_err(RemediationResultUseCaseError::Repository)?
        else {
            return Err(RemediationResultUseCaseError::NotFound("remediation"));
        };
        let mut request = remediation_record_to_request(&record)
            .map_err(RemediationResultUseCaseError::Domain)?;
        ensure_remediation_job_matches(&request, &input.job_id)?;
        if request.status == RemediationStatus::Running {
            return Ok(RemediationLifecycleOutput {
                remediation: record,
            });
        }
        request
            .mark_running()
            .map_err(|error| RemediationResultUseCaseError::Domain(format!("{error:?}")))?;
        let updated = update_remediation_record::<R, A>(repo, record, &request, input.occurred_at)?;
        write_remediation_lifecycle_audit(
            audit,
            "remediation_job_running",
            &input.actor,
            &request,
            input.occurred_at,
        )?;
        Ok(RemediationLifecycleOutput {
            remediation: updated,
        })
    }
}

pub struct RecordRemediationJobResult;

impl RecordRemediationJobResult {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: RecordRemediationJobResultInput,
    ) -> RemediationResultUseCaseResult<R, A>
    where
        R: RemediationResultRepository,
        A: AuditWriter,
    {
        let Some(record) = repo
            .find_remediation_request(&input.remediation_id)
            .map_err(RemediationResultUseCaseError::Repository)?
        else {
            return Err(RemediationResultUseCaseError::NotFound("remediation"));
        };
        let mut request = remediation_record_to_request(&record)
            .map_err(RemediationResultUseCaseError::Domain)?;
        ensure_remediation_job_matches(&request, &input.job_id)?;
        let action = match input.status {
            RemediationJobResultStatus::Succeeded => {
                if request.status != RemediationStatus::Running {
                    return Ok(RemediationLifecycleOutput {
                        remediation: record,
                    });
                }
                request
                    .job_succeeded()
                    .map_err(|error| RemediationResultUseCaseError::Domain(format!("{error:?}")))?;
                "remediation_job_succeeded_pending_verify"
            }
            RemediationJobResultStatus::Failed => {
                if request.status == RemediationStatus::Failed {
                    return Ok(RemediationLifecycleOutput {
                        remediation: record,
                    });
                }
                request
                    .mark_failed()
                    .map_err(|error| RemediationResultUseCaseError::Domain(format!("{error:?}")))?;
                "remediation_job_failed"
            }
            RemediationJobResultStatus::Canceled => {
                request
                    .cancel()
                    .map_err(|error| RemediationResultUseCaseError::Domain(format!("{error:?}")))?;
                "remediation_job_canceled"
            }
            RemediationJobResultStatus::Expired => {
                request
                    .expire()
                    .map_err(|error| RemediationResultUseCaseError::Domain(format!("{error:?}")))?;
                "remediation_job_expired"
            }
        };
        let updated = update_remediation_record::<R, A>(repo, record, &request, input.occurred_at)?;
        write_remediation_lifecycle_audit(
            audit,
            action,
            &input.actor,
            &request,
            input.occurred_at,
        )?;
        Ok(RemediationLifecycleOutput {
            remediation: updated,
        })
    }
}

pub struct VerifyRemediationResolution;

/// Persisted evidence required to resolve a remediation after its verification drift job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveRemediationVerificationEvidenceInput {
    pub remediation_id: String,
    pub verification_job_id: String,
    pub verification_task_id: String,
    pub evidence_report_id: DriftReportId,
    pub agent_id: String,
    pub policy_id: String,
    pub policy_name: String,
    pub policy_version: u32,
    pub status: DriftStatus,
    pub checked_at: SystemTime,
    /// Terminal completion time of the remediation execution Job, not the
    /// verification Job that emits this evidence.
    pub remediation_execution_completed_at: SystemTime,
    pub actor: String,
}

/// Commits the resolved remediation, its origin drift report, and audit together.
pub trait RemediationVerificationResolutionRepository:
    RemediationVerificationJobRepository
{
    fn resolve_remediation_verification_evidence(
        &mut self,
        remediation: RemediationRequestRecord,
        origin_drift_report_id: DriftReportId,
        evidence_report_id: DriftReportId,
        verification_job_id: &str,
        verification_task_id: &str,
        audit: AuditEvent,
    ) -> Result<RemediationRequestRecord, <Self as RemediationRequestRepository>::Error>;
}

/// Resolves only an evidence record that is fresh, compliant, and bound to the verification job.
pub struct ResolveRemediationVerificationEvidence;

impl ResolveRemediationVerificationEvidence {
    pub fn execute<R, A>(
        repo: &mut R,
        _audit: &mut A,
        input: ResolveRemediationVerificationEvidenceInput,
    ) -> Result<
        Option<RemediationLifecycleOutput>,
        RemediationResultUseCaseError<<R as RemediationRequestRepository>::Error, A::Error>,
    >
    where
        R: RemediationVerificationResolutionRepository,
        A: AuditWriter,
    {
        if input.status != DriftStatus::Compliant
            || input.checked_at <= input.remediation_execution_completed_at
        {
            return Ok(None);
        }
        let Some(record) = repo
            .find_remediation_request(&input.remediation_id)
            .map_err(RemediationResultUseCaseError::Repository)?
        else {
            return Err(RemediationResultUseCaseError::NotFound("remediation"));
        };
        if repo
            .find_remediation_verification_job(&input.remediation_id)
            .map_err(RemediationResultUseCaseError::Repository)?
            .as_deref()
            != Some(input.verification_job_id.as_str())
        {
            return Ok(None);
        }
        let mut request = remediation_record_to_request(&record)
            .map_err(RemediationResultUseCaseError::Domain)?;
        if record.policy_version != Some(input.policy_version)
            || ensure_remediation_evidence_matches::<
                <R as RemediationRequestRepository>::Error,
                A::Error,
            >(
                &request,
                &input.agent_id,
                &input.policy_id,
                &input.policy_name,
                request.job_id.as_deref().unwrap_or_default(),
            )
            .is_err()
        {
            return Ok(None);
        }
        request
            .verify_resolved()
            .map_err(|error| RemediationResultUseCaseError::Domain(format!("{error:?}")))?;
        let Some(origin_drift_report_id) = record.origin_drift_report_id else {
            return Ok(None);
        };
        let updated = repo
            .resolve_remediation_verification_evidence(
                RemediationRequestRecord {
                    status: request.status.as_str().to_owned(),
                    job_id: request.job_id.clone(),
                    updated_at: input.checked_at,
                    ..record
                },
                origin_drift_report_id,
                input.evidence_report_id,
                &input.verification_job_id,
                &input.verification_task_id,
                AuditEvent {
                    category: AuditCategory::Policy,
                    action: "remediation_resolved_by_verification".to_owned(),
                    actor: AuditActor::new(input.actor),
                    target: AuditTarget::new(request.agent_id.clone()),
                    value: AuditValue::Plain(format!(
                        "remediation_id={},verification_job_id={},evidence_report_id={}",
                        request.id,
                        input.verification_job_id,
                        input.evidence_report_id.as_i64()
                    )),
                    occurred_at: input.checked_at,
                },
            )
            .map_err(RemediationResultUseCaseError::Repository)?;
        Ok(Some(RemediationLifecycleOutput {
            remediation: updated,
        }))
    }
}

impl VerifyRemediationResolution {
    pub fn execute<R, A>(
        repo: &mut R,
        audit: &mut A,
        input: VerifyRemediationResolutionInput,
    ) -> RemediationResultUseCaseResult<R, A>
    where
        R: RemediationResultRepository,
        A: AuditWriter,
    {
        let Some(record) = repo
            .find_remediation_request(&input.remediation_id)
            .map_err(RemediationResultUseCaseError::Repository)?
        else {
            return Err(RemediationResultUseCaseError::NotFound("remediation"));
        };
        let mut request = remediation_record_to_request(&record)
            .map_err(RemediationResultUseCaseError::Domain)?;
        ensure_remediation_evidence_matches(
            &request,
            &input.agent_id,
            &input.policy_id,
            &input.policy_name,
            &input.job_id,
        )?;
        request
            .verify_resolved()
            .map_err(|error| RemediationResultUseCaseError::Domain(format!("{error:?}")))?;
        let drift_changed = repo
            .mark_latest_drift_resolved(
                &input.agent_id,
                &input.policy_name,
                &input.job_id,
                input.verified_at,
            )
            .map_err(RemediationResultUseCaseError::Repository)?;
        if !drift_changed {
            return Err(RemediationResultUseCaseError::NotFound("drift"));
        }
        let updated = update_remediation_record::<R, A>(repo, record, &request, input.verified_at)?;
        write_remediation_lifecycle_audit(
            audit,
            "remediation_resolved",
            &input.actor,
            &request,
            input.verified_at,
        )?;
        Ok(RemediationLifecycleOutput {
            remediation: updated,
        })
    }
}

fn ensure_remediation_job_matches<RepoError, AuditError>(
    request: &RemediationRequest,
    job_id: &str,
) -> Result<(), RemediationResultUseCaseError<RepoError, AuditError>> {
    if request.job_id.as_deref() == Some(job_id) {
        Ok(())
    } else {
        Err(RemediationResultUseCaseError::Mismatch("job_id"))
    }
}

fn ensure_remediation_evidence_matches<RepoError, AuditError>(
    request: &RemediationRequest,
    agent_id: &str,
    policy_id: &str,
    policy_name: &str,
    job_id: &str,
) -> Result<(), RemediationResultUseCaseError<RepoError, AuditError>> {
    if request.agent_id != agent_id {
        return Err(RemediationResultUseCaseError::Mismatch("agent_id"));
    }
    if request.policy_id != policy_id {
        return Err(RemediationResultUseCaseError::Mismatch("policy_id"));
    }
    if request.policy_name != policy_name {
        return Err(RemediationResultUseCaseError::Mismatch("policy_name"));
    }
    ensure_remediation_job_matches(request, job_id)
}

fn update_remediation_record<R, A>(
    repo: &mut R,
    record: RemediationRequestRecord,
    request: &RemediationRequest,
    updated_at: SystemTime,
) -> Result<
    RemediationRequestRecord,
    RemediationResultUseCaseError<<R as PolicyRepository>::Error, <A as AuditWriter>::Error>,
>
where
    R: RemediationResultRepository,
    A: AuditWriter,
{
    repo.update_remediation_request_status(
        &request.id,
        request.status.as_str(),
        request.job_id.as_deref(),
        updated_at,
    )
    .map_err(RemediationResultUseCaseError::Repository)?;
    Ok(RemediationRequestRecord {
        status: request.status.as_str().to_owned(),
        job_id: request.job_id.clone(),
        updated_at,
        ..record
    })
}

fn write_remediation_lifecycle_audit<A, RepoError>(
    audit: &mut A,
    action: &str,
    actor: &str,
    request: &RemediationRequest,
    occurred_at: SystemTime,
) -> Result<(), RemediationResultUseCaseError<RepoError, A::Error>>
where
    A: AuditWriter,
{
    audit
        .write(AuditEvent {
            category: AuditCategory::Policy,
            action: action.to_owned(),
            actor: AuditActor::new(actor.to_owned()),
            target: AuditTarget::new(request.agent_id.clone()),
            value: AuditValue::Plain(format!(
                "remediation_id={},policy_id={},policy_name={},agent_id={},job_id={},status={}",
                request.id,
                request.policy_id,
                request.policy_name,
                request.agent_id,
                request.job_id.as_deref().unwrap_or(""),
                request.status.as_str()
            )),
            occurred_at,
        })
        .map_err(RemediationResultUseCaseError::Audit)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SignedRunbookJob {
    job: Job,
    task: RunbookExecutionTask,
    targets: Vec<JobTarget>,
    envelopes: Vec<TaskEnvelope>,
    approval_requirement: fleet_domain::ApprovalRequirement,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BuildSignedRunbookJobError<SignError> {
    Domain(JobError),
    Agent(AgentError),
    InvalidRunbook(String),
    NoTargets,
    Sign(SignError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildSignedRunbookJobInput {
    job_id: String,
    target_agent_ids: Vec<String>,
    runbook_document: String,
    timeout: Duration,
    issued_at: SystemTime,
    expires_at: SystemTime,
    nonce_prefix: String,
}

fn build_signed_runbook_job<S>(
    signer: &mut S,
    input: BuildSignedRunbookJobInput,
) -> Result<SignedRunbookJob, BuildSignedRunbookJobError<S::Error>>
where
    S: TaskEnvelopeSigner,
{
    if input.target_agent_ids.is_empty() {
        return Err(BuildSignedRunbookJobError::NoTargets);
    }

    fleet_domain::parse_runbook_document(&input.runbook_document)
        .map_err(|error| BuildSignedRunbookJobError::InvalidRunbook(error.to_string()))?;
    let task = RunbookExecutionTask::new(input.runbook_document, input.timeout)
        .map_err(BuildSignedRunbookJobError::Domain)?;
    let task_kind = TaskKind::RunbookExecution(task.clone());
    let approval_requirement =
        approval_requirement_for_task(&task_kind, input.target_agent_ids.len());
    let job = Job::new(
        JobId::new(input.job_id.clone()).map_err(BuildSignedRunbookJobError::Domain)?,
        task.risk(),
        approval_requirement,
        input.timeout,
    );

    let targets = input
        .target_agent_ids
        .iter()
        .map(|id| {
            AgentId::new(id.clone())
                .map(|agent_id| JobTarget { agent_id })
                .map_err(BuildSignedRunbookJobError::Agent)
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut envelopes = Vec::with_capacity(targets.len());
    for (index, target) in targets.iter().enumerate() {
        let payload_hash = runbook_payload_hash(&task, target, index);
        let signature = signer
            .sign(&payload_hash)
            .map_err(BuildSignedRunbookJobError::Sign)?;
        envelopes.push(TaskEnvelope {
            job_id: JobId::new(input.job_id.clone()).map_err(BuildSignedRunbookJobError::Domain)?,
            task_id: TaskId::new(format!("{}-task-{index}", input.job_id))
                .map_err(BuildSignedRunbookJobError::Domain)?,
            target_agent_id: target.agent_id.clone(),
            issued_at: input.issued_at,
            expires_at: TaskExpiry::new(input.expires_at),
            nonce: TaskNonce::new(format!("{}-{index}", input.nonce_prefix))
                .map_err(BuildSignedRunbookJobError::Domain)?,
            payload_hash,
            signature: Some(
                TaskSignature::new(signature).map_err(BuildSignedRunbookJobError::Domain)?,
            ),
        });
    }

    Ok(SignedRunbookJob {
        job,
        task,
        targets,
        envelopes,
        approval_requirement,
    })
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

        repo.save_command_job_with_assignments(job, &task, &envelopes)
            .map_err(CreateCommandJobError::Repository)?;
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
    pub provenance: Option<DriftJobProvenance>,
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

        repo.save_drift_check_job_with_assignments_and_provenance(
            job,
            &task,
            &envelopes,
            input.provenance.as_ref(),
        )
        .map_err(CreateDriftCheckJobError::Repository)?;
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
        let mut signed = build_signed_runbook_job(
            signer,
            BuildSignedRunbookJobInput {
                job_id: input.job_id.clone(),
                target_agent_ids: input.target_agent_ids,
                runbook_document: input.runbook_document,
                timeout: input.timeout,
                issued_at: input.issued_at,
                expires_at: input.expires_at,
                nonce_prefix: input.nonce_prefix,
            },
        )
        .map_err(map_create_runbook_job_error)?;
        if signed.approval_requirement == fleet_domain::ApprovalRequirement::NotRequired {
            signed
                .job
                .queue(false)
                .map_err(CreateRunbookJobError::Domain)?;
        }

        repo.save_runbook_job_with_assignments(signed.job, &signed.task, &signed.envelopes)
            .map_err(CreateRunbookJobError::Repository)?;
        let approval_request =
            if signed.approval_requirement != fleet_domain::ApprovalRequirement::NotRequired {
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
                            signed.targets.len()
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
                    signed.targets.len()
                )),
                occurred_at: input.issued_at,
            })
            .map_err(CreateRunbookJobError::Audit)?;

        Ok(CreateRunbookJobOutput {
            task: signed.task,
            targets: signed.targets,
            envelopes: signed.envelopes,
            approval_request,
        })
    }
}

fn map_create_runbook_job_error<RepoError, AuditError, SignError>(
    error: BuildSignedRunbookJobError<SignError>,
) -> CreateRunbookJobError<RepoError, AuditError, SignError> {
    match error {
        BuildSignedRunbookJobError::Domain(error) => CreateRunbookJobError::Domain(error),
        BuildSignedRunbookJobError::Agent(error) => CreateRunbookJobError::Agent(error),
        BuildSignedRunbookJobError::InvalidRunbook(error) => {
            CreateRunbookJobError::InvalidRunbook(error)
        }
        BuildSignedRunbookJobError::NoTargets => CreateRunbookJobError::NoTargets,
        BuildSignedRunbookJobError::Sign(error) => CreateRunbookJobError::Sign(error),
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

pub const CAPABILITY_SNAPSHOT_MAX_AGE: Duration = Duration::from_secs(86_400);

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
    fn latest_agent_capability_snapshot(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<AgentCapabilitySnapshot>, Self::Error>;
    fn mark_assignment_rejected(
        &mut self,
        task_id: &TaskId,
        now: SystemTime,
        reason: &str,
    ) -> Result<(), Self::Error>;
    fn mark_assignment_dispatched(
        &mut self,
        task_id: &TaskId,
        now: SystemTime,
    ) -> Result<(), Self::Error>;
    fn claim_assignment_for_dispatch(
        &mut self,
        task_id: &TaskId,
        now: SystemTime,
    ) -> Result<bool, Self::Error>;
    fn release_assignment_dispatch_claim(
        &mut self,
        task_id: &TaskId,
        now: SystemTime,
        reason: &str,
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

            if let Some(capability_snapshot) = repo
                .latest_agent_capability_snapshot(&assignment.envelope.target_agent_id)
                .map_err(DispatchPendingAssignmentsError::Repository)?
            {
                let capability_snapshot =
                    capability_snapshot.stale_if_older_than(input.now, CAPABILITY_SNAPSHOT_MAX_AGE);
                let capability_evaluation =
                    capability_snapshot.evaluate(RuntimePrimitive::for_task(&assignment.task));
                if capability_evaluation.status != CapabilitySnapshotStatus::Compatible {
                    let reason = capability_rejection_reason(&capability_evaluation);
                    repo.mark_assignment_rejected(&assignment.envelope.task_id, input.now, &reason)
                        .map_err(DispatchPendingAssignmentsError::Repository)?;
                    audit
                        .write(dispatch_audit_event(
                            "assignment_rejected_capability",
                            &assignment,
                            AuditValue::Plain(format!(
                                "agent_id={},task_id={},assignment_status=rejected,reason_code=capability_unsupported,reason={}",
                                assignment.envelope.target_agent_id.as_str(),
                                assignment.envelope.task_id.as_str(),
                                reason
                            )),
                            input.now,
                        ))
                        .map_err(DispatchPendingAssignmentsError::Audit)?;
                    output.failed_count += 1;
                    continue;
                }
            }

            if !dispatcher.has_active_session(&assignment.envelope.target_agent_id) {
                output.queued_count += 1;
                continue;
            }

            if !repo
                .claim_assignment_for_dispatch(&assignment.envelope.task_id, input.now)
                .map_err(DispatchPendingAssignmentsError::Repository)?
            {
                output.queued_count += 1;
                continue;
            }

            match dispatcher.dispatch(&assignment) {
                Ok(()) => {
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
                    let error = error.to_string();
                    repo.release_assignment_dispatch_claim(
                        &assignment.envelope.task_id,
                        input.now,
                        &error,
                    )
                    .map_err(DispatchPendingAssignmentsError::Repository)?;
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

fn capability_rejection_reason(evaluation: &fleet_domain::CapabilityEvaluation) -> String {
    if evaluation.status == CapabilitySnapshotStatus::Unknown {
        return "capability_unsupported: snapshot_unknown".to_owned();
    }
    if evaluation.status == CapabilitySnapshotStatus::Stale {
        return "capability_unsupported: snapshot_stale".to_owned();
    }
    let missing = evaluation
        .missing
        .iter()
        .map(|requirement| match requirement {
            fleet_domain::CapabilityRequirement::Capability(capability) => {
                capability.as_str().to_owned()
            }
            fleet_domain::CapabilityRequirement::PrivilegeAtLeast(level) => {
                format!("privilege_at_least:{}", level.as_str())
            }
            fleet_domain::CapabilityRequirement::PackageManager => "package_manager".to_owned(),
            fleet_domain::CapabilityRequirement::ServiceManager => "service_manager".to_owned(),
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("capability_unsupported: missing={missing}")
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

pub struct ExportAuditEvents;

impl ExportAuditEvents {
    pub fn execute<R>(
        repo: &R,
        category: Option<fleet_domain::AuditCategory>,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AuditEventPageRecord>, R::Error>
    where
        R: AuditRepository,
    {
        repo.export_page(category, limit, before)
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
    fn static_secret_provider_resolves_without_displaying_raw_secret() {
        let reference = SecretRef::parse("secret://app/api-token").unwrap();
        let raw_secret = "super-secret-fixture-value";
        let provider = StaticSecretProvider::new().with_secret(reference.clone(), raw_secret);

        let resolved = provider
            .resolve_secret(&reference)
            .expect("static provider should resolve");

        assert_eq!(resolved.expose_secret_for_rendering(), raw_secret);
        assert_eq!(resolved.to_string(), "[REDACTED]");
        assert!(!format!("{resolved:?}").contains(raw_secret));
        assert!(!format!("{reference:?}").contains("api-token"));
    }

    #[test]
    fn disabled_secret_provider_denies_without_reference_leak() {
        let reference = SecretRef::parse("secret://app/disabled-token").unwrap();
        let provider = DisabledSecretProvider;

        let error = provider.resolve_secret(&reference).unwrap_err();

        assert!(matches!(error, SecretProviderError::Denied { .. }));
        assert_eq!(error.reference(), &reference);
        assert_eq!(error.to_string(), "secret reference access was denied");
        assert!(!error.to_string().contains("disabled-token"));
        assert!(!format!("{error:?}").contains("disabled-token"));
    }

    #[test]
    fn static_secret_provider_errors_are_typed_and_redacted() {
        let missing = SecretRef::parse("secret://app/missing-token").unwrap();
        let denied = SecretRef::parse("secret://app/denied-token").unwrap();
        let provider = StaticSecretProvider::new().with_denied(denied.clone());

        let missing_error = provider.resolve_secret(&missing).unwrap_err();
        assert!(matches!(
            missing_error,
            SecretProviderError::NotFound { .. }
        ));
        assert_eq!(missing_error.reference(), &missing);
        assert!(!missing_error.to_string().contains("missing-token"));
        assert!(!format!("{missing_error:?}").contains("missing-token"));

        let denied_error = provider.resolve_secret(&denied).unwrap_err();
        assert!(matches!(denied_error, SecretProviderError::Denied { .. }));
        assert_eq!(denied_error.reference(), &denied);
        assert!(!denied_error.to_string().contains("denied-token"));
        assert!(!format!("{denied_error:?}").contains("denied-token"));
    }

    #[test]
    fn render_template_content_with_provider_resolves_secret_refs() {
        let reference = SecretRef::parse("secret://app/api-token").unwrap();
        let raw_secret = "render-secret-fixture-value";
        let provider = StaticSecretProvider::new().with_secret(reference.clone(), raw_secret);
        let mut variables = BTreeMap::new();
        variables.insert(
            "api_token".to_owned(),
            TemplateVariableValue::SecretRef(reference),
        );

        let rendered =
            render_template_content_with_provider("token={{ api_token }}", &variables, &provider)
                .unwrap();

        assert_eq!(rendered, format!("token={raw_secret}"));
    }

    #[test]
    fn render_template_content_with_secret_provider_errors_are_redacted() {
        let reference = SecretRef::parse("secret://app/denied-token").unwrap();
        let provider = StaticSecretProvider::new().with_denied(reference.clone());
        let mut variables = BTreeMap::new();
        variables.insert(
            "api_token".to_owned(),
            TemplateVariableValue::SecretRef(reference),
        );

        let error =
            render_template_content_with_provider("token={{ api_token }}", &variables, &provider)
                .unwrap_err();

        assert!(matches!(
            error,
            TemplateRenderError::SecretRefResolutionFailed {
                reason: TemplateSecretResolutionFailure::Denied,
                ..
            }
        ));
        assert!(!error.to_string().contains("denied-token"));
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
    fn dispatch_pending_assignments_rejects_reported_unsupported_capability() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut repo = FakeDispatchAssignmentRepository {
            agents: vec![agent("web-01", "web")],
            assignments: vec![pending_assignment(
                "job-1",
                "task-1",
                "web-01",
                now + Duration::from_secs(60),
            )],
            capability_snapshots: BTreeMap::from([(
                "web-01".to_owned(),
                AgentCapabilitySnapshot::reported(
                    fleet_domain::AgentRuntimeProfile::new(
                        fleet_domain::PrivilegeLevel::Unprivileged,
                        None,
                        None,
                        Vec::new(),
                    ),
                    now,
                ),
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
        assert!(dispatcher.sent_task_ids.is_empty());
        assert_eq!(repo.rejected_assignments.len(), 1);
        assert_eq!(repo.rejected_assignments[0].0, "task-1");
        assert!(
            repo.rejected_assignments[0]
                .1
                .contains("capability_unsupported")
        );
        assert_eq!(audit.events[0].action, "assignment_rejected_capability");
        assert!(matches!(
            &audit.events[0].value,
            AuditValue::Plain(value)
                if value.contains("assignment_status=rejected")
                    && value.contains("reason_code=capability_unsupported")
        ));
    }

    #[test]
    fn dispatch_pending_assignments_rejects_stale_capability_snapshot() {
        let now = SystemTime::UNIX_EPOCH + CAPABILITY_SNAPSHOT_MAX_AGE + Duration::from_secs(100);
        let mut repo = FakeDispatchAssignmentRepository {
            agents: vec![agent("web-01", "web")],
            assignments: vec![pending_assignment(
                "job-1",
                "task-1",
                "web-01",
                now + Duration::from_secs(60),
            )],
            capability_snapshots: BTreeMap::from([(
                "web-01".to_owned(),
                AgentCapabilitySnapshot::reported(
                    fleet_domain::AgentRuntimeProfile::new(
                        fleet_domain::PrivilegeLevel::Unprivileged,
                        None,
                        None,
                        vec![
                            fleet_domain::AgentCapability::PersistentSession,
                            fleet_domain::AgentCapability::CommandExecution,
                        ],
                    ),
                    SystemTime::UNIX_EPOCH,
                ),
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
        assert!(dispatcher.sent_task_ids.is_empty());
        assert_eq!(repo.rejected_assignments.len(), 1);
        assert!(repo.rejected_assignments[0].1.contains("snapshot_stale"));
        assert_eq!(audit.events[0].action, "assignment_rejected_capability");
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
        assert_eq!(
            repo.released_dispatch_claims,
            vec![("task-1".to_owned(), "send failed".to_owned())]
        );
        assert_eq!(audit.events[0].action, "task_dispatch_failed");
        assert!(matches!(
            &audit.events[0].value,
            AuditValue::Plain(value)
                if value.contains("dispatch_state=queued")
                    && value.contains("failure_reason=")
        ));
    }

    #[test]
    fn dispatch_pending_assignments_does_not_send_when_claim_fails() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut repo = FakeDispatchAssignmentRepository {
            agents: vec![agent("web-01", "web")],
            assignments: vec![pending_assignment(
                "job-1",
                "task-1",
                "web-01",
                now + Duration::from_secs(60),
            )],
            claim_failed_task_ids: vec!["task-1".to_owned()],
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
                queued_count: 1,
                ..Default::default()
            }
        );
        assert!(dispatcher.sent_task_ids.is_empty());
        assert!(repo.dispatched_assignments.is_empty());
        assert!(repo.running_jobs.is_empty());
        assert!(audit.events.is_empty());
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
        assert_eq!(repo.atomic_save_count, 1);
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
                provenance: None,
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
        assert_eq!(repo.atomic_save_count, 1);
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
        assert_eq!(repo.atomic_save_count, 1);
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
        assert_eq!(repo.atomic_save_count, 1);
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
            id: DriftReportId::new(1).unwrap(),
            agent_id: "agent-1".to_owned(),
            report: DriftReport {
                policy_name: "nginx-running".to_owned(),
                status: fleet_domain::DriftStatus::Compliant,
                severity: fleet_domain::DriftSeverity::None,
                acknowledgement: fleet_domain::DriftAcknowledgement::Open,
                expected: "service nginx running".to_owned(),
                actual: "service nginx running".to_owned(),
            },
            provenance: DriftReportProvenance::uncorrelated(),
            checked_at: SystemTime::UNIX_EPOCH,
        });
        repo.drift_pages.push(DriftReportPageRecord {
            id: DriftReportId::new(1).unwrap(),
            agent_id: "agent-1".to_owned(),
            report: DriftReport {
                policy_name: "nginx-running".to_owned(),
                status: fleet_domain::DriftStatus::Compliant,
                severity: fleet_domain::DriftSeverity::None,
                acknowledgement: fleet_domain::DriftAcknowledgement::Open,
                expected: "service nginx running".to_owned(),
                actual: "service nginx running".to_owned(),
            },
            provenance: DriftReportProvenance::uncorrelated(),
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
    fn run_due_scheduled_drift_creates_signed_drift_check_job() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(600);
        let mut repo = scheduled_drift_repo_fixture(now - Duration::from_secs(1));
        let mut audit = FakeAuditWriter::default();
        let mut signer = FakeSigner;

        let output = RunDueScheduledDrift::execute(
            &mut repo,
            &mut audit,
            &mut signer,
            scheduled_drift_input(now),
        )
        .unwrap();

        assert_eq!(
            output,
            RunDueScheduledDriftOutput {
                created_count: 1,
                ..Default::default()
            }
        );
        assert_eq!(repo.saved_drift_policies.len(), 1);
        assert!(repo.saved_drift_policies[0].contains("name: nginx-running"));
        assert_eq!(repo.saved_assignments.len(), 1);
        assert_eq!(
            repo.saved_assignments[0].target_agent_id.as_str(),
            "agent-1"
        );
        assert_eq!(repo.schedules[0].last_checked_at, Some(now));
        assert_eq!(
            repo.schedules[0].next_due_at,
            now + Duration::from_secs(300)
        );
        assert!(
            audit
                .events
                .iter()
                .any(|event| event.action == "drift_check_job_created")
        );
        assert!(
            audit
                .events
                .iter()
                .any(|event| event.action == "scheduled_drift_job_created")
        );
        assert_eq!(
            repo.saved_drift_job_provenance,
            Some(DriftJobProvenance::scheduled("nginx-running", 1))
        );
    }

    #[test]
    fn run_due_scheduled_drift_audits_missed_schedule() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(600);
        let mut repo = scheduled_drift_repo_fixture(now - Duration::from_secs(120));
        let mut audit = FakeAuditWriter::default();
        let mut signer = FakeSigner;

        let output = RunDueScheduledDrift::execute(
            &mut repo,
            &mut audit,
            &mut signer,
            scheduled_drift_input(now),
        )
        .unwrap();

        assert_eq!(output.created_count, 1);
        assert_eq!(output.missed_count, 1);
        assert!(
            audit
                .events
                .iter()
                .any(|event| event.action == "scheduled_drift_missed")
        );
    }

    #[test]
    fn run_due_scheduled_drift_skips_disabled_agent_without_assignment() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(600);
        let mut repo = scheduled_drift_repo_fixture(now - Duration::from_secs(1));
        repo.agents[0].disable();
        let mut audit = FakeAuditWriter::default();
        let mut signer = FakeSigner;

        let output = RunDueScheduledDrift::execute(
            &mut repo,
            &mut audit,
            &mut signer,
            scheduled_drift_input(now),
        )
        .unwrap();

        assert_eq!(
            output,
            RunDueScheduledDriftOutput {
                skipped_disabled_count: 1,
                ..Default::default()
            }
        );
        assert!(repo.saved_assignments.is_empty());
        assert_eq!(repo.schedules[0].last_checked_at, Some(now));
        assert!(
            audit
                .events
                .iter()
                .any(|event| event.action == "scheduled_drift_skipped_disabled_agent")
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

    #[test]
    fn remediation_proposal_writes_redacted_audit_without_dispatch() {
        let policy = fleet_domain::parse_policy_document(
            r#"
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Policy
metadata:
  name: nginx-running
spec:
  selector:
    matchLabels:
      role: web
  checks:
    - id: nginx-service
      service:
        name: nginx
        state: running
  remediation:
    runbookRef: runbooks/nginx-remediate.yml
    approvalRequired: true
"#,
        )
        .unwrap();
        let origin = VerifiedDriftEvidence {
            report_id: DriftReportId::new(1).unwrap(),
            agent_id: "agent-1".to_owned(),
            report: DriftReport::drifted("nginx-running", "expected", "actual"),
            provenance: DriftReportProvenance::verified(
                JobId::new("job-drift").unwrap(),
                TaskId::new("task-drift").unwrap(),
                "nginx-running",
                policy.version,
                fleet_domain::DriftCheckPurpose::Evaluation,
            ),
        };
        let mut repo = FakeProposalRepository::default();

        let input = CreateRemediationRequestInput {
            remediation_id: "rem-1".to_owned(),
            policy,
            origin,
            actor: "operator-1".to_owned(),
            requested_at: SystemTime::UNIX_EPOCH,
        };
        let record = CreateRemediationRequestProposal::execute(&mut repo, input.clone()).unwrap();

        assert!(record.created);
        assert_eq!(record.remediation.id, "rem-1");
        assert_eq!(record.remediation.status, "proposed");
        assert_eq!(record.remediation.policy_id, "nginx-running");
        assert_eq!(record.remediation.agent_id, "agent-1");
        assert_eq!(
            record.remediation.runbook_ref,
            "runbooks/nginx-remediate.yml"
        );
        assert!(record.remediation.job_id.is_none());
        assert_eq!(
            record.remediation.origin_drift_report_id,
            Some(DriftReportId::new(1).unwrap())
        );
        assert_eq!(repo.audits.len(), 1);
        assert_eq!(repo.audits[0].action, "remediation_requested");
        let audit_value = match &repo.audits[0].value {
            AuditValue::Plain(value) => value.as_str(),
            _ => panic!("expected plain remediation audit value"),
        };
        assert!(audit_value.contains("policy_id=nginx-running"));
        assert!(audit_value.contains("runbook_ref=runbooks/nginx-remediate.yml"));
        assert!(!audit_value.contains("kind: Runbook"));
        assert!(!audit_value.contains("secret"));

        let duplicate = CreateRemediationRequestProposal::execute(&mut repo, input).unwrap();
        assert!(!duplicate.created);
        assert_eq!(duplicate.remediation.id, "rem-1");
        assert_eq!(repo.audits.len(), 1);
    }

    #[test]
    fn remediation_proposal_rejects_uncorrelated_origin_without_audit() {
        let policy = fleet_domain::parse_policy_document(
            r#"
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Policy
metadata:
  name: nginx-running
spec:
  selector:
    matchLabels:
      role: web
  checks:
    - id: nginx-service
      service:
        name: nginx
        state: running
  remediation:
    runbookRef: runbooks/nginx-remediate.yml
    approvalRequired: true
"#,
        )
        .unwrap();
        let mut repo = FakeProposalRepository::default();

        let result = CreateRemediationRequestProposal::execute(
            &mut repo,
            CreateRemediationRequestInput {
                remediation_id: "rem-uncorrelated".to_owned(),
                policy,
                origin: VerifiedDriftEvidence {
                    report_id: DriftReportId::new(2).unwrap(),
                    agent_id: "agent-1".to_owned(),
                    report: DriftReport::drifted("nginx-running", "expected", "actual"),
                    provenance: DriftReportProvenance::uncorrelated(),
                },
                actor: "operator-1".to_owned(),
                requested_at: SystemTime::UNIX_EPOCH,
            },
        );

        assert!(matches!(result, Err(PolicyUseCaseError::Domain(_))));
        assert!(repo.audits.is_empty());
    }

    #[test]
    fn remediation_approval_request_persists_pending_status() {
        let mut repo = FakeCommandJobRepository {
            remediation_requests: vec![remediation_request_record("proposed")],
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();

        let output = RequestRemediationApproval::execute(
            &mut repo,
            &mut audit,
            RequestRemediationApprovalInput {
                remediation_id: "rem-1".to_owned(),
                approval_id: "approval-rem-1".to_owned(),
                job_id: "job-rem-1".to_owned(),
                requester: "operator-1".to_owned(),
                reason: "policy remediation requires approval".to_owned(),
                expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(600),
                now: SystemTime::UNIX_EPOCH,
            },
        )
        .unwrap();

        assert_eq!(output.remediation.status, "pending_approval");
        assert_eq!(repo.remediation_requests[0].status, "pending_approval");
        assert_eq!(repo.approval_requests.len(), 1);
        assert_eq!(repo.approval_requests[0].status, "pending");
        assert_eq!(audit.events[0].action, "remediation_approval_requested");
        let audit_value = plain_audit_value(&audit.events[0]);
        assert!(audit_value.contains("remediation_id=rem-1"));
        assert!(audit_value.contains("approval_id=approval-rem-1"));
        assert!(!audit_value.contains("kind: Runbook"));
        assert!(!audit_value.contains("secret"));
    }

    #[test]
    fn approved_remediation_creates_signed_runbook_job_and_updates_request() {
        let mut repo = FakeCommandJobRepository {
            remediation_requests: vec![remediation_request_record("proposed")],
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();
        let mut signer = FakeSigner;

        RequestRemediationApproval::execute(
            &mut repo,
            &mut audit,
            RequestRemediationApprovalInput {
                remediation_id: "rem-1".to_owned(),
                approval_id: "approval-rem-1".to_owned(),
                job_id: "job-rem-1".to_owned(),
                requester: "operator-1".to_owned(),
                reason: "policy remediation requires approval".to_owned(),
                expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(600),
                now: SystemTime::UNIX_EPOCH,
            },
        )
        .unwrap();

        let output = ApproveRemediationRunbookJob::execute(
            &mut repo,
            &mut audit,
            &mut signer,
            ApproveRemediationRunbookJobInput {
                remediation_id: "rem-1".to_owned(),
                approval_id: "approval-rem-1".to_owned(),
                job_id: "job-rem-1".to_owned(),
                runbook_document: remediation_runbook_document(),
                timeout: Duration::from_secs(30),
                approver: "manager-1".to_owned(),
                approval_reason: "approved maintenance window".to_owned(),
                issued_at: SystemTime::UNIX_EPOCH + Duration::from_secs(10),
                expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(70),
                nonce_prefix: "nonce-rem".to_owned(),
            },
        )
        .unwrap();

        assert_eq!(repo.saved_count, 1);
        assert_eq!(repo.atomic_save_count, 1);
        assert_eq!(repo.saved_assignments.len(), 1);
        assert!(output.envelopes[0].signature.is_some());
        assert_eq!(repo.approval_requests.len(), 1);
        assert_eq!(repo.approval_requests[0].status, "approved");
        assert_eq!(
            repo.approval_requests[0].approver.as_deref(),
            Some("manager-1")
        );
        assert_eq!(repo.remediation_requests[0].status, "job_created");
        assert_eq!(
            repo.remediation_requests[0].job_id.as_deref(),
            Some("job-rem-1")
        );
        assert_eq!(output.remediation.status, "job_created");
        assert_eq!(output.approval.status, "approved");
        assert_eq!(output.targets.len(), 1);
        assert_eq!(
            output.task.runbook_document(),
            remediation_runbook_document()
        );
        assert!(
            audit
                .events
                .iter()
                .any(|event| event.action == "approval_approved")
        );
        assert!(
            audit
                .events
                .iter()
                .any(|event| event.action == "runbook_job_created")
        );
        assert!(
            audit
                .events
                .iter()
                .any(|event| event.action == "remediation_job_created")
        );
        assert!(
            audit
                .events
                .iter()
                .all(|event| !plain_audit_value(event).contains("kind: Runbook"))
        );
        assert!(
            audit
                .events
                .iter()
                .all(|event| !plain_audit_value(event).contains("secret"))
        );
    }

    #[test]
    fn remediation_cannot_create_job_before_approval_request_state() {
        let mut repo = FakeCommandJobRepository {
            remediation_requests: vec![remediation_request_record("proposed")],
            approval_requests: vec![approval_request_record(
                "approval-rem-1",
                "job-rem-1",
                SystemTime::UNIX_EPOCH + Duration::from_secs(600),
            )],
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();
        let mut signer = FakeSigner;

        let result = ApproveRemediationRunbookJob::execute(
            &mut repo,
            &mut audit,
            &mut signer,
            approve_remediation_input(),
        );

        assert!(matches!(
            result,
            Err(ApproveRemediationRunbookJobError::Domain(_))
        ));
        assert_eq!(repo.saved_count, 0);
        assert_eq!(repo.saved_assignments.len(), 0);
        assert_eq!(repo.approval_requests[0].status, "pending");
        assert!(audit.events.is_empty());
    }

    #[test]
    fn terminal_remediation_request_cannot_create_job() {
        let mut repo = FakeCommandJobRepository {
            remediation_requests: vec![remediation_request_record("rejected")],
            approval_requests: vec![approval_request_record(
                "approval-rem-1",
                "job-rem-1",
                SystemTime::UNIX_EPOCH + Duration::from_secs(600),
            )],
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();
        let mut signer = FakeSigner;

        let result = ApproveRemediationRunbookJob::execute(
            &mut repo,
            &mut audit,
            &mut signer,
            approve_remediation_input(),
        );

        assert!(matches!(
            result,
            Err(ApproveRemediationRunbookJobError::Domain(_))
        ));
        assert_eq!(repo.saved_count, 0);
        assert_eq!(repo.saved_assignments.len(), 0);
        assert_eq!(repo.remediation_requests[0].status, "rejected");
        assert!(audit.events.is_empty());
    }

    #[test]
    fn remediation_job_success_waits_for_verification_before_resolved() {
        let mut repo = FakePolicyRepository {
            remediation_requests: vec![remediation_request_record_with_job(
                "job_created",
                "job-rem-1",
            )],
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();

        let running = MarkRemediationJobRunning::execute(
            &mut repo,
            &mut audit,
            MarkRemediationJobRunningInput {
                remediation_id: "rem-1".to_owned(),
                job_id: "job-rem-1".to_owned(),
                actor: "agent-1".to_owned(),
                occurred_at: SystemTime::UNIX_EPOCH + Duration::from_secs(20),
            },
        )
        .unwrap();

        assert_eq!(running.remediation.status, "running");
        let result = RecordRemediationJobResult::execute(
            &mut repo,
            &mut audit,
            RecordRemediationJobResultInput {
                remediation_id: "rem-1".to_owned(),
                job_id: "job-rem-1".to_owned(),
                status: RemediationJobResultStatus::Succeeded,
                actor: "agent-1".to_owned(),
                occurred_at: SystemTime::UNIX_EPOCH + Duration::from_secs(30),
            },
        )
        .unwrap();

        assert_eq!(result.remediation.status, "succeeded_pending_verify");
        assert!(repo.resolved_reports.is_empty());
        assert_eq!(
            repo.remediation_requests[0].status,
            "succeeded_pending_verify"
        );
        assert!(
            audit
                .events
                .iter()
                .any(|event| event.action == "remediation_job_succeeded_pending_verify")
        );
    }

    #[test]
    fn successful_remediation_creates_one_signed_verification_job() {
        let mut repo = scheduled_drift_repo_fixture(SystemTime::UNIX_EPOCH);
        repo.remediation_requests = vec![RemediationRequestRecord {
            policy_version: Some(1),
            ..remediation_request_record_with_job("succeeded_pending_verify", "job-remediation-1")
        }];
        let mut signer = FakeSigner;
        let input = CreateRemediationVerificationJobInput {
            remediation_id: "rem-1".to_owned(),
            verification_job_id: "job-remediation-verify-1".to_owned(),
            timeout: Duration::from_secs(30),
            actor: "controller".to_owned(),
            issued_at: SystemTime::UNIX_EPOCH + Duration::from_secs(31),
            expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(91),
            nonce_prefix: "nonce-remediation-verify".to_owned(),
        };

        let first =
            CreateRemediationVerificationJob::execute(&mut repo, &mut signer, input.clone())
                .unwrap();
        let duplicate =
            CreateRemediationVerificationJob::execute(&mut repo, &mut signer, input).unwrap();

        assert!(first.created);
        assert!(!duplicate.created);
        assert_eq!(first.job_id, "job-remediation-verify-1");
        assert_eq!(duplicate.job_id, first.job_id);
        assert_eq!(repo.saved_assignments.len(), 1);
        assert_eq!(repo.remediation_verification_audits.len(), 1);
        assert_eq!(
            repo.saved_drift_job_provenance,
            Some(DriftJobProvenance::remediation_verification(
                "nginx-running",
                1
            ))
        );
    }

    #[test]
    fn pending_remediation_verification_recovery_is_bounded_and_excludes_correlated_rows() {
        let repo = FakePolicyRepository {
            remediation_requests: vec![
                remediation_request_record_with_job("succeeded_pending_verify", "job-rem-1"),
                RemediationRequestRecord {
                    id: "rem-2".to_owned(),
                    ..remediation_request_record_with_job("succeeded_pending_verify", "job-rem-2")
                },
                RemediationRequestRecord {
                    id: "rem-3".to_owned(),
                    ..remediation_request_record_with_job("succeeded_pending_verify", "job-rem-3")
                },
            ],
            remediation_verification_jobs: vec![("rem-2".to_owned(), "job-verify-2".to_owned())],
            ..Default::default()
        };

        let records = ListPendingRemediationVerificationRecovery::execute(&repo, 1).unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "rem-1");
    }

    #[test]
    fn remediation_job_failure_marks_failed() {
        let mut repo = FakePolicyRepository {
            remediation_requests: vec![remediation_request_record_with_job(
                "job_created",
                "job-rem-1",
            )],
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();

        let result = RecordRemediationJobResult::execute(
            &mut repo,
            &mut audit,
            RecordRemediationJobResultInput {
                remediation_id: "rem-1".to_owned(),
                job_id: "job-rem-1".to_owned(),
                status: RemediationJobResultStatus::Failed,
                actor: "agent-1".to_owned(),
                occurred_at: SystemTime::UNIX_EPOCH + Duration::from_secs(30),
            },
        )
        .unwrap();

        assert_eq!(result.remediation.status, "failed");
        assert_eq!(repo.remediation_requests[0].status, "failed");
        assert!(
            audit
                .events
                .iter()
                .any(|event| event.action == "remediation_job_failed")
        );
    }

    #[test]
    fn remediation_job_cancel_and_timeout_keep_distinct_terminal_states() {
        for (status, expected, action) in [
            (
                RemediationJobResultStatus::Canceled,
                "canceled",
                "remediation_job_canceled",
            ),
            (
                RemediationJobResultStatus::Expired,
                "expired",
                "remediation_job_expired",
            ),
        ] {
            let mut repo = FakePolicyRepository {
                remediation_requests: vec![remediation_request_record_with_job(
                    "running",
                    "job-rem-1",
                )],
                ..Default::default()
            };
            let mut audit = FakeAuditWriter::default();
            let result = RecordRemediationJobResult::execute(
                &mut repo,
                &mut audit,
                RecordRemediationJobResultInput {
                    remediation_id: "rem-1".to_owned(),
                    job_id: "job-rem-1".to_owned(),
                    status,
                    actor: "agent-1".to_owned(),
                    occurred_at: SystemTime::UNIX_EPOCH,
                },
            )
            .unwrap();
            assert_eq!(result.remediation.status, expected);
            assert!(audit.events.iter().any(|event| event.action == action));
        }
    }

    #[test]
    fn matching_verification_resolves_remediation_and_latest_drift() {
        let mut repo = FakePolicyRepository {
            remediation_requests: vec![remediation_request_record_with_job(
                "succeeded_pending_verify",
                "job-rem-1",
            )],
            resolve_latest_result: true,
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();

        let result = VerifyRemediationResolution::execute(
            &mut repo,
            &mut audit,
            VerifyRemediationResolutionInput {
                remediation_id: "rem-1".to_owned(),
                agent_id: "agent-1".to_owned(),
                policy_id: "nginx-running".to_owned(),
                policy_name: "nginx-running".to_owned(),
                job_id: "job-rem-1".to_owned(),
                actor: "verifier".to_owned(),
                verified_at: SystemTime::UNIX_EPOCH + Duration::from_secs(40),
            },
        )
        .unwrap();

        assert_eq!(result.remediation.status, "resolved");
        assert_eq!(repo.remediation_requests[0].status, "resolved");
        assert_eq!(
            repo.resolved_reports,
            vec![(
                "agent-1".to_owned(),
                "nginx-running".to_owned(),
                "job-rem-1".to_owned()
            )]
        );
        assert!(
            audit
                .events
                .iter()
                .any(|event| event.action == "remediation_resolved")
        );
        assert!(
            audit
                .events
                .iter()
                .all(|event| !plain_audit_value(event).contains("stdout"))
        );
        assert!(
            audit
                .events
                .iter()
                .all(|event| !plain_audit_value(event).contains("secret"))
        );
    }

    #[test]
    fn fresh_compliant_verification_evidence_rejects_stale_noncompliant_and_mismatched_reports() {
        let mut repo = FakePolicyRepository {
            remediation_requests: vec![RemediationRequestRecord {
                policy_version: Some(1),
                ..remediation_request_record_with_job("succeeded_pending_verify", "job-rem-1")
            }],
            remediation_verification_jobs: vec![("rem-1".to_owned(), "job-verify-1".to_owned())],
            resolve_latest_result: true,
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();
        let input = ResolveRemediationVerificationEvidenceInput {
            remediation_id: "rem-1".to_owned(),
            verification_job_id: "job-verify-1".to_owned(),
            verification_task_id: "task-verify-1".to_owned(),
            evidence_report_id: DriftReportId::new(1).unwrap(),
            agent_id: "agent-1".to_owned(),
            policy_id: "nginx-running".to_owned(),
            policy_name: "nginx-running".to_owned(),
            policy_version: 1,
            status: DriftStatus::Compliant,
            checked_at: SystemTime::UNIX_EPOCH + Duration::from_secs(20),
            remediation_execution_completed_at: SystemTime::UNIX_EPOCH + Duration::from_secs(30),
            actor: "controller".to_owned(),
        };
        let stale =
            ResolveRemediationVerificationEvidence::execute(&mut repo, &mut audit, input.clone());

        assert!(matches!(stale, Ok(None)));
        assert_eq!(
            repo.remediation_requests[0].status,
            "succeeded_pending_verify"
        );
        assert!(audit.events.is_empty());

        repo.remediation_requests[0].origin_drift_report_id = Some(DriftReportId::new(7).unwrap());
        for status in [DriftStatus::Drifted, DriftStatus::Unknown] {
            let rejected = ResolveRemediationVerificationEvidence::execute(
                &mut repo,
                &mut audit,
                ResolveRemediationVerificationEvidenceInput {
                    status,
                    checked_at: SystemTime::UNIX_EPOCH + Duration::from_secs(20),
                    remediation_execution_completed_at: SystemTime::UNIX_EPOCH
                        + Duration::from_secs(10),
                    ..input.clone()
                },
            );
            assert!(matches!(rejected, Ok(None)));
        }
        let mismatched = ResolveRemediationVerificationEvidence::execute(
            &mut repo,
            &mut audit,
            ResolveRemediationVerificationEvidenceInput {
                policy_id: "other-policy".to_owned(),
                checked_at: SystemTime::UNIX_EPOCH + Duration::from_secs(20),
                remediation_execution_completed_at: SystemTime::UNIX_EPOCH
                    + Duration::from_secs(10),
                ..input.clone()
            },
        );
        assert!(matches!(mismatched, Ok(None)));
        assert_eq!(
            repo.remediation_requests[0].status,
            "succeeded_pending_verify"
        );
        assert!(audit.events.is_empty());

        let fresh = ResolveRemediationVerificationEvidence::execute(
            &mut repo,
            &mut audit,
            ResolveRemediationVerificationEvidenceInput {
                evidence_report_id: DriftReportId::new(2).unwrap(),
                // A verification report can be emitted before its final TaskResult. It is
                // still fresh when it follows the remediation execution completion.
                checked_at: SystemTime::UNIX_EPOCH + Duration::from_secs(20),
                remediation_execution_completed_at: SystemTime::UNIX_EPOCH
                    + Duration::from_secs(10),
                ..input
            },
        )
        .unwrap()
        .unwrap();

        assert_eq!(fresh.remediation.status, "resolved");
        assert_eq!(repo.remediation_requests[0].status, "resolved");
        assert_eq!(repo.remediation_verification_audits.len(), 1);
    }

    #[test]
    fn mismatched_verification_evidence_is_rejected() {
        let mut repo = FakePolicyRepository {
            remediation_requests: vec![remediation_request_record_with_job(
                "succeeded_pending_verify",
                "job-rem-1",
            )],
            resolve_latest_result: true,
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();

        let result = VerifyRemediationResolution::execute(
            &mut repo,
            &mut audit,
            VerifyRemediationResolutionInput {
                remediation_id: "rem-1".to_owned(),
                agent_id: "agent-1".to_owned(),
                policy_id: "different-policy".to_owned(),
                policy_name: "nginx-running".to_owned(),
                job_id: "job-rem-1".to_owned(),
                actor: "verifier".to_owned(),
                verified_at: SystemTime::UNIX_EPOCH + Duration::from_secs(40),
            },
        );

        assert!(matches!(
            result,
            Err(RemediationResultUseCaseError::Mismatch("policy_id"))
        ));
        assert_eq!(
            repo.remediation_requests[0].status,
            "succeeded_pending_verify"
        );
        assert!(repo.resolved_reports.is_empty());
        assert!(audit.events.is_empty());
    }

    #[test]
    fn terminal_remediation_result_cannot_be_modified() {
        let mut repo = FakePolicyRepository {
            remediation_requests: vec![remediation_request_record_with_job(
                "resolved",
                "job-rem-1",
            )],
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();

        let result = RecordRemediationJobResult::execute(
            &mut repo,
            &mut audit,
            RecordRemediationJobResultInput {
                remediation_id: "rem-1".to_owned(),
                job_id: "job-rem-1".to_owned(),
                status: RemediationJobResultStatus::Failed,
                actor: "agent-1".to_owned(),
                occurred_at: SystemTime::UNIX_EPOCH + Duration::from_secs(30),
            },
        );

        assert!(matches!(
            result,
            Err(RemediationResultUseCaseError::Domain(_))
        ));
        assert_eq!(repo.remediation_requests[0].status, "resolved");
        assert!(audit.events.is_empty());
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
            runbook_document: remediation_runbook_document(),
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

    fn remediation_runbook_document() -> String {
        r#"
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
        .to_owned()
    }

    fn remediation_request_record(status: &str) -> RemediationRequestRecord {
        RemediationRequestRecord {
            id: "rem-1".to_owned(),
            policy_id: "nginx-running".to_owned(),
            policy_name: "nginx-running".to_owned(),
            agent_id: "agent-1".to_owned(),
            runbook_ref: "runbooks/nginx-remediate.yml".to_owned(),
            status: status.to_owned(),
            approval_required: true,
            risk_summary: "drifted policy nginx-running requires approved runbook remediation"
                .to_owned(),
            job_id: None,
            origin_drift_report_id: None,
            policy_version: None,
            created_at: SystemTime::UNIX_EPOCH,
            updated_at: SystemTime::UNIX_EPOCH,
        }
    }

    fn remediation_request_record_with_job(status: &str, job_id: &str) -> RemediationRequestRecord {
        RemediationRequestRecord {
            job_id: Some(job_id.to_owned()),
            ..remediation_request_record(status)
        }
    }

    fn approve_remediation_input() -> ApproveRemediationRunbookJobInput {
        ApproveRemediationRunbookJobInput {
            remediation_id: "rem-1".to_owned(),
            approval_id: "approval-rem-1".to_owned(),
            job_id: "job-rem-1".to_owned(),
            runbook_document: remediation_runbook_document(),
            timeout: Duration::from_secs(30),
            approver: "manager-1".to_owned(),
            approval_reason: "approved maintenance window".to_owned(),
            issued_at: SystemTime::UNIX_EPOCH + Duration::from_secs(10),
            expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(70),
            nonce_prefix: "nonce-rem".to_owned(),
        }
    }

    fn plain_audit_value(event: &AuditEvent) -> &str {
        match &event.value {
            AuditValue::Plain(value) => value.as_str(),
            AuditValue::SecretRef(_) | AuditValue::Redacted => "",
        }
    }

    #[derive(Default)]
    struct FakeCommandJobRepository {
        saved_count: usize,
        atomic_save_count: usize,
        saved_program: Option<String>,
        saved_drift_policy: Option<String>,
        saved_runbook_document: Option<String>,
        saved_assignments: Vec<TaskEnvelope>,
        approval_requests: Vec<ApprovalRequestRecord>,
        approval_status_updates: Vec<(String, JobStatus)>,
        remediation_requests: Vec<RemediationRequestRecord>,
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

        fn save_command_job_with_assignments(
            &mut self,
            job: Job,
            task: &CommandTask,
            assignments: &[TaskEnvelope],
        ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
            self.atomic_save_count += 1;
            self.save_command_job(job, task)?;
            self.saved_assignments.extend(assignments.iter().cloned());
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

        fn save_drift_check_job_with_assignments(
            &mut self,
            job: Job,
            task: &DriftCheckTask,
            assignments: &[TaskEnvelope],
        ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
            self.atomic_save_count += 1;
            self.save_drift_check_job(job, task)?;
            self.saved_assignments.extend(assignments.iter().cloned());
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

        fn save_runbook_job_with_assignments(
            &mut self,
            job: Job,
            task: &RunbookExecutionTask,
            assignments: &[TaskEnvelope],
        ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
            self.atomic_save_count += 1;
            self.save_runbook_job(job, task)?;
            self.saved_assignments.extend(assignments.iter().cloned());
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

    impl RemediationRequestRepository for FakeCommandJobRepository {
        type Error = Infallible;

        fn save_remediation_request(
            &mut self,
            request: RemediationRequestRecord,
        ) -> Result<(), Self::Error> {
            self.remediation_requests.push(request);
            Ok(())
        }

        fn find_remediation_request(
            &self,
            request_id: &str,
        ) -> Result<Option<RemediationRequestRecord>, Self::Error> {
            Ok(self
                .remediation_requests
                .iter()
                .find(|request| request.id == request_id)
                .cloned())
        }

        fn find_remediation_request_by_job_id(
            &self,
            job_id: &str,
        ) -> Result<Option<RemediationRequestRecord>, Self::Error> {
            Ok(self
                .remediation_requests
                .iter()
                .find(|request| request.job_id.as_deref() == Some(job_id))
                .cloned())
        }

        fn list_remediation_requests(
            &self,
            agent_id: Option<&str>,
            policy_id: Option<&str>,
            limit: usize,
        ) -> Result<Vec<RemediationRequestRecord>, Self::Error> {
            Ok(self
                .remediation_requests
                .iter()
                .filter(|request| agent_id.is_none_or(|agent_id| request.agent_id == agent_id))
                .filter(|request| policy_id.is_none_or(|policy_id| request.policy_id == policy_id))
                .take(limit)
                .cloned()
                .collect())
        }

        fn update_remediation_request_status(
            &mut self,
            request_id: &str,
            status: &str,
            job_id: Option<&str>,
            updated_at: SystemTime,
        ) -> Result<(), Self::Error> {
            if let Some(request) = self
                .remediation_requests
                .iter_mut()
                .find(|request| request.id == request_id)
            {
                request.status = status.to_owned();
                request.job_id = job_id.map(str::to_owned);
                request.updated_at = updated_at;
            }
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeDispatchAssignmentRepository {
        agents: Vec<Agent>,
        assignments: Vec<PendingTaskAssignment>,
        gates: BTreeMap<String, JobDispatchGate>,
        capability_snapshots: BTreeMap<String, AgentCapabilitySnapshot>,
        claim_failed_task_ids: Vec<String>,
        dispatched_assignments: Vec<String>,
        released_dispatch_claims: Vec<(String, String)>,
        rejected_assignments: Vec<(String, String)>,
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

        fn latest_agent_capability_snapshot(
            &self,
            agent_id: &AgentId,
        ) -> Result<Option<AgentCapabilitySnapshot>, Self::Error> {
            Ok(self.capability_snapshots.get(agent_id.as_str()).cloned())
        }

        fn mark_assignment_rejected(
            &mut self,
            task_id: &TaskId,
            _now: SystemTime,
            reason: &str,
        ) -> Result<(), Self::Error> {
            self.rejected_assignments
                .push((task_id.as_str().to_owned(), reason.to_owned()));
            Ok(())
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

        fn claim_assignment_for_dispatch(
            &mut self,
            task_id: &TaskId,
            _now: SystemTime,
        ) -> Result<bool, Self::Error> {
            if self
                .claim_failed_task_ids
                .iter()
                .any(|failed_task_id| failed_task_id == task_id.as_str())
            {
                return Ok(false);
            }
            self.dispatched_assignments
                .push(task_id.as_str().to_owned());
            Ok(true)
        }

        fn release_assignment_dispatch_claim(
            &mut self,
            task_id: &TaskId,
            _now: SystemTime,
            reason: &str,
        ) -> Result<(), Self::Error> {
            self.released_dispatch_claims
                .push((task_id.as_str().to_owned(), reason.to_owned()));
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
                id: DriftReportId::new(1).unwrap(),
                agent_id: agent_id.to_owned(),
                report: report.clone(),
                provenance: DriftReportProvenance::uncorrelated(),
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

        fn export_page(
            &self,
            category: Option<fleet_domain::AuditCategory>,
            limit: usize,
            before: Option<SnapshotPageCursor>,
        ) -> Result<Vec<AuditEventPageRecord>, Self::Error> {
            let before_row = before.map(|cursor| cursor.row_id).unwrap_or(i64::MAX);
            Ok(self
                .audit
                .iter()
                .enumerate()
                .filter(|(index, event)| {
                    category
                        .as_ref()
                        .is_none_or(|category| event.category == *category)
                        && ((*index as i64) + 1) < before_row
                })
                .take(limit)
                .map(|(index, event)| AuditEventPageRecord {
                    event: event.clone(),
                    cursor: SnapshotPageCursor {
                        occurred_at: event.occurred_at,
                        row_id: (index as i64) + 1,
                    },
                })
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

    #[test]
    fn retention_policy_rejects_zero_duration() {
        let policy = RetentionPolicy {
            job_output: Duration::ZERO,
            facts: Duration::from_secs(30 * 86_400),
            metrics: Duration::from_secs(7 * 86_400),
            agent_logs: Duration::from_secs(86_400),
        };

        assert!(policy.validate().is_err());
    }

    #[test]
    fn run_retention_cleanup_calculates_separate_artifact_cutoffs() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(40 * 86_400);
        let mut repo = FakeRetentionRepository::default();
        let mut audit = FakeAuditWriter::default();

        let output = RunRetentionCleanup::execute(
            &mut repo,
            &mut audit,
            RunRetentionCleanupInput {
                now,
                policy: RetentionPolicy::mvp_defaults(),
                dry_run: true,
                actor: "test".to_owned(),
                target: "controller-store".to_owned(),
            },
        )
        .unwrap();

        assert_eq!(output.state, RetentionRunState::DryRun);
        assert_eq!(repo.calls.len(), 1);
        assert!(repo.calls[0].dry_run);
        assert_eq!(
            repo.calls[0].cutoffs.job_output,
            now - Duration::from_secs(14 * 86_400)
        );
        assert_eq!(
            repo.calls[0].cutoffs.metrics,
            now - Duration::from_secs(7 * 86_400)
        );
        assert_eq!(
            repo.calls[0].cutoffs.facts,
            now - Duration::from_secs(30 * 86_400)
        );
        assert_eq!(
            repo.calls[0].cutoffs.agent_logs,
            now - Duration::from_secs(86_400)
        );
        assert!(audit.events.is_empty());
    }

    #[test]
    fn run_retention_cleanup_writes_summary_audit_for_real_cleanup() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(40 * 86_400);
        let mut repo = FakeRetentionRepository {
            summary: RetentionCleanupSummary {
                job_output_chunks: 2,
                facts_snapshots: 3,
                metrics_snapshots: 5,
                agent_log_chunks: 7,
            },
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();

        let output = RunRetentionCleanup::execute(
            &mut repo,
            &mut audit,
            RunRetentionCleanupInput {
                now,
                policy: RetentionPolicy::mvp_defaults(),
                dry_run: false,
                actor: "worker".to_owned(),
                target: "controller-store".to_owned(),
            },
        )
        .unwrap();

        assert_eq!(output.state, RetentionRunState::Completed);
        assert_eq!(output.summary.total(), 17);
        assert_eq!(audit.events.len(), 1);
        assert_eq!(audit.events[0].category, AuditCategory::Security);
        assert_eq!(audit.events[0].action, "retention_cleanup");
        assert_eq!(audit.events[0].actor.as_str(), "worker");
        assert_eq!(
            audit.events[0].value,
            AuditValue::Plain(
                "job_output_chunks=2,facts_snapshots=3,metrics_snapshots=5,agent_log_chunks=7,total=17"
                    .to_owned()
            )
        );
    }

    #[test]
    fn artifact_store_contract_is_storage_backend_neutral() {
        let bytes = b"artifact body".to_vec();
        let checksum = ArtifactChecksum::sha256(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap();
        let id = ArtifactId::new("artifact-1").unwrap();
        let mut store = FakeArtifactStore::default();

        let record = store
            .put(ArtifactStorePut {
                id: id.clone(),
                retention_class: ArtifactRetentionClass::RenderedTemplate,
                expected_checksum: checksum.clone(),
                bytes: bytes.clone(),
            })
            .unwrap();

        assert_eq!(record.id, id);
        assert_eq!(
            record.retention_class,
            ArtifactRetentionClass::RenderedTemplate
        );
        assert_eq!(
            store
                .get(&id, ArtifactRetentionClass::RenderedTemplate)
                .unwrap(),
            Some(bytes)
        );
        assert!(matches!(
            store
                .verify(&id, ArtifactRetentionClass::RenderedTemplate, &checksum)
                .unwrap(),
            ArtifactVerification::Verified(_)
        ));
        assert_eq!(
            store
                .delete(&id, ArtifactRetentionClass::RenderedTemplate)
                .unwrap(),
            ArtifactDeleteOutcome::Deleted
        );
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

    fn scheduled_drift_repo_fixture(next_due_at: SystemTime) -> FakePolicyRepository {
        let source = policy_document("nginx-running");
        FakePolicyRepository {
            agents: vec![agent("agent-1", "web")],
            policies: vec![PolicyRecord {
                id: "nginx-running".to_owned(),
                name: "nginx-running".to_owned(),
                version: 1,
                source,
                created_at: SystemTime::UNIX_EPOCH,
                updated_at: SystemTime::UNIX_EPOCH,
            }],
            schedules: vec![ScheduledDriftRecord {
                policy_id: "nginx-running".to_owned(),
                agent_id: "agent-1".to_owned(),
                interval_seconds: 300,
                next_due_at,
                last_checked_at: None,
            }],
            ..Default::default()
        }
    }

    fn scheduled_drift_input(now: SystemTime) -> RunDueScheduledDriftInput {
        RunDueScheduledDriftInput {
            now,
            grace_duration: Duration::from_secs(60),
            limit: 10,
            job_timeout: Duration::from_secs(30),
            job_expires_in: Duration::from_secs(300),
            actor: "scheduler".to_owned(),
            job_id_prefix: "scheduled-drift".to_owned(),
            nonce_prefix: "scheduled-nonce".to_owned(),
        }
    }

    #[derive(Default)]
    struct FakePolicyRepository {
        agents: Vec<Agent>,
        policies: Vec<PolicyRecord>,
        assignments: Vec<PolicyAssignmentRecord>,
        schedules: Vec<ScheduledDriftRecord>,
        saved_drift_policies: Vec<String>,
        saved_drift_job_provenance: Option<DriftJobProvenance>,
        saved_assignments: Vec<TaskEnvelope>,
        approval_requests: Vec<ApprovalRequestRecord>,
        approval_status_updates: Vec<(String, JobStatus)>,
        acknowledged_reports: Vec<(String, String, String)>,
        resolved_reports: Vec<(String, String, String)>,
        resolve_latest_result: bool,
        remediation_requests: Vec<RemediationRequestRecord>,
        remediation_verification_jobs: Vec<(String, String)>,
        remediation_verification_audits: Vec<AuditEvent>,
    }

    impl AgentRepository for FakePolicyRepository {
        type Error = Infallible;

        fn save(&mut self, agent: Agent) -> Result<(), Self::Error> {
            self.agents.push(agent);
            Ok(())
        }

        fn find_by_id(&self, id: &AgentId) -> Result<Option<Agent>, Self::Error> {
            Ok(self.agents.iter().find(|agent| agent.id() == id).cloned())
        }

        fn list(&self) -> Result<Vec<Agent>, Self::Error> {
            Ok(self.agents.clone())
        }
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

    impl TaskAssignmentRepository for FakePolicyRepository {
        type Error = Infallible;

        fn save_assignment(&mut self, envelope: TaskEnvelope) -> Result<(), Self::Error> {
            self.saved_assignments.push(envelope);
            Ok(())
        }
    }

    impl RemediationRequestRepository for FakePolicyRepository {
        type Error = Infallible;

        fn save_remediation_request(
            &mut self,
            request: RemediationRequestRecord,
        ) -> Result<(), Self::Error> {
            self.remediation_requests.push(request);
            Ok(())
        }

        fn find_remediation_request(
            &self,
            request_id: &str,
        ) -> Result<Option<RemediationRequestRecord>, Self::Error> {
            Ok(self
                .remediation_requests
                .iter()
                .find(|request| request.id == request_id)
                .cloned())
        }

        fn find_remediation_request_by_job_id(
            &self,
            job_id: &str,
        ) -> Result<Option<RemediationRequestRecord>, Self::Error> {
            Ok(self
                .remediation_requests
                .iter()
                .find(|request| request.job_id.as_deref() == Some(job_id))
                .cloned())
        }

        fn list_remediation_requests(
            &self,
            agent_id: Option<&str>,
            policy_id: Option<&str>,
            limit: usize,
        ) -> Result<Vec<RemediationRequestRecord>, Self::Error> {
            Ok(self
                .remediation_requests
                .iter()
                .filter(|request| agent_id.is_none_or(|agent_id| request.agent_id == agent_id))
                .filter(|request| policy_id.is_none_or(|policy_id| request.policy_id == policy_id))
                .take(limit)
                .cloned()
                .collect())
        }

        fn update_remediation_request_status(
            &mut self,
            request_id: &str,
            status: &str,
            job_id: Option<&str>,
            updated_at: SystemTime,
        ) -> Result<(), Self::Error> {
            if let Some(request) = self
                .remediation_requests
                .iter_mut()
                .find(|request| request.id == request_id)
            {
                request.status = status.to_owned();
                request.job_id = job_id.map(str::to_owned);
                request.updated_at = updated_at;
            }
            Ok(())
        }
    }

    impl RemediationVerificationJobRepository for FakePolicyRepository {
        fn find_remediation_verification_job(
            &self,
            remediation_id: &str,
        ) -> Result<Option<String>, <Self as RemediationRequestRepository>::Error> {
            Ok(self
                .remediation_verification_jobs
                .iter()
                .find(|(existing_remediation_id, _)| existing_remediation_id == remediation_id)
                .map(|(_, job_id)| job_id.clone()))
        }

        fn save_remediation_verification_job(
            &mut self,
            input: RemediationVerificationJobPersistenceInput,
        ) -> Result<RemediationVerificationJobSave, <Self as RemediationRequestRepository>::Error>
        {
            if let Some(job_id) = self.find_remediation_verification_job(&input.remediation_id)? {
                return Ok(RemediationVerificationJobSave {
                    job_id,
                    created: false,
                });
            }
            let job_id = input.job.id().as_str().to_owned();
            self.remediation_verification_jobs
                .push((input.remediation_id, job_id.clone()));
            self.saved_drift_policies
                .push(input.task.policy_document().to_owned());
            self.saved_drift_job_provenance = Some(input.provenance);
            self.saved_assignments.push(input.assignment);
            self.remediation_verification_audits.push(input.audit);
            Ok(RemediationVerificationJobSave {
                job_id,
                created: true,
            })
        }
    }

    impl RemediationVerificationResolutionRepository for FakePolicyRepository {
        fn resolve_remediation_verification_evidence(
            &mut self,
            remediation: RemediationRequestRecord,
            _origin_drift_report_id: DriftReportId,
            _evidence_report_id: DriftReportId,
            _verification_job_id: &str,
            _verification_task_id: &str,
            audit: AuditEvent,
        ) -> Result<RemediationRequestRecord, <Self as RemediationRequestRepository>::Error>
        {
            self.update_remediation_request_status(
                &remediation.id,
                &remediation.status,
                remediation.job_id.as_deref(),
                remediation.updated_at,
            )?;
            self.remediation_verification_audits.push(audit);
            Ok(remediation)
        }
    }

    impl RemediationVerificationRecoveryRepository for FakePolicyRepository {
        type Error = Infallible;

        fn list_pending_remediation_verification_recovery(
            &self,
            limit: usize,
        ) -> Result<Vec<RemediationRequestRecord>, Self::Error> {
            Ok(self
                .remediation_requests
                .iter()
                .filter(|request| request.status == "succeeded_pending_verify")
                .filter(|request| {
                    !self
                        .remediation_verification_jobs
                        .iter()
                        .any(|(remediation_id, _)| remediation_id == &request.id)
                })
                .take(limit)
                .cloned()
                .collect())
        }
    }

    impl ApprovalRepository for FakePolicyRepository {
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

    impl DriftCheckJobRepository for FakePolicyRepository {
        fn save_drift_check_job(
            &mut self,
            _job: Job,
            task: &DriftCheckTask,
        ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
            self.saved_drift_policies
                .push(task.policy_document().to_owned());
            Ok(())
        }

        fn save_drift_check_job_with_assignments_and_provenance(
            &mut self,
            job: Job,
            task: &DriftCheckTask,
            assignments: &[TaskEnvelope],
            provenance: Option<&DriftJobProvenance>,
        ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
            self.saved_drift_job_provenance = provenance.cloned();
            self.save_drift_check_job_with_assignments(job, task, assignments)
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

    #[derive(Default)]
    struct FakeProposalRepository {
        remediations: Vec<RemediationRequestRecord>,
        audits: Vec<AuditEvent>,
    }

    impl RemediationProposalRepository for FakeProposalRepository {
        type Error = Infallible;

        fn save_remediation_proposal(
            &mut self,
            remediation: RemediationRequestRecord,
            audit: AuditEvent,
        ) -> Result<RemediationProposalSave, Self::Error> {
            if let Some(existing) = self.remediations.iter().find(|existing| {
                existing.agent_id == remediation.agent_id
                    && existing.policy_id == remediation.policy_id
                    && !matches!(
                        existing.status.as_str(),
                        "resolved" | "failed" | "rejected" | "expired" | "canceled"
                    )
            }) {
                return Ok(RemediationProposalSave {
                    remediation: existing.clone(),
                    created: false,
                });
            }
            self.remediations.push(remediation.clone());
            self.audits.push(audit);
            Ok(RemediationProposalSave {
                remediation,
                created: true,
            })
        }
    }

    #[derive(Debug, Clone)]
    struct FakeRetentionCleanupCall {
        cutoffs: RetentionCutoffs,
        dry_run: bool,
    }

    #[derive(Default)]
    struct FakeRetentionRepository {
        calls: Vec<FakeRetentionCleanupCall>,
        summary: RetentionCleanupSummary,
    }

    impl RetentionRepository for FakeRetentionRepository {
        type Error = Infallible;

        fn cleanup_retention(
            &mut self,
            cutoffs: RetentionCutoffs,
            dry_run: bool,
        ) -> Result<RetentionCleanupSummary, Self::Error> {
            self.calls
                .push(FakeRetentionCleanupCall { cutoffs, dry_run });
            Ok(self.summary)
        }
    }

    #[derive(Default)]
    struct FakeArtifactStore {
        record: Option<ArtifactStoreRecord>,
        bytes: Option<Vec<u8>>,
    }

    impl ArtifactStore for FakeArtifactStore {
        type Error = Infallible;

        fn put(&mut self, input: ArtifactStorePut) -> Result<ArtifactStoreRecord, Self::Error> {
            let record = ArtifactStoreRecord {
                id: input.id,
                retention_class: input.retention_class,
                checksum: input.expected_checksum,
                size_bytes: input.bytes.len() as u64,
            };
            self.bytes = Some(input.bytes);
            self.record = Some(record.clone());
            Ok(record)
        }

        fn get(
            &self,
            _id: &ArtifactId,
            _retention_class: ArtifactRetentionClass,
        ) -> Result<Option<Vec<u8>>, Self::Error> {
            Ok(self.bytes.clone())
        }

        fn verify(
            &self,
            _id: &ArtifactId,
            _retention_class: ArtifactRetentionClass,
            _expected: &ArtifactChecksum,
        ) -> Result<ArtifactVerification, Self::Error> {
            Ok(self
                .record
                .clone()
                .map(ArtifactVerification::Verified)
                .unwrap_or(ArtifactVerification::Missing))
        }

        fn delete(
            &mut self,
            _id: &ArtifactId,
            _retention_class: ArtifactRetentionClass,
        ) -> Result<ArtifactDeleteOutcome, Self::Error> {
            let existed = self.record.take().is_some();
            self.bytes = None;
            Ok(if existed {
                ArtifactDeleteOutcome::Deleted
            } else {
                ArtifactDeleteOutcome::Missing
            })
        }
    }

    #[test]
    fn signer_rotation_policy_selects_new_signing_fingerprint_after_activation() {
        let mut rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new("old-signing-fingerprint").unwrap(),
        );
        rotation
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new("new-signing-fingerprint").unwrap(),
                UNIX_EPOCH + Duration::from_secs(10),
                UNIX_EPOCH + Duration::from_secs(40),
            )
            .unwrap();
        rotation
            .validate_new_material(UNIX_EPOCH + Duration::from_secs(12))
            .unwrap();
        rotation
            .activate_dual_trust(UNIX_EPOCH + Duration::from_secs(20))
            .unwrap();

        assert_eq!(
            rotation
                .current_signing_fingerprint(UNIX_EPOCH + Duration::from_secs(20))
                .as_str(),
            "new-signing-fingerprint"
        );
        assert!(rotation.can_verify_signature_from(
            &fleet_domain::SigningKeyFingerprint::new("old-signing-fingerprint").unwrap(),
            UNIX_EPOCH + Duration::from_secs(19),
            UNIX_EPOCH + Duration::from_secs(39),
        ));
    }

    #[test]
    fn signing_key_rotation_use_case_saves_state_without_private_material() {
        let mut repo = FakeSigningKeyRotationRepository::default();
        let mut rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new("old-signing-fingerprint").unwrap(),
        );
        rotation
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new("new-signing-fingerprint").unwrap(),
                UNIX_EPOCH + Duration::from_secs(10),
                UNIX_EPOCH + Duration::from_secs(40),
            )
            .unwrap();

        let output = SaveSigningKeyRotation::execute(
            &mut repo,
            SaveSigningKeyRotationInput {
                controller_id: "controller-default".to_owned(),
                rotation,
                actor: "operator-1".to_owned(),
                now: UNIX_EPOCH + Duration::from_secs(11),
            },
        )
        .unwrap();

        let loaded =
            SigningKeyRotationRepository::load_signing_key_rotation(&repo, "controller-default")
                .unwrap()
                .expect("rotation record should load");
        assert_eq!(loaded.rotation.state().as_str(), "rotation_requested");
        assert_eq!(
            output.audit_event.action,
            "controller_signing_key_rotation_state_saved"
        );
        assert!(!format!("{loaded:?}").contains("PRIVATE KEY"));
        assert!(!format!("{loaded:?}").contains("private_key"));
    }

    #[test]
    fn signing_key_selection_uses_domain_rotation_decision() {
        let mut rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new("old-signing-fingerprint").unwrap(),
        );
        rotation
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new("new-signing-fingerprint").unwrap(),
                UNIX_EPOCH + Duration::from_secs(10),
                UNIX_EPOCH + Duration::from_secs(40),
            )
            .unwrap();
        rotation
            .validate_new_material(UNIX_EPOCH + Duration::from_secs(12))
            .unwrap();
        rotation
            .activate_dual_trust(UNIX_EPOCH + Duration::from_secs(20))
            .unwrap();

        assert_eq!(
            select_controller_signing_fingerprint(&rotation, UNIX_EPOCH + Duration::from_secs(19))
                .fingerprint
                .as_str(),
            "old-signing-fingerprint"
        );
        assert_eq!(
            select_controller_signing_fingerprint(&rotation, UNIX_EPOCH + Duration::from_secs(20))
                .fingerprint
                .as_str(),
            "new-signing-fingerprint"
        );
    }

    #[test]
    fn signing_rotation_status_missing_state_reports_active_steady_readiness() {
        let repo = FakeSigningKeyRotationRepository::default();

        let status = QueryControllerSigningRotationStatus::execute(
            &repo,
            ControllerSigningRotationStatusInput {
                controller_id: "controller-default".to_owned(),
                active_fingerprint: fleet_domain::SigningKeyFingerprint::new(
                    "active-signing-fingerprint",
                )
                .unwrap(),
                now: UNIX_EPOCH + Duration::from_secs(1),
            },
        )
        .unwrap();

        assert!(!status.persisted_record_present);
        assert_eq!(status.persisted_state, "steady");
        assert_eq!(
            status.readiness,
            ControllerSigningRotationReadiness::SteadyReady
        );
        assert_eq!(status.bootstrap_guard, "active_matches_selected");
        assert_eq!(status.agent_trust_rollout, "not_required");
        assert_eq!(status.new_fingerprint_prefix, None);
    }

    #[test]
    fn signing_rotation_status_dual_trust_uses_prefixes_without_material_leak() {
        let mut rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new("old-signing-private-key-secret").unwrap(),
        );
        rotation
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new("new-signing-private-key-secret").unwrap(),
                UNIX_EPOCH + Duration::from_secs(10),
                UNIX_EPOCH + Duration::from_secs(40),
            )
            .unwrap();
        rotation
            .validate_new_material(UNIX_EPOCH + Duration::from_secs(12))
            .unwrap();
        rotation
            .activate_dual_trust(UNIX_EPOCH + Duration::from_secs(20))
            .unwrap();
        let repo = FakeSigningKeyRotationRepository {
            record: Some(SigningKeyRotationRecord {
                controller_id: "controller-default".to_owned(),
                rotation,
                updated_at: UNIX_EPOCH + Duration::from_secs(20),
            }),
            ..Default::default()
        };

        let status = QueryControllerSigningRotationStatus::execute(
            &repo,
            ControllerSigningRotationStatusInput {
                controller_id: "controller-default".to_owned(),
                active_fingerprint: fleet_domain::SigningKeyFingerprint::new(
                    "new-signing-private-key-secret",
                )
                .unwrap(),
                now: UNIX_EPOCH + Duration::from_secs(30),
            },
        )
        .unwrap();
        let dump = format!("{status:?}");

        assert_eq!(
            status.readiness,
            ControllerSigningRotationReadiness::DualTrustActiveAgentsMigrating
        );
        assert_eq!(status.agent_trust_rollout, "agents_migrating");
        assert_eq!(status.old_key_verifies_until_ms, Some(40_000));
        assert!(status.old_fingerprint_prefix.starts_with("old-signing"));
        assert!(
            status
                .new_fingerprint_prefix
                .as_deref()
                .unwrap()
                .starts_with("new-signing")
        );
        assert!(!dump.contains("private-key-secret"));
    }

    #[test]
    fn signing_rotation_status_terminal_and_retirement_readiness_are_explicit() {
        let retired = {
            let mut record = activated_signing_rotation_record();
            record
                .rotation
                .retire_old_key(UNIX_EPOCH + Duration::from_secs(40))
                .unwrap();
            record
        };
        let failed = {
            let mut record = requested_signing_rotation_record();
            record
                .rotation
                .fail_rotation(UNIX_EPOCH + Duration::from_secs(13))
                .unwrap();
            record
        };
        let canceled = {
            let mut record = requested_signing_rotation_record();
            record.rotation.cancel_before_activation().unwrap();
            record
        };

        for (record, readiness, rollout) in [
            (
                retired,
                ControllerSigningRotationReadiness::TerminalRetired,
                "completed",
            ),
            (
                failed,
                ControllerSigningRotationReadiness::TerminalFailed,
                "failed",
            ),
            (
                canceled,
                ControllerSigningRotationReadiness::TerminalCanceled,
                "canceled",
            ),
        ] {
            let repo = FakeSigningKeyRotationRepository {
                record: Some(record),
                ..Default::default()
            };

            let status = QueryControllerSigningRotationStatus::execute(
                &repo,
                ControllerSigningRotationStatusInput {
                    controller_id: "controller-default".to_owned(),
                    active_fingerprint: fleet_domain::SigningKeyFingerprint::new(
                        "old-signing-fingerprint",
                    )
                    .unwrap(),
                    now: UNIX_EPOCH + Duration::from_secs(41),
                },
            )
            .unwrap();

            assert_eq!(status.readiness, readiness);
            assert_eq!(status.agent_trust_rollout, rollout);
        }
    }

    #[test]
    fn signing_rotation_status_reports_old_key_retirement_available_after_window() {
        let repo = FakeSigningKeyRotationRepository {
            record: Some(activated_signing_rotation_record()),
            ..Default::default()
        };

        let status = QueryControllerSigningRotationStatus::execute(
            &repo,
            ControllerSigningRotationStatusInput {
                controller_id: "controller-default".to_owned(),
                active_fingerprint: fleet_domain::SigningKeyFingerprint::new(
                    "new-signing-fingerprint",
                )
                .unwrap(),
                now: UNIX_EPOCH + Duration::from_secs(40),
            },
        )
        .unwrap();

        assert_eq!(
            status.readiness,
            ControllerSigningRotationReadiness::OldKeyRetirementAvailable
        );
        assert_eq!(status.agent_trust_rollout, "retirement_available");
    }

    #[test]
    fn request_signing_key_rotation_persists_state_and_security_audit() {
        let mut repo = FakeSigningKeyRotationRepository::default();
        let mut audit = FakeAuditWriter::default();

        let output = RequestSigningKeyRotation::execute(
            &mut repo,
            &mut audit,
            RequestSigningKeyRotationInput {
                controller_id: "controller-default".to_owned(),
                actor: "operator-1".to_owned(),
                old_fingerprint: fleet_domain::SigningKeyFingerprint::new(
                    "old-signing-fingerprint",
                )
                .unwrap(),
                new_fingerprint: fleet_domain::SigningKeyFingerprint::new(
                    "new-signing-fingerprint",
                )
                .unwrap(),
                requested_at: UNIX_EPOCH + Duration::from_secs(10),
                old_key_verifies_until: UNIX_EPOCH + Duration::from_secs(40),
            },
        )
        .unwrap();

        assert_eq!(
            output.record.rotation.state().as_str(),
            "rotation_requested"
        );
        assert_eq!(repo.save_count, 1);
        assert_eq!(audit.events.len(), 1);
        assert_eq!(audit.events[0].category, AuditCategory::Security);
        assert_eq!(
            audit.events[0].action,
            "controller_signing_key_rotation_requested"
        );
        assert_eq!(
            plain_audit_value(&audit.events[0]),
            "state=rotation_requested,old_fingerprint_prefix=old-signing-,new_fingerprint_prefix=new-signing-"
        );
    }

    #[test]
    fn validate_signing_key_rotation_loads_state_saves_and_audits() {
        let mut repo = FakeSigningKeyRotationRepository {
            record: Some(requested_signing_rotation_record()),
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();

        let output = ValidateSigningKeyRotation::execute(
            &mut repo,
            &mut audit,
            ValidateSigningKeyRotationInput {
                controller_id: "controller-default".to_owned(),
                actor: "operator-1".to_owned(),
                validated_new_fingerprint: fleet_domain::SigningKeyFingerprint::new(
                    "new-signing-fingerprint",
                )
                .unwrap(),
                validated_at: UNIX_EPOCH + Duration::from_secs(12),
            },
        )
        .unwrap();

        assert_eq!(
            output.record.rotation.state().as_str(),
            "new_material_validated"
        );
        assert_eq!(repo.save_count, 1);
        assert_eq!(
            audit.events[0].action,
            "controller_signing_key_rotation_validated"
        );
    }

    #[test]
    fn validate_signing_key_rotation_rejects_unrequested_fingerprint_without_save_or_audit() {
        let mut repo = FakeSigningKeyRotationRepository {
            record: Some(requested_signing_rotation_record()),
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();

        let result = ValidateSigningKeyRotation::execute(
            &mut repo,
            &mut audit,
            ValidateSigningKeyRotationInput {
                controller_id: "controller-default".to_owned(),
                actor: "operator-1".to_owned(),
                validated_new_fingerprint: fleet_domain::SigningKeyFingerprint::new(
                    "other-signing-fingerprint",
                )
                .unwrap(),
                validated_at: UNIX_EPOCH + Duration::from_secs(12),
            },
        );

        assert!(matches!(
            result,
            Err(SigningKeyRotationUseCaseError::FingerprintMismatch)
        ));
        assert_eq!(repo.save_count, 0);
        assert!(audit.events.is_empty());
        assert_eq!(
            repo.record.unwrap().rotation.state().as_str(),
            "rotation_requested"
        );
    }

    #[test]
    fn activate_signing_key_rotation_uses_persisted_state_and_audits() {
        let mut repo = FakeSigningKeyRotationRepository {
            record: Some(validated_signing_rotation_record()),
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();

        let output = ActivateSigningKeyRotation::execute(
            &mut repo,
            &mut audit,
            ActivateSigningKeyRotationInput {
                controller_id: "controller-default".to_owned(),
                actor: "operator-1".to_owned(),
                activated_at: UNIX_EPOCH + Duration::from_secs(20),
            },
        )
        .unwrap();

        assert_eq!(output.record.rotation.state().as_str(), "dual_trust_active");
        assert_eq!(repo.save_count, 1);
        assert_eq!(
            audit.events[0].action,
            "controller_signing_key_rotation_activated"
        );
    }

    #[test]
    fn retire_signing_key_rotation_rejects_before_guard_and_succeeds_after() {
        let mut repo = FakeSigningKeyRotationRepository {
            record: Some(activated_signing_rotation_record()),
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();

        let early = RetireSigningKeyRotation::execute(
            &mut repo,
            &mut audit,
            RetireSigningKeyRotationInput {
                controller_id: "controller-default".to_owned(),
                actor: "operator-1".to_owned(),
                retired_at: UNIX_EPOCH + Duration::from_secs(39),
            },
        );

        assert!(matches!(
            early,
            Err(SigningKeyRotationUseCaseError::Domain(
                fleet_domain::SigningKeyRotationError::RetirementGuardNotSatisfied
            ))
        ));
        assert_eq!(repo.save_count, 0);
        assert!(audit.events.is_empty());

        let output = RetireSigningKeyRotation::execute(
            &mut repo,
            &mut audit,
            RetireSigningKeyRotationInput {
                controller_id: "controller-default".to_owned(),
                actor: "operator-1".to_owned(),
                retired_at: UNIX_EPOCH + Duration::from_secs(40),
            },
        )
        .unwrap();

        assert_eq!(output.record.rotation.state().as_str(), "old_key_retired");
        assert_eq!(repo.save_count, 1);
        assert_eq!(
            audit.events[0].action,
            "controller_signing_key_rotation_retired"
        );
    }

    #[test]
    fn fail_signing_key_rotation_records_terminal_failure_without_leaking_summary() {
        let mut repo = FakeSigningKeyRotationRepository {
            record: Some(requested_signing_rotation_record()),
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();

        let output = FailSigningKeyRotation::execute(
            &mut repo,
            &mut audit,
            FailSigningKeyRotationInput {
                controller_id: "controller-default".to_owned(),
                actor: "operator-1".to_owned(),
                failed_at: UNIX_EPOCH + Duration::from_secs(13),
                failure_summary: "PRIVATE KEY PEM parse failed at /secret/key.pem".to_owned(),
            },
        )
        .unwrap();

        assert_eq!(output.record.rotation.state().as_str(), "rotation_failed");
        let audit_dump = format!("{:?}", audit.events);
        assert!(!audit_dump.contains("PRIVATE KEY"));
        assert!(!audit_dump.contains("/secret/key.pem"));
        assert_eq!(
            audit.events[0].action,
            "controller_signing_key_rotation_failed"
        );
    }

    #[test]
    fn invalid_signing_key_rotation_transition_does_not_save_or_audit_success() {
        let mut repo = FakeSigningKeyRotationRepository {
            record: Some(requested_signing_rotation_record()),
            ..Default::default()
        };
        let mut audit = FakeAuditWriter::default();

        let result = ActivateSigningKeyRotation::execute(
            &mut repo,
            &mut audit,
            ActivateSigningKeyRotationInput {
                controller_id: "controller-default".to_owned(),
                actor: "operator-1".to_owned(),
                activated_at: UNIX_EPOCH + Duration::from_secs(20),
            },
        );

        assert!(matches!(
            result,
            Err(SigningKeyRotationUseCaseError::Domain(
                fleet_domain::SigningKeyRotationError::InvalidTransition { .. }
            ))
        ));
        assert_eq!(repo.save_count, 0);
        assert!(audit.events.is_empty());
        assert_eq!(
            repo.record.unwrap().rotation.state().as_str(),
            "rotation_requested"
        );
    }

    #[test]
    fn agent_certificate_lifecycle_use_case_persists_issuance_and_audits() {
        let mut repo = FakeAgentCertificateLifecycleRepository::default();
        let mut audit = FakeAuditWriter::default();
        let agent_id = AgentId::new("agent-1").unwrap();

        RequestAgentCertificateIssuance::execute(
            &mut repo,
            &mut audit,
            RequestAgentCertificateIssuanceInput {
                agent_id: agent_id.clone(),
                actor: "admin".to_owned(),
                requested_at: UNIX_EPOCH + Duration::from_secs(10),
            },
        )
        .unwrap();

        assert_eq!(repo.save_count, 1);
        assert_eq!(
            repo.record.as_ref().unwrap().lifecycle.state,
            fleet_domain::AgentCertificateLifecycleState::IssuanceRequested
        );
        assert_eq!(
            audit.events[0].action,
            "agent_certificate_issuance_requested"
        );

        IssueAgentCertificate::execute(
            &mut repo,
            &mut audit,
            IssueAgentCertificateInput {
                agent_id,
                actor: "admin".to_owned(),
                certificate: agent_certificate("serial-1", "0123456789abcdef", 10, 110),
                issued_at: UNIX_EPOCH + Duration::from_secs(11),
            },
        )
        .unwrap();

        assert_eq!(repo.save_count, 2);
        assert_eq!(
            repo.record.as_ref().unwrap().lifecycle.state,
            fleet_domain::AgentCertificateLifecycleState::Issued
        );
        assert_eq!(audit.events[1].action, "agent_certificate_issued");
        assert_eq!(audit.events[1].category, AuditCategory::Security);
        assert_eq!(audit.events[1].target.as_str(), "agent-1");
    }

    #[test]
    fn agent_certificate_lifecycle_use_case_rotates_with_grace_window() {
        let mut repo = FakeAgentCertificateLifecycleRepository::default();
        let mut audit = FakeAuditWriter::default();
        let agent_id = AgentId::new("agent-1").unwrap();

        let mut lifecycle = fleet_domain::AgentCertificateLifecycle::new(agent_id.clone());
        lifecycle
            .request_issuance(UNIX_EPOCH + Duration::from_secs(10))
            .unwrap();
        lifecycle
            .issue(
                agent_certificate("serial-1", "0123456789abcdef", 10, 110),
                UNIX_EPOCH + Duration::from_secs(11),
            )
            .unwrap();
        repo.record = Some(AgentCertificateLifecycleRecord {
            agent_id: agent_id.clone(),
            lifecycle: lifecycle.snapshot(),
            updated_at: UNIX_EPOCH + Duration::from_secs(11),
        });

        RequestAgentCertificateRenewal::execute(
            &mut repo,
            &mut audit,
            RequestAgentCertificateRenewalInput {
                agent_id: agent_id.clone(),
                actor: "admin".to_owned(),
                requested_at: UNIX_EPOCH + Duration::from_secs(80),
                policy: agent_certificate_policy(),
            },
        )
        .unwrap();
        ActivateAgentCertificateRenewal::execute(
            &mut repo,
            &mut audit,
            ActivateAgentCertificateRenewalInput {
                agent_id: agent_id.clone(),
                actor: "admin".to_owned(),
                certificate: agent_certificate("serial-2", "fedcba9876543210", 80, 200),
                activated_at: UNIX_EPOCH + Duration::from_secs(81),
                policy: agent_certificate_policy(),
            },
        )
        .unwrap();

        assert_eq!(
            repo.record.as_ref().unwrap().lifecycle.state,
            fleet_domain::AgentCertificateLifecycleState::DualCertificateActive
        );

        CompleteAgentCertificateRotation::execute(
            &mut repo,
            &mut audit,
            CompleteAgentCertificateRotationInput {
                agent_id,
                actor: "admin".to_owned(),
                completed_at: UNIX_EPOCH + Duration::from_secs(91),
            },
        )
        .unwrap();

        assert_eq!(
            repo.record.as_ref().unwrap().lifecycle.state,
            fleet_domain::AgentCertificateLifecycleState::Issued
        );
        assert_eq!(
            repo.record
                .as_ref()
                .unwrap()
                .lifecycle
                .current_certificate
                .as_ref()
                .unwrap()
                .serial()
                .as_str(),
            "serial-2"
        );
        assert_eq!(
            audit.events[0].action,
            "agent_certificate_renewal_requested"
        );
        assert_eq!(
            audit.events[1].action,
            "agent_certificate_renewal_activated"
        );
        assert_eq!(
            audit.events[2].action,
            "agent_certificate_rotation_completed"
        );
    }

    #[test]
    fn controller_signing_staged_rollout_repository_contract_saves_public_state_only() {
        let mut repo = FakeControllerSigningStagedRolloutRepository::default();
        let mut rollout = fleet_domain::ControllerSigningStagedRollout::new(
            vec!["agent-a".to_owned(), "agent-b".to_owned()],
            fleet_domain::ControllerSigningStagedRolloutConfig {
                batch_size: 1,
                max_failures: 1,
                ack_timeout: Duration::from_secs(30),
            },
        )
        .unwrap();
        let plan = rollout
            .plan_next_batch(
                &[
                    fleet_domain::ControllerSigningStagedRolloutTarget::observed(
                        "agent-a", true, false, None,
                    ),
                    fleet_domain::ControllerSigningStagedRolloutTarget::observed(
                        "agent-b", true, false, None,
                    ),
                ],
                UNIX_EPOCH + Duration::from_secs(10),
            )
            .unwrap();
        rollout
            .batch_dispatched(&plan.agent_ids, UNIX_EPOCH + Duration::from_secs(10))
            .unwrap();
        let record = ControllerSigningStagedRolloutRecord {
            controller_id: "controller-default".to_owned(),
            current_fingerprint: "new-signing-fingerprint".to_owned(),
            previous_fingerprint: Some("old-signing-fingerprint".to_owned()),
            rollout,
            updated_at: UNIX_EPOCH + Duration::from_secs(11),
        };

        repo.save_controller_signing_staged_rollout(record.clone())
            .unwrap();
        let loaded = repo
            .load_controller_signing_staged_rollout("controller-default")
            .unwrap()
            .expect("record should load");
        let debug = format!("{loaded:?}");

        assert_eq!(loaded, record);
        assert_eq!(repo.save_count, 1);
        for forbidden in [
            "private_key",
            "private-key-secret",
            "public-key-body",
            "controller_public.key",
            "admin-token",
            "websocket",
        ] {
            assert!(
                !debug.contains(forbidden),
                "staged rollout record contract must not expose {forbidden}"
            );
        }
    }

    #[derive(Default)]
    struct FakeSigningKeyRotationRepository {
        record: Option<SigningKeyRotationRecord>,
        save_count: usize,
    }

    impl SigningKeyRotationRepository for FakeSigningKeyRotationRepository {
        type Error = Infallible;

        fn save_signing_key_rotation(
            &mut self,
            record: SigningKeyRotationRecord,
        ) -> Result<(), Self::Error> {
            self.save_count += 1;
            self.record = Some(record);
            Ok(())
        }

        fn load_signing_key_rotation(
            &self,
            controller_id: &str,
        ) -> Result<Option<SigningKeyRotationRecord>, Self::Error> {
            Ok(self
                .record
                .clone()
                .filter(|record| record.controller_id == controller_id))
        }
    }

    #[derive(Default)]
    struct FakeControllerSigningStagedRolloutRepository {
        record: Option<ControllerSigningStagedRolloutRecord>,
        save_count: usize,
    }

    impl ControllerSigningStagedRolloutRepository for FakeControllerSigningStagedRolloutRepository {
        type Error = Infallible;

        fn save_controller_signing_staged_rollout(
            &mut self,
            record: ControllerSigningStagedRolloutRecord,
        ) -> Result<(), Self::Error> {
            self.save_count += 1;
            self.record = Some(record);
            Ok(())
        }

        fn load_controller_signing_staged_rollout(
            &self,
            controller_id: &str,
        ) -> Result<Option<ControllerSigningStagedRolloutRecord>, Self::Error> {
            Ok(self
                .record
                .clone()
                .filter(|record| record.controller_id == controller_id))
        }
    }

    #[derive(Default)]
    struct FakeAgentCertificateLifecycleRepository {
        record: Option<AgentCertificateLifecycleRecord>,
        save_count: usize,
    }

    impl AgentCertificateLifecycleRepository for FakeAgentCertificateLifecycleRepository {
        type Error = Infallible;

        fn save_agent_certificate_lifecycle(
            &mut self,
            record: AgentCertificateLifecycleRecord,
        ) -> Result<(), Self::Error> {
            self.save_count += 1;
            self.record = Some(record);
            Ok(())
        }

        fn load_agent_certificate_lifecycle(
            &self,
            agent_id: &AgentId,
        ) -> Result<Option<AgentCertificateLifecycleRecord>, Self::Error> {
            Ok(self
                .record
                .clone()
                .filter(|record| &record.agent_id == agent_id))
        }
    }

    fn agent_certificate(
        serial: &str,
        fingerprint: &str,
        not_before: u64,
        not_after: u64,
    ) -> fleet_domain::AgentCertificate {
        fleet_domain::AgentCertificate::new(
            fleet_domain::AgentCertificateSerial::new(serial).unwrap(),
            fleet_domain::AgentCertificateFingerprint::new(fingerprint).unwrap(),
            fleet_domain::AgentCertificateValidity::new(
                UNIX_EPOCH + Duration::from_secs(not_before),
                UNIX_EPOCH + Duration::from_secs(not_after),
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn agent_certificate_policy() -> fleet_domain::AgentCertificateRenewalPolicy {
        fleet_domain::AgentCertificateRenewalPolicy::new(
            Duration::from_secs(30),
            Duration::from_secs(10),
        )
        .unwrap()
    }

    fn requested_signing_rotation_record() -> SigningKeyRotationRecord {
        let mut rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new("old-signing-fingerprint").unwrap(),
        );
        rotation
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new("new-signing-fingerprint").unwrap(),
                UNIX_EPOCH + Duration::from_secs(10),
                UNIX_EPOCH + Duration::from_secs(40),
            )
            .unwrap();
        SigningKeyRotationRecord {
            controller_id: "controller-default".to_owned(),
            rotation,
            updated_at: UNIX_EPOCH + Duration::from_secs(10),
        }
    }

    fn validated_signing_rotation_record() -> SigningKeyRotationRecord {
        let mut record = requested_signing_rotation_record();
        record
            .rotation
            .validate_new_material(UNIX_EPOCH + Duration::from_secs(12))
            .unwrap();
        record.updated_at = UNIX_EPOCH + Duration::from_secs(12);
        record
    }

    fn activated_signing_rotation_record() -> SigningKeyRotationRecord {
        let mut record = validated_signing_rotation_record();
        record
            .rotation
            .activate_dual_trust(UNIX_EPOCH + Duration::from_secs(20))
            .unwrap();
        record.updated_at = UNIX_EPOCH + Duration::from_secs(20);
        record
    }

    struct FakeSigner;

    impl TaskEnvelopeSigner for FakeSigner {
        type Error = Infallible;

        fn sign(&mut self, payload: &str) -> Result<String, Self::Error> {
            Ok(format!("sig:{payload}"))
        }
    }
}
