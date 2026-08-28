use fleet_application::{
    AdminTokenRecord as AppAdminTokenRecord, AdminTokenRepository, AgentCapabilityRepository,
    AgentCertificateLifecycleRecord, AgentCertificateLifecycleRepository,
    AgentIdentityRecord as AppAgentIdentityRecord, AgentIdentityRepository,
    AgentLogChunkPageRecord as AppAgentLogChunkPageRecord, AgentLogRepository, AgentRepository,
    ApprovalRepository, ApprovalRequestRecord as AppApprovalRequestRecord, ArtifactDeleteOutcome,
    ArtifactMetadataRepository, ArtifactStore, ArtifactStorePut,
    ArtifactStoreRecord as AppArtifactStoreRecord, ArtifactVerification, AuditEventPageRecord,
    AuditRepository, AuditWriter, CommandJobRepository, ControllerIdentityMetadata,
    ControllerIdentityRepository, ControllerSigningStagedRolloutRecord,
    ControllerSigningStagedRolloutRepository, DispatchAssignmentRepository,
    DriftCheckJobRepository, DriftReportPageRecord as AppDriftReportPageRecord,
    DriftReportRecord as AppDriftReportRecord, DriftRepository,
    EnrollmentTokenRecord as AppEnrollmentTokenRecord, EnrollmentTokenRepository, FactsRepository,
    FactsSnapshotPageRecord as AppFactsSnapshotPageRecord,
    FactsSnapshotRecord as AppFactsSnapshotRecord, JobDispatchGate as AppJobDispatchGate,
    JobOutputChunk, JobOutputRepository, JobOutputStream, JobQueryRepository, JobRepository,
    JobSummaryRecord as AppJobSummaryRecord, JobTargetSummaryRecord as AppJobTargetSummaryRecord,
    MetricsRepository, MetricsSnapshotPageRecord as AppMetricsSnapshotPageRecord,
    MetricsSnapshotRecord as AppMetricsSnapshotRecord,
    PendingTaskAssignment as AppPendingTaskAssignment,
    PersistVerifiedDriftProposalInput as AppPersistVerifiedDriftProposalInput,
    PersistVerifiedDriftProposalOutput as AppPersistVerifiedDriftProposalOutput,
    PolicyAssignmentRecord as AppPolicyAssignmentRecord, PolicyRecord as AppPolicyRecord,
    PolicyRepository,
    RemediationExecutionPersistenceInput as AppRemediationExecutionPersistenceInput,
    RemediationExecutionPersistenceRepository, RemediationProposalRepository,
    RemediationProposalSave as AppRemediationProposalSave,
    RemediationRequestRecord as AppRemediationRequestRecord, RemediationRequestRepository,
    RemediationVerificationJobPersistenceInput as AppRemediationVerificationJobPersistenceInput,
    RemediationVerificationJobRepository,
    RemediationVerificationJobSave as AppRemediationVerificationJobSave,
    RemediationVerificationRecoveryRepository, RemediationVerificationResolutionRepository,
    RetentionCleanupSummary as AppRetentionCleanupSummary, RetentionCutoffs, RetentionRepository,
    RunbookJobRepository, ScheduledDriftRecord as AppScheduledDriftRecord,
    SigningKeyRotationRecord, SigningKeyRotationRepository, SnapshotPageCursor,
    TaskAssignmentRepository, VerifiedDriftProposalRepository,
};
use fleet_domain::{
    Agent, AgentCapability, AgentCapabilitySnapshot, AgentCertificate, AgentCertificateFingerprint,
    AgentCertificateLifecycle, AgentCertificateLifecycleSnapshot, AgentCertificateLifecycleState,
    AgentCertificateRevocationReason, AgentCertificateSerial, AgentCertificateValidity, AgentError,
    AgentFingerprint, AgentId, AgentIdentity, AgentLabel, AgentName, AgentPublicKey,
    AgentRuntimeProfile, AgentStatus, ArtifactChecksum, ArtifactId, ArtifactRetentionClass,
    AssignmentStatus, AuditActor, AuditCategory, AuditEvent, AuditTarget, AuditValue, CommandTask,
    ControllerPublicKey, ControllerSigningKeyRotation, ControllerSigningKeyRotationSnapshot,
    DriftAcknowledgement, DriftCheckPurpose, DriftCheckTask, DriftJobProvenance, DriftReport,
    DriftReportId, DriftReportProvenance, DriftSeverity, DriftStatus, Job, JobId, JobStatus,
    PackageManager, PrivilegeLevel, RenderedArtifactMetadata, RunbookExecutionTask, ServiceManager,
    SigningKeyFingerprint, SigningKeyRotationState, TaskEnvelope, TaskExpiry, TaskId, TaskKind,
    TaskNonce, TaskSignature, aggregate_job_status,
};
use rusqlite::{Connection, ErrorCode, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const CURRENT_SCHEMA_VERSION: i64 = 19;

#[derive(Debug)]
pub enum StoreError {
    DuplicateAgent,
    ConstraintViolation(String),
    NotFound,
    Sqlite(rusqlite::Error),
    Postgres(String),
    Domain(String),
}

impl PartialEq for StoreError {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::DuplicateAgent, Self::DuplicateAgent)
                | (Self::NotFound, Self::NotFound)
                | (Self::ConstraintViolation(_), Self::ConstraintViolation(_))
                | (Self::Sqlite(_), Self::Sqlite(_))
                | (Self::Postgres(_), Self::Postgres(_))
                | (Self::Domain(_), Self::Domain(_))
        )
    }
}

impl Eq for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(value: rusqlite::Error) -> Self {
        if let rusqlite::Error::SqliteFailure(error, Some(message)) = &value
            && error.code == ErrorCode::ConstraintViolation
        {
            return Self::ConstraintViolation(message.clone());
        }
        Self::Sqlite(value)
    }
}

impl From<AgentError> for StoreError {
    fn from(value: AgentError) -> Self {
        Self::Domain(value.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreMigrationState {
    NotStarted,
    SchemaChecked,
    MigrationPlanned,
    MigrationApplied,
    MigrationVerified,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreMigrationEvent {
    CheckSchema,
    Plan,
    Apply,
    Verify,
    Fail,
}

impl StoreMigrationState {
    pub fn transition(self, event: StoreMigrationEvent) -> Result<Self, StoreError> {
        match (self, event) {
            (Self::NotStarted, StoreMigrationEvent::CheckSchema) => Ok(Self::SchemaChecked),
            (Self::SchemaChecked, StoreMigrationEvent::Plan) => Ok(Self::MigrationPlanned),
            (Self::MigrationPlanned, StoreMigrationEvent::Apply) => Ok(Self::MigrationApplied),
            (Self::MigrationApplied, StoreMigrationEvent::Verify) => Ok(Self::MigrationVerified),
            (
                Self::NotStarted
                | Self::SchemaChecked
                | Self::MigrationPlanned
                | Self::MigrationApplied,
                StoreMigrationEvent::Fail,
            ) => Ok(Self::Failed),
            _ => Err(StoreError::Domain(format!(
                "invalid migration transition: {self:?} -> {event:?}"
            ))),
        }
    }
}

#[derive(Default)]
pub struct MemoryAgentRepository {
    agents: BTreeMap<String, Agent>,
}

impl AgentRepository for MemoryAgentRepository {
    type Error = StoreError;

    fn save(&mut self, agent: Agent) -> Result<(), Self::Error> {
        let key = agent.id().as_str().to_owned();
        if self.agents.contains_key(&key) {
            return Err(StoreError::DuplicateAgent);
        }
        self.agents.insert(key, agent);
        Ok(())
    }

    fn find_by_id(&self, id: &AgentId) -> Result<Option<Agent>, Self::Error> {
        Ok(self.agents.get(id.as_str()).cloned())
    }

    fn list(&self) -> Result<Vec<Agent>, Self::Error> {
        Ok(self.agents.values().cloned().collect())
    }
}

pub struct SqliteStore {
    connection: Connection,
}

pub struct LocalArtifactStore {
    root: PathBuf,
}

#[cfg(feature = "postgres")]
pub struct PostgresStore {
    pool: PostgresClientPool,
}

#[cfg(feature = "postgres")]
struct PostgresClientPool {
    clients: Vec<std::cell::RefCell<postgres::Client>>,
    next_index: std::cell::Cell<usize>,
    checkout_timeout: Duration,
}

#[cfg(feature = "postgres")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PostgresStoreSslMode {
    #[default]
    Disable,
    Prefer,
    Require,
}

#[cfg(feature = "postgres")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PostgresConnectionSecurity {
    NoTls,
    TlsPreferred,
    TlsRequired,
}

#[cfg(feature = "postgres")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostgresStoreConnectSettings<'a> {
    url: &'a str,
    ssl_mode: PostgresStoreSslMode,
    connect_timeout: Duration,
    pool_max_connections: usize,
    pool_checkout_timeout: Duration,
}

#[cfg(feature = "postgres")]
impl<'a> PostgresStoreConnectSettings<'a> {
    pub const DEFAULT_POOL_MAX_CONNECTIONS: usize = 4;
    pub const DEFAULT_POOL_CHECKOUT_TIMEOUT: Duration = Duration::from_secs(5);

    pub fn new(
        url: &'a str,
        ssl_mode: PostgresStoreSslMode,
        connect_timeout: Duration,
    ) -> Result<Self, StoreError> {
        Self::with_pool_settings(
            url,
            ssl_mode,
            connect_timeout,
            Self::DEFAULT_POOL_MAX_CONNECTIONS,
            Self::DEFAULT_POOL_CHECKOUT_TIMEOUT,
        )
    }

    pub fn with_pool_settings(
        url: &'a str,
        ssl_mode: PostgresStoreSslMode,
        connect_timeout: Duration,
        pool_max_connections: usize,
        pool_checkout_timeout: Duration,
    ) -> Result<Self, StoreError> {
        if connect_timeout.is_zero() {
            return Err(StoreError::Postgres(
                "postgres connect timeout must be greater than zero".to_owned(),
            ));
        }
        if pool_max_connections == 0 {
            return Err(StoreError::Postgres(
                "postgres pool max connections must be greater than zero".to_owned(),
            ));
        }
        if pool_checkout_timeout.is_zero() {
            return Err(StoreError::Postgres(
                "postgres pool checkout timeout must be greater than zero".to_owned(),
            ));
        }
        Ok(Self {
            url,
            ssl_mode,
            connect_timeout,
            pool_max_connections,
            pool_checkout_timeout,
        })
    }

    pub fn url(&self) -> &str {
        self.url
    }

    pub fn ssl_mode(&self) -> PostgresStoreSslMode {
        self.ssl_mode
    }

    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }

    pub fn pool_max_connections(&self) -> usize {
        self.pool_max_connections
    }

    pub fn pool_checkout_timeout(&self) -> Duration {
        self.pool_checkout_timeout
    }
}

#[cfg(feature = "postgres")]
impl PostgresClientPool {
    fn connect(settings: &PostgresStoreConnectSettings<'_>) -> Result<Self, StoreError> {
        let mut clients = Vec::with_capacity(settings.pool_max_connections);
        for _ in 0..settings.pool_max_connections {
            let client = postgres_connect_client(settings)?;
            clients.push(std::cell::RefCell::new(client));
        }
        Ok(Self {
            clients,
            next_index: std::cell::Cell::new(0),
            checkout_timeout: settings.pool_checkout_timeout,
        })
    }

    fn checkout(&self) -> Result<std::cell::RefMut<'_, postgres::Client>, StoreError> {
        if self.clients.is_empty() {
            return Err(postgres_pool_checkout_error());
        }

        let started_at = std::time::Instant::now();
        loop {
            let start = self.next_index.get() % self.clients.len();
            for offset in 0..self.clients.len() {
                let index = (start + offset) % self.clients.len();
                if let Ok(client) = self.clients[index].try_borrow_mut() {
                    self.next_index.set((index + 1) % self.clients.len());
                    return Ok(client);
                }
            }
            if started_at.elapsed() >= self.checkout_timeout {
                return Err(postgres_pool_checkout_error());
            }
            std::thread::yield_now();
        }
    }
}

#[cfg(feature = "postgres")]
fn postgres_connection_security(
    settings: &PostgresStoreConnectSettings<'_>,
) -> PostgresConnectionSecurity {
    match settings.ssl_mode {
        PostgresStoreSslMode::Disable => PostgresConnectionSecurity::NoTls,
        PostgresStoreSslMode::Prefer => PostgresConnectionSecurity::TlsPreferred,
        PostgresStoreSslMode::Require => PostgresConnectionSecurity::TlsRequired,
    }
}

#[cfg(feature = "postgres")]
fn postgres_connect_client(
    settings: &PostgresStoreConnectSettings<'_>,
) -> Result<postgres::Client, StoreError> {
    match postgres_connection_security(settings) {
        PostgresConnectionSecurity::NoTls => postgres_connect_notls(settings),
        PostgresConnectionSecurity::TlsRequired => postgres_connect_tls(settings),
        PostgresConnectionSecurity::TlsPreferred => match postgres_connect_tls(settings) {
            Ok(client) => Ok(client),
            Err(_) => postgres_connect_notls(settings),
        },
    }
}

#[cfg(feature = "postgres")]
fn postgres_config(
    settings: &PostgresStoreConnectSettings<'_>,
) -> Result<postgres::Config, StoreError> {
    let mut config = settings
        .url
        .parse::<postgres::Config>()
        .map_err(|_| postgres_connection_error())?;
    config.connect_timeout(settings.connect_timeout);
    Ok(config)
}

#[cfg(feature = "postgres")]
fn postgres_connect_notls(
    settings: &PostgresStoreConnectSettings<'_>,
) -> Result<postgres::Client, StoreError> {
    postgres_config(settings)?
        .connect(postgres::NoTls)
        .map_err(|_| postgres_connection_error())
}

#[cfg(feature = "postgres")]
fn postgres_connect_tls(
    settings: &PostgresStoreConnectSettings<'_>,
) -> Result<postgres::Client, StoreError> {
    let connector = native_tls::TlsConnector::builder()
        .build()
        .map_err(|_| postgres_tls_adapter_error())?;
    let connector = postgres_native_tls::MakeTlsConnector::new(connector);
    postgres_config(settings)?
        .connect(connector)
        .map_err(|_| postgres_connection_error())
}

#[cfg(feature = "postgres")]
impl PostgresStore {
    pub fn connect(url: &str) -> Result<Self, StoreError> {
        Self::connect_with_settings(&PostgresStoreConnectSettings::new(
            url,
            PostgresStoreSslMode::Disable,
            Duration::from_secs(10),
        )?)
    }

    pub fn connect_with_settings(
        settings: &PostgresStoreConnectSettings<'_>,
    ) -> Result<Self, StoreError> {
        Ok(Self {
            pool: PostgresClientPool::connect(settings)?,
        })
    }

    fn checkout_client(&self) -> Result<std::cell::RefMut<'_, postgres::Client>, StoreError> {
        self.pool.checkout()
    }

    pub fn find_remediation_verification_job_id(
        &self,
        remediation_id: &str,
    ) -> Result<Option<String>, StoreError> {
        self.checkout_client()?
            .query_opt(
                "SELECT job_id FROM remediation_verification_jobs WHERE remediation_id = $1",
                &[&remediation_id],
            )
            .map(|row| row.map(|row| row.get(0)))
            .map_err(|_| postgres_error("postgres remediation verification lookup failed"))
    }

    pub fn find_remediation_request_by_verification_job_id(
        &self,
        verification_job_id: &str,
    ) -> Result<Option<AppRemediationRequestRecord>, StoreError> {
        let row = self
            .checkout_client()?
            .query_opt(
                "SELECT remediation_requests.id, policy_id, policy_name, agent_id, runbook_ref, status,
                        approval_required, risk_summary, remediation_requests.job_id,
                        origin_drift_report_id, policy_version, remediation_requests.created_at,
                        remediation_requests.updated_at
                 FROM remediation_requests
                 JOIN remediation_verification_jobs
                   ON remediation_verification_jobs.remediation_id = remediation_requests.id
                 WHERE remediation_verification_jobs.job_id = $1",
                &[&verification_job_id],
            )
            .map_err(|_| postgres_error("postgres remediation verification request lookup failed"))?;
        row.map(|row| postgres_row_to_remediation_request_record(&row))
            .transpose()
    }

    pub fn find_drift_report_by_correlation(
        &self,
        job_id: &str,
        task_id: &str,
    ) -> Result<Option<AppDriftReportRecord>, StoreError> {
        let row = self
            .checkout_client()?
            .query_opt(
                "SELECT id, agent_id, policy_name, status, expected, actual, checked_at,
                        severity, acknowledged_at, acknowledged_by, resolved_at, resolution_job_id,
                        job_id, task_id, policy_id, policy_version, purpose
                 FROM drift_reports WHERE job_id = $1 AND task_id = $2",
                &[&job_id, &task_id],
            )
            .map_err(|_| postgres_error("postgres drift correlation lookup failed"))?;
        row.map(|row| postgres_row_to_drift_report_record(&row))
            .transpose()
    }

    pub fn find_task_assignment_state_for_job(
        &self,
        job_id: &str,
    ) -> Result<Option<TaskAssignmentStateRecord>, StoreError> {
        self.checkout_client()?
            .query_opt(
                "SELECT job_id, id, agent_id, status, completed_at FROM task_assignments
                 WHERE job_id = $1 ORDER BY created_at, id LIMIT 1",
                &[&job_id],
            )
            .map(|row| {
                row.map(|row| TaskAssignmentStateRecord {
                    job_id: row.get(0),
                    task_id: row.get(1),
                    agent_id: row.get(2),
                    status: row.get(3),
                    completed_at: row.get::<_, Option<i64>>(4).map(unix_secs_to_system_time),
                })
            })
            .map_err(|_| postgres_error("postgres assignment state lookup failed"))
    }

    pub fn save_remediation_verification_job(
        &self,
        remediation_id: &str,
        job_id: &str,
        created_at: SystemTime,
    ) -> Result<bool, StoreError> {
        self.checkout_client()?
            .execute(
                "INSERT INTO remediation_verification_jobs (remediation_id, job_id, created_at)
                 VALUES ($1, $2, $3) ON CONFLICT (remediation_id) DO NOTHING",
                &[
                    &remediation_id,
                    &job_id,
                    &system_time_to_unix_secs(created_at),
                ],
            )
            .map(|inserted| inserted > 0)
            .map_err(|_| postgres_error("postgres remediation verification insert failed"))
    }

    #[cfg(test)]
    fn empty_pool_for_test(checkout_timeout: Duration) -> Self {
        Self {
            pool: PostgresClientPool {
                clients: Vec::new(),
                next_index: std::cell::Cell::new(0),
                checkout_timeout,
            },
        }
    }

    pub fn migrate(&mut self) -> Result<(), StoreError> {
        let mut state = StoreMigrationState::NotStarted
            .transition(StoreMigrationEvent::CheckSchema)?
            .transition(StoreMigrationEvent::Plan)?;
        let mut client = self.checkout_client()?;
        client
            .batch_execute(
                "CREATE TABLE IF NOT EXISTS schema_migrations (
                    name TEXT PRIMARY KEY,
                    version BIGINT NOT NULL,
                    applied_at BIGINT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS controller_identity (
                    id BIGINT PRIMARY KEY CHECK (id = 1),
                    public_key TEXT NOT NULL,
                    public_fingerprint TEXT NOT NULL,
                    private_key_path TEXT NOT NULL,
                    created_at BIGINT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS controller_signing_key_rotation (
                    controller_id TEXT PRIMARY KEY,
                    state TEXT NOT NULL,
                    old_fingerprint TEXT NOT NULL,
                    new_fingerprint TEXT,
                    requested_at BIGINT,
                    validated_at BIGINT,
                    activated_at BIGINT,
                    old_key_verifies_until BIGINT,
                    retired_at BIGINT,
                    failed_at BIGINT,
                    updated_at BIGINT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS controller_signing_staged_rollout (
                    controller_id TEXT PRIMARY KEY,
                    state TEXT NOT NULL,
                    target_ids TEXT NOT NULL,
                    batch_size BIGINT NOT NULL,
                    max_failures BIGINT NOT NULL,
                    ack_timeout_seconds BIGINT NOT NULL,
                    acknowledged_agent_ids TEXT NOT NULL,
                    unavailable_agent_ids TEXT NOT NULL,
                    failed_agent_ids TEXT NOT NULL,
                    in_flight_attempts TEXT NOT NULL,
                    failure_reason_code TEXT,
                    current_fingerprint TEXT NOT NULL,
                    previous_fingerprint TEXT,
                    updated_at BIGINT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS agent_certificate_lifecycle (
                    agent_id TEXT PRIMARY KEY,
                    state TEXT NOT NULL,
                    current_serial TEXT,
                    current_fingerprint TEXT,
                    current_not_before BIGINT,
                    current_not_after BIGINT,
                    next_serial TEXT,
                    next_fingerprint TEXT,
                    next_not_before BIGINT,
                    next_not_after BIGINT,
                    grace_until BIGINT,
                    revocation_reason TEXT,
                    updated_at BIGINT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS admin_tokens (
                    id BIGINT PRIMARY KEY CHECK (id = 1),
                    token_hash TEXT NOT NULL,
                    actor_id TEXT NOT NULL DEFAULT 'bootstrap-admin',
                    role TEXT NOT NULL DEFAULT 'owner',
                    created_at BIGINT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS agents (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL UNIQUE,
                    public_key TEXT NOT NULL,
                    fingerprint TEXT NOT NULL UNIQUE,
                    labels TEXT NOT NULL DEFAULT '',
                    status TEXT NOT NULL,
                    last_seen_at BIGINT,
                    pinned_controller TEXT NOT NULL DEFAULT '',
                    created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM now())::BIGINT),
                    updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM now())::BIGINT)
                );

                CREATE TABLE IF NOT EXISTS agent_identities (
                    agent_id TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
                    public_key TEXT NOT NULL,
                    fingerprint TEXT NOT NULL UNIQUE,
                    created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM now())::BIGINT)
                );

                CREATE TABLE IF NOT EXISTS enrollment_tokens (
                    id TEXT PRIMARY KEY,
                    token_hash TEXT NOT NULL UNIQUE,
                    default_labels TEXT NOT NULL DEFAULT '',
                    expires_at BIGINT NOT NULL,
                    max_uses BIGINT NOT NULL,
                    used_count BIGINT NOT NULL DEFAULT 0,
                    revoked_at BIGINT,
                    created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM now())::BIGINT)
                );

                CREATE TABLE IF NOT EXISTS jobs (
                    id TEXT PRIMARY KEY,
                    status TEXT NOT NULL,
                    risk TEXT NOT NULL,
                    approval_requirement TEXT NOT NULL,
                    timeout_ms BIGINT NOT NULL,
                    command_program TEXT,
                    command_args_json TEXT NOT NULL DEFAULT '[]',
                    command_max_output_bytes BIGINT NOT NULL DEFAULT 1048576,
                    drift_policy_document TEXT,
                    drift_policy_id TEXT,
                    drift_policy_version BIGINT,
                    drift_purpose TEXT,
                    runbook_document TEXT,
                    selector_kind TEXT NOT NULL DEFAULT 'explicit_ids',
                    selector_source TEXT NOT NULL DEFAULT '',
                    strategy_concurrency BIGINT NOT NULL DEFAULT 1,
                    strategy_max_failures BIGINT,
                    created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM now())::BIGINT)
                );

                CREATE TABLE IF NOT EXISTS job_targets (
                    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
                    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
                    status TEXT NOT NULL,
                    agent_display_name TEXT NOT NULL DEFAULT '',
                    agent_status_snapshot TEXT NOT NULL DEFAULT 'unknown',
                    labels_snapshot TEXT NOT NULL DEFAULT '',
                    PRIMARY KEY (job_id, agent_id)
                );

                ALTER TABLE jobs ADD COLUMN IF NOT EXISTS drift_policy_id TEXT;
                ALTER TABLE jobs ADD COLUMN IF NOT EXISTS drift_policy_version BIGINT;
                ALTER TABLE jobs ADD COLUMN IF NOT EXISTS drift_purpose TEXT;

                CREATE TABLE IF NOT EXISTS task_assignments (
                    id TEXT PRIMARY KEY,
                    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
                    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
                    status TEXT NOT NULL DEFAULT 'queued',
                    nonce TEXT NOT NULL UNIQUE,
                    payload_hash TEXT NOT NULL,
                    signature TEXT NOT NULL,
                    issued_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM now())::BIGINT),
                    expires_at BIGINT NOT NULL,
                    dispatched_at BIGINT,
                    accepted_at BIGINT,
                    started_at BIGINT,
                    completed_at BIGINT,
                    last_error TEXT NOT NULL DEFAULT '',
                    created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM now())::BIGINT)
                );

                CREATE TABLE IF NOT EXISTS job_output_chunks (
                    id BIGSERIAL PRIMARY KEY,
                    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
                    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
                    stream TEXT NOT NULL,
                    chunk_index BIGINT NOT NULL,
                    body TEXT NOT NULL,
                    created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM now())::BIGINT),
                    UNIQUE(job_id, agent_id, stream, chunk_index)
                );

                CREATE TABLE IF NOT EXISTS rendered_artifacts (
                    id TEXT PRIMARY KEY,
                    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
                    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
                    task_id TEXT NOT NULL,
                    destination TEXT NOT NULL,
                    checksum_sha256 TEXT NOT NULL,
                    size_bytes BIGINT NOT NULL CHECK(size_bytes > 0),
                    retention_class TEXT NOT NULL,
                    created_at BIGINT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS rendered_artifacts_job_order_idx
                    ON rendered_artifacts (job_id, created_at, id);

                CREATE TABLE IF NOT EXISTS remediation_requests (
                    id TEXT PRIMARY KEY,
                    policy_id TEXT NOT NULL,
                    policy_name TEXT NOT NULL,
                    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
                    runbook_ref TEXT NOT NULL,
                    status TEXT NOT NULL,
                    approval_required BOOLEAN NOT NULL,
                    risk_summary TEXT NOT NULL,
                    job_id TEXT,
                    origin_drift_report_id BIGINT,
                    policy_version BIGINT,
                    created_at BIGINT NOT NULL,
                    updated_at BIGINT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS remediation_requests_filter_order_idx
                    ON remediation_requests (agent_id, policy_id, created_at, id);

                ALTER TABLE remediation_requests ADD COLUMN IF NOT EXISTS origin_drift_report_id BIGINT;
                ALTER TABLE remediation_requests ADD COLUMN IF NOT EXISTS policy_version BIGINT;

                CREATE UNIQUE INDEX IF NOT EXISTS remediation_requests_active_policy_unique_idx
                    ON remediation_requests (agent_id, policy_id)
                    WHERE origin_drift_report_id IS NOT NULL
                      AND status NOT IN ('resolved', 'failed', 'rejected', 'expired', 'canceled');

                CREATE TABLE IF NOT EXISTS remediation_verification_jobs (
                    remediation_id TEXT PRIMARY KEY REFERENCES remediation_requests(id) ON DELETE CASCADE,
                    job_id TEXT NOT NULL UNIQUE REFERENCES jobs(id) ON DELETE RESTRICT,
                    created_at BIGINT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS facts_snapshots (
                    id BIGSERIAL PRIMARY KEY,
                    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
                    body TEXT NOT NULL,
                    collected_at BIGINT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS metrics_snapshots (
                    id BIGSERIAL PRIMARY KEY,
                    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
                    body TEXT NOT NULL,
                    collected_at BIGINT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS agent_log_chunks (
                    id BIGSERIAL PRIMARY KEY,
                    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
                    line TEXT NOT NULL,
                    collected_at BIGINT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS job_output_chunks_agent_order_idx
                    ON job_output_chunks (job_id, agent_id, chunk_index, stream);

                CREATE INDEX IF NOT EXISTS facts_snapshots_agent_page_idx
                    ON facts_snapshots (agent_id, collected_at DESC, id DESC);

                CREATE INDEX IF NOT EXISTS metrics_snapshots_agent_page_idx
                    ON metrics_snapshots (agent_id, collected_at DESC, id DESC);

                CREATE INDEX IF NOT EXISTS agent_log_chunks_agent_page_idx
                    ON agent_log_chunks (agent_id, collected_at DESC, id DESC);

                CREATE TABLE IF NOT EXISTS agent_capability_snapshots (
                    agent_id TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
                    privilege_level TEXT NOT NULL,
                    package_manager TEXT,
                    service_manager TEXT,
                    capabilities_json TEXT NOT NULL,
                    reported_at BIGINT NOT NULL,
                    updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM now())::BIGINT)
                );

                CREATE TABLE IF NOT EXISTS drift_reports (
                    id BIGSERIAL PRIMARY KEY,
                    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
                    policy_name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    severity TEXT NOT NULL DEFAULT 'unknown',
                    expected TEXT NOT NULL,
                    actual TEXT NOT NULL,
                    checked_at BIGINT NOT NULL,
                    acknowledged_at BIGINT,
                    acknowledged_by TEXT,
                    resolved_at BIGINT,
                    resolution_job_id TEXT,
                    job_id TEXT,
                    task_id TEXT,
                    policy_id TEXT,
                    policy_version BIGINT,
                    purpose TEXT
                );

                CREATE INDEX IF NOT EXISTS drift_reports_agent_page_idx
                    ON drift_reports (agent_id, checked_at DESC, id DESC);

                ALTER TABLE drift_reports ADD COLUMN IF NOT EXISTS job_id TEXT;
                ALTER TABLE drift_reports ADD COLUMN IF NOT EXISTS task_id TEXT;
                ALTER TABLE drift_reports ADD COLUMN IF NOT EXISTS policy_id TEXT;
                ALTER TABLE drift_reports ADD COLUMN IF NOT EXISTS policy_version BIGINT;
                ALTER TABLE drift_reports ADD COLUMN IF NOT EXISTS purpose TEXT;

                CREATE UNIQUE INDEX IF NOT EXISTS drift_reports_correlated_task_unique_idx
                    ON drift_reports (job_id, task_id)
                    WHERE job_id IS NOT NULL AND task_id IS NOT NULL;

                CREATE TABLE IF NOT EXISTS policies (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    version BIGINT NOT NULL,
                    source TEXT NOT NULL,
                    created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM now())::BIGINT),
                    updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM now())::BIGINT)
                );

                CREATE TABLE IF NOT EXISTS policy_assignments (
                    policy_id TEXT NOT NULL REFERENCES policies(id) ON DELETE CASCADE,
                    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
                    assigned_at BIGINT NOT NULL,
                    PRIMARY KEY (policy_id, agent_id)
                );

                CREATE INDEX IF NOT EXISTS policy_assignments_agent_idx
                    ON policy_assignments (agent_id, policy_id);

                CREATE TABLE IF NOT EXISTS policy_drift_schedules (
                    policy_id TEXT NOT NULL REFERENCES policies(id) ON DELETE CASCADE,
                    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
                    interval_seconds BIGINT NOT NULL,
                    next_due_at BIGINT NOT NULL,
                    last_checked_at BIGINT,
                    PRIMARY KEY (policy_id, agent_id)
                );

                CREATE INDEX IF NOT EXISTS policy_drift_schedules_due_idx
                    ON policy_drift_schedules (next_due_at ASC, policy_id, agent_id);

                CREATE TABLE IF NOT EXISTS approval_requests (
                    id TEXT PRIMARY KEY,
                    job_id TEXT NOT NULL,
                    requester TEXT NOT NULL,
                    approver TEXT,
                    reason TEXT NOT NULL DEFAULT '',
                    status TEXT NOT NULL,
                    expires_at BIGINT NOT NULL,
                    created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM now())::BIGINT),
                    decided_at BIGINT
                );

                CREATE INDEX IF NOT EXISTS approval_requests_status_created_idx
                    ON approval_requests (status, created_at DESC, id DESC);

                CREATE INDEX IF NOT EXISTS approval_requests_job_pending_idx
                    ON approval_requests (job_id, status, created_at DESC, id DESC);

                CREATE TABLE IF NOT EXISTS audit_events (
                    id BIGSERIAL PRIMARY KEY,
                    category TEXT NOT NULL,
                    action TEXT NOT NULL,
                    actor TEXT NOT NULL,
                    target TEXT NOT NULL,
                    value_kind TEXT NOT NULL,
                    value_text TEXT NOT NULL DEFAULT '',
                    occurred_at BIGINT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS audit_events_category_id_idx
                    ON audit_events (category, id DESC);

                CREATE INDEX IF NOT EXISTS audit_events_category_cursor_idx
                    ON audit_events (category, occurred_at DESC, id DESC);

                CREATE INDEX IF NOT EXISTS audit_events_cursor_idx
                    ON audit_events (occurred_at DESC, id DESC)",
            )
            .map_err(|_| StoreError::Postgres("postgres schema migration failed".to_owned()))?;
        client
            .execute(
                "INSERT INTO schema_migrations (name, version, applied_at)
                 VALUES ($1, $2, EXTRACT(EPOCH FROM now())::BIGINT)
                 ON CONFLICT (name) DO UPDATE
                 SET version = EXCLUDED.version,
                     applied_at = CASE
                         WHEN schema_migrations.version = EXCLUDED.version
                         THEN schema_migrations.applied_at
                         ELSE EXCLUDED.applied_at
                     END",
                &[&"fleet_store", &CURRENT_SCHEMA_VERSION],
            )
            .map_err(|_| StoreError::Postgres("postgres schema migration failed".to_owned()))?;
        state = state.transition(StoreMigrationEvent::Apply)?;
        let _verified = state.transition(StoreMigrationEvent::Verify)?;
        Ok(())
    }

    pub fn schema_version(&mut self) -> Result<Option<i64>, StoreError> {
        self.checkout_client()?
            .query_opt(
                "SELECT version FROM schema_migrations WHERE name = $1",
                &[&"fleet_store"],
            )
            .map(|row| row.map(|row| row.get(0)))
            .map_err(|_| StoreError::Postgres("postgres schema version query failed".to_owned()))
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCommandAssignment {
    pub envelope: TaskEnvelope,
    pub command: CommandTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDriftCheckAssignment {
    pub envelope: TaskEnvelope,
    pub drift_check: DriftCheckTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRunbookAssignment {
    pub envelope: TaskEnvelope,
    pub runbook: RunbookExecutionTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAssignmentStateRecord {
    pub job_id: String,
    pub task_id: String,
    pub agent_id: String,
    pub status: String,
    pub completed_at: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftAssignmentProvenanceRecord {
    pub job_id: String,
    pub task_id: String,
    pub agent_id: String,
    pub policy_id: String,
    pub policy_version: u32,
    pub purpose: DriftCheckPurpose,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAssignmentSummaryRecord {
    pub job_id: String,
    pub task_id: String,
    pub agent_id: String,
    pub status: String,
    pub last_error: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobStrategyRecord {
    pub concurrency: u32,
    pub max_failures: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobDispatchGateRecord {
    pub concurrency: u32,
    pub max_failures: Option<u32>,
    pub active_count: usize,
    pub failure_count: usize,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftReportRecord {
    pub id: DriftReportId,
    pub agent_id: String,
    pub report: DriftReport,
    pub provenance: DriftReportProvenance,
    pub checked_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftReportPageRecord {
    pub id: DriftReportId,
    pub agent_id: String,
    pub report: DriftReport,
    pub provenance: DriftReportProvenance,
    pub checked_at: SystemTime,
    pub cursor: SnapshotPageCursor,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLogChunkRecord {
    pub agent_id: String,
    pub line: String,
    pub collected_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLogChunkPageRecord {
    pub agent_id: String,
    pub line: String,
    pub collected_at: SystemTime,
    pub cursor: SnapshotPageCursor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self, StoreError> {
        let connection = Connection::open_in_memory()?;
        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn migrate(&self) -> Result<(), StoreError> {
        self.connection.execute_batch(SCHEMA_SQL)?;
        self.ensure_column(
            "jobs",
            "drift_policy_document",
            "ALTER TABLE jobs ADD COLUMN drift_policy_document TEXT",
        )?;
        self.ensure_column(
            "jobs",
            "runbook_document",
            "ALTER TABLE jobs ADD COLUMN runbook_document TEXT",
        )?;
        self.ensure_column(
            "jobs",
            "drift_policy_id",
            "ALTER TABLE jobs ADD COLUMN drift_policy_id TEXT",
        )?;
        self.ensure_column(
            "jobs",
            "drift_policy_version",
            "ALTER TABLE jobs ADD COLUMN drift_policy_version INTEGER",
        )?;
        self.ensure_column(
            "jobs",
            "drift_purpose",
            "ALTER TABLE jobs ADD COLUMN drift_purpose TEXT",
        )?;
        self.ensure_column(
            "jobs",
            "selector_kind",
            "ALTER TABLE jobs ADD COLUMN selector_kind TEXT NOT NULL DEFAULT 'explicit_ids'",
        )?;
        self.ensure_column(
            "jobs",
            "selector_source",
            "ALTER TABLE jobs ADD COLUMN selector_source TEXT NOT NULL DEFAULT ''",
        )?;
        self.ensure_column(
            "jobs",
            "strategy_concurrency",
            "ALTER TABLE jobs ADD COLUMN strategy_concurrency INTEGER NOT NULL DEFAULT 1",
        )?;
        self.ensure_column(
            "jobs",
            "strategy_max_failures",
            "ALTER TABLE jobs ADD COLUMN strategy_max_failures INTEGER",
        )?;
        self.ensure_column(
            "job_targets",
            "agent_display_name",
            "ALTER TABLE job_targets ADD COLUMN agent_display_name TEXT NOT NULL DEFAULT ''",
        )?;
        self.ensure_column(
            "job_targets",
            "agent_status_snapshot",
            "ALTER TABLE job_targets ADD COLUMN agent_status_snapshot TEXT NOT NULL DEFAULT 'unknown'",
        )?;
        self.ensure_column(
            "job_targets",
            "labels_snapshot",
            "ALTER TABLE job_targets ADD COLUMN labels_snapshot TEXT NOT NULL DEFAULT ''",
        )?;
        self.ensure_column(
            "task_assignments",
            "status",
            "ALTER TABLE task_assignments ADD COLUMN status TEXT NOT NULL DEFAULT 'queued'",
        )?;
        self.ensure_column(
            "task_assignments",
            "dispatched_at",
            "ALTER TABLE task_assignments ADD COLUMN dispatched_at INTEGER",
        )?;
        self.ensure_column(
            "task_assignments",
            "accepted_at",
            "ALTER TABLE task_assignments ADD COLUMN accepted_at INTEGER",
        )?;
        self.ensure_column(
            "task_assignments",
            "started_at",
            "ALTER TABLE task_assignments ADD COLUMN started_at INTEGER",
        )?;
        self.ensure_column(
            "task_assignments",
            "completed_at",
            "ALTER TABLE task_assignments ADD COLUMN completed_at INTEGER",
        )?;
        self.ensure_column(
            "task_assignments",
            "last_error",
            "ALTER TABLE task_assignments ADD COLUMN last_error TEXT NOT NULL DEFAULT ''",
        )?;
        self.ensure_column(
            "admin_tokens",
            "actor_id",
            "ALTER TABLE admin_tokens ADD COLUMN actor_id TEXT NOT NULL DEFAULT 'bootstrap-admin'",
        )?;
        self.ensure_column(
            "admin_tokens",
            "role",
            "ALTER TABLE admin_tokens ADD COLUMN role TEXT NOT NULL DEFAULT 'owner'",
        )?;
        self.ensure_column(
            "drift_reports",
            "severity",
            "ALTER TABLE drift_reports ADD COLUMN severity TEXT NOT NULL DEFAULT 'unknown'",
        )?;
        self.ensure_column(
            "remediation_requests",
            "origin_drift_report_id",
            "ALTER TABLE remediation_requests ADD COLUMN origin_drift_report_id INTEGER",
        )?;
        self.ensure_column(
            "remediation_requests",
            "policy_version",
            "ALTER TABLE remediation_requests ADD COLUMN policy_version INTEGER",
        )?;
        self.connection.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS remediation_requests_active_policy_unique_idx
             ON remediation_requests (agent_id, policy_id)
             WHERE origin_drift_report_id IS NOT NULL
               AND status NOT IN ('resolved', 'failed', 'rejected', 'expired', 'canceled');",
        )?;
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS remediation_verification_jobs (
                remediation_id TEXT PRIMARY KEY REFERENCES remediation_requests(id) ON DELETE CASCADE,
                job_id TEXT NOT NULL UNIQUE REFERENCES jobs(id) ON DELETE RESTRICT,
                created_at INTEGER NOT NULL
            );",
        )?;
        self.ensure_column(
            "drift_reports",
            "acknowledged_at",
            "ALTER TABLE drift_reports ADD COLUMN acknowledged_at INTEGER",
        )?;
        self.ensure_column(
            "drift_reports",
            "acknowledged_by",
            "ALTER TABLE drift_reports ADD COLUMN acknowledged_by TEXT",
        )?;
        self.ensure_column(
            "drift_reports",
            "resolved_at",
            "ALTER TABLE drift_reports ADD COLUMN resolved_at INTEGER",
        )?;
        self.ensure_column(
            "drift_reports",
            "resolution_job_id",
            "ALTER TABLE drift_reports ADD COLUMN resolution_job_id TEXT",
        )?;
        self.ensure_column(
            "drift_reports",
            "job_id",
            "ALTER TABLE drift_reports ADD COLUMN job_id TEXT",
        )?;
        self.ensure_column(
            "drift_reports",
            "task_id",
            "ALTER TABLE drift_reports ADD COLUMN task_id TEXT",
        )?;
        self.ensure_column(
            "drift_reports",
            "policy_id",
            "ALTER TABLE drift_reports ADD COLUMN policy_id TEXT",
        )?;
        self.ensure_column(
            "drift_reports",
            "policy_version",
            "ALTER TABLE drift_reports ADD COLUMN policy_version INTEGER",
        )?;
        self.ensure_column(
            "drift_reports",
            "purpose",
            "ALTER TABLE drift_reports ADD COLUMN purpose TEXT",
        )?;
        self.connection.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS drift_reports_correlated_task_unique_idx
             ON drift_reports (job_id, task_id)
             WHERE job_id IS NOT NULL AND task_id IS NOT NULL;",
        )?;
        self.ensure_approval_requests_allow_reserved_job_id()?;
        self.record_schema_version()?;
        Ok(())
    }

    fn record_schema_version(&self) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO schema_migrations (name, version, applied_at)
             VALUES ('fleet_store', ?1, unixepoch())
             ON CONFLICT(name) DO UPDATE SET
                version = excluded.version,
                applied_at = CASE
                    WHEN schema_migrations.version = excluded.version
                    THEN schema_migrations.applied_at
                    ELSE excluded.applied_at
                END",
            params![CURRENT_SCHEMA_VERSION],
        )?;
        Ok(())
    }

    pub fn current_schema_version(&self) -> Result<Option<i64>, StoreError> {
        self.connection
            .query_row(
                "SELECT version FROM schema_migrations WHERE name = 'fleet_store'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn find_remediation_request_by_verification_job_id_record(
        &self,
        verification_job_id: &str,
    ) -> Result<Option<AppRemediationRequestRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT remediation_requests.id, policy_id, policy_name, agent_id, runbook_ref, status,
                        approval_required, risk_summary, remediation_requests.job_id,
                        origin_drift_report_id, policy_version, remediation_requests.created_at,
                        remediation_requests.updated_at
                 FROM remediation_requests
                 JOIN remediation_verification_jobs
                   ON remediation_verification_jobs.remediation_id = remediation_requests.id
                 WHERE remediation_verification_jobs.job_id = ?1",
                params![verification_job_id],
                row_to_remediation_request_record,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn find_drift_report_by_correlation(
        &self,
        job_id: &str,
        task_id: &str,
    ) -> Result<Option<AppDriftReportRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT id, agent_id, policy_name, status, expected, actual, checked_at,
                        severity, acknowledged_at, acknowledged_by, resolved_at, resolution_job_id,
                        job_id, task_id, policy_id, policy_version, purpose
                 FROM drift_reports WHERE job_id = ?1 AND task_id = ?2",
                params![job_id, task_id],
                |row| {
                    row_to_drift_report_page_record(row).map(|record| AppDriftReportRecord {
                        id: record.id,
                        agent_id: record.agent_id,
                        report: record.report,
                        provenance: record.provenance,
                        checked_at: record.checked_at,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn integrity_check(&self) -> Result<String, StoreError> {
        self.connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(StoreError::from)
    }

    fn ensure_column(&self, table: &str, column: &str, statement: &str) -> Result<(), StoreError> {
        if !self.has_column(table, column)? {
            self.connection.execute(statement, [])?;
        }
        Ok(())
    }

    fn ensure_approval_requests_allow_reserved_job_id(&self) -> Result<(), StoreError> {
        if !self.approval_requests_has_job_fk()? {
            return Ok(());
        }

        self.connection
            .execute_batch("PRAGMA foreign_keys = OFF;")?;
        let rebuild = self.connection.execute_batch(
            "
            CREATE TABLE approval_requests_without_job_fk (
                id TEXT PRIMARY KEY,
                job_id TEXT NOT NULL,
                requester TEXT NOT NULL,
                approver TEXT,
                reason TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL,
                expires_at INTEGER NOT NULL,
                created_at INTEGER NOT NULL DEFAULT (unixepoch()),
                decided_at INTEGER
            );
            INSERT INTO approval_requests_without_job_fk (
                id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
            )
            SELECT id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
            FROM approval_requests;
            DROP TABLE approval_requests;
            ALTER TABLE approval_requests_without_job_fk RENAME TO approval_requests;
            ",
        );
        let restore = self.connection.execute_batch("PRAGMA foreign_keys = ON;");

        match (rebuild, restore) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), _) => Err(StoreError::from(error)),
            (Ok(()), Err(error)) => Err(StoreError::from(error)),
        }
    }

    fn approval_requests_has_job_fk(&self) -> Result<bool, StoreError> {
        let mut statement = self
            .connection
            .prepare("PRAGMA foreign_key_list(approval_requests)")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let table: String = row.get(2)?;
            let from: String = row.get(3)?;
            if table == "jobs" && from == "job_id" {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn has_column(&self, table: &str, column: &str) -> Result<bool, StoreError> {
        let mut statement = self
            .connection
            .prepare(&format!("PRAGMA table_info({table})"))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get(1)?;
            if name == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn insert_admin_token_hash(&self, token_hash: &str) -> Result<(), StoreError> {
        self.insert_admin_token_hash_with_identity(token_hash, "bootstrap-admin", "owner")
    }

    pub fn insert_admin_token_hash_with_identity(
        &self,
        token_hash: &str,
        actor_id: &str,
        role: &str,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO admin_tokens (id, token_hash, actor_id, role, created_at)
             VALUES (1, ?1, ?2, ?3, unixepoch())
             ON CONFLICT(id) DO NOTHING",
            params![token_hash, actor_id, role],
        )?;
        Ok(())
    }

    pub fn admin_token_exists(&self) -> Result<bool, StoreError> {
        Ok(self
            .connection
            .prepare("SELECT 1 FROM admin_tokens WHERE id = 1")?
            .exists([])?)
    }

    pub fn verify_admin_token_hash(&self, token_hash: &str) -> Result<bool, StoreError> {
        Ok(self
            .connection
            .prepare("SELECT 1 FROM admin_tokens WHERE id = 1 AND token_hash = ?1")?
            .exists(params![token_hash])?)
    }

    pub fn find_admin_token_record(
        &self,
        token_hash: &str,
    ) -> Result<Option<AppAdminTokenRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT actor_id, role
                 FROM admin_tokens
                 WHERE id = 1 AND token_hash = ?1",
                params![token_hash],
                |row| {
                    Ok(AppAdminTokenRecord {
                        actor_id: row.get(0)?,
                        role: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn insert_enrollment_token_hash(
        &self,
        id: &str,
        token_hash: &str,
        default_labels: &str,
        expires_at: SystemTime,
        max_uses: u32,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO enrollment_tokens (
                id, token_hash, default_labels, expires_at, max_uses, used_count
             ) VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            params![
                id,
                token_hash,
                default_labels,
                system_time_to_unix_secs(expires_at),
                max_uses,
            ],
        )?;
        Ok(())
    }

    pub fn list_enrollment_tokens(&self) -> Result<Vec<EnrollmentTokenRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, default_labels, expires_at, max_uses, used_count, revoked_at
             FROM enrollment_tokens
             ORDER BY created_at DESC",
        )?;
        let mut rows = statement.query([])?;
        let mut records = Vec::new();
        while let Some(row) = rows.next()? {
            records.push(EnrollmentTokenRecord {
                id: row.get(0)?,
                default_labels: row.get(1)?,
                expires_at: unix_secs_to_system_time(row.get(2)?),
                max_uses: row.get::<_, i64>(3)?.max(0) as u32,
                used_count: row.get::<_, i64>(4)?.max(0) as u32,
                revoked: row.get::<_, Option<i64>>(5)?.is_some(),
            });
        }
        Ok(records)
    }

    pub fn revoke_enrollment_token(&self, id: &str) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE enrollment_tokens
             SET revoked_at = unixepoch()
             WHERE id = ?1 AND revoked_at IS NULL",
            params![id],
        )?;
        Ok(changed > 0)
    }

    pub fn consume_enrollment_token_hash(
        &self,
        token_hash: &str,
        now: SystemTime,
    ) -> Result<EnrollmentTokenRecord, StoreError> {
        let record = self
            .connection
            .query_row(
                "SELECT id, default_labels, expires_at, max_uses, used_count, revoked_at
                 FROM enrollment_tokens
                 WHERE token_hash = ?1",
                params![token_hash],
                |row| {
                    Ok(EnrollmentTokenRecord {
                        id: row.get(0)?,
                        default_labels: row.get(1)?,
                        expires_at: unix_secs_to_system_time(row.get(2)?),
                        max_uses: row.get::<_, i64>(3)?.max(0) as u32,
                        used_count: row.get::<_, i64>(4)?.max(0) as u32,
                        revoked: row.get::<_, Option<i64>>(5)?.is_some(),
                    })
                },
            )
            .optional()?
            .ok_or(StoreError::NotFound)?;

        if record.revoked {
            return Err(StoreError::Domain("enrollment token is revoked".to_owned()));
        }
        if now >= record.expires_at {
            return Err(StoreError::Domain("enrollment token is expired".to_owned()));
        }
        if record.used_count >= record.max_uses {
            return Err(StoreError::Domain(
                "enrollment token max uses exceeded".to_owned(),
            ));
        }

        self.connection.execute(
            "UPDATE enrollment_tokens
             SET used_count = used_count + 1
             WHERE id = ?1",
            params![record.id],
        )?;

        Ok(record)
    }

    pub fn save_agent(&self, agent: Agent) -> Result<(), StoreError> {
        self.insert_agent(&agent)
    }

    pub fn agent_count(&self) -> Result<usize, StoreError> {
        let count: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM agents", [], |row| row.get(0))?;
        Ok(count.max(0) as usize)
    }

    pub fn list_agents(&self) -> Result<Vec<Agent>, StoreError> {
        <Self as AgentRepository>::list(self)
    }

    pub fn find_agent_by_id(&self, agent_id: &str) -> Result<Option<Agent>, StoreError> {
        let agent_id = AgentId::new(agent_id).map_err(StoreError::from)?;
        <Self as AgentRepository>::find_by_id(self, &agent_id)
    }

    pub fn update_agent_labels(
        &self,
        agent_id: &str,
        labels: &[AgentLabel],
    ) -> Result<bool, StoreError> {
        let labels = encode_labels(labels);
        let changed = self.connection.execute(
            "UPDATE agents
             SET labels = ?2, updated_at = unixepoch()
             WHERE id = ?1",
            params![agent_id, labels],
        )?;
        Ok(changed > 0)
    }

    pub fn revoke_agent_key(&self, agent_id: &str) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE agents
             SET status = 'disabled', updated_at = unixepoch()
             WHERE id = ?1",
            params![agent_id],
        )?;
        Ok(changed > 0)
    }

    pub fn insert_facts_snapshot(
        &self,
        agent_id: &str,
        body: &str,
        collected_at: SystemTime,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO facts_snapshots (agent_id, body, collected_at)
             VALUES (?1, ?2, ?3)",
            params![agent_id, body, system_time_to_unix_secs(collected_at)],
        )?;
        Ok(())
    }

    pub fn latest_facts_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<FactsSnapshotRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT agent_id, body, collected_at
                 FROM facts_snapshots
                 WHERE agent_id = ?1
                 ORDER BY collected_at DESC, id DESC
                 LIMIT 1",
                params![agent_id],
                |row| {
                    Ok(FactsSnapshotRecord {
                        agent_id: row.get(0)?,
                        body: row.get(1)?,
                        collected_at: unix_secs_to_system_time(row.get(2)?),
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn list_facts_snapshots(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<FactsSnapshotPageRecord>, StoreError> {
        let limit = limit.clamp(1, 501) as i64;
        if let Some(before) = before {
            let before_secs = system_time_to_unix_secs(before.occurred_at);
            let mut statement = self.connection.prepare(
                "SELECT id, agent_id, body, collected_at
                 FROM facts_snapshots
                 WHERE agent_id = ?1
                   AND (collected_at < ?2 OR (collected_at = ?2 AND id < ?3))
                 ORDER BY collected_at DESC, id DESC
                 LIMIT ?4",
            )?;
            return statement
                .query_map(
                    params![agent_id, before_secs, before.row_id, limit],
                    row_to_facts_snapshot_page_record,
                )?
                .collect::<Result<Vec<_>, _>>()
                .map_err(StoreError::from);
        }

        let mut statement = self.connection.prepare(
            "SELECT id, agent_id, body, collected_at
             FROM facts_snapshots
             WHERE agent_id = ?1
             ORDER BY collected_at DESC, id DESC
             LIMIT ?2",
        )?;
        statement
            .query_map(params![agent_id, limit], row_to_facts_snapshot_page_record)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn insert_metrics_snapshot(
        &self,
        agent_id: &str,
        body: &str,
        collected_at: SystemTime,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO metrics_snapshots (agent_id, body, collected_at)
             VALUES (?1, ?2, ?3)",
            params![agent_id, body, system_time_to_unix_secs(collected_at)],
        )?;
        Ok(())
    }

    pub fn latest_metrics_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<MetricsSnapshotRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT agent_id, body, collected_at
                 FROM metrics_snapshots
                 WHERE agent_id = ?1
                 ORDER BY collected_at DESC, id DESC
                 LIMIT 1",
                params![agent_id],
                |row| {
                    Ok(MetricsSnapshotRecord {
                        agent_id: row.get(0)?,
                        body: row.get(1)?,
                        collected_at: unix_secs_to_system_time(row.get(2)?),
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn save_agent_capability_snapshot(
        &self,
        agent_id: &str,
        snapshot: AgentCapabilitySnapshot,
    ) -> Result<(), StoreError> {
        let Some(profile) = snapshot.profile() else {
            return Err(StoreError::Domain(
                "capability snapshot profile is required".to_owned(),
            ));
        };
        let Some(reported_at) = snapshot.reported_at() else {
            return Err(StoreError::Domain(
                "capability snapshot reported_at is required".to_owned(),
            ));
        };
        let capabilities_json = serde_json::to_string(
            &profile
                .capabilities()
                .iter()
                .map(|capability| capability.as_str())
                .collect::<Vec<_>>(),
        )
        .map_err(|error| StoreError::Domain(error.to_string()))?;
        self.connection.execute(
            "INSERT INTO agent_capability_snapshots (
                agent_id, privilege_level, package_manager, service_manager,
                capabilities_json, reported_at, updated_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch())
             ON CONFLICT(agent_id) DO UPDATE SET
                privilege_level = excluded.privilege_level,
                package_manager = excluded.package_manager,
                service_manager = excluded.service_manager,
                capabilities_json = excluded.capabilities_json,
                reported_at = excluded.reported_at,
                updated_at = excluded.updated_at",
            params![
                agent_id,
                profile.privilege().as_str(),
                profile.package_manager().map(|manager| manager.as_str()),
                profile.service_manager().map(|manager| manager.as_str()),
                capabilities_json,
                system_time_to_unix_secs(reported_at),
            ],
        )?;
        Ok(())
    }

    pub fn latest_agent_capability_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<AgentCapabilitySnapshot>, StoreError> {
        let Some((
            privilege_level,
            package_manager,
            service_manager,
            capabilities_json,
            reported_at,
        )) = self
            .connection
            .query_row(
                "SELECT privilege_level, package_manager, service_manager, capabilities_json, reported_at
                 FROM agent_capability_snapshots
                 WHERE agent_id = ?1",
                params![agent_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(StoreError::from)?
        else {
            return Ok(None);
        };
        capability_snapshot_from_row(
            &privilege_level,
            package_manager.as_deref(),
            service_manager.as_deref(),
            &capabilities_json,
            reported_at,
        )
        .map(Some)
    }

    pub fn list_metrics_snapshots(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<MetricsSnapshotPageRecord>, StoreError> {
        let limit = limit.clamp(1, 501) as i64;
        if let Some(before) = before {
            let before_secs = system_time_to_unix_secs(before.occurred_at);
            let mut statement = self.connection.prepare(
                "SELECT id, agent_id, body, collected_at
                 FROM metrics_snapshots
                 WHERE agent_id = ?1
                   AND (collected_at < ?2 OR (collected_at = ?2 AND id < ?3))
                 ORDER BY collected_at DESC, id DESC
                 LIMIT ?4",
            )?;
            return statement
                .query_map(
                    params![agent_id, before_secs, before.row_id, limit],
                    row_to_metrics_snapshot_page_record,
                )?
                .collect::<Result<Vec<_>, _>>()
                .map_err(StoreError::from);
        }

        let mut statement = self.connection.prepare(
            "SELECT id, agent_id, body, collected_at
             FROM metrics_snapshots
             WHERE agent_id = ?1
             ORDER BY collected_at DESC, id DESC
             LIMIT ?2",
        )?;
        statement
            .query_map(
                params![agent_id, limit],
                row_to_metrics_snapshot_page_record,
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn insert_drift_report(
        &self,
        agent_id: &str,
        report: &DriftReport,
        checked_at: SystemTime,
    ) -> Result<(), StoreError> {
        self.insert_drift_report_with_provenance(
            agent_id,
            report,
            &DriftReportProvenance::uncorrelated(),
            checked_at,
        )
    }

    pub fn insert_drift_report_with_provenance(
        &self,
        agent_id: &str,
        report: &DriftReport,
        provenance: &DriftReportProvenance,
        checked_at: SystemTime,
    ) -> Result<(), StoreError> {
        self.connection
            .execute(
                "INSERT INTO drift_reports (
                agent_id, policy_name, status, severity, expected, actual, checked_at,
                job_id, task_id, policy_id, policy_version, purpose
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    agent_id,
                    report.policy_name.as_str(),
                    drift_status_to_str(&report.status),
                    drift_severity_to_str(report.severity),
                    report.expected.as_str(),
                    report.actual.as_str(),
                    system_time_to_unix_secs(checked_at),
                    provenance.job_id.as_ref().map(JobId::as_str),
                    provenance.task_id.as_ref().map(TaskId::as_str),
                    provenance.policy_id.as_deref(),
                    provenance.policy_version.map(i64::from),
                    provenance.purpose.map(DriftCheckPurpose::as_str),
                ],
            )
            .map_err(map_drift_report_constraint)?;
        Ok(())
    }

    pub fn latest_drift_report(
        &self,
        agent_id: &str,
    ) -> Result<Option<DriftReportRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT id, agent_id, policy_name, status, expected, actual, checked_at
                    , severity, acknowledged_at, acknowledged_by, resolved_at, resolution_job_id
                    , job_id, task_id, policy_id, policy_version, purpose
                 FROM drift_reports
                 WHERE agent_id = ?1
                 ORDER BY checked_at DESC, id DESC
                 LIMIT 1",
                params![agent_id],
                |row| {
                    Ok(DriftReportRecord {
                        id: DriftReportId::new(row.get(0)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        agent_id: row.get(1)?,
                        report: DriftReport {
                            policy_name: row.get(2)?,
                            status: parse_drift_status(&row.get::<_, String>(3)?),
                            severity: parse_drift_severity(&row.get::<_, String>(7)?),
                            acknowledgement: row_to_drift_acknowledgement(row, 8, 9, 10, 11)?,
                            expected: row.get(4)?,
                            actual: row.get(5)?,
                        },
                        provenance: row_to_drift_report_provenance(row, 12, 13, 14, 15, 16)?,
                        checked_at: unix_secs_to_system_time(row.get(6)?),
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn list_drift_reports(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<DriftReportPageRecord>, StoreError> {
        let limit = limit.clamp(1, 501) as i64;
        if let Some(before) = before {
            let before_secs = system_time_to_unix_secs(before.occurred_at);
            let mut statement = self.connection.prepare(
                "SELECT id, agent_id, policy_name, status, expected, actual, checked_at
                    , severity, acknowledged_at, acknowledged_by, resolved_at, resolution_job_id
                    , job_id, task_id, policy_id, policy_version, purpose
                 FROM drift_reports
                 WHERE agent_id = ?1
                   AND (checked_at < ?2 OR (checked_at = ?2 AND id < ?3))
                 ORDER BY checked_at DESC, id DESC
                 LIMIT ?4",
            )?;
            return statement
                .query_map(
                    params![agent_id, before_secs, before.row_id, limit],
                    row_to_drift_report_page_record,
                )?
                .collect::<Result<Vec<_>, _>>()
                .map_err(StoreError::from);
        }

        let mut statement = self.connection.prepare(
            "SELECT id, agent_id, policy_name, status, expected, actual, checked_at
                , severity, acknowledged_at, acknowledged_by, resolved_at, resolution_job_id
                , job_id, task_id, policy_id, policy_version, purpose
             FROM drift_reports
             WHERE agent_id = ?1
             ORDER BY checked_at DESC, id DESC
             LIMIT ?2",
        )?;
        statement
            .query_map(params![agent_id, limit], row_to_drift_report_page_record)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn save_policy_source(
        &self,
        policy_id: &str,
        name: &str,
        version: u32,
        source: &str,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO policies (id, name, version, source, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, unixepoch(), unixepoch())
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                version = excluded.version,
                source = excluded.source,
                updated_at = excluded.updated_at",
            params![policy_id, name, version as i64, source],
        )?;
        Ok(())
    }

    pub fn list_policies(&self) -> Result<Vec<PolicyRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, name, version, source, created_at, updated_at
             FROM policies
             ORDER BY id",
        )?;
        statement
            .query_map([], row_to_policy_record)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn find_policy(&self, policy_id: &str) -> Result<Option<PolicyRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT id, name, version, source, created_at, updated_at
                 FROM policies
                 WHERE id = ?1",
                params![policy_id],
                row_to_policy_record,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn assign_policy_to_agent(
        &self,
        policy_id: &str,
        agent_id: &str,
        assigned_at: SystemTime,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO policy_assignments (policy_id, agent_id, assigned_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(policy_id, agent_id) DO UPDATE SET
                assigned_at = excluded.assigned_at",
            params![policy_id, agent_id, system_time_to_unix_secs(assigned_at)],
        )?;
        Ok(())
    }

    pub fn policies_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<PolicyAssignmentRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT policy_id, agent_id, assigned_at
             FROM policy_assignments
             WHERE agent_id = ?1
             ORDER BY policy_id",
        )?;
        statement
            .query_map(params![agent_id], row_to_policy_assignment_record)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn assigned_policy_ids_for_agent(&self, agent_id: &str) -> Result<Vec<String>, StoreError> {
        Ok(self
            .policies_for_agent(agent_id)?
            .into_iter()
            .map(|record| record.policy_id)
            .collect())
    }

    pub fn upsert_policy_schedule(
        &self,
        policy_id: &str,
        agent_id: &str,
        interval: Duration,
        next_due_at: SystemTime,
    ) -> Result<(), StoreError> {
        if interval.is_zero() {
            return Err(StoreError::Domain(
                "policy schedule interval must be positive".to_owned(),
            ));
        }
        self.connection.execute(
            "INSERT INTO policy_drift_schedules (
                policy_id, agent_id, interval_seconds, next_due_at, last_checked_at
             ) VALUES (?1, ?2, ?3, ?4, NULL)
             ON CONFLICT(policy_id, agent_id) DO UPDATE SET
                interval_seconds = excluded.interval_seconds,
                next_due_at = excluded.next_due_at",
            params![
                policy_id,
                agent_id,
                interval.as_secs().max(1) as i64,
                system_time_to_unix_secs(next_due_at),
            ],
        )?;
        Ok(())
    }

    pub fn due_scheduled_drift_checks(
        &self,
        now: SystemTime,
        limit: usize,
    ) -> Result<Vec<ScheduledDriftRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT policy_id, agent_id, interval_seconds, next_due_at, last_checked_at
             FROM policy_drift_schedules
             WHERE next_due_at <= ?1
             ORDER BY next_due_at ASC
             LIMIT ?2",
        )?;
        statement
            .query_map(
                params![system_time_to_unix_secs(now), limit.clamp(1, 500) as i64],
                row_to_scheduled_drift_record,
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn record_scheduled_drift_check(
        &self,
        policy_id: &str,
        agent_id: &str,
        checked_at: SystemTime,
    ) -> Result<(), StoreError> {
        let changed = self.connection.execute(
            "UPDATE policy_drift_schedules
             SET last_checked_at = ?3,
                 next_due_at = ?3 + interval_seconds
             WHERE policy_id = ?1 AND agent_id = ?2",
            params![policy_id, agent_id, system_time_to_unix_secs(checked_at),],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    pub fn acknowledge_latest_drift_report(
        &self,
        agent_id: &str,
        policy_name: &str,
        actor: &str,
        acknowledged_at: SystemTime,
    ) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE drift_reports
             SET acknowledged_at = ?3, acknowledged_by = ?4
             WHERE id = (
                SELECT id FROM drift_reports
                WHERE agent_id = ?1 AND policy_name = ?2
                ORDER BY checked_at DESC, id DESC
                LIMIT 1
             )",
            params![
                agent_id,
                policy_name,
                system_time_to_unix_secs(acknowledged_at),
                actor,
            ],
        )?;
        Ok(changed > 0)
    }

    pub fn mark_latest_drift_resolved(
        &self,
        agent_id: &str,
        policy_name: &str,
        job_id: &str,
        resolved_at: SystemTime,
    ) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE drift_reports
             SET resolved_at = ?3, resolution_job_id = ?4
             WHERE id = (
                SELECT id FROM drift_reports
                WHERE agent_id = ?1 AND policy_name = ?2
                ORDER BY checked_at DESC, id DESC
                LIMIT 1
             )",
            params![
                agent_id,
                policy_name,
                system_time_to_unix_secs(resolved_at),
                job_id,
            ],
        )?;
        Ok(changed > 0)
    }

    pub fn insert_agent_log_chunk(
        &self,
        agent_id: &str,
        line: &str,
        collected_at: SystemTime,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO agent_log_chunks (agent_id, line, collected_at)
             VALUES (?1, ?2, ?3)",
            params![agent_id, line, system_time_to_unix_secs(collected_at)],
        )?;
        Ok(())
    }

    pub fn list_agent_log_chunks(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<AgentLogChunkRecord>, StoreError> {
        let limit = limit.clamp(1, 501) as i64;
        let mut statement = self.connection.prepare(
            "SELECT agent_id, line, collected_at
             FROM agent_log_chunks
             WHERE agent_id = ?1
             ORDER BY collected_at DESC, id DESC
             LIMIT ?2",
        )?;
        statement
            .query_map(params![agent_id, limit], |row| {
                Ok(AgentLogChunkRecord {
                    agent_id: row.get(0)?,
                    line: row.get(1)?,
                    collected_at: unix_secs_to_system_time(row.get(2)?),
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn list_agent_log_chunks_page(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AgentLogChunkPageRecord>, StoreError> {
        let limit = limit.clamp(1, 501) as i64;
        if let Some(before) = before {
            let before_secs = system_time_to_unix_secs(before.occurred_at);
            let mut statement = self.connection.prepare(
                "SELECT id, agent_id, line, collected_at
                 FROM agent_log_chunks
                 WHERE agent_id = ?1
                   AND (collected_at < ?2 OR (collected_at = ?2 AND id < ?3))
                 ORDER BY collected_at DESC, id DESC
                 LIMIT ?4",
            )?;
            return statement
                .query_map(
                    params![agent_id, before_secs, before.row_id, limit],
                    row_to_agent_log_chunk_page_record,
                )?
                .collect::<Result<Vec<_>, _>>()
                .map_err(StoreError::from);
        }

        let mut statement = self.connection.prepare(
            "SELECT id, agent_id, line, collected_at
             FROM agent_log_chunks
             WHERE agent_id = ?1
             ORDER BY collected_at DESC, id DESC
             LIMIT ?2",
        )?;
        statement
            .query_map(params![agent_id, limit], row_to_agent_log_chunk_page_record)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn cleanup_retention(
        &self,
        cutoff: SystemTime,
        dry_run: bool,
    ) -> Result<RetentionCleanupSummary, StoreError> {
        self.cleanup_retention_with_cutoffs(
            RetentionCutoffs {
                job_output: cutoff,
                facts: cutoff,
                metrics: cutoff,
                agent_logs: cutoff,
            },
            dry_run,
        )
    }

    pub fn cleanup_retention_with_cutoffs(
        &self,
        cutoffs: RetentionCutoffs,
        dry_run: bool,
    ) -> Result<RetentionCleanupSummary, StoreError> {
        let job_output_cutoff = system_time_to_unix_secs(cutoffs.job_output);
        let facts_cutoff = system_time_to_unix_secs(cutoffs.facts);
        let metrics_cutoff = system_time_to_unix_secs(cutoffs.metrics);
        let agent_logs_cutoff = system_time_to_unix_secs(cutoffs.agent_logs);
        let summary = RetentionCleanupSummary {
            job_output_chunks: self.count_before(
                "job_output_chunks",
                "created_at",
                job_output_cutoff,
            )?,
            facts_snapshots: self.count_before("facts_snapshots", "collected_at", facts_cutoff)?,
            metrics_snapshots: self.count_before(
                "metrics_snapshots",
                "collected_at",
                metrics_cutoff,
            )?,
            agent_log_chunks: self.count_before(
                "agent_log_chunks",
                "collected_at",
                agent_logs_cutoff,
            )?,
        };
        if dry_run {
            return Ok(summary);
        }
        self.delete_before("job_output_chunks", "created_at", job_output_cutoff)?;
        self.delete_before("facts_snapshots", "collected_at", facts_cutoff)?;
        self.delete_before("metrics_snapshots", "collected_at", metrics_cutoff)?;
        self.delete_before("agent_log_chunks", "collected_at", agent_logs_cutoff)?;
        Ok(summary)
    }

    fn count_before(&self, table: &str, column: &str, cutoff: i64) -> Result<usize, StoreError> {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE {column} < ?1");
        let count: i64 = self
            .connection
            .query_row(&sql, params![cutoff], |row| row.get(0))?;
        Ok(count.max(0) as usize)
    }

    fn delete_before(&self, table: &str, column: &str, cutoff: i64) -> Result<usize, StoreError> {
        let sql = format!("DELETE FROM {table} WHERE {column} < ?1");
        self.connection
            .execute(&sql, params![cutoff])
            .map_err(StoreError::from)
    }

    pub fn write_audit_event(&self, event: AuditEvent) -> Result<(), StoreError> {
        self.insert_audit(&event)
    }

    pub fn audit_count_by_category(&self, category: AuditCategory) -> Result<usize, StoreError> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM audit_events WHERE category = ?1",
            params![category.as_str()],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as usize)
    }

    pub fn list_audit_events_by_category(
        &self,
        category: AuditCategory,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, StoreError> {
        self.query_audit(Some(category), limit)
    }

    pub fn list_audit_events(&self, limit: usize) -> Result<Vec<AuditEvent>, StoreError> {
        self.query_audit(None, limit)
    }

    pub fn export_audit_events(
        &self,
        category: Option<AuditCategory>,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AuditEventPageRecord>, StoreError> {
        self.query_audit_page(category, limit, before)
    }

    pub fn find_agent_fingerprint(&self, agent_id: &str) -> Result<Option<String>, StoreError> {
        self.connection
            .query_row(
                "SELECT fingerprint FROM agents WHERE id = ?1",
                params![agent_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn find_agent_identity(
        &self,
        agent_id: &str,
    ) -> Result<Option<(String, String)>, StoreError> {
        self.connection
            .query_row(
                "SELECT public_key, fingerprint FROM agents WHERE id = ?1 AND status != 'disabled'",
                params![agent_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn mark_agent_online(&self, agent_id: &str, at: SystemTime) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE agents
             SET status = 'online', last_seen_at = ?2, updated_at = unixepoch()
             WHERE id = ?1 AND status != 'disabled'",
            params![agent_id, system_time_to_unix_secs(at)],
        )?;
        Ok(changed > 0)
    }

    pub fn mark_agent_degraded(&self, agent_id: &str, at: SystemTime) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE agents
             SET status = 'degraded', last_seen_at = ?2, updated_at = unixepoch()
             WHERE id = ?1 AND status != 'disabled'",
            params![agent_id, system_time_to_unix_secs(at)],
        )?;
        Ok(changed > 0)
    }

    pub fn mark_stale_agents_offline(
        &self,
        cutoff: SystemTime,
        now: SystemTime,
    ) -> Result<usize, StoreError> {
        let changed = self.connection.execute(
            "UPDATE agents
             SET status = 'offline', updated_at = ?2
             WHERE status IN ('online', 'busy', 'degraded')
               AND last_seen_at IS NOT NULL
               AND last_seen_at < ?1",
            params![
                system_time_to_unix_secs(cutoff),
                system_time_to_unix_secs(now),
            ],
        )?;
        Ok(changed)
    }

    pub fn save_job_record(&self, job: &Job) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO jobs (id, status, risk, approval_requirement, timeout_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                job.id().as_str(),
                job_status_to_str(job.status()),
                task_risk_to_str(job.risk()),
                approval_requirement_to_str(job.approval_requirement()),
                job.timeout().as_millis() as i64,
            ],
        )?;
        Ok(())
    }

    pub fn save_command_job_record(&self, job: &Job, task: &CommandTask) -> Result<(), StoreError> {
        let args = serde_json::to_string(task.args())
            .map_err(|error| StoreError::Domain(error.to_string()))?;
        self.connection.execute(
            "INSERT INTO jobs (
                id, status, risk, approval_requirement, timeout_ms,
                command_program, command_args_json, command_max_output_bytes
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                job.id().as_str(),
                job_status_to_str(job.status()),
                task_risk_to_str(job.risk()),
                approval_requirement_to_str(job.approval_requirement()),
                job.timeout().as_millis() as i64,
                task.program(),
                args,
                task.max_output_bytes() as i64,
            ],
        )?;
        Ok(())
    }

    pub fn save_command_job_with_assignments_record(
        &self,
        job: &Job,
        task: &CommandTask,
        assignments: &[TaskEnvelope],
    ) -> Result<(), StoreError> {
        let args = serde_json::to_string(task.args())
            .map_err(|error| StoreError::Domain(error.to_string()))?;
        self.run_sqlite_transaction(|| {
            self.connection.execute(
                "INSERT INTO jobs (
                    id, status, risk, approval_requirement, timeout_ms,
                    command_program, command_args_json, command_max_output_bytes
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    job.id().as_str(),
                    job_status_to_str(job.status()),
                    task_risk_to_str(job.risk()),
                    approval_requirement_to_str(job.approval_requirement()),
                    job.timeout().as_millis() as i64,
                    task.program(),
                    args,
                    task.max_output_bytes() as i64,
                ],
            )?;
            for assignment in assignments {
                sqlite_insert_task_assignment_in_connection(&self.connection, assignment)?;
            }
            Ok(())
        })
    }

    pub fn save_drift_check_job_record(
        &self,
        job: &Job,
        task: &DriftCheckTask,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO jobs (
                id, status, risk, approval_requirement, timeout_ms,
                drift_policy_document
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                job.id().as_str(),
                job_status_to_str(job.status()),
                task_risk_to_str(job.risk()),
                approval_requirement_to_str(job.approval_requirement()),
                job.timeout().as_millis() as i64,
                task.policy_document(),
            ],
        )?;
        Ok(())
    }

    pub fn save_drift_check_job_with_assignments_record(
        &self,
        job: &Job,
        task: &DriftCheckTask,
        assignments: &[TaskEnvelope],
    ) -> Result<(), StoreError> {
        self.save_drift_check_job_with_assignments_and_provenance_record(
            job,
            task,
            assignments,
            None,
        )
    }

    pub fn save_drift_check_job_with_assignments_and_provenance_record(
        &self,
        job: &Job,
        task: &DriftCheckTask,
        assignments: &[TaskEnvelope],
        provenance: Option<&DriftJobProvenance>,
    ) -> Result<(), StoreError> {
        self.run_sqlite_transaction(|| {
            self.connection.execute(
                "INSERT INTO jobs (
                    id, status, risk, approval_requirement, timeout_ms,
                    drift_policy_document, drift_policy_id, drift_policy_version, drift_purpose
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    job.id().as_str(),
                    job_status_to_str(job.status()),
                    task_risk_to_str(job.risk()),
                    approval_requirement_to_str(job.approval_requirement()),
                    job.timeout().as_millis() as i64,
                    task.policy_document(),
                    provenance.map(|value| value.policy_id.as_str()),
                    provenance.map(|value| i64::from(value.policy_version)),
                    provenance.map(|value| value.purpose.as_str()),
                ],
            )?;
            for assignment in assignments {
                sqlite_insert_task_assignment_in_connection(&self.connection, assignment)?;
            }
            Ok(())
        })
    }

    /// Atomically persists the one verification drift job correlated to a remediation request.
    pub fn save_remediation_verification_job_record(
        &self,
        input: &AppRemediationVerificationJobPersistenceInput,
    ) -> Result<AppRemediationVerificationJobSave, StoreError> {
        self.run_sqlite_transaction(|| {
            if let Some(job_id) =
                self.find_remediation_verification_job_id(&input.remediation_id)?
            {
                return Ok(AppRemediationVerificationJobSave {
                    job_id,
                    created: false,
                });
            }
            self.connection.execute(
                "INSERT INTO jobs (
                    id, status, risk, approval_requirement, timeout_ms,
                    drift_policy_document, drift_policy_id, drift_policy_version, drift_purpose
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    input.job.id().as_str(),
                    job_status_to_str(input.job.status()),
                    task_risk_to_str(input.job.risk()),
                    approval_requirement_to_str(input.job.approval_requirement()),
                    input.job.timeout().as_millis() as i64,
                    input.task.policy_document(),
                    input.provenance.policy_id.as_str(),
                    i64::from(input.provenance.policy_version),
                    input.provenance.purpose.as_str(),
                ],
            )?;
            sqlite_insert_task_assignment_in_connection(&self.connection, &input.assignment)?;
            let inserted = self.connection.execute(
                "INSERT INTO remediation_verification_jobs (remediation_id, job_id, created_at)
                 VALUES (?1, ?2, ?3)",
                params![
                    input.remediation_id.as_str(),
                    input.job.id().as_str(),
                    system_time_to_unix_secs(input.audit.occurred_at),
                ],
            )?;
            if inserted != 1 {
                return Err(StoreError::Domain(
                    "remediation verification correlation was not created".to_owned(),
                ));
            }
            self.insert_audit(&input.audit)?;
            Ok(AppRemediationVerificationJobSave {
                job_id: input.job.id().as_str().to_owned(),
                created: true,
            })
        })
    }

    /// Atomically records a resolution derived from one persisted verification evidence report.
    pub fn resolve_remediation_verification_evidence_record(
        &self,
        remediation: &AppRemediationRequestRecord,
        origin_drift_report_id: &DriftReportId,
        evidence_report_id: &DriftReportId,
        verification_job_id: &str,
        verification_task_id: &str,
        audit: &AuditEvent,
    ) -> Result<AppRemediationRequestRecord, StoreError> {
        self.run_sqlite_transaction(|| {
            let correlation = self.find_remediation_verification_job_id(&remediation.id)?;
            if correlation.as_deref() != Some(verification_job_id) {
                return Err(StoreError::NotFound);
            }
            let evidence_exists = self.connection.query_row(
                "SELECT 1 FROM drift_reports
                 WHERE id = ?1 AND agent_id = ?2 AND job_id = ?3 AND task_id = ?4
                   AND policy_id = ?5 AND policy_version = ?6
                   AND purpose = 'remediation_verification' AND status = 'compliant'",
                params![
                    evidence_report_id.as_i64(),
                    remediation.agent_id,
                    verification_job_id,
                    verification_task_id,
                    remediation.policy_id,
                    remediation.policy_version.map(i64::from),
                ],
                |row| row.get::<_, i64>(0),
            ).optional()?;
            if evidence_exists.is_none() {
                return Err(StoreError::NotFound);
            }
            let remediation_changed = self.connection.execute(
                "UPDATE remediation_requests SET status = ?2, job_id = ?3, updated_at = ?4 WHERE id = ?1",
                params![
                    remediation.id,
                    remediation.status,
                    remediation.job_id,
                    system_time_to_unix_secs(remediation.updated_at),
                ],
            )?;
            if remediation_changed != 1 {
                return Err(StoreError::NotFound);
            }
            let origin_changed = self.connection.execute(
                "UPDATE drift_reports SET resolved_at = ?2, resolution_job_id = ?3 WHERE id = ?1",
                params![
                    origin_drift_report_id.as_i64(),
                    system_time_to_unix_secs(audit.occurred_at),
                    verification_job_id,
                ],
            )?;
            if origin_changed != 1 {
                return Err(StoreError::NotFound);
            }
            self.insert_audit(audit)?;
            Ok(remediation.clone())
        })
    }

    pub fn save_runbook_job_record(
        &self,
        job: &Job,
        task: &RunbookExecutionTask,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO jobs (
                id, status, risk, approval_requirement, timeout_ms,
                runbook_document
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                job.id().as_str(),
                job_status_to_str(job.status()),
                task_risk_to_str(job.risk()),
                approval_requirement_to_str(job.approval_requirement()),
                job.timeout().as_millis() as i64,
                task.runbook_document(),
            ],
        )?;
        Ok(())
    }

    pub fn save_runbook_job_with_assignments_record(
        &self,
        job: &Job,
        task: &RunbookExecutionTask,
        assignments: &[TaskEnvelope],
    ) -> Result<(), StoreError> {
        self.run_sqlite_transaction(|| {
            self.connection.execute(
                "INSERT INTO jobs (
                    id, status, risk, approval_requirement, timeout_ms,
                    runbook_document
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    job.id().as_str(),
                    job_status_to_str(job.status()),
                    task_risk_to_str(job.risk()),
                    approval_requirement_to_str(job.approval_requirement()),
                    job.timeout().as_millis() as i64,
                    task.runbook_document(),
                ],
            )?;
            for assignment in assignments {
                sqlite_insert_task_assignment_in_connection(&self.connection, assignment)?;
            }
            Ok(())
        })
    }

    fn run_sqlite_transaction<T, F>(&self, operation: F) -> Result<T, StoreError>
    where
        F: FnOnce() -> Result<T, StoreError>,
    {
        self.connection.execute_batch("BEGIN IMMEDIATE")?;
        match operation() {
            Ok(value) => {
                self.connection.execute_batch("COMMIT")?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub fn save_task_assignment_record(&self, envelope: &TaskEnvelope) -> Result<(), StoreError> {
        sqlite_insert_task_assignment_in_connection(&self.connection, envelope)
    }

    pub fn update_job_selector_snapshot(
        &self,
        job_id: &str,
        selector_kind: &str,
        selector_source: &str,
    ) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE jobs
             SET selector_kind = ?2, selector_source = ?3
             WHERE id = ?1",
            params![job_id, selector_kind, selector_source],
        )?;
        Ok(changed > 0)
    }

    pub fn update_job_strategy(
        &self,
        job_id: &str,
        concurrency: u32,
        max_failures: Option<u32>,
    ) -> Result<bool, StoreError> {
        let concurrency = concurrency.max(1);
        let changed = self.connection.execute(
            "UPDATE jobs
             SET strategy_concurrency = ?2, strategy_max_failures = ?3
             WHERE id = ?1",
            params![job_id, concurrency as i64, max_failures.map(i64::from)],
        )?;
        Ok(changed > 0)
    }

    pub fn job_strategy(&self, job_id: &str) -> Result<Option<JobStrategyRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT strategy_concurrency, strategy_max_failures
                 FROM jobs
                 WHERE id = ?1",
                params![job_id],
                |row| {
                    let concurrency = row.get::<_, i64>(0)?.max(1) as u32;
                    let max_failures = row
                        .get::<_, Option<i64>>(1)?
                        .and_then(|value| u32::try_from(value).ok())
                        .filter(|value| *value > 0);
                    Ok(JobStrategyRecord {
                        concurrency,
                        max_failures,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn job_dispatch_gate(
        &self,
        job_id: &str,
    ) -> Result<Option<JobDispatchGateRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT
                    j.strategy_concurrency,
                    j.strategy_max_failures,
                    COALESCE(SUM(CASE WHEN ta.status IN ('dispatched', 'accepted', 'started') THEN 1 ELSE 0 END), 0) AS active_count,
                    COALESCE(SUM(CASE WHEN ta.status IN ('failed', 'rejected', 'expired') THEN 1 ELSE 0 END), 0) AS failure_count
                 FROM jobs j
                 LEFT JOIN task_assignments ta ON ta.job_id = j.id
                 WHERE j.id = ?1
                 GROUP BY j.id",
                params![job_id],
                |row| {
                    let concurrency = row.get::<_, i64>(0)?.max(1) as u32;
                    let max_failures = row
                        .get::<_, Option<i64>>(1)?
                        .and_then(|value| u32::try_from(value).ok())
                        .filter(|value| *value > 0);
                    Ok(JobDispatchGateRecord {
                        concurrency,
                        max_failures,
                        active_count: row.get::<_, i64>(2)?.max(0) as usize,
                        failure_count: row.get::<_, i64>(3)?.max(0) as usize,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn update_task_assignment_status(
        &self,
        task_id: &str,
        status: AssignmentStatus,
        occurred_at: SystemTime,
        last_error: Option<&str>,
    ) -> Result<bool, StoreError> {
        let status_value = assignment_status_to_str(status);
        let occurred_at = system_time_to_unix_secs(occurred_at);
        let changed = match status {
            AssignmentStatus::Dispatched => self.connection.execute(
                "UPDATE task_assignments
                 SET status = ?2, dispatched_at = ?3
                 WHERE id = ?1",
                params![task_id, status_value, occurred_at],
            )?,
            AssignmentStatus::Accepted => self.connection.execute(
                "UPDATE task_assignments
                 SET status = ?2, accepted_at = ?3
                 WHERE id = ?1",
                params![task_id, status_value, occurred_at],
            )?,
            AssignmentStatus::Started => self.connection.execute(
                "UPDATE task_assignments
                 SET status = ?2, started_at = ?3
                 WHERE id = ?1",
                params![task_id, status_value, occurred_at],
            )?,
            AssignmentStatus::Succeeded
            | AssignmentStatus::Failed
            | AssignmentStatus::Rejected
            | AssignmentStatus::Canceled
            | AssignmentStatus::Expired => self.connection.execute(
                "UPDATE task_assignments
                 SET status = ?2, completed_at = ?3, last_error = COALESCE(?4, last_error)
                 WHERE id = ?1",
                params![task_id, status_value, occurred_at, last_error],
            )?,
            AssignmentStatus::Queued => self.connection.execute(
                "UPDATE task_assignments
                 SET status = ?2, last_error = COALESCE(?3, last_error)
                 WHERE id = ?1",
                params![task_id, status_value, last_error],
            )?,
        };
        Ok(changed > 0)
    }

    pub fn update_active_task_assignment_status(
        &self,
        task_id: &str,
        status: AssignmentStatus,
        occurred_at: SystemTime,
        last_error: Option<&str>,
    ) -> Result<bool, StoreError> {
        let Some(current_status) = self.find_task_assignment_status(task_id)? else {
            return Ok(false);
        };
        if assignment_status_value_is_terminal(&current_status) {
            return Ok(false);
        }
        self.update_task_assignment_status(task_id, status, occurred_at, last_error)
    }

    pub fn persist_remediation_execution_transition_record(
        &self,
        input: &AppRemediationExecutionPersistenceInput,
    ) -> Result<bool, StoreError> {
        self.run_sqlite_transaction(|| {
            let Some(current_status) = self.find_task_assignment_status(&input.task_id)? else {
                return Ok(false);
            };
            if assignment_status_value_is_terminal(&current_status) {
                return Ok(false);
            }
            let occurred_at = system_time_to_unix_secs(input.occurred_at);
            let changed = match input.assignment_status.as_str() {
                "started" => self.connection.execute(
                    "UPDATE task_assignments SET status = ?2, started_at = ?3 WHERE id = ?1",
                    params![input.task_id, input.assignment_status, occurred_at],
                )?,
                "succeeded" | "failed" | "canceled" | "expired" => self.connection.execute(
                    "UPDATE task_assignments
                     SET status = ?2, completed_at = ?3, last_error = COALESCE(?4, last_error)
                     WHERE id = ?1",
                    params![
                        input.task_id,
                        input.assignment_status,
                        occurred_at,
                        input.assignment_last_error
                    ],
                )?,
                _ => {
                    return Err(StoreError::Domain(
                        "unsupported remediation assignment transition".to_owned(),
                    ));
                }
            };
            if changed == 0 {
                return Ok(false);
            }
            if let Some(remediation) = &input.remediation {
                self.update_remediation_request_status_record(
                    &remediation.id,
                    &remediation.status,
                    remediation.job_id.as_deref(),
                    input.occurred_at,
                )?;
            }
            if let Some(audit) = &input.remediation_audit {
                self.insert_audit(audit)?;
            }
            Ok(true)
        })
    }

    pub fn claim_task_assignment_for_dispatch(
        &self,
        task_id: &str,
        occurred_at: SystemTime,
    ) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE task_assignments
             SET status = 'dispatched', dispatched_at = ?2, last_error = ''
             WHERE id = ?1 AND status = 'queued'",
            params![task_id, system_time_to_unix_secs(occurred_at)],
        )?;
        Ok(changed > 0)
    }

    pub fn release_task_assignment_dispatch_claim(
        &self,
        task_id: &str,
        reason: &str,
    ) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE task_assignments
             SET status = 'queued', dispatched_at = NULL, last_error = ?2
             WHERE id = ?1 AND status = 'dispatched'",
            params![task_id, reason],
        )?;
        Ok(changed > 0)
    }

    pub fn find_task_assignment_status(&self, task_id: &str) -> Result<Option<String>, StoreError> {
        self.connection
            .query_row(
                "SELECT status FROM task_assignments WHERE id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn find_task_assignment_job_id(&self, task_id: &str) -> Result<Option<String>, StoreError> {
        self.connection
            .query_row(
                "SELECT job_id FROM task_assignments WHERE id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn find_drift_assignment_provenance(
        &self,
        task_id: &str,
    ) -> Result<Option<DriftAssignmentProvenanceRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT ta.job_id, ta.id, ta.agent_id, j.drift_policy_id,
                        j.drift_policy_version, j.drift_purpose
                 FROM task_assignments ta
                 JOIN jobs j ON j.id = ta.job_id
                 WHERE ta.id = ?1
                   AND j.drift_policy_document IS NOT NULL
                   AND j.drift_policy_id IS NOT NULL
                   AND j.drift_policy_version IS NOT NULL
                   AND j.drift_purpose IS NOT NULL",
                params![task_id],
                |row| {
                    let purpose = row.get::<_, String>(5)?;
                    let Some(purpose) = DriftCheckPurpose::parse(&purpose) else {
                        return Err(rusqlite::Error::InvalidQuery);
                    };
                    Ok(DriftAssignmentProvenanceRecord {
                        job_id: row.get(0)?,
                        task_id: row.get(1)?,
                        agent_id: row.get(2)?,
                        policy_id: row.get(3)?,
                        policy_version: row.get::<_, i64>(4)?.max(0) as u32,
                        purpose,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn find_task_assignment_state_for_job(
        &self,
        job_id: &str,
    ) -> Result<Option<TaskAssignmentStateRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT job_id, id, agent_id, status, completed_at
                 FROM task_assignments
                 WHERE job_id = ?1
                 ORDER BY created_at, id
                 LIMIT 1",
                params![job_id],
                |row| {
                    Ok(TaskAssignmentStateRecord {
                        job_id: row.get(0)?,
                        task_id: row.get(1)?,
                        agent_id: row.get(2)?,
                        status: row.get(3)?,
                        completed_at: row.get::<_, Option<i64>>(4)?.map(unix_secs_to_system_time),
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn list_task_assignment_summaries_for_job(
        &self,
        job_id: &str,
    ) -> Result<Vec<TaskAssignmentSummaryRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT job_id, id, agent_id, status, last_error
             FROM task_assignments
             WHERE job_id = ?1
             ORDER BY created_at, id",
        )?;
        statement
            .query_map(params![job_id], |row| {
                Ok(TaskAssignmentSummaryRecord {
                    job_id: row.get(0)?,
                    task_id: row.get(1)?,
                    agent_id: row.get(2)?,
                    status: row.get(3)?,
                    last_error: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn recompute_job_status_from_assignments(
        &self,
        job_id: &str,
    ) -> Result<Option<JobStatus>, StoreError> {
        let strategy = self.job_strategy(job_id)?;
        let statuses = self
            .list_task_assignment_summaries_for_job(job_id)?
            .into_iter()
            .map(|assignment| {
                AssignmentStatus::parse(&assignment.status).ok_or_else(|| {
                    StoreError::Domain(format!(
                        "invalid task assignment status: {}",
                        assignment.status
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if statuses.is_empty() && strategy.is_none() {
            return Ok(None);
        }
        let status = aggregate_job_status(
            &statuses,
            strategy.and_then(|strategy| strategy.max_failures),
        );
        self.update_job_status(job_id, status)?;
        Ok(Some(status))
    }

    pub fn cancel_queued_assignments_after_max_failures(
        &self,
        job_id: &str,
        occurred_at: SystemTime,
        reason: &str,
    ) -> Result<usize, StoreError> {
        let Some(gate) = self.job_dispatch_gate(job_id)? else {
            return Ok(0);
        };
        if !matches!(gate.max_failures, Some(limit) if limit > 0 && gate.failure_count >= limit as usize)
        {
            return Ok(0);
        }
        let changed = self.connection.execute(
            "UPDATE task_assignments
             SET status = 'canceled', completed_at = ?2, last_error = ?3
             WHERE job_id = ?1
               AND status = 'queued'",
            params![job_id, system_time_to_unix_secs(occurred_at), reason],
        )?;
        Ok(changed)
    }

    pub fn list_pending_command_assignments_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<PendingCommandAssignment>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT
                ta.job_id, ta.id, ta.agent_id, ta.nonce, ta.payload_hash,
                ta.signature, ta.issued_at, ta.expires_at,
                j.command_program, j.command_args_json, j.timeout_ms
             FROM task_assignments ta
             JOIN jobs j ON j.id = ta.job_id
             WHERE ta.agent_id = ?1
               AND ta.status = 'queued'
               AND j.status IN ('queued', 'running')
               AND j.command_program IS NOT NULL
             ORDER BY ta.created_at, ta.id",
        )?;
        let rows = statement
            .query_map(params![agent_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, i64>(10)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(
                |(
                    job_id,
                    task_id,
                    target_agent_id,
                    nonce,
                    payload_hash,
                    signature,
                    issued_at,
                    expires_at,
                    command_program,
                    command_args_json,
                    timeout_ms,
                )| {
                    let command_args = parse_command_args(&command_args_json)?;
                    Ok(PendingCommandAssignment {
                        envelope: TaskEnvelope {
                            job_id: JobId::new(job_id)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            task_id: TaskId::new(task_id)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            target_agent_id: AgentId::new(target_agent_id)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            issued_at: unix_secs_to_system_time(issued_at),
                            expires_at: TaskExpiry::new(unix_secs_to_system_time(expires_at)),
                            nonce: TaskNonce::new(nonce)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            payload_hash,
                            signature: Some(
                                TaskSignature::new(signature)
                                    .map_err(|error| StoreError::Domain(error.to_string()))?,
                            ),
                        },
                        command: CommandTask::new(
                            command_program,
                            command_args,
                            Duration::from_millis(timeout_ms as u64),
                        )
                        .map_err(|error| StoreError::Domain(error.to_string()))?,
                    })
                },
            )
            .collect()
    }

    pub fn list_pending_drift_check_assignments_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<PendingDriftCheckAssignment>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT
                ta.job_id, ta.id, ta.agent_id, ta.nonce, ta.payload_hash,
                ta.signature, ta.issued_at, ta.expires_at,
                j.drift_policy_document, j.timeout_ms
             FROM task_assignments ta
             JOIN jobs j ON j.id = ta.job_id
             WHERE ta.agent_id = ?1
               AND ta.status = 'queued'
               AND j.status IN ('queued', 'running')
               AND j.drift_policy_document IS NOT NULL
             ORDER BY ta.created_at, ta.id",
        )?;
        let rows = statement
            .query_map(params![agent_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(
                |(
                    job_id,
                    task_id,
                    target_agent_id,
                    nonce,
                    payload_hash,
                    signature,
                    issued_at,
                    expires_at,
                    policy_document,
                    timeout_ms,
                )| {
                    Ok(PendingDriftCheckAssignment {
                        envelope: TaskEnvelope {
                            job_id: JobId::new(job_id)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            task_id: TaskId::new(task_id)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            target_agent_id: AgentId::new(target_agent_id)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            issued_at: unix_secs_to_system_time(issued_at),
                            expires_at: TaskExpiry::new(unix_secs_to_system_time(expires_at)),
                            nonce: TaskNonce::new(nonce)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            payload_hash,
                            signature: Some(
                                TaskSignature::new(signature)
                                    .map_err(|error| StoreError::Domain(error.to_string()))?,
                            ),
                        },
                        drift_check: DriftCheckTask::new(
                            policy_document,
                            Duration::from_millis(timeout_ms as u64),
                        )
                        .map_err(|error| StoreError::Domain(error.to_string()))?,
                    })
                },
            )
            .collect()
    }

    pub fn list_pending_runbook_assignments_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<PendingRunbookAssignment>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT
                ta.job_id, ta.id, ta.agent_id, ta.nonce, ta.payload_hash,
                ta.signature, ta.issued_at, ta.expires_at,
                j.runbook_document, j.timeout_ms
             FROM task_assignments ta
             JOIN jobs j ON j.id = ta.job_id
             WHERE ta.agent_id = ?1
               AND ta.status = 'queued'
               AND j.status IN ('queued', 'running')
               AND j.runbook_document IS NOT NULL
             ORDER BY ta.created_at, ta.id",
        )?;
        let rows = statement
            .query_map(params![agent_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(
                |(
                    job_id,
                    task_id,
                    target_agent_id,
                    nonce,
                    payload_hash,
                    signature,
                    issued_at,
                    expires_at,
                    runbook_document,
                    timeout_ms,
                )| {
                    Ok(PendingRunbookAssignment {
                        envelope: TaskEnvelope {
                            job_id: JobId::new(job_id)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            task_id: TaskId::new(task_id)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            target_agent_id: AgentId::new(target_agent_id)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            issued_at: unix_secs_to_system_time(issued_at),
                            expires_at: TaskExpiry::new(unix_secs_to_system_time(expires_at)),
                            nonce: TaskNonce::new(nonce)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            payload_hash,
                            signature: Some(
                                TaskSignature::new(signature)
                                    .map_err(|error| StoreError::Domain(error.to_string()))?,
                            ),
                        },
                        runbook: RunbookExecutionTask::new(
                            runbook_document,
                            Duration::from_millis(timeout_ms as u64),
                        )
                        .map_err(|error| StoreError::Domain(error.to_string()))?,
                    })
                },
            )
            .collect()
    }

    pub fn list_pending_dispatch_assignments(
        &self,
        agent_id: Option<&AgentId>,
        job_id: Option<&JobId>,
        limit: usize,
    ) -> Result<Vec<AppPendingTaskAssignment>, StoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let agent_id = agent_id.map(AgentId::as_str);
        let job_id = job_id.map(JobId::as_str);
        let limit = limit.min(100) as i64;
        let mut statement = self.connection.prepare(
            "SELECT
                ta.job_id, ta.id, ta.agent_id, ta.nonce, ta.payload_hash,
                ta.signature, ta.issued_at, ta.expires_at,
                j.command_program, j.command_args_json, j.drift_policy_document,
                j.runbook_document, j.timeout_ms
             FROM task_assignments ta
             JOIN jobs j ON j.id = ta.job_id
             WHERE ta.status = 'queued'
               AND j.status IN ('queued', 'running')
               AND (?1 IS NULL OR ta.agent_id = ?1)
               AND (?2 IS NULL OR ta.job_id = ?2)
               AND (
                    j.command_program IS NOT NULL
                 OR j.drift_policy_document IS NOT NULL
                 OR j.runbook_document IS NOT NULL
               )
             ORDER BY ta.created_at, ta.id
             LIMIT ?3",
        )?;
        let rows = statement
            .query_map(params![agent_id, job_id, limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, i64>(12)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        rows.into_iter()
            .map(
                |(
                    job_id,
                    task_id,
                    target_agent_id,
                    nonce,
                    payload_hash,
                    signature,
                    issued_at,
                    expires_at,
                    command_program,
                    command_args_json,
                    drift_policy_document,
                    runbook_document,
                    timeout_ms,
                )| {
                    let envelope = TaskEnvelope {
                        job_id: JobId::new(job_id)
                            .map_err(|error| StoreError::Domain(error.to_string()))?,
                        task_id: TaskId::new(task_id)
                            .map_err(|error| StoreError::Domain(error.to_string()))?,
                        target_agent_id: AgentId::new(target_agent_id)
                            .map_err(|error| StoreError::Domain(error.to_string()))?,
                        issued_at: unix_secs_to_system_time(issued_at),
                        expires_at: TaskExpiry::new(unix_secs_to_system_time(expires_at)),
                        nonce: TaskNonce::new(nonce)
                            .map_err(|error| StoreError::Domain(error.to_string()))?,
                        payload_hash,
                        signature: Some(
                            TaskSignature::new(signature)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                        ),
                    };
                    let timeout = Duration::from_millis(timeout_ms as u64);
                    let task = if let Some(program) = command_program {
                        TaskKind::Command(
                            CommandTask::new(
                                program,
                                parse_command_args(&command_args_json)?,
                                timeout,
                            )
                            .map_err(|error| StoreError::Domain(error.to_string()))?,
                        )
                    } else if let Some(policy_document) = drift_policy_document {
                        TaskKind::DriftCheck(
                            DriftCheckTask::new(policy_document, timeout)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                        )
                    } else if let Some(runbook_document) = runbook_document {
                        TaskKind::RunbookExecution(
                            RunbookExecutionTask::new(runbook_document, timeout)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                        )
                    } else {
                        return Err(StoreError::Domain(
                            "pending assignment has no task payload".to_owned(),
                        ));
                    };

                    Ok(AppPendingTaskAssignment { envelope, task })
                },
            )
            .collect()
    }

    pub fn update_job_status(&self, job_id: &str, status: JobStatus) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE jobs SET status = ?2 WHERE id = ?1",
            params![job_id, job_status_to_str(status)],
        )?;
        Ok(changed > 0)
    }

    pub fn find_job_status_value(&self, job_id: &str) -> Result<Option<String>, StoreError> {
        self.connection
            .query_row(
                "SELECT status FROM jobs WHERE id = ?1",
                params![job_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn insert_approval_request(
        &self,
        request: AppApprovalRequestRecord,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO approval_requests (
                id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                request.id,
                request.job_id,
                request.requester,
                request.approver,
                request.reason,
                request.status,
                system_time_to_unix_secs(request.expires_at),
                system_time_to_unix_secs(request.created_at),
                request.decided_at.map(system_time_to_unix_secs),
            ],
        )?;
        Ok(())
    }

    pub fn find_approval_request(
        &self,
        approval_id: &str,
    ) -> Result<Option<AppApprovalRequestRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
                 FROM approval_requests
                 WHERE id = ?1",
                params![approval_id],
                row_to_app_approval_request_record,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn find_pending_approval_for_job(
        &self,
        job_id: &str,
    ) -> Result<Option<AppApprovalRequestRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
                 FROM approval_requests
                 WHERE job_id = ?1 AND status = 'pending'
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
                params![job_id],
                row_to_app_approval_request_record,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn list_approval_requests(
        &self,
        status: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AppApprovalRequestRecord>, StoreError> {
        let limit = limit.clamp(1, 500) as i64;
        let mut statement = self.connection.prepare(
            "SELECT id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
             FROM approval_requests
             WHERE (?1 IS NULL OR status = ?1)
             ORDER BY created_at DESC, id DESC
             LIMIT ?2",
        )?;
        let rows = statement
            .query_map(params![status, limit], row_to_app_approval_request_record)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn update_approval_request(
        &self,
        request: AppApprovalRequestRecord,
    ) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE approval_requests
             SET requester = ?2,
                 approver = ?3,
                 reason = ?4,
                 status = ?5,
                 expires_at = ?6,
                 created_at = ?7,
                 decided_at = ?8
             WHERE id = ?1",
            params![
                request.id,
                request.requester,
                request.approver,
                request.reason,
                request.status,
                system_time_to_unix_secs(request.expires_at),
                system_time_to_unix_secs(request.created_at),
                request.decided_at.map(system_time_to_unix_secs),
            ],
        )?;
        Ok(changed > 0)
    }

    pub fn list_job_summaries(&self, limit: usize) -> Result<Vec<JobSummaryRecord>, StoreError> {
        self.list_job_summaries_filtered(None, limit)
    }

    pub fn find_job_summary(&self, job_id: &str) -> Result<Option<JobSummaryRecord>, StoreError> {
        Ok(self
            .list_job_summaries_filtered(Some(job_id), 1)?
            .into_iter()
            .next())
    }

    fn list_job_summaries_filtered(
        &self,
        job_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<JobSummaryRecord>, StoreError> {
        let limit = limit.min(100) as i64;
        let mut statement = self.connection.prepare(
            "SELECT
                j.id,
                j.status,
                j.risk,
                j.command_program,
                j.command_args_json,
                j.created_at,
                j.selector_kind,
                j.selector_source,
                j.strategy_concurrency,
                j.strategy_max_failures,
                COUNT(ta.id) AS target_count,
                COALESCE(GROUP_CONCAT(
                    COALESCE(jt.agent_id, ta.agent_id, '') || char(30) ||
                    COALESCE(NULLIF(jt.agent_display_name, ''), a.name, jt.agent_id, ta.agent_id, '') || char(30) ||
                    COALESCE(NULLIF(jt.agent_status_snapshot, ''), jt.status, a.status, 'unknown') || char(30) ||
                    COALESCE(jt.labels_snapshot, '') || char(30) ||
                    COALESCE(ta.id, '') || char(30) ||
                    COALESCE(ta.status, '') || char(30) ||
                    COALESCE(ta.last_error, ''),
                    char(31)
                ), '') AS target_agents,
                MAX(ta.expires_at) AS expires_at
             FROM jobs j
             LEFT JOIN task_assignments ta ON ta.job_id = j.id
             LEFT JOIN job_targets jt ON jt.job_id = j.id AND jt.agent_id = ta.agent_id
             LEFT JOIN agents a ON a.id = ta.agent_id
             WHERE (?1 IS NULL OR j.id = ?1)
             GROUP BY j.id
             ORDER BY j.created_at DESC, j.id DESC
             LIMIT ?2",
        )?;
        let rows = statement
            .query_map(params![job_id, limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, Option<i64>>(12)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(
                |(
                    id,
                    status,
                    risk,
                    command_program,
                    command_args_json,
                    created_at,
                    selector_kind,
                    selector_source,
                    strategy_concurrency,
                    strategy_max_failures,
                    target_count,
                    target_agents,
                    expires_at,
                )| {
                    Ok(JobSummaryRecord {
                        id,
                        status,
                        risk,
                        command_program,
                        command_args: parse_command_args(&command_args_json)?,
                        selector_kind,
                        selector_source,
                        strategy_concurrency: strategy_concurrency.max(1) as u32,
                        strategy_max_failures: strategy_max_failures
                            .and_then(|value| u32::try_from(value).ok())
                            .filter(|value| *value > 0),
                        target_count: target_count.max(0) as usize,
                        target_agents: parse_job_target_summaries(&target_agents),
                        created_at: unix_secs_to_system_time(created_at),
                        expires_at: expires_at.map(unix_secs_to_system_time),
                    })
                },
            )
            .collect()
    }

    pub fn append_job_output_chunk_record(&self, chunk: &JobOutputChunk) -> Result<(), StoreError> {
        let stream = output_stream_to_str(chunk.stream);
        let result = self.connection.execute(
            "INSERT INTO job_output_chunks (
                job_id, agent_id, stream, chunk_index, body
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                chunk.job_id.as_str(),
                chunk.agent_id.as_str(),
                stream,
                chunk.sequence as i64,
                chunk.body.as_str(),
            ],
        );

        match result {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(error, Some(message)))
                if error.code == ErrorCode::ConstraintViolation =>
            {
                let existing_body = self
                    .connection
                    .query_row(
                        "SELECT body
                         FROM job_output_chunks
                         WHERE job_id = ?1
                           AND agent_id = ?2
                           AND stream = ?3
                           AND chunk_index = ?4",
                        params![
                            chunk.job_id.as_str(),
                            chunk.agent_id.as_str(),
                            stream,
                            chunk.sequence as i64,
                        ],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?;
                if existing_body.as_deref() == Some(chunk.body.as_str()) {
                    Ok(())
                } else {
                    Err(StoreError::ConstraintViolation(message))
                }
            }
            Err(error) => Err(StoreError::from(error)),
        }
    }

    pub fn list_job_output_chunks(
        &self,
        job_id: &str,
        agent_id: &str,
    ) -> Result<Vec<JobOutputChunk>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT job_id, agent_id, stream, chunk_index, body
             FROM job_output_chunks
             WHERE job_id = ?1 AND agent_id = ?2
             ORDER BY chunk_index",
        )?;
        let mut rows = statement.query(params![job_id, agent_id])?;
        let mut chunks = Vec::new();
        while let Some(row) = rows.next()? {
            chunks.push(JobOutputChunk {
                job_id: row.get(0)?,
                agent_id: row.get(1)?,
                stream: parse_output_stream(&row.get::<_, String>(2)?),
                sequence: row.get::<_, i64>(3)?.max(0) as u64,
                body: row.get(4)?,
            });
        }
        Ok(chunks)
    }

    pub fn list_job_output_chunks_for_job(
        &self,
        job_id: &str,
    ) -> Result<Vec<JobOutputChunk>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT job_id, agent_id, stream, chunk_index, body
             FROM job_output_chunks
             WHERE job_id = ?1
             ORDER BY agent_id, chunk_index, stream",
        )?;
        let mut rows = statement.query(params![job_id])?;
        let mut chunks = Vec::new();
        while let Some(row) = rows.next()? {
            chunks.push(JobOutputChunk {
                job_id: row.get(0)?,
                agent_id: row.get(1)?,
                stream: parse_output_stream(&row.get::<_, String>(2)?),
                sequence: row.get::<_, i64>(3)?.max(0) as u64,
                body: row.get(4)?,
            });
        }
        Ok(chunks)
    }

    pub fn save_rendered_artifact_metadata_record(
        &self,
        metadata: &RenderedArtifactMetadata,
    ) -> Result<(), StoreError> {
        let size_bytes = i64::try_from(metadata.size_bytes).map_err(|_| {
            StoreError::Domain("artifact size_bytes exceeds sqlite range".to_owned())
        })?;
        self.connection.execute(
            "INSERT INTO rendered_artifacts (
                id, job_id, agent_id, task_id, destination, checksum_sha256,
                size_bytes, retention_class, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                metadata.id.as_str(),
                metadata.job_id.as_str(),
                metadata.agent_id.as_str(),
                metadata.task_id.as_str(),
                metadata.destination.as_str(),
                metadata.checksum.as_sha256(),
                size_bytes,
                metadata.retention_class.as_str(),
                system_time_to_unix_secs(metadata.created_at),
            ],
        )?;
        Ok(())
    }

    pub fn list_rendered_artifact_metadata_for_job(
        &self,
        job_id: &str,
    ) -> Result<Vec<RenderedArtifactMetadata>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, job_id, agent_id, task_id, destination, checksum_sha256,
                    size_bytes, retention_class, created_at
             FROM rendered_artifacts
             WHERE job_id = ?1
             ORDER BY created_at, id",
        )?;
        let mut rows = statement.query(params![job_id])?;
        let mut records = Vec::new();
        while let Some(row) = rows.next()? {
            records.push(row_to_rendered_artifact_metadata(row)?);
        }
        Ok(records)
    }

    pub fn save_remediation_request_record(
        &self,
        request: &AppRemediationRequestRecord,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO remediation_requests (
                id, policy_id, policy_name, agent_id, runbook_ref, status,
                approval_required, risk_summary, job_id, origin_drift_report_id, policy_version,
                created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                request.id,
                request.policy_id,
                request.policy_name,
                request.agent_id,
                request.runbook_ref,
                request.status,
                if request.approval_required {
                    1_i64
                } else {
                    0_i64
                },
                request.risk_summary,
                request.job_id,
                request.origin_drift_report_id.map(DriftReportId::as_i64),
                request.policy_version.map(i64::from),
                system_time_to_unix_secs(request.created_at),
                system_time_to_unix_secs(request.updated_at),
            ],
        )?;
        Ok(())
    }

    pub fn save_remediation_proposal_record(
        &self,
        request: &AppRemediationRequestRecord,
        audit: &AuditEvent,
    ) -> Result<AppRemediationProposalSave, StoreError> {
        self.run_sqlite_transaction(|| {
            let existing = self
                .connection
                .query_row(
                    "SELECT id, policy_id, policy_name, agent_id, runbook_ref, status,
                            approval_required, risk_summary, job_id, origin_drift_report_id,
                            policy_version, created_at, updated_at
                     FROM remediation_requests
                     WHERE agent_id = ?1 AND policy_id = ?2
                       AND origin_drift_report_id IS NOT NULL
                       AND status NOT IN ('resolved', 'failed', 'rejected', 'expired', 'canceled')
                     ORDER BY created_at, id
                     LIMIT 1",
                    params![request.agent_id, request.policy_id],
                    row_to_remediation_request_record,
                )
                .optional()?;
            if let Some(remediation) = existing {
                return Ok(AppRemediationProposalSave {
                    remediation,
                    created: false,
                });
            }
            self.save_remediation_request_record(request)?;
            self.insert_audit(audit)?;
            Ok(AppRemediationProposalSave {
                remediation: request.clone(),
                created: true,
            })
        })
    }

    pub fn save_verified_drift_proposal_record(
        &self,
        input: &AppPersistVerifiedDriftProposalInput,
    ) -> Result<AppPersistVerifiedDriftProposalOutput, StoreError> {
        self.run_sqlite_transaction(|| {
            let job_id = input
                .provenance
                .job_id
                .as_ref()
                .ok_or_else(|| StoreError::Domain("verified drift requires job id".to_owned()))?;
            let task_id = input
                .provenance
                .task_id
                .as_ref()
                .ok_or_else(|| StoreError::Domain("verified drift requires task id".to_owned()))?;
            let existing = self
                .connection
                .query_row(
                    "SELECT id, agent_id, policy_name, status, expected, actual, checked_at,
                            severity, acknowledged_at, acknowledged_by, resolved_at, resolution_job_id,
                            job_id, task_id, policy_id, policy_version, purpose
                     FROM drift_reports WHERE job_id = ?1 AND task_id = ?2",
                    params![job_id.as_str(), task_id.as_str()],
                    |row| {
                        row_to_drift_report_page_record(row).map(|record| AppDriftReportRecord {
                            id: record.id,
                            agent_id: record.agent_id,
                            report: record.report,
                            provenance: record.provenance,
                            checked_at: record.checked_at,
                        })
                    },
                )
                .optional()?;
            if let Some(report) = existing {
                let remediation = self
                    .connection
                    .query_row(
                        "SELECT id, policy_id, policy_name, agent_id, runbook_ref, status,
                                approval_required, risk_summary, job_id, origin_drift_report_id,
                                policy_version, created_at, updated_at
                         FROM remediation_requests WHERE origin_drift_report_id = ?1
                         ORDER BY created_at, id LIMIT 1",
                        params![report.id.as_i64()],
                        row_to_remediation_request_record,
                    )
                    .optional()?
                    .ok_or_else(|| {
                        StoreError::Domain("verified drift has no remediation proposal".to_owned())
                    })?;
                return Ok(AppPersistVerifiedDriftProposalOutput {
                    report,
                    proposal: AppRemediationProposalSave {
                        remediation,
                        created: false,
                    },
                });
            }
            self.connection
                .execute(
                    "INSERT INTO drift_reports (
                        agent_id, policy_name, status, severity, expected, actual, checked_at,
                        job_id, task_id, policy_id, policy_version, purpose
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        input.agent_id,
                        input.report.policy_name,
                        drift_status_to_str(&input.report.status),
                        drift_severity_to_str(input.report.severity),
                        input.report.expected,
                        input.report.actual,
                        system_time_to_unix_secs(input.checked_at),
                        job_id.as_str(),
                        task_id.as_str(),
                        input.provenance.policy_id,
                        input.provenance.policy_version.map(i64::from),
                        input.provenance.purpose.map(DriftCheckPurpose::as_str),
                    ],
                )
                .map_err(map_drift_report_constraint)?;
            let report_id = DriftReportId::new(self.connection.last_insert_rowid())
                .map_err(|error| StoreError::Domain(format!("{error:?}")))?;
            let report = AppDriftReportRecord {
                id: report_id,
                agent_id: input.agent_id.clone(),
                report: input.report.clone(),
                provenance: input.provenance.clone(),
                checked_at: input.checked_at,
            };
            self.insert_audit(&input.drift_audit)?;
            let mut remediation = input.remediation.clone();
            remediation.origin_drift_report_id = Some(report_id);
            let existing = self
                .connection
                .query_row(
                    "SELECT id, policy_id, policy_name, agent_id, runbook_ref, status,
                            approval_required, risk_summary, job_id, origin_drift_report_id,
                            policy_version, created_at, updated_at
                     FROM remediation_requests
                     WHERE agent_id = ?1 AND policy_id = ?2
                       AND origin_drift_report_id IS NOT NULL
                       AND status NOT IN ('resolved', 'failed', 'rejected', 'expired', 'canceled')
                     ORDER BY created_at, id LIMIT 1",
                    params![remediation.agent_id, remediation.policy_id],
                    row_to_remediation_request_record,
                )
                .optional()?;
            let proposal = if let Some(existing) = existing {
                AppRemediationProposalSave {
                    remediation: existing,
                    created: false,
                }
            } else {
                self.save_remediation_request_record(&remediation)?;
                self.insert_audit(&input.proposal_audit)?;
                AppRemediationProposalSave {
                    remediation,
                    created: true,
                }
            };
            Ok(AppPersistVerifiedDriftProposalOutput { report, proposal })
        })
    }

    pub fn find_remediation_request_record(
        &self,
        request_id: &str,
    ) -> Result<Option<AppRemediationRequestRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT id, policy_id, policy_name, agent_id, runbook_ref, status,
                        approval_required, risk_summary, job_id, origin_drift_report_id, policy_version,
                        created_at, updated_at
                 FROM remediation_requests
                 WHERE id = ?1",
                params![request_id],
                row_to_remediation_request_record,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn find_remediation_request_by_job_id_record(
        &self,
        job_id: &str,
    ) -> Result<Option<AppRemediationRequestRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT id, policy_id, policy_name, agent_id, runbook_ref, status,
                        approval_required, risk_summary, job_id, origin_drift_report_id, policy_version,
                        created_at, updated_at
                 FROM remediation_requests
                 WHERE job_id = ?1",
                params![job_id],
                row_to_remediation_request_record,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn find_remediation_verification_job_id(
        &self,
        remediation_id: &str,
    ) -> Result<Option<String>, StoreError> {
        self.connection
            .query_row(
                "SELECT job_id FROM remediation_verification_jobs WHERE remediation_id = ?1",
                params![remediation_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn save_remediation_verification_job(
        &self,
        remediation_id: &str,
        job_id: &str,
        created_at: SystemTime,
    ) -> Result<bool, StoreError> {
        let inserted = self.connection.execute(
            "INSERT INTO remediation_verification_jobs (remediation_id, job_id, created_at)
             VALUES (?1, ?2, ?3) ON CONFLICT (remediation_id) DO NOTHING",
            params![remediation_id, job_id, system_time_to_unix_secs(created_at)],
        )?;
        Ok(inserted > 0)
    }

    pub fn list_remediation_request_records(
        &self,
        agent_id: Option<&str>,
        policy_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AppRemediationRequestRecord>, StoreError> {
        let limit = limit.clamp(1, 500);
        let mut statement = self.connection.prepare(
            "SELECT id, policy_id, policy_name, agent_id, runbook_ref, status,
                    approval_required, risk_summary, job_id, origin_drift_report_id, policy_version,
                    created_at, updated_at
             FROM remediation_requests
             ORDER BY created_at, id",
        )?;
        let rows = statement.query_map([], row_to_remediation_request_record)?;
        let mut records = Vec::new();
        for row in rows {
            let record = row?;
            if agent_id.is_some_and(|expected| record.agent_id != expected) {
                continue;
            }
            if policy_id.is_some_and(|expected| record.policy_id != expected) {
                continue;
            }
            records.push(record);
            if records.len() == limit {
                break;
            }
        }
        Ok(records)
    }

    /// Returns the bounded, correlation-free verification backlog for controller startup.
    pub fn list_pending_remediation_verification_recovery_records(
        &self,
        limit: usize,
    ) -> Result<Vec<AppRemediationRequestRecord>, StoreError> {
        let limit = limit.clamp(1, 100);
        let mut statement = self.connection.prepare(
            "SELECT remediation_requests.id, remediation_requests.policy_id,
                    remediation_requests.policy_name, remediation_requests.agent_id,
                    remediation_requests.runbook_ref, remediation_requests.status,
                    remediation_requests.approval_required, remediation_requests.risk_summary,
                    remediation_requests.job_id, remediation_requests.origin_drift_report_id,
                    remediation_requests.policy_version,
                    remediation_requests.created_at, remediation_requests.updated_at
             FROM remediation_requests
             LEFT JOIN remediation_verification_jobs
               ON remediation_verification_jobs.remediation_id = remediation_requests.id
             WHERE remediation_requests.status = 'succeeded_pending_verify'
               AND remediation_verification_jobs.remediation_id IS NULL
             ORDER BY remediation_requests.created_at, remediation_requests.id
             LIMIT ?1",
        )?;
        statement
            .query_map(params![limit], row_to_remediation_request_record)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn update_remediation_request_status_record(
        &self,
        request_id: &str,
        status: &str,
        job_id: Option<&str>,
        updated_at: SystemTime,
    ) -> Result<(), StoreError> {
        let changed = self.connection.execute(
            "UPDATE remediation_requests
             SET status = ?2, job_id = ?3, updated_at = ?4
             WHERE id = ?1",
            params![
                request_id,
                status,
                job_id,
                system_time_to_unix_secs(updated_at)
            ],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    fn insert_agent(&self, agent: &Agent) -> Result<(), StoreError> {
        let status = status_to_str(agent.status());
        let labels = encode_labels(agent.labels());
        let last_seen_at = agent.last_seen_at().map(system_time_to_unix_secs);
        let pinned_controller = agent
            .pinned_controller()
            .map(ControllerPublicKey::as_str)
            .unwrap_or_default();

        match self.connection.execute(
            "INSERT INTO agents (
                id, name, public_key, fingerprint, labels, status, last_seen_at, pinned_controller
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                agent.id().as_str(),
                agent.name().as_str(),
                agent.identity().public_key.as_str(),
                agent.identity().fingerprint.as_str(),
                labels,
                status,
                last_seen_at,
                pinned_controller,
            ],
        ) {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == ErrorCode::ConstraintViolation =>
            {
                Err(StoreError::DuplicateAgent)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn row_to_agent(row: StoredAgentRow) -> Result<Agent, StoreError> {
        let labels = decode_labels(&row.labels)?;
        let pinned_controller = if row.pinned_controller.is_empty() {
            None
        } else {
            Some(ControllerPublicKey::new(row.pinned_controller)?)
        };

        Ok(Agent::restore(
            AgentId::new(row.id)?,
            AgentName::new(row.name)?,
            AgentIdentity {
                public_key: AgentPublicKey::new(row.public_key)?,
                fingerprint: AgentFingerprint::new(row.fingerprint)?,
            },
            labels,
            parse_status(&row.status),
            row.last_seen_at.map(unix_secs_to_system_time),
            pinned_controller,
        ))
    }

    fn insert_audit(&self, event: &AuditEvent) -> Result<(), StoreError> {
        let (value_kind, value_text) = encode_audit_value(&event.value);
        self.connection.execute(
            "INSERT INTO audit_events (
                category, action, actor, target, value_kind, value_text, occurred_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                event.category.as_str(),
                event.action,
                event.actor.as_str(),
                event.target.as_str(),
                value_kind,
                value_text,
                system_time_to_unix_secs(event.occurred_at),
            ],
        )?;
        Ok(())
    }

    fn query_audit(
        &self,
        category: Option<AuditCategory>,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, StoreError> {
        let limit = limit.clamp(1, 500) as i64;
        let mut events = Vec::new();

        if let Some(category) = category {
            let mut statement = self.connection.prepare(
                "SELECT category, action, actor, target, value_kind, value_text, occurred_at
                 FROM audit_events
                 WHERE category = ?1
                 ORDER BY id DESC
                 LIMIT ?2",
            )?;
            let mut rows = statement.query(params![category.as_str(), limit])?;
            while let Some(row) = rows.next()? {
                events.push(row_to_audit(row)?);
            }
        } else {
            let mut statement = self.connection.prepare(
                "SELECT category, action, actor, target, value_kind, value_text, occurred_at
                 FROM audit_events
                 ORDER BY id DESC
                 LIMIT ?1",
            )?;
            let mut rows = statement.query(params![limit])?;
            while let Some(row) = rows.next()? {
                events.push(row_to_audit(row)?);
            }
        }

        Ok(events)
    }

    fn query_audit_page(
        &self,
        category: Option<AuditCategory>,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AuditEventPageRecord>, StoreError> {
        let limit = limit.clamp(1, 500) as i64;
        let before_seconds = before.map(|cursor| system_time_to_unix_secs(cursor.occurred_at));
        let before_row_id = before.map(|cursor| cursor.row_id);
        let mut events = Vec::new();

        match (category, before_seconds, before_row_id) {
            (Some(category), Some(before_seconds), Some(before_row_id)) => {
                let mut statement = self.connection.prepare(
                    "SELECT id, category, action, actor, target, value_kind, value_text, occurred_at
                     FROM audit_events
                     WHERE category = ?1
                       AND (occurred_at < ?2 OR (occurred_at = ?2 AND id < ?3))
                     ORDER BY occurred_at DESC, id DESC
                     LIMIT ?4",
                )?;
                let mut rows = statement.query(params![
                    category.as_str(),
                    before_seconds,
                    before_row_id,
                    limit
                ])?;
                while let Some(row) = rows.next()? {
                    events.push(row_to_audit_page_record(row)?);
                }
            }
            (Some(category), None, None) => {
                let mut statement = self.connection.prepare(
                    "SELECT id, category, action, actor, target, value_kind, value_text, occurred_at
                     FROM audit_events
                     WHERE category = ?1
                     ORDER BY occurred_at DESC, id DESC
                     LIMIT ?2",
                )?;
                let mut rows = statement.query(params![category.as_str(), limit])?;
                while let Some(row) = rows.next()? {
                    events.push(row_to_audit_page_record(row)?);
                }
            }
            (None, Some(before_seconds), Some(before_row_id)) => {
                let mut statement = self.connection.prepare(
                    "SELECT id, category, action, actor, target, value_kind, value_text, occurred_at
                     FROM audit_events
                     WHERE occurred_at < ?1 OR (occurred_at = ?1 AND id < ?2)
                     ORDER BY occurred_at DESC, id DESC
                     LIMIT ?3",
                )?;
                let mut rows = statement.query(params![before_seconds, before_row_id, limit])?;
                while let Some(row) = rows.next()? {
                    events.push(row_to_audit_page_record(row)?);
                }
            }
            (None, None, None) => {
                let mut statement = self.connection.prepare(
                    "SELECT id, category, action, actor, target, value_kind, value_text, occurred_at
                     FROM audit_events
                     ORDER BY occurred_at DESC, id DESC
                     LIMIT ?1",
                )?;
                let mut rows = statement.query(params![limit])?;
                while let Some(row) = rows.next()? {
                    events.push(row_to_audit_page_record(row)?);
                }
            }
            _ => return Err(StoreError::Domain("invalid audit page cursor".to_owned())),
        }

        Ok(events)
    }
}

impl AgentRepository for SqliteStore {
    type Error = StoreError;

    fn save(&mut self, agent: Agent) -> Result<(), Self::Error> {
        self.insert_agent(&agent)
    }

    fn find_by_id(&self, id: &AgentId) -> Result<Option<Agent>, Self::Error> {
        let row = self
            .connection
            .query_row(
                "SELECT id, name, public_key, fingerprint, labels, status, last_seen_at, pinned_controller
                 FROM agents
                 WHERE id = ?1",
                params![id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()?;

        row.map(
            |(id, name, public_key, fingerprint, labels, status, last_seen_at, pinned)| {
                Self::row_to_agent(StoredAgentRow {
                    id,
                    name,
                    public_key,
                    fingerprint,
                    labels,
                    status,
                    last_seen_at,
                    pinned_controller: pinned,
                })
            },
        )
        .transpose()
    }

    fn list(&self) -> Result<Vec<Agent>, Self::Error> {
        let mut statement = self.connection.prepare(
            "SELECT id, name, public_key, fingerprint, labels, status, last_seen_at, pinned_controller
             FROM agents
             ORDER BY name",
        )?;
        let mut rows = statement.query([])?;
        let mut agents = Vec::new();
        while let Some(row) = rows.next()? {
            agents.push(Self::row_to_agent(StoredAgentRow {
                id: row.get(0)?,
                name: row.get(1)?,
                public_key: row.get(2)?,
                fingerprint: row.get(3)?,
                labels: row.get(4)?,
                status: row.get(5)?,
                last_seen_at: row.get(6)?,
                pinned_controller: row.get(7)?,
            })?);
        }
        Ok(agents)
    }
}

struct StoredAgentRow {
    id: String,
    name: String,
    public_key: String,
    fingerprint: String,
    labels: String,
    status: String,
    last_seen_at: Option<i64>,
    pinned_controller: String,
}

fn artifact_sha256(bytes: &[u8]) -> Result<ArtifactChecksum, StoreError> {
    let checksum = Sha256::digest(bytes);
    ArtifactChecksum::sha256(format!("{checksum:x}"))
        .map_err(|error| StoreError::Domain(error.to_string()))
}

fn safe_artifact_filename(id: &ArtifactId) -> Result<String, StoreError> {
    let value = id.as_str();
    let path = Path::new(value);
    if path.is_absolute() {
        return Err(StoreError::Domain(
            "artifact id must be relative".to_owned(),
        ));
    }

    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(component)), None) => {
            let filename = component.to_string_lossy();
            if filename == "."
                || filename == ".."
                || filename.contains(std::path::MAIN_SEPARATOR)
                || filename.contains('\\')
            {
                return Err(StoreError::Domain(
                    "artifact id must be a safe object key".to_owned(),
                ));
            }
            Ok(format!("{filename}.blob"))
        }
        _ => Err(StoreError::Domain(
            "artifact id must be a safe object key".to_owned(),
        )),
    }
}

fn artifact_io_error(context: &'static str) -> StoreError {
    StoreError::Domain(context.to_owned())
}

#[cfg(feature = "postgres")]
fn postgres_error(context: &'static str) -> StoreError {
    StoreError::Postgres(context.to_owned())
}

#[cfg(feature = "postgres")]
fn postgres_connection_error() -> StoreError {
    StoreError::Postgres("postgres connection failed".to_owned())
}

#[cfg(feature = "postgres")]
fn postgres_tls_adapter_error() -> StoreError {
    StoreError::Postgres("postgres TLS adapter initialization failed".to_owned())
}

#[cfg(feature = "postgres")]
fn postgres_pool_checkout_error() -> StoreError {
    StoreError::Postgres("postgres client checkout failed".to_owned())
}

#[cfg(feature = "postgres")]
fn postgres_is_unique_violation(error: &postgres::Error) -> bool {
    error
        .code()
        .is_some_and(|code| *code == postgres::error::SqlState::UNIQUE_VIOLATION)
}

#[cfg(feature = "postgres")]
fn postgres_duplicate_or_context(error: postgres::Error, context: &'static str) -> StoreError {
    if postgres_is_unique_violation(&error) {
        StoreError::DuplicateAgent
    } else {
        postgres_error(context)
    }
}

#[cfg(feature = "postgres")]
fn postgres_constraint_or_context(error: postgres::Error, context: &'static str) -> StoreError {
    if postgres_is_unique_violation(&error) {
        StoreError::ConstraintViolation("postgres unique constraint violation".to_owned())
    } else {
        postgres_error(context)
    }
}

#[cfg(feature = "postgres")]
fn postgres_insert_task_assignment_in_transaction(
    transaction: &mut postgres::Transaction<'_>,
    envelope: &TaskEnvelope,
) -> Result<(), StoreError> {
    let signature = envelope
        .signature
        .as_ref()
        .ok_or_else(|| StoreError::Domain("task assignment must be signed".to_owned()))?;
    transaction
        .execute(
            "INSERT INTO task_assignments (
                id, job_id, agent_id, nonce, payload_hash, signature, issued_at, expires_at
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            &[
                &envelope.task_id.as_str(),
                &envelope.job_id.as_str(),
                &envelope.target_agent_id.as_str(),
                &envelope.nonce.as_str(),
                &envelope.payload_hash,
                &signature.as_str(),
                &system_time_to_unix_secs(envelope.issued_at),
                &system_time_to_unix_secs(envelope.expires_at.as_system_time()),
            ],
        )
        .map_err(|error| {
            postgres_constraint_or_context(error, "postgres task assignment insert failed")
        })?;
    transaction
        .execute(
            "INSERT INTO job_targets (
                job_id, agent_id, status, agent_display_name, agent_status_snapshot, labels_snapshot
             )
             SELECT
                $1,
                a.id,
                a.status,
                a.name,
                a.status,
                a.labels
             FROM agents a
             WHERE a.id = $2
             ON CONFLICT (job_id, agent_id) DO NOTHING",
            &[
                &envelope.job_id.as_str(),
                &envelope.target_agent_id.as_str(),
            ],
        )
        .map_err(|_| postgres_error("postgres job target snapshot insert failed"))?;
    Ok(())
}

#[cfg(feature = "postgres")]
impl AgentRepository for PostgresStore {
    type Error = StoreError;

    fn save(&mut self, agent: Agent) -> Result<(), Self::Error> {
        let status = status_to_str(agent.status());
        let labels = encode_labels(agent.labels());
        let last_seen_at = agent.last_seen_at().map(system_time_to_unix_secs);
        let pinned_controller = agent
            .pinned_controller()
            .map(ControllerPublicKey::as_str)
            .unwrap_or_default();
        let mut client = self.checkout_client()?;
        let mut transaction = client
            .transaction()
            .map_err(|_| postgres_error("postgres transaction failed"))?;
        transaction
            .execute(
                "INSERT INTO agents (
                    id, name, public_key, fingerprint, labels, status, last_seen_at, pinned_controller
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                &[
                    &agent.id().as_str(),
                    &agent.name().as_str(),
                    &agent.identity().public_key.as_str(),
                    &agent.identity().fingerprint.as_str(),
                    &labels,
                    &status,
                    &last_seen_at,
                    &pinned_controller,
                ],
            )
            .map_err(|error| postgres_duplicate_or_context(error, "postgres agent insert failed"))?;
        transaction
            .execute(
                "INSERT INTO agent_identities (agent_id, public_key, fingerprint)
                 VALUES ($1, $2, $3)",
                &[
                    &agent.id().as_str(),
                    &agent.identity().public_key.as_str(),
                    &agent.identity().fingerprint.as_str(),
                ],
            )
            .map_err(|error| {
                postgres_duplicate_or_context(error, "postgres agent identity insert failed")
            })?;
        transaction
            .commit()
            .map_err(|_| postgres_error("postgres transaction commit failed"))?;
        Ok(())
    }

    fn find_by_id(&self, id: &AgentId) -> Result<Option<Agent>, Self::Error> {
        let mut client = self.checkout_client()?;
        let row = client
            .query_opt(
                "SELECT id, name, public_key, fingerprint, labels, status, last_seen_at, pinned_controller
                 FROM agents
                 WHERE id = $1",
                &[&id.as_str()],
            )
            .map_err(|_| postgres_error("postgres agent query failed"))?;

        row.map(|row| {
            SqliteStore::row_to_agent(StoredAgentRow {
                id: row.get(0),
                name: row.get(1),
                public_key: row.get(2),
                fingerprint: row.get(3),
                labels: row.get(4),
                status: row.get(5),
                last_seen_at: row.get(6),
                pinned_controller: row.get(7),
            })
        })
        .transpose()
    }

    fn list(&self) -> Result<Vec<Agent>, Self::Error> {
        let mut client = self.checkout_client()?;
        let rows = client
            .query(
                "SELECT id, name, public_key, fingerprint, labels, status, last_seen_at, pinned_controller
                 FROM agents
                 ORDER BY name",
                &[],
            )
            .map_err(|_| postgres_error("postgres agent list failed"))?;

        rows.into_iter()
            .map(|row| {
                SqliteStore::row_to_agent(StoredAgentRow {
                    id: row.get(0),
                    name: row.get(1),
                    public_key: row.get(2),
                    fingerprint: row.get(3),
                    labels: row.get(4),
                    status: row.get(5),
                    last_seen_at: row.get(6),
                    pinned_controller: row.get(7),
                })
            })
            .collect()
    }
}

#[cfg(feature = "postgres")]
impl EnrollmentTokenRepository for PostgresStore {
    type Error = StoreError;

    fn insert_enrollment_token_hash(
        &mut self,
        id: &str,
        token_hash: &str,
        default_labels: &str,
        expires_at: SystemTime,
        max_uses: u32,
    ) -> Result<(), Self::Error> {
        self.checkout_client()?
            .execute(
                "INSERT INTO enrollment_tokens (
                    id, token_hash, default_labels, expires_at, max_uses, used_count
                 ) VALUES ($1, $2, $3, $4, $5, 0)",
                &[
                    &id,
                    &token_hash,
                    &default_labels,
                    &system_time_to_unix_secs(expires_at),
                    &(max_uses as i64),
                ],
            )
            .map(|_| ())
            .map_err(|error| {
                postgres_constraint_or_context(error, "postgres enrollment token insert failed")
            })
    }

    fn list_enrollment_tokens(&self) -> Result<Vec<AppEnrollmentTokenRecord>, Self::Error> {
        let mut client = self.checkout_client()?;
        let rows = client
            .query(
                "SELECT id, default_labels, expires_at, max_uses, used_count, revoked_at
                 FROM enrollment_tokens
                 ORDER BY created_at DESC, id DESC",
                &[],
            )
            .map_err(|_| postgres_error("postgres enrollment token list failed"))?;

        Ok(rows
            .into_iter()
            .map(|row| AppEnrollmentTokenRecord {
                id: row.get(0),
                default_labels: row.get(1),
                expires_at: unix_secs_to_system_time(row.get(2)),
                max_uses: row.get::<_, i64>(3).max(0) as u32,
                used_count: row.get::<_, i64>(4).max(0) as u32,
                revoked: row.get::<_, Option<i64>>(5).is_some(),
            })
            .collect())
    }

    fn revoke_enrollment_token(&mut self, id: &str) -> Result<bool, Self::Error> {
        self.checkout_client()?
            .execute(
                "UPDATE enrollment_tokens
                 SET revoked_at = EXTRACT(EPOCH FROM now())::BIGINT
                 WHERE id = $1 AND revoked_at IS NULL",
                &[&id],
            )
            .map(|changed| changed > 0)
            .map_err(|_| postgres_error("postgres enrollment token revoke failed"))
    }

    fn consume_enrollment_token_hash(
        &mut self,
        token_hash: &str,
        now: SystemTime,
    ) -> Result<AppEnrollmentTokenRecord, Self::Error> {
        let mut client = self.checkout_client()?;
        let mut transaction = client
            .transaction()
            .map_err(|_| postgres_error("postgres transaction failed"))?;
        let record = transaction
            .query_opt(
                "SELECT id, default_labels, expires_at, max_uses, used_count, revoked_at
                 FROM enrollment_tokens
                 WHERE token_hash = $1",
                &[&token_hash],
            )
            .map_err(|_| postgres_error("postgres enrollment token query failed"))?
            .map(|row| AppEnrollmentTokenRecord {
                id: row.get(0),
                default_labels: row.get(1),
                expires_at: unix_secs_to_system_time(row.get(2)),
                max_uses: row.get::<_, i64>(3).max(0) as u32,
                used_count: row.get::<_, i64>(4).max(0) as u32,
                revoked: row.get::<_, Option<i64>>(5).is_some(),
            })
            .ok_or(StoreError::NotFound)?;

        if record.revoked {
            return Err(StoreError::Domain("enrollment token is revoked".to_owned()));
        }
        if now >= record.expires_at {
            return Err(StoreError::Domain("enrollment token is expired".to_owned()));
        }
        if record.used_count >= record.max_uses {
            return Err(StoreError::Domain(
                "enrollment token max uses exceeded".to_owned(),
            ));
        }

        transaction
            .execute(
                "UPDATE enrollment_tokens
                 SET used_count = used_count + 1
                 WHERE id = $1",
                &[&record.id],
            )
            .map_err(|_| postgres_error("postgres enrollment token consume failed"))?;
        transaction
            .commit()
            .map_err(|_| postgres_error("postgres transaction commit failed"))?;

        Ok(record)
    }
}

#[cfg(feature = "postgres")]
impl AgentIdentityRepository for PostgresStore {
    type Error = StoreError;

    fn find_agent_identity(
        &self,
        agent_id: &str,
    ) -> Result<Option<AppAgentIdentityRecord>, Self::Error> {
        let mut client = self.checkout_client()?;
        client
            .query_opt(
                "SELECT public_key, fingerprint
                 FROM agent_identities
                 WHERE agent_id = $1",
                &[&agent_id],
            )
            .map(|row| {
                row.map(|row| AppAgentIdentityRecord {
                    public_key: row.get(0),
                    fingerprint: row.get(1),
                })
            })
            .map_err(|_| postgres_error("postgres agent identity query failed"))
    }
}

#[cfg(feature = "postgres")]
impl AdminTokenRepository for PostgresStore {
    type Error = StoreError;

    fn admin_token_exists(&self) -> Result<bool, Self::Error> {
        let mut client = self.checkout_client()?;
        client
            .query_opt("SELECT 1 FROM admin_tokens WHERE id = 1", &[])
            .map(|row| row.is_some())
            .map_err(|_| postgres_error("postgres admin token query failed"))
    }

    fn insert_admin_token_hash(&mut self, token_hash: &str) -> Result<(), Self::Error> {
        self.checkout_client()?
            .execute(
                "INSERT INTO admin_tokens (id, token_hash, actor_id, role, created_at)
                 VALUES (1, $1, 'bootstrap-admin', 'owner', EXTRACT(EPOCH FROM now())::BIGINT)
                 ON CONFLICT (id) DO NOTHING",
                &[&token_hash],
            )
            .map(|_| ())
            .map_err(|_| postgres_error("postgres admin token insert failed"))
    }

    fn verify_admin_token_hash(&self, token_hash: &str) -> Result<bool, Self::Error> {
        let mut client = self.checkout_client()?;
        client
            .query_opt(
                "SELECT 1 FROM admin_tokens WHERE id = 1 AND token_hash = $1",
                &[&token_hash],
            )
            .map(|row| row.is_some())
            .map_err(|_| postgres_error("postgres admin token verify failed"))
    }

    fn find_admin_token_record(
        &self,
        token_hash: &str,
    ) -> Result<Option<AppAdminTokenRecord>, Self::Error> {
        let mut client = self.checkout_client()?;
        client
            .query_opt(
                "SELECT actor_id, role
                 FROM admin_tokens
                 WHERE id = 1 AND token_hash = $1",
                &[&token_hash],
            )
            .map(|row| {
                row.map(|row| AppAdminTokenRecord {
                    actor_id: row.get(0),
                    role: row.get(1),
                })
            })
            .map_err(|_| postgres_error("postgres admin token record query failed"))
    }
}

#[cfg(feature = "postgres")]
impl ControllerIdentityRepository for PostgresStore {
    type Error = StoreError;

    fn save_controller_identity_metadata(
        &mut self,
        metadata: ControllerIdentityMetadata,
    ) -> Result<(), Self::Error> {
        self.checkout_client()?
            .execute(
                "INSERT INTO controller_identity (
                    id, public_key, public_fingerprint, private_key_path, created_at
                 ) VALUES (1, $1, $2, $3, $4)
                 ON CONFLICT (id) DO UPDATE SET
                    public_key = EXCLUDED.public_key,
                    public_fingerprint = EXCLUDED.public_fingerprint,
                    private_key_path = EXCLUDED.private_key_path,
                    created_at = EXCLUDED.created_at",
                &[
                    &metadata.public_key,
                    &metadata.public_fingerprint,
                    &metadata.private_key_path,
                    &system_time_to_unix_secs(metadata.created_at),
                ],
            )
            .map(|_| ())
            .map_err(|_| postgres_error("postgres controller identity save failed"))
    }

    fn controller_identity_metadata(
        &self,
    ) -> Result<Option<ControllerIdentityMetadata>, Self::Error> {
        let mut client = self.checkout_client()?;
        client
            .query_opt(
                "SELECT public_key, public_fingerprint, private_key_path, created_at
                 FROM controller_identity
                 WHERE id = 1",
                &[],
            )
            .map(|row| {
                row.map(|row| ControllerIdentityMetadata {
                    public_key: row.get(0),
                    public_fingerprint: row.get(1),
                    private_key_path: row.get(2),
                    created_at: unix_secs_to_system_time(row.get(3)),
                })
            })
            .map_err(|_| postgres_error("postgres controller identity query failed"))
    }
}

#[cfg(feature = "postgres")]
impl SigningKeyRotationRepository for PostgresStore {
    type Error = StoreError;

    fn save_signing_key_rotation(
        &mut self,
        record: SigningKeyRotationRecord,
    ) -> Result<(), Self::Error> {
        let snapshot = record.rotation.snapshot();
        let new_fingerprint = snapshot
            .new_fingerprint
            .as_ref()
            .map(|fingerprint| fingerprint.as_str().to_owned());
        let requested_at = snapshot.requested_at.map(system_time_to_unix_secs);
        let validated_at = snapshot.validated_at.map(system_time_to_unix_secs);
        let activated_at = snapshot.activated_at.map(system_time_to_unix_secs);
        let old_key_verifies_until = snapshot
            .old_key_verifies_until
            .map(system_time_to_unix_secs);
        let retired_at = snapshot.retired_at.map(system_time_to_unix_secs);
        let failed_at = snapshot.failed_at.map(system_time_to_unix_secs);
        let updated_at = system_time_to_unix_secs(record.updated_at);
        self.checkout_client()?
            .execute(
                "INSERT INTO controller_signing_key_rotation (
                    controller_id, state, old_fingerprint, new_fingerprint, requested_at,
                    validated_at, activated_at, old_key_verifies_until, retired_at, failed_at,
                    updated_at
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                 ON CONFLICT (controller_id) DO UPDATE SET
                    state = EXCLUDED.state,
                    old_fingerprint = EXCLUDED.old_fingerprint,
                    new_fingerprint = EXCLUDED.new_fingerprint,
                    requested_at = EXCLUDED.requested_at,
                    validated_at = EXCLUDED.validated_at,
                    activated_at = EXCLUDED.activated_at,
                    old_key_verifies_until = EXCLUDED.old_key_verifies_until,
                    retired_at = EXCLUDED.retired_at,
                    failed_at = EXCLUDED.failed_at,
                    updated_at = EXCLUDED.updated_at",
                &[
                    &record.controller_id,
                    &snapshot.state.as_str(),
                    &snapshot.old_fingerprint.as_str(),
                    &new_fingerprint,
                    &requested_at,
                    &validated_at,
                    &activated_at,
                    &old_key_verifies_until,
                    &retired_at,
                    &failed_at,
                    &updated_at,
                ],
            )
            .map(|_| ())
            .map_err(|_| postgres_error("postgres signing key rotation save failed"))
    }

    fn load_signing_key_rotation(
        &self,
        controller_id: &str,
    ) -> Result<Option<SigningKeyRotationRecord>, Self::Error> {
        let mut client = self.checkout_client()?;
        client
            .query_opt(
                "SELECT controller_id, state, old_fingerprint, new_fingerprint, requested_at,
                        validated_at, activated_at, old_key_verifies_until, retired_at,
                        failed_at, updated_at
                 FROM controller_signing_key_rotation
                 WHERE controller_id = $1",
                &[&controller_id],
            )
            .map_err(|_| postgres_error("postgres signing key rotation query failed"))?
            .map(|row| {
                signing_key_rotation_record_from_storage(
                    postgres_row_to_signing_key_rotation_storage(&row),
                )
            })
            .transpose()
    }
}

#[cfg(feature = "postgres")]
impl ControllerSigningStagedRolloutRepository for PostgresStore {
    type Error = StoreError;

    fn save_controller_signing_staged_rollout(
        &mut self,
        record: ControllerSigningStagedRolloutRecord,
    ) -> Result<(), Self::Error> {
        let storage = controller_signing_staged_rollout_record_to_storage(record)?;
        self.checkout_client()?
            .execute(
                "INSERT INTO controller_signing_staged_rollout (
                    controller_id, state, target_ids, batch_size, max_failures,
                    ack_timeout_seconds, acknowledged_agent_ids, unavailable_agent_ids,
                    failed_agent_ids, in_flight_attempts, failure_reason_code,
                    current_fingerprint, previous_fingerprint, updated_at
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
                 ON CONFLICT (controller_id) DO UPDATE SET
                    state = EXCLUDED.state,
                    target_ids = EXCLUDED.target_ids,
                    batch_size = EXCLUDED.batch_size,
                    max_failures = EXCLUDED.max_failures,
                    ack_timeout_seconds = EXCLUDED.ack_timeout_seconds,
                    acknowledged_agent_ids = EXCLUDED.acknowledged_agent_ids,
                    unavailable_agent_ids = EXCLUDED.unavailable_agent_ids,
                    failed_agent_ids = EXCLUDED.failed_agent_ids,
                    in_flight_attempts = EXCLUDED.in_flight_attempts,
                    failure_reason_code = EXCLUDED.failure_reason_code,
                    current_fingerprint = EXCLUDED.current_fingerprint,
                    previous_fingerprint = EXCLUDED.previous_fingerprint,
                    updated_at = EXCLUDED.updated_at",
                &[
                    &storage.controller_id,
                    &storage.state,
                    &storage.target_ids,
                    &storage.batch_size,
                    &storage.max_failures,
                    &storage.ack_timeout_seconds,
                    &storage.acknowledged_agent_ids,
                    &storage.unavailable_agent_ids,
                    &storage.failed_agent_ids,
                    &storage.in_flight_attempts,
                    &storage.failure_reason_code,
                    &storage.current_fingerprint,
                    &storage.previous_fingerprint,
                    &storage.updated_at,
                ],
            )
            .map(|_| ())
            .map_err(|_| postgres_error("postgres staged rollout save failed"))
    }

    fn load_controller_signing_staged_rollout(
        &self,
        controller_id: &str,
    ) -> Result<Option<ControllerSigningStagedRolloutRecord>, Self::Error> {
        let mut client = self.checkout_client()?;
        client
            .query_opt(
                "SELECT controller_id, state, target_ids, batch_size, max_failures,
                        ack_timeout_seconds, acknowledged_agent_ids, unavailable_agent_ids,
                        failed_agent_ids, in_flight_attempts, failure_reason_code,
                        current_fingerprint, previous_fingerprint, updated_at
                 FROM controller_signing_staged_rollout
                 WHERE controller_id = $1",
                &[&controller_id],
            )
            .map_err(|_| postgres_error("postgres staged rollout query failed"))?
            .map(|row| {
                controller_signing_staged_rollout_record_from_storage(
                    postgres_row_to_controller_signing_staged_rollout_storage(&row),
                )
            })
            .transpose()
    }
}

#[cfg(feature = "postgres")]
impl AgentCertificateLifecycleRepository for PostgresStore {
    type Error = StoreError;

    fn save_agent_certificate_lifecycle(
        &mut self,
        record: AgentCertificateLifecycleRecord,
    ) -> Result<(), Self::Error> {
        let snapshot = record.lifecycle;
        let current = agent_certificate_storage_parts(snapshot.current_certificate.as_ref());
        let next = agent_certificate_storage_parts(snapshot.next_certificate.as_ref());
        let revocation_reason = snapshot
            .revocation_reason
            .map(AgentCertificateRevocationReason::as_str);
        let updated_at = system_time_to_unix_secs(record.updated_at);
        self.checkout_client()?
            .execute(
                "INSERT INTO agent_certificate_lifecycle (
                    agent_id, state, current_serial, current_fingerprint,
                    current_not_before, current_not_after, next_serial, next_fingerprint,
                    next_not_before, next_not_after, grace_until, revocation_reason, updated_at
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                 ON CONFLICT (agent_id) DO UPDATE SET
                    state = EXCLUDED.state,
                    current_serial = EXCLUDED.current_serial,
                    current_fingerprint = EXCLUDED.current_fingerprint,
                    current_not_before = EXCLUDED.current_not_before,
                    current_not_after = EXCLUDED.current_not_after,
                    next_serial = EXCLUDED.next_serial,
                    next_fingerprint = EXCLUDED.next_fingerprint,
                    next_not_before = EXCLUDED.next_not_before,
                    next_not_after = EXCLUDED.next_not_after,
                    grace_until = EXCLUDED.grace_until,
                    revocation_reason = EXCLUDED.revocation_reason,
                    updated_at = EXCLUDED.updated_at",
                &[
                    &record.agent_id.as_str(),
                    &snapshot.state.as_str(),
                    &current.serial,
                    &current.fingerprint,
                    &current.not_before,
                    &current.not_after,
                    &next.serial,
                    &next.fingerprint,
                    &next.not_before,
                    &next.not_after,
                    &snapshot.grace_until.map(system_time_to_unix_secs),
                    &revocation_reason,
                    &updated_at,
                ],
            )
            .map(|_| ())
            .map_err(|_| postgres_error("postgres agent certificate lifecycle save failed"))
    }

    fn load_agent_certificate_lifecycle(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<AgentCertificateLifecycleRecord>, Self::Error> {
        let mut client = self.checkout_client()?;
        client
            .query_opt(
                "SELECT agent_id, state, current_serial, current_fingerprint,
                        current_not_before, current_not_after, next_serial, next_fingerprint,
                        next_not_before, next_not_after, grace_until, revocation_reason, updated_at
                 FROM agent_certificate_lifecycle
                 WHERE agent_id = $1",
                &[&agent_id.as_str()],
            )
            .map_err(|_| postgres_error("postgres agent certificate lifecycle query failed"))?
            .map(|row| {
                agent_certificate_lifecycle_record_from_storage(
                    postgres_row_to_agent_certificate_lifecycle_storage(&row),
                )
            })
            .transpose()
    }
}

#[cfg(feature = "postgres")]
impl AuditWriter for PostgresStore {
    type Error = StoreError;

    fn write(&mut self, event: AuditEvent) -> Result<(), Self::Error> {
        let (value_kind, value_text) = encode_audit_value(&event.value);
        let category = event.category.as_str();
        let actor = event.actor.as_str();
        let target = event.target.as_str();
        let occurred_at = system_time_to_unix_secs(event.occurred_at);
        self.checkout_client()?
            .execute(
                "INSERT INTO audit_events (
                    category, action, actor, target, value_kind, value_text, occurred_at
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
                &[
                    &category,
                    &event.action,
                    &actor,
                    &target,
                    &value_kind,
                    &value_text,
                    &occurred_at,
                ],
            )
            .map(|_| ())
            .map_err(|_| postgres_error("postgres audit insert failed"))
    }
}

#[cfg(feature = "postgres")]
impl AuditRepository for PostgresStore {
    fn list(&self, limit: usize) -> Result<Vec<AuditEvent>, Self::Error> {
        postgres_query_audit(self, None, limit)
    }

    fn list_by_category(
        &self,
        category: AuditCategory,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, Self::Error> {
        postgres_query_audit(self, Some(category), limit)
    }

    fn export_page(
        &self,
        category: Option<AuditCategory>,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AuditEventPageRecord>, Self::Error> {
        postgres_query_audit_page(self, category, limit, before)
    }
}

#[cfg(feature = "postgres")]
impl ApprovalRepository for PostgresStore {
    type Error = StoreError;

    fn insert_approval_request(
        &mut self,
        request: AppApprovalRequestRecord,
    ) -> Result<(), Self::Error> {
        self.checkout_client()?
            .execute(
                "INSERT INTO approval_requests (
                    id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                &[
                    &request.id,
                    &request.job_id,
                    &request.requester,
                    &request.approver,
                    &request.reason,
                    &request.status,
                    &system_time_to_unix_secs(request.expires_at),
                    &system_time_to_unix_secs(request.created_at),
                    &request.decided_at.map(system_time_to_unix_secs),
                ],
            )
            .map(|_| ())
            .map_err(|error| {
                postgres_constraint_or_context(error, "postgres approval request insert failed")
            })
    }

    fn find_approval_request(
        &self,
        approval_id: &str,
    ) -> Result<Option<AppApprovalRequestRecord>, Self::Error> {
        let mut client = self.checkout_client()?;
        client
            .query_opt(
                "SELECT id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
                 FROM approval_requests
                 WHERE id = $1",
                &[&approval_id],
            )
            .map(|row| row.map(|row| postgres_row_to_app_approval_request_record(&row)))
            .map_err(|_| postgres_error("postgres approval request query failed"))?
            .transpose()
    }

    fn find_pending_approval_for_job(
        &self,
        job_id: &str,
    ) -> Result<Option<AppApprovalRequestRecord>, Self::Error> {
        let mut client = self.checkout_client()?;
        client
            .query_opt(
                "SELECT id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
                 FROM approval_requests
                 WHERE job_id = $1 AND status = 'pending'
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
                &[&job_id],
            )
            .map(|row| row.map(|row| postgres_row_to_app_approval_request_record(&row)))
            .map_err(|_| postgres_error("postgres pending approval request query failed"))?
            .transpose()
    }

    fn list_approval_requests(
        &self,
        status: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AppApprovalRequestRecord>, Self::Error> {
        let limit = limit.clamp(1, 500) as i64;
        let mut client = self.checkout_client()?;
        let rows = client
            .query(
                "SELECT id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
                 FROM approval_requests
                 WHERE ($1::TEXT IS NULL OR status = $1)
                 ORDER BY created_at DESC, id DESC
                 LIMIT $2",
                &[&status, &limit],
            )
            .map_err(|_| postgres_error("postgres approval request list failed"))?;

        rows.into_iter()
            .map(|row| postgres_row_to_app_approval_request_record(&row))
            .collect()
    }

    fn update_approval_request(
        &mut self,
        request: AppApprovalRequestRecord,
    ) -> Result<bool, Self::Error> {
        self.checkout_client()?
            .execute(
                "UPDATE approval_requests
                 SET requester = $2,
                     approver = $3,
                     reason = $4,
                     status = $5,
                     expires_at = $6,
                     created_at = $7,
                     decided_at = $8
                 WHERE id = $1",
                &[
                    &request.id,
                    &request.requester,
                    &request.approver,
                    &request.reason,
                    &request.status,
                    &system_time_to_unix_secs(request.expires_at),
                    &system_time_to_unix_secs(request.created_at),
                    &request.decided_at.map(system_time_to_unix_secs),
                ],
            )
            .map(|changed| changed > 0)
            .map_err(|_| postgres_error("postgres approval request update failed"))
    }

    fn update_job_status_for_approval(
        &mut self,
        job_id: &str,
        status: JobStatus,
    ) -> Result<bool, Self::Error> {
        self.checkout_client()?
            .execute(
                "UPDATE jobs SET status = $2 WHERE id = $1",
                &[&job_id, &job_status_to_str(status)],
            )
            .map(|changed| changed > 0)
            .map_err(|_| postgres_error("postgres approval job status update failed"))
    }
}

#[cfg(feature = "postgres")]
impl JobRepository for PostgresStore {
    type Error = StoreError;

    fn save(&mut self, job: Job) -> Result<(), Self::Error> {
        self.checkout_client()?
            .execute(
                "INSERT INTO jobs (
                    id, status, risk, approval_requirement, timeout_ms
                 ) VALUES ($1, $2, $3, $4, $5)",
                &[
                    &job.id().as_str(),
                    &job_status_to_str(job.status()),
                    &task_risk_to_str(job.risk()),
                    &approval_requirement_to_str(job.approval_requirement()),
                    &(job.timeout().as_millis() as i64),
                ],
            )
            .map(|_| ())
            .map_err(|error| postgres_constraint_or_context(error, "postgres job insert failed"))
    }
}

#[cfg(feature = "postgres")]
impl TaskAssignmentRepository for PostgresStore {
    type Error = StoreError;

    fn save_assignment(&mut self, envelope: TaskEnvelope) -> Result<(), Self::Error> {
        let signature = envelope
            .signature
            .as_ref()
            .ok_or_else(|| StoreError::Domain("task assignment must be signed".to_owned()))?;
        let mut client = self.checkout_client()?;
        let mut transaction = client
            .transaction()
            .map_err(|_| postgres_error("postgres transaction failed"))?;
        transaction
            .execute(
                "INSERT INTO task_assignments (
                    id, job_id, agent_id, nonce, payload_hash, signature, issued_at, expires_at
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                &[
                    &envelope.task_id.as_str(),
                    &envelope.job_id.as_str(),
                    &envelope.target_agent_id.as_str(),
                    &envelope.nonce.as_str(),
                    &envelope.payload_hash,
                    &signature.as_str(),
                    &system_time_to_unix_secs(envelope.issued_at),
                    &system_time_to_unix_secs(envelope.expires_at.as_system_time()),
                ],
            )
            .map_err(|error| {
                postgres_constraint_or_context(error, "postgres task assignment insert failed")
            })?;
        transaction
            .execute(
                "INSERT INTO job_targets (
                    job_id, agent_id, status, agent_display_name, agent_status_snapshot, labels_snapshot
                 )
                 SELECT
                    $1,
                    a.id,
                    a.status,
                    a.name,
                    a.status,
                    a.labels
                 FROM agents a
                 WHERE a.id = $2
                 ON CONFLICT (job_id, agent_id) DO NOTHING",
                &[&envelope.job_id.as_str(), &envelope.target_agent_id.as_str()],
            )
            .map_err(|_| postgres_error("postgres job target snapshot insert failed"))?;
        transaction
            .commit()
            .map_err(|_| postgres_error("postgres transaction commit failed"))?;
        Ok(())
    }
}

impl LocalArtifactStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        if root.as_os_str().is_empty() {
            return Err(StoreError::Domain(
                "artifact store root cannot be empty".to_owned(),
            ));
        }
        if let Ok(metadata) = std::fs::symlink_metadata(&root)
            && metadata.file_type().is_symlink()
        {
            return Err(StoreError::Domain(
                "artifact store root cannot be a symlink".to_owned(),
            ));
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn object_path(
        &self,
        id: &ArtifactId,
        retention_class: ArtifactRetentionClass,
    ) -> Result<PathBuf, StoreError> {
        let filename = safe_artifact_filename(id)?;
        Ok(self.root.join(retention_class.as_str()).join(filename))
    }

    fn ensure_class_dir(
        &self,
        retention_class: ArtifactRetentionClass,
    ) -> Result<PathBuf, StoreError> {
        std::fs::create_dir_all(&self.root)
            .map_err(|_| artifact_io_error("artifact root create failed"))?;
        let class_dir = self.root.join(retention_class.as_str());
        if let Ok(metadata) = std::fs::symlink_metadata(&class_dir)
            && metadata.file_type().is_symlink()
        {
            return Err(StoreError::Domain(
                "artifact class directory cannot be a symlink".to_owned(),
            ));
        }
        std::fs::create_dir_all(&class_dir)
            .map_err(|_| artifact_io_error("artifact class directory create failed"))?;
        Ok(class_dir)
    }
}

impl ArtifactStore for LocalArtifactStore {
    type Error = StoreError;

    fn put(&mut self, input: ArtifactStorePut) -> Result<AppArtifactStoreRecord, Self::Error> {
        let actual = artifact_sha256(&input.bytes)?;
        if actual != input.expected_checksum {
            return Err(StoreError::Domain("artifact checksum mismatch".to_owned()));
        }

        self.ensure_class_dir(input.retention_class)?;
        let path = self.object_path(&input.id, input.retention_class)?;
        if let Ok(metadata) = std::fs::symlink_metadata(&path)
            && metadata.file_type().is_symlink()
        {
            return Err(StoreError::Domain(
                "artifact object cannot be a symlink".to_owned(),
            ));
        }
        std::fs::write(&path, &input.bytes)
            .map_err(|_| artifact_io_error("artifact write failed"))?;

        Ok(AppArtifactStoreRecord {
            id: input.id,
            retention_class: input.retention_class,
            checksum: actual,
            size_bytes: input.bytes.len() as u64,
        })
    }

    fn get(
        &self,
        id: &ArtifactId,
        retention_class: ArtifactRetentionClass,
    ) -> Result<Option<Vec<u8>>, Self::Error> {
        let path = self.object_path(id, retention_class)?;
        if !path.exists() {
            return Ok(None);
        }
        if std::fs::symlink_metadata(&path)
            .map_err(|_| artifact_io_error("artifact metadata read failed"))?
            .file_type()
            .is_symlink()
        {
            return Err(StoreError::Domain(
                "artifact object cannot be a symlink".to_owned(),
            ));
        }
        std::fs::read(&path)
            .map(Some)
            .map_err(|_| artifact_io_error("artifact read failed"))
    }

    fn verify(
        &self,
        id: &ArtifactId,
        retention_class: ArtifactRetentionClass,
        expected: &ArtifactChecksum,
    ) -> Result<ArtifactVerification, Self::Error> {
        let Some(bytes) = self.get(id, retention_class)? else {
            return Ok(ArtifactVerification::Missing);
        };
        let actual = artifact_sha256(&bytes)?;
        if &actual == expected {
            Ok(ArtifactVerification::Verified(AppArtifactStoreRecord {
                id: id.clone(),
                retention_class,
                checksum: actual,
                size_bytes: bytes.len() as u64,
            }))
        } else {
            Ok(ArtifactVerification::Corrupt {
                expected: expected.clone(),
                actual,
            })
        }
    }

    fn delete(
        &mut self,
        id: &ArtifactId,
        retention_class: ArtifactRetentionClass,
    ) -> Result<ArtifactDeleteOutcome, Self::Error> {
        let path = self.object_path(id, retention_class)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(ArtifactDeleteOutcome::Deleted),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(ArtifactDeleteOutcome::Missing)
            }
            Err(_) => Err(artifact_io_error("artifact delete failed")),
        }
    }
}

#[cfg(feature = "postgres")]
impl PostgresStore {
    pub fn find_drift_assignment_provenance(
        &self,
        task_id: &str,
    ) -> Result<Option<DriftAssignmentProvenanceRecord>, StoreError> {
        let row = self
            .checkout_client()?
            .query_opt(
                "SELECT ta.job_id, ta.id, ta.agent_id, j.drift_policy_id,
                    j.drift_policy_version, j.drift_purpose
             FROM task_assignments ta JOIN jobs j ON j.id = ta.job_id
             WHERE ta.id = $1 AND j.drift_policy_document IS NOT NULL
               AND j.drift_policy_id IS NOT NULL
               AND j.drift_policy_version IS NOT NULL
               AND j.drift_purpose IS NOT NULL",
                &[&task_id],
            )
            .map_err(|_| postgres_error("postgres drift provenance query failed"))?;
        Ok(row.and_then(|row| {
            DriftCheckPurpose::parse(&row.get::<_, String>(5)).map(|purpose| {
                DriftAssignmentProvenanceRecord {
                    job_id: row.get(0),
                    task_id: row.get(1),
                    agent_id: row.get(2),
                    policy_id: row.get(3),
                    policy_version: row.get::<_, i64>(4).max(0) as u32,
                    purpose,
                }
            })
        }))
    }

    pub fn save_agent(&mut self, agent: Agent) -> Result<(), StoreError> {
        <Self as AgentRepository>::save(self, agent)
    }

    pub fn list_agents(&self) -> Result<Vec<Agent>, StoreError> {
        <Self as AgentRepository>::list(self)
    }

    pub fn find_agent_by_id(&self, agent_id: &str) -> Result<Option<Agent>, StoreError> {
        let agent_id = AgentId::new(agent_id).map_err(StoreError::from)?;
        <Self as AgentRepository>::find_by_id(self, &agent_id)
    }

    pub fn update_agent_labels(
        &self,
        agent_id: &str,
        labels: &[AgentLabel],
    ) -> Result<bool, StoreError> {
        self.checkout_client()?
            .execute(
                "UPDATE agents
                 SET labels = $2, updated_at = EXTRACT(EPOCH FROM now())::BIGINT
                 WHERE id = $1",
                &[&agent_id, &encode_labels(labels)],
            )
            .map(|changed| changed > 0)
            .map_err(|_| postgres_error("postgres agent label update failed"))
    }

    pub fn revoke_agent_key(&self, agent_id: &str) -> Result<bool, StoreError> {
        self.checkout_client()?
            .execute(
                "UPDATE agents
                 SET status = 'disabled', updated_at = EXTRACT(EPOCH FROM now())::BIGINT
                 WHERE id = $1",
                &[&agent_id],
            )
            .map(|changed| changed > 0)
            .map_err(|_| postgres_error("postgres agent revoke failed"))
    }

    pub fn find_agent_identity(
        &self,
        agent_id: &str,
    ) -> Result<Option<(String, String)>, StoreError> {
        self.checkout_client()?
            .query_opt(
                "SELECT public_key, fingerprint
                 FROM agents
                 WHERE id = $1 AND status != 'disabled'",
                &[&agent_id],
            )
            .map(|row| row.map(|row| (row.get(0), row.get(1))))
            .map_err(|_| postgres_error("postgres agent identity query failed"))
    }

    pub fn mark_agent_online(&self, agent_id: &str, at: SystemTime) -> Result<bool, StoreError> {
        self.checkout_client()?
            .execute(
                "UPDATE agents
                 SET status = 'online', last_seen_at = $2, updated_at = EXTRACT(EPOCH FROM now())::BIGINT
                 WHERE id = $1 AND status != 'disabled'",
                &[&agent_id, &system_time_to_unix_secs(at)],
            )
            .map(|changed| changed > 0)
            .map_err(|_| postgres_error("postgres agent online update failed"))
    }

    pub fn mark_agent_degraded(&self, agent_id: &str, at: SystemTime) -> Result<bool, StoreError> {
        self.checkout_client()?
            .execute(
                "UPDATE agents
                 SET status = 'degraded', last_seen_at = $2, updated_at = EXTRACT(EPOCH FROM now())::BIGINT
                 WHERE id = $1 AND status != 'disabled'",
                &[&agent_id, &system_time_to_unix_secs(at)],
            )
            .map(|changed| changed > 0)
            .map_err(|_| postgres_error("postgres agent degraded update failed"))
    }

    pub fn mark_stale_agents_offline(
        &self,
        cutoff: SystemTime,
        now: SystemTime,
    ) -> Result<usize, StoreError> {
        self.checkout_client()?
            .execute(
                "UPDATE agents
                 SET status = 'offline', updated_at = $2
                 WHERE status IN ('online', 'busy', 'degraded')
                   AND last_seen_at IS NOT NULL
                   AND last_seen_at < $1",
                &[
                    &system_time_to_unix_secs(cutoff),
                    &system_time_to_unix_secs(now),
                ],
            )
            .map(|changed| changed as usize)
            .map_err(|_| postgres_error("postgres stale agent update failed"))
    }

    pub fn insert_agent_log_chunk(
        &self,
        agent_id: &str,
        line: &str,
        collected_at: SystemTime,
    ) -> Result<(), StoreError> {
        self.checkout_client()?
            .execute(
                "INSERT INTO agent_log_chunks (agent_id, line, collected_at)
                 VALUES ($1, $2, $3)",
                &[&agent_id, &line, &system_time_to_unix_secs(collected_at)],
            )
            .map(|_| ())
            .map_err(|_| postgres_error("postgres agent log insert failed"))
    }

    pub fn update_task_assignment_status(
        &self,
        task_id: &str,
        status: AssignmentStatus,
        occurred_at: SystemTime,
        last_error: Option<&str>,
    ) -> Result<bool, StoreError> {
        postgres_update_task_assignment_status(self, task_id, status, occurred_at, last_error)
    }

    pub fn update_active_task_assignment_status(
        &self,
        task_id: &str,
        status: AssignmentStatus,
        occurred_at: SystemTime,
        last_error: Option<&str>,
    ) -> Result<bool, StoreError> {
        postgres_update_active_task_assignment_status(
            self,
            task_id,
            status,
            occurred_at,
            last_error,
        )
    }

    pub fn find_task_assignment_status(&self, task_id: &str) -> Result<Option<String>, StoreError> {
        postgres_find_task_assignment_status(self, task_id)
    }

    pub fn find_task_assignment_job_id(&self, task_id: &str) -> Result<Option<String>, StoreError> {
        postgres_find_task_assignment_job_id(self, task_id)
    }

    pub fn list_task_assignment_summaries_for_job(
        &self,
        job_id: &str,
    ) -> Result<Vec<TaskAssignmentSummaryRecord>, StoreError> {
        let rows = self
            .checkout_client()?
            .query(
                "SELECT job_id, id, agent_id, status, last_error
                 FROM task_assignments
                 WHERE job_id = $1
                 ORDER BY created_at, id",
                &[&job_id],
            )
            .map_err(|_| postgres_error("postgres task assignment summary query failed"))?;
        Ok(rows
            .into_iter()
            .map(|row| TaskAssignmentSummaryRecord {
                job_id: row.get(0),
                task_id: row.get(1),
                agent_id: row.get(2),
                status: row.get(3),
                last_error: row.get(4),
            })
            .collect())
    }

    pub fn recompute_job_status_from_assignments(
        &self,
        job_id: &str,
    ) -> Result<Option<JobStatus>, StoreError> {
        postgres_recompute_job_status_from_assignments(self, job_id)
    }

    pub fn cancel_queued_assignments_after_max_failures(
        &self,
        job_id: &str,
        occurred_at: SystemTime,
        reason: &str,
    ) -> Result<usize, StoreError> {
        let Some(gate) = postgres_job_dispatch_gate(self, job_id)? else {
            return Ok(0);
        };
        if !matches!(gate.max_failures, Some(limit) if limit > 0 && gate.failure_count >= limit as usize)
        {
            return Ok(0);
        }
        self.checkout_client()?
            .execute(
                "UPDATE task_assignments
                 SET status = 'canceled', completed_at = $2, last_error = $3
                 WHERE job_id = $1
                   AND status = 'queued'",
                &[&job_id, &system_time_to_unix_secs(occurred_at), &reason],
            )
            .map(|changed| changed as usize)
            .map_err(|_| postgres_error("postgres max-failures cancel failed"))
    }

    pub fn update_job_status(&self, job_id: &str, status: JobStatus) -> Result<bool, StoreError> {
        postgres_update_job_status(self, job_id, status)
    }

    pub fn update_job_strategy(
        &self,
        job_id: &str,
        concurrency: u32,
        max_failures: Option<u32>,
    ) -> Result<(), StoreError> {
        self.checkout_client()?
            .execute(
                "UPDATE jobs
                 SET strategy_concurrency = $2, strategy_max_failures = $3
                 WHERE id = $1",
                &[&job_id, &(concurrency as i64), &max_failures.map(i64::from)],
            )
            .map(|_| ())
            .map_err(|_| postgres_error("postgres job strategy update failed"))
    }

    pub fn update_job_selector_snapshot(
        &self,
        job_id: &str,
        selector_kind: &str,
        selector_source: &str,
    ) -> Result<(), StoreError> {
        self.checkout_client()?
            .execute(
                "UPDATE jobs
                 SET selector_kind = $2, selector_source = $3
                 WHERE id = $1",
                &[&job_id, &selector_kind, &selector_source],
            )
            .map(|_| ())
            .map_err(|_| postgres_error("postgres job selector snapshot update failed"))
    }

    pub fn find_job_status_value(&self, job_id: &str) -> Result<Option<String>, StoreError> {
        self.checkout_client()?
            .query_opt("SELECT status FROM jobs WHERE id = $1", &[&job_id])
            .map(|row| row.map(|row| row.get(0)))
            .map_err(|_| postgres_error("postgres job status query failed"))
    }

    pub fn append_job_output_chunk_record(&self, chunk: &JobOutputChunk) -> Result<(), StoreError> {
        postgres_append_job_output_chunk(self, chunk)
    }

    pub fn save_rendered_artifact_metadata_record(
        &self,
        metadata: &RenderedArtifactMetadata,
    ) -> Result<(), StoreError> {
        postgres_save_rendered_artifact_metadata(self, metadata)
    }

    pub fn assigned_policy_ids_for_agent(&self, agent_id: &str) -> Result<Vec<String>, StoreError> {
        Ok(postgres_policies_for_agent(self, agent_id)?
            .into_iter()
            .map(|record| record.policy_id)
            .collect())
    }
}

fn sqlite_insert_task_assignment_in_connection(
    connection: &Connection,
    envelope: &TaskEnvelope,
) -> Result<(), StoreError> {
    let signature = envelope
        .signature
        .as_ref()
        .ok_or_else(|| StoreError::Domain("task assignment must be signed".to_owned()))?;
    connection.execute(
        "INSERT INTO task_assignments (
            id, job_id, agent_id, nonce, payload_hash, signature, issued_at, expires_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            envelope.task_id.as_str(),
            envelope.job_id.as_str(),
            envelope.target_agent_id.as_str(),
            envelope.nonce.as_str(),
            envelope.payload_hash.as_str(),
            signature.as_str(),
            system_time_to_unix_secs(envelope.issued_at),
            system_time_to_unix_secs(envelope.expires_at.as_system_time()),
        ],
    )?;
    connection.execute(
        "INSERT INTO job_targets (
            job_id, agent_id, status, agent_display_name, agent_status_snapshot, labels_snapshot
         )
         SELECT
            ?1,
            a.id,
            a.status,
            a.name,
            a.status,
            a.labels
         FROM agents a
         WHERE a.id = ?2
         ON CONFLICT(job_id, agent_id) DO NOTHING",
        params![envelope.job_id.as_str(), envelope.target_agent_id.as_str()],
    )?;
    Ok(())
}

#[cfg(feature = "postgres")]
impl CommandJobRepository for PostgresStore {
    fn save_command_job(
        &mut self,
        job: Job,
        task: &CommandTask,
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        let args = serde_json::to_string(task.args())
            .map_err(|error| StoreError::Domain(error.to_string()))?;
        self.checkout_client()?
            .execute(
                "INSERT INTO jobs (
                    id, status, risk, approval_requirement, timeout_ms,
                    command_program, command_args_json, command_max_output_bytes
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                &[
                    &job.id().as_str(),
                    &job_status_to_str(job.status()),
                    &task_risk_to_str(job.risk()),
                    &approval_requirement_to_str(job.approval_requirement()),
                    &(job.timeout().as_millis() as i64),
                    &task.program(),
                    &args,
                    &(task.max_output_bytes() as i64),
                ],
            )
            .map(|_| ())
            .map_err(|error| {
                postgres_constraint_or_context(error, "postgres command job insert failed")
            })
    }

    fn save_command_job_with_assignments(
        &mut self,
        job: Job,
        task: &CommandTask,
        assignments: &[TaskEnvelope],
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        let args = serde_json::to_string(task.args())
            .map_err(|error| StoreError::Domain(error.to_string()))?;
        let mut client = self.checkout_client()?;
        let mut transaction = client
            .transaction()
            .map_err(|_| postgres_error("postgres transaction failed"))?;
        transaction
            .execute(
                "INSERT INTO jobs (
                    id, status, risk, approval_requirement, timeout_ms,
                    command_program, command_args_json, command_max_output_bytes
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                &[
                    &job.id().as_str(),
                    &job_status_to_str(job.status()),
                    &task_risk_to_str(job.risk()),
                    &approval_requirement_to_str(job.approval_requirement()),
                    &(job.timeout().as_millis() as i64),
                    &task.program(),
                    &args,
                    &(task.max_output_bytes() as i64),
                ],
            )
            .map_err(|error| {
                postgres_constraint_or_context(error, "postgres command job insert failed")
            })?;
        for assignment in assignments {
            postgres_insert_task_assignment_in_transaction(&mut transaction, assignment)?;
        }
        transaction
            .commit()
            .map_err(|_| postgres_error("postgres transaction commit failed"))?;
        Ok(())
    }
}

#[cfg(feature = "postgres")]
impl DriftCheckJobRepository for PostgresStore {
    fn save_drift_check_job(
        &mut self,
        job: Job,
        task: &DriftCheckTask,
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.checkout_client()?
            .execute(
                "INSERT INTO jobs (
                    id, status, risk, approval_requirement, timeout_ms,
                    drift_policy_document
                 ) VALUES ($1, $2, $3, $4, $5, $6)",
                &[
                    &job.id().as_str(),
                    &job_status_to_str(job.status()),
                    &task_risk_to_str(job.risk()),
                    &approval_requirement_to_str(job.approval_requirement()),
                    &(job.timeout().as_millis() as i64),
                    &task.policy_document(),
                ],
            )
            .map(|_| ())
            .map_err(|error| {
                postgres_constraint_or_context(error, "postgres drift job insert failed")
            })
    }

    fn save_drift_check_job_with_assignments(
        &mut self,
        job: Job,
        task: &DriftCheckTask,
        assignments: &[TaskEnvelope],
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.save_drift_check_job_with_assignments_and_provenance(job, task, assignments, None)
    }

    fn save_drift_check_job_with_assignments_and_provenance(
        &mut self,
        job: Job,
        task: &DriftCheckTask,
        assignments: &[TaskEnvelope],
        provenance: Option<&DriftJobProvenance>,
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        let mut client = self.checkout_client()?;
        let mut transaction = client
            .transaction()
            .map_err(|_| postgres_error("postgres transaction failed"))?;
        transaction
            .execute(
                "INSERT INTO jobs (
                    id, status, risk, approval_requirement, timeout_ms,
                    drift_policy_document, drift_policy_id, drift_policy_version, drift_purpose
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                &[
                    &job.id().as_str(),
                    &job_status_to_str(job.status()),
                    &task_risk_to_str(job.risk()),
                    &approval_requirement_to_str(job.approval_requirement()),
                    &(job.timeout().as_millis() as i64),
                    &task.policy_document(),
                    &provenance.map(|value| value.policy_id.as_str()),
                    &provenance.map(|value| i64::from(value.policy_version)),
                    &provenance.map(|value| value.purpose.as_str()),
                ],
            )
            .map_err(|error| {
                postgres_constraint_or_context(error, "postgres drift job insert failed")
            })?;
        for assignment in assignments {
            postgres_insert_task_assignment_in_transaction(&mut transaction, assignment)?;
        }
        transaction
            .commit()
            .map_err(|_| postgres_error("postgres transaction commit failed"))?;
        Ok(())
    }
}

#[cfg(feature = "postgres")]
impl RunbookJobRepository for PostgresStore {
    fn save_runbook_job(
        &mut self,
        job: Job,
        task: &RunbookExecutionTask,
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.checkout_client()?
            .execute(
                "INSERT INTO jobs (
                    id, status, risk, approval_requirement, timeout_ms,
                    runbook_document
                 ) VALUES ($1, $2, $3, $4, $5, $6)",
                &[
                    &job.id().as_str(),
                    &job_status_to_str(job.status()),
                    &task_risk_to_str(job.risk()),
                    &approval_requirement_to_str(job.approval_requirement()),
                    &(job.timeout().as_millis() as i64),
                    &task.runbook_document(),
                ],
            )
            .map(|_| ())
            .map_err(|error| {
                postgres_constraint_or_context(error, "postgres runbook job insert failed")
            })
    }

    fn save_runbook_job_with_assignments(
        &mut self,
        job: Job,
        task: &RunbookExecutionTask,
        assignments: &[TaskEnvelope],
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        let mut client = self.checkout_client()?;
        let mut transaction = client
            .transaction()
            .map_err(|_| postgres_error("postgres transaction failed"))?;
        transaction
            .execute(
                "INSERT INTO jobs (
                    id, status, risk, approval_requirement, timeout_ms,
                    runbook_document
                 ) VALUES ($1, $2, $3, $4, $5, $6)",
                &[
                    &job.id().as_str(),
                    &job_status_to_str(job.status()),
                    &task_risk_to_str(job.risk()),
                    &approval_requirement_to_str(job.approval_requirement()),
                    &(job.timeout().as_millis() as i64),
                    &task.runbook_document(),
                ],
            )
            .map_err(|error| {
                postgres_constraint_or_context(error, "postgres runbook job insert failed")
            })?;
        for assignment in assignments {
            postgres_insert_task_assignment_in_transaction(&mut transaction, assignment)?;
        }
        transaction
            .commit()
            .map_err(|_| postgres_error("postgres transaction commit failed"))?;
        Ok(())
    }
}

#[cfg(feature = "postgres")]
impl DispatchAssignmentRepository for PostgresStore {
    type Error = StoreError;

    fn list_pending_assignments(
        &self,
        agent_id: Option<&AgentId>,
        job_id: Option<&JobId>,
        limit: usize,
    ) -> Result<Vec<AppPendingTaskAssignment>, Self::Error> {
        postgres_list_pending_dispatch_assignments(self, agent_id, job_id, limit)
    }

    fn find_dispatch_agent(&self, agent_id: &AgentId) -> Result<Option<Agent>, Self::Error> {
        <PostgresStore as AgentRepository>::find_by_id(self, agent_id)
    }

    fn dispatch_gate(&self, job_id: &JobId) -> Result<AppJobDispatchGate, Self::Error> {
        let gate =
            postgres_job_dispatch_gate(self, job_id.as_str())?.unwrap_or(JobDispatchGateRecord {
                concurrency: 1,
                max_failures: None,
                active_count: 0,
                failure_count: 0,
            });
        Ok(AppJobDispatchGate {
            concurrency: gate.concurrency as usize,
            max_failures: gate.max_failures,
            active_count: gate.active_count,
            failure_count: gate.failure_count,
        })
    }

    fn latest_agent_capability_snapshot(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<AgentCapabilitySnapshot>, Self::Error> {
        postgres_latest_agent_capability_snapshot(self, agent_id.as_str())
    }

    fn mark_assignment_rejected(
        &mut self,
        task_id: &TaskId,
        now: SystemTime,
        reason: &str,
    ) -> Result<(), Self::Error> {
        let job_id = postgres_find_task_assignment_job_id(self, task_id.as_str())?;
        let changed = postgres_update_active_task_assignment_status(
            self,
            task_id.as_str(),
            AssignmentStatus::Rejected,
            now,
            Some(reason),
        )?;
        if changed && let Some(job_id) = job_id {
            postgres_recompute_job_status_from_assignments(self, &job_id)?;
        }
        Ok(())
    }

    fn mark_assignment_dispatched(
        &mut self,
        task_id: &TaskId,
        now: SystemTime,
    ) -> Result<(), Self::Error> {
        postgres_update_task_assignment_status(
            self,
            task_id.as_str(),
            AssignmentStatus::Dispatched,
            now,
            None,
        )?;
        Ok(())
    }

    fn claim_assignment_for_dispatch(
        &mut self,
        task_id: &TaskId,
        now: SystemTime,
    ) -> Result<bool, Self::Error> {
        postgres_claim_task_assignment_for_dispatch(self, task_id.as_str(), now)
    }

    fn release_assignment_dispatch_claim(
        &mut self,
        task_id: &TaskId,
        _now: SystemTime,
        reason: &str,
    ) -> Result<(), Self::Error> {
        postgres_release_task_assignment_dispatch_claim(self, task_id.as_str(), reason)?;
        Ok(())
    }

    fn mark_job_running(&mut self, job_id: &JobId, _now: SystemTime) -> Result<(), Self::Error> {
        postgres_update_job_status(self, job_id.as_str(), JobStatus::Running)?;
        Ok(())
    }

    fn mark_job_expired(&mut self, job_id: &JobId, _now: SystemTime) -> Result<(), Self::Error> {
        postgres_update_job_status(self, job_id.as_str(), JobStatus::Expired)?;
        Ok(())
    }
}

#[cfg(feature = "postgres")]
impl JobOutputRepository for PostgresStore {
    type Error = StoreError;

    fn append_output_chunk(&mut self, chunk: JobOutputChunk) -> Result<(), Self::Error> {
        postgres_append_job_output_chunk(self, &chunk)
    }

    fn list_output_chunks(
        &self,
        job_id: &str,
        agent_id: &str,
    ) -> Result<Vec<JobOutputChunk>, Self::Error> {
        postgres_list_job_output_chunks(self, job_id, Some(agent_id))
    }

    fn list_output_chunks_for_job(&self, job_id: &str) -> Result<Vec<JobOutputChunk>, Self::Error> {
        postgres_list_job_output_chunks(self, job_id, None)
    }
}

#[cfg(feature = "postgres")]
impl FactsRepository for PostgresStore {
    type Error = StoreError;

    fn insert_facts_snapshot(
        &mut self,
        agent_id: &str,
        body: &str,
        collected_at: SystemTime,
    ) -> Result<(), Self::Error> {
        postgres_insert_facts_snapshot(self, agent_id, body, collected_at)
    }

    fn latest_facts_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<AppFactsSnapshotRecord>, Self::Error> {
        postgres_latest_facts_snapshot(self, agent_id)
    }

    fn list_facts_snapshots(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AppFactsSnapshotPageRecord>, Self::Error> {
        postgres_list_facts_snapshots(self, agent_id, limit, before)
    }
}

#[cfg(feature = "postgres")]
impl MetricsRepository for PostgresStore {
    type Error = StoreError;

    fn insert_metrics_snapshot(
        &mut self,
        agent_id: &str,
        body: &str,
        collected_at: SystemTime,
    ) -> Result<(), Self::Error> {
        postgres_insert_metrics_snapshot(self, agent_id, body, collected_at)
    }

    fn latest_metrics_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<AppMetricsSnapshotRecord>, Self::Error> {
        postgres_latest_metrics_snapshot(self, agent_id)
    }

    fn list_metrics_snapshots(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AppMetricsSnapshotPageRecord>, Self::Error> {
        postgres_list_metrics_snapshots(self, agent_id, limit, before)
    }
}

#[cfg(feature = "postgres")]
impl AgentLogRepository for PostgresStore {
    type Error = StoreError;

    fn list_agent_log_chunks(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AppAgentLogChunkPageRecord>, Self::Error> {
        postgres_list_agent_log_chunks(self, agent_id, limit, before)
    }
}

#[cfg(feature = "postgres")]
impl AgentCapabilityRepository for PostgresStore {
    type Error = StoreError;

    fn save_agent_capability_snapshot(
        &mut self,
        agent_id: &AgentId,
        snapshot: AgentCapabilitySnapshot,
    ) -> Result<(), Self::Error> {
        postgres_save_agent_capability_snapshot(self, agent_id.as_str(), snapshot)
    }

    fn latest_agent_capability_snapshot(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<AgentCapabilitySnapshot>, Self::Error> {
        postgres_latest_agent_capability_snapshot(self, agent_id.as_str())
    }
}

#[cfg(feature = "postgres")]
impl DriftRepository for PostgresStore {
    type Error = StoreError;

    fn insert_drift_report(
        &mut self,
        agent_id: &str,
        report: &DriftReport,
        checked_at: SystemTime,
    ) -> Result<(), Self::Error> {
        postgres_insert_drift_report(self, agent_id, report, checked_at)
    }

    fn insert_drift_report_with_provenance(
        &mut self,
        agent_id: &str,
        report: &DriftReport,
        provenance: &DriftReportProvenance,
        checked_at: SystemTime,
    ) -> Result<(), Self::Error> {
        postgres_insert_drift_report_with_provenance(self, agent_id, report, provenance, checked_at)
    }

    fn latest_drift_report(
        &self,
        agent_id: &str,
    ) -> Result<Option<AppDriftReportRecord>, Self::Error> {
        postgres_latest_drift_report(self, agent_id)
    }

    fn list_drift_reports(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AppDriftReportPageRecord>, Self::Error> {
        postgres_list_drift_reports(self, agent_id, limit, before)
    }
}

#[cfg(feature = "postgres")]
impl PolicyRepository for PostgresStore {
    type Error = StoreError;

    fn save_policy_source(
        &mut self,
        policy_id: &str,
        name: &str,
        version: u32,
        source: &str,
    ) -> Result<(), Self::Error> {
        postgres_save_policy_source(self, policy_id, name, version, source)
    }

    fn list_policies(&self) -> Result<Vec<AppPolicyRecord>, Self::Error> {
        postgres_list_policies(self)
    }

    fn find_policy(&self, policy_id: &str) -> Result<Option<AppPolicyRecord>, Self::Error> {
        postgres_find_policy(self, policy_id)
    }

    fn assign_policy_to_agent(
        &mut self,
        policy_id: &str,
        agent_id: &str,
        assigned_at: SystemTime,
    ) -> Result<(), Self::Error> {
        postgres_assign_policy_to_agent(self, policy_id, agent_id, assigned_at)
    }

    fn policies_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<AppPolicyAssignmentRecord>, Self::Error> {
        postgres_policies_for_agent(self, agent_id)
    }

    fn upsert_policy_schedule(
        &mut self,
        policy_id: &str,
        agent_id: &str,
        interval: Duration,
        next_due_at: SystemTime,
    ) -> Result<(), Self::Error> {
        postgres_upsert_policy_schedule(self, policy_id, agent_id, interval, next_due_at)
    }

    fn due_scheduled_drift_checks(
        &self,
        now: SystemTime,
        limit: usize,
    ) -> Result<Vec<AppScheduledDriftRecord>, Self::Error> {
        postgres_due_scheduled_drift_checks(self, now, limit)
    }

    fn record_scheduled_drift_check(
        &mut self,
        policy_id: &str,
        agent_id: &str,
        checked_at: SystemTime,
    ) -> Result<(), Self::Error> {
        postgres_record_scheduled_drift_check(self, policy_id, agent_id, checked_at)
    }

    fn acknowledge_latest_drift_report(
        &mut self,
        agent_id: &str,
        policy_name: &str,
        actor: &str,
        acknowledged_at: SystemTime,
    ) -> Result<bool, Self::Error> {
        postgres_acknowledge_latest_drift_report(
            self,
            agent_id,
            policy_name,
            actor,
            acknowledged_at,
        )
    }

    fn mark_latest_drift_resolved(
        &mut self,
        agent_id: &str,
        policy_name: &str,
        job_id: &str,
        resolved_at: SystemTime,
    ) -> Result<bool, Self::Error> {
        postgres_mark_latest_drift_resolved(self, agent_id, policy_name, job_id, resolved_at)
    }
}

#[cfg(feature = "postgres")]
impl JobQueryRepository for PostgresStore {
    type Error = StoreError;

    fn list_job_summaries(&self, limit: usize) -> Result<Vec<AppJobSummaryRecord>, Self::Error> {
        postgres_list_job_summaries_filtered(self, None, limit)
    }

    fn find_job_summary(&self, job_id: &str) -> Result<Option<AppJobSummaryRecord>, Self::Error> {
        Ok(postgres_list_job_summaries_filtered(self, Some(job_id), 1)?
            .into_iter()
            .next())
    }
}

#[cfg(feature = "postgres")]
impl ArtifactMetadataRepository for PostgresStore {
    type Error = StoreError;

    fn save_rendered_artifact_metadata(
        &mut self,
        metadata: RenderedArtifactMetadata,
    ) -> Result<(), Self::Error> {
        postgres_save_rendered_artifact_metadata(self, &metadata)
    }

    fn list_rendered_artifacts_for_job(
        &self,
        job_id: &JobId,
    ) -> Result<Vec<RenderedArtifactMetadata>, Self::Error> {
        postgres_list_rendered_artifacts_for_job(self, job_id.as_str())
    }
}

#[cfg(feature = "postgres")]
impl RetentionRepository for PostgresStore {
    type Error = StoreError;

    fn cleanup_retention(
        &mut self,
        cutoffs: RetentionCutoffs,
        dry_run: bool,
    ) -> Result<AppRetentionCleanupSummary, Self::Error> {
        postgres_cleanup_retention(self, cutoffs, dry_run)
    }
}

#[cfg(feature = "postgres")]
impl RemediationRequestRepository for PostgresStore {
    type Error = StoreError;

    fn save_remediation_request(
        &mut self,
        request: AppRemediationRequestRecord,
    ) -> Result<(), Self::Error> {
        postgres_save_remediation_request(self, &request)
    }

    fn find_remediation_request(
        &self,
        request_id: &str,
    ) -> Result<Option<AppRemediationRequestRecord>, Self::Error> {
        postgres_find_remediation_request(self, request_id)
    }

    fn find_remediation_request_by_job_id(
        &self,
        job_id: &str,
    ) -> Result<Option<AppRemediationRequestRecord>, Self::Error> {
        postgres_list_remediation_requests(self, None, None, 500).map(|requests| {
            requests
                .into_iter()
                .find(|request| request.job_id.as_deref() == Some(job_id))
        })
    }

    fn list_remediation_requests(
        &self,
        agent_id: Option<&str>,
        policy_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AppRemediationRequestRecord>, Self::Error> {
        postgres_list_remediation_requests(self, agent_id, policy_id, limit)
    }

    fn update_remediation_request_status(
        &mut self,
        request_id: &str,
        status: &str,
        job_id: Option<&str>,
        updated_at: SystemTime,
    ) -> Result<(), Self::Error> {
        postgres_update_remediation_request_status(self, request_id, status, job_id, updated_at)
    }
}

#[cfg(feature = "postgres")]
impl RemediationExecutionPersistenceRepository for PostgresStore {
    type Error = StoreError;

    fn persist_remediation_execution_transition(
        &mut self,
        input: AppRemediationExecutionPersistenceInput,
    ) -> Result<bool, Self::Error> {
        let mut client = self.checkout_client()?;
        let mut transaction = client
            .transaction()
            .map_err(|_| postgres_error("postgres remediation execution transaction failed"))?;
        let current_status = transaction
            .query_opt(
                "SELECT status FROM task_assignments WHERE id = $1",
                &[&input.task_id],
            )
            .map_err(|_| postgres_error("postgres remediation execution assignment lookup failed"))?
            .map(|row| row.get::<_, String>(0));
        let Some(current_status) = current_status else {
            transaction.rollback().ok();
            return Ok(false);
        };
        if assignment_status_value_is_terminal(&current_status) {
            transaction.rollback().ok();
            return Ok(false);
        }
        let occurred_at = system_time_to_unix_secs(input.occurred_at);
        let changed = match input.assignment_status.as_str() {
            "started" => transaction.execute(
                "UPDATE task_assignments SET status = $2, started_at = $3 WHERE id = $1",
                &[&input.task_id, &input.assignment_status, &occurred_at],
            ),
            "succeeded" | "failed" | "canceled" | "expired" => transaction.execute(
                "UPDATE task_assignments
                 SET status = $2, completed_at = $3, last_error = COALESCE($4, last_error)
                 WHERE id = $1",
                &[
                    &input.task_id,
                    &input.assignment_status,
                    &occurred_at,
                    &input.assignment_last_error,
                ],
            ),
            _ => {
                return Err(StoreError::Domain(
                    "unsupported remediation assignment transition".to_owned(),
                ));
            }
        }
        .map_err(|_| postgres_error("postgres remediation execution assignment update failed"))?;
        if changed == 0 {
            transaction.rollback().ok();
            return Ok(false);
        }
        if let Some(remediation) = &input.remediation {
            transaction.execute(
                "UPDATE remediation_requests SET status = $2, job_id = $3, updated_at = $4 WHERE id = $1",
                &[&remediation.id, &remediation.status, &remediation.job_id, &occurred_at],
            ).map_err(|_| postgres_error("postgres remediation execution update failed"))?;
        }
        if let Some(audit) = &input.remediation_audit {
            postgres_insert_audit_in_transaction(&mut transaction, audit)?;
        }
        transaction
            .commit()
            .map_err(|_| postgres_error("postgres remediation execution commit failed"))?;
        Ok(true)
    }
}

#[cfg(feature = "postgres")]
impl RemediationVerificationJobRepository for PostgresStore {
    fn find_remediation_verification_job(
        &self,
        remediation_id: &str,
    ) -> Result<Option<String>, <Self as RemediationRequestRepository>::Error> {
        self.find_remediation_verification_job_id(remediation_id)
    }

    fn save_remediation_verification_job(
        &mut self,
        input: AppRemediationVerificationJobPersistenceInput,
    ) -> Result<AppRemediationVerificationJobSave, <Self as RemediationRequestRepository>::Error>
    {
        let mut client = self.checkout_client()?;
        let mut transaction = client
            .transaction()
            .map_err(|_| postgres_error("postgres remediation verification transaction failed"))?;
        let remediation_exists = transaction
            .query_opt(
                "SELECT id FROM remediation_requests WHERE id = $1 FOR UPDATE",
                &[&input.remediation_id],
            )
            .map_err(|_| postgres_error("postgres remediation verification lookup failed"))?;
        if remediation_exists.is_none() {
            transaction.rollback().ok();
            return Err(StoreError::NotFound);
        }
        if let Some(existing) = transaction
            .query_opt(
                "SELECT job_id FROM remediation_verification_jobs WHERE remediation_id = $1",
                &[&input.remediation_id],
            )
            .map_err(|_| {
                postgres_error("postgres remediation verification correlation lookup failed")
            })?
            .map(|row| row.get(0))
        {
            transaction.rollback().ok();
            return Ok(AppRemediationVerificationJobSave {
                job_id: existing,
                created: false,
            });
        }
        transaction
            .execute(
                "INSERT INTO jobs (
                    id, status, risk, approval_requirement, timeout_ms,
                    drift_policy_document, drift_policy_id, drift_policy_version, drift_purpose
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                &[
                    &input.job.id().as_str(),
                    &job_status_to_str(input.job.status()),
                    &task_risk_to_str(input.job.risk()),
                    &approval_requirement_to_str(input.job.approval_requirement()),
                    &(input.job.timeout().as_millis() as i64),
                    &input.task.policy_document(),
                    &input.provenance.policy_id,
                    &i64::from(input.provenance.policy_version),
                    &input.provenance.purpose.as_str(),
                ],
            )
            .map_err(|error| {
                postgres_constraint_or_context(
                    error,
                    "postgres remediation verification job insert failed",
                )
            })?;
        postgres_insert_task_assignment_in_transaction(&mut transaction, &input.assignment)?;
        transaction
            .execute(
                "INSERT INTO remediation_verification_jobs (remediation_id, job_id, created_at)
                 VALUES ($1, $2, $3)",
                &[
                    &input.remediation_id,
                    &input.job.id().as_str(),
                    &system_time_to_unix_secs(input.audit.occurred_at),
                ],
            )
            .map_err(|error| {
                postgres_constraint_or_context(
                    error,
                    "postgres remediation verification correlation insert failed",
                )
            })?;
        postgres_insert_audit_in_transaction(&mut transaction, &input.audit)?;
        transaction
            .commit()
            .map_err(|_| postgres_error("postgres remediation verification commit failed"))?;
        Ok(AppRemediationVerificationJobSave {
            job_id: input.job.id().as_str().to_owned(),
            created: true,
        })
    }
}

#[cfg(feature = "postgres")]
impl RemediationVerificationResolutionRepository for PostgresStore {
    fn resolve_remediation_verification_evidence(
        &mut self,
        remediation: AppRemediationRequestRecord,
        origin_drift_report_id: DriftReportId,
        evidence_report_id: DriftReportId,
        verification_job_id: &str,
        verification_task_id: &str,
        audit: AuditEvent,
    ) -> Result<AppRemediationRequestRecord, <Self as RemediationRequestRepository>::Error> {
        let mut client = self.checkout_client()?;
        let mut transaction = client.transaction().map_err(|_| {
            postgres_error("postgres remediation verification resolution transaction failed")
        })?;
        let correlation = transaction
            .query_opt(
                "SELECT job_id FROM remediation_verification_jobs WHERE remediation_id = $1 FOR UPDATE",
                &[&remediation.id],
            )
            .map_err(|_| postgres_error("postgres remediation verification resolution lookup failed"))?
            .map(|row| row.get::<_, String>(0));
        if correlation.as_deref() != Some(verification_job_id) {
            transaction.rollback().ok();
            return Err(StoreError::NotFound);
        }
        let evidence_exists = transaction
            .query_opt(
                "SELECT 1 FROM drift_reports
                 WHERE id = $1 AND agent_id = $2 AND job_id = $3 AND task_id = $4
                   AND policy_id = $5 AND policy_version = $6
                   AND purpose = 'remediation_verification' AND status = 'compliant'",
                &[
                    &evidence_report_id.as_i64(),
                    &remediation.agent_id,
                    &verification_job_id,
                    &verification_task_id,
                    &remediation.policy_id,
                    &remediation.policy_version.map(i64::from),
                ],
            )
            .map_err(|_| {
                postgres_error("postgres remediation verification evidence lookup failed")
            })?;
        if evidence_exists.is_none() {
            transaction.rollback().ok();
            return Err(StoreError::NotFound);
        }
        let updated = transaction
            .execute(
                "UPDATE remediation_requests SET status = $2, job_id = $3, updated_at = $4 WHERE id = $1",
                &[
                    &remediation.id,
                    &remediation.status,
                    &remediation.job_id,
                    &system_time_to_unix_secs(remediation.updated_at),
                ],
            )
            .map_err(|_| postgres_error("postgres remediation verification resolution update failed"))?;
        if updated != 1 {
            transaction.rollback().ok();
            return Err(StoreError::NotFound);
        }
        let origin_updated = transaction
            .execute(
                "UPDATE drift_reports SET resolved_at = $2, resolution_job_id = $3 WHERE id = $1",
                &[
                    &origin_drift_report_id.as_i64(),
                    &system_time_to_unix_secs(audit.occurred_at),
                    &verification_job_id,
                ],
            )
            .map_err(|_| {
                postgres_error("postgres remediation verification origin update failed")
            })?;
        if origin_updated != 1 {
            transaction.rollback().ok();
            return Err(StoreError::NotFound);
        }
        postgres_insert_audit_in_transaction(&mut transaction, &audit)?;
        transaction.commit().map_err(|_| {
            postgres_error("postgres remediation verification resolution commit failed")
        })?;
        Ok(remediation)
    }
}

#[cfg(feature = "postgres")]
impl RemediationVerificationRecoveryRepository for PostgresStore {
    type Error = StoreError;

    fn list_pending_remediation_verification_recovery(
        &self,
        limit: usize,
    ) -> Result<Vec<AppRemediationRequestRecord>, Self::Error> {
        postgres_list_pending_remediation_verification_recovery(self, limit)
    }
}

#[cfg(feature = "postgres")]
impl RemediationProposalRepository for PostgresStore {
    type Error = StoreError;

    fn save_remediation_proposal(
        &mut self,
        remediation: AppRemediationRequestRecord,
        audit: AuditEvent,
    ) -> Result<AppRemediationProposalSave, Self::Error> {
        postgres_save_remediation_proposal(self, &remediation, &audit)
    }
}

#[cfg(feature = "postgres")]
impl VerifiedDriftProposalRepository for PostgresStore {
    type Error = StoreError;

    fn save_verified_drift_proposal(
        &mut self,
        input: AppPersistVerifiedDriftProposalInput,
    ) -> Result<AppPersistVerifiedDriftProposalOutput, Self::Error> {
        postgres_save_verified_drift_proposal(self, &input)
    }
}

impl AuditWriter for SqliteStore {
    type Error = StoreError;

    fn write(&mut self, event: AuditEvent) -> Result<(), Self::Error> {
        self.insert_audit(&event)
    }
}

impl AuditWriter for &SqliteStore {
    type Error = StoreError;

    fn write(&mut self, event: AuditEvent) -> Result<(), Self::Error> {
        self.insert_audit(&event)
    }
}

impl AuditRepository for SqliteStore {
    fn list(&self, limit: usize) -> Result<Vec<AuditEvent>, Self::Error> {
        self.query_audit(None, limit)
    }

    fn list_by_category(
        &self,
        category: AuditCategory,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, Self::Error> {
        self.query_audit(Some(category), limit)
    }

    fn export_page(
        &self,
        category: Option<AuditCategory>,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AuditEventPageRecord>, Self::Error> {
        self.query_audit_page(category, limit, before)
    }
}

impl AdminTokenRepository for SqliteStore {
    type Error = StoreError;

    fn admin_token_exists(&self) -> Result<bool, Self::Error> {
        SqliteStore::admin_token_exists(self)
    }

    fn insert_admin_token_hash(&mut self, token_hash: &str) -> Result<(), Self::Error> {
        SqliteStore::insert_admin_token_hash(self, token_hash)
    }

    fn verify_admin_token_hash(&self, token_hash: &str) -> Result<bool, Self::Error> {
        SqliteStore::verify_admin_token_hash(self, token_hash)
    }

    fn find_admin_token_record(
        &self,
        token_hash: &str,
    ) -> Result<Option<AppAdminTokenRecord>, Self::Error> {
        SqliteStore::find_admin_token_record(self, token_hash)
    }
}

impl AgentIdentityRepository for SqliteStore {
    type Error = StoreError;

    fn find_agent_identity(
        &self,
        agent_id: &str,
    ) -> Result<Option<AppAgentIdentityRecord>, Self::Error> {
        Ok(
            SqliteStore::find_agent_identity(self, agent_id)?.map(|(public_key, fingerprint)| {
                AppAgentIdentityRecord {
                    public_key,
                    fingerprint,
                }
            }),
        )
    }
}

impl ControllerIdentityRepository for SqliteStore {
    type Error = StoreError;

    fn save_controller_identity_metadata(
        &mut self,
        metadata: ControllerIdentityMetadata,
    ) -> Result<(), Self::Error> {
        self.connection.execute(
            "INSERT INTO controller_identity (
                id, public_key, public_fingerprint, private_key_path, created_at
             ) VALUES (1, ?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                public_key = excluded.public_key,
                public_fingerprint = excluded.public_fingerprint,
                private_key_path = excluded.private_key_path,
                created_at = excluded.created_at",
            params![
                metadata.public_key,
                metadata.public_fingerprint,
                metadata.private_key_path,
                system_time_to_unix_secs(metadata.created_at),
            ],
        )?;
        Ok(())
    }

    fn controller_identity_metadata(
        &self,
    ) -> Result<Option<ControllerIdentityMetadata>, Self::Error> {
        self.connection
            .query_row(
                "SELECT public_key, public_fingerprint, private_key_path, created_at
                 FROM controller_identity
                 WHERE id = 1",
                [],
                |row| {
                    Ok(ControllerIdentityMetadata {
                        public_key: row.get(0)?,
                        public_fingerprint: row.get(1)?,
                        private_key_path: row.get(2)?,
                        created_at: unix_secs_to_system_time(row.get(3)?),
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }
}

impl SigningKeyRotationRepository for SqliteStore {
    type Error = StoreError;

    fn save_signing_key_rotation(
        &mut self,
        record: SigningKeyRotationRecord,
    ) -> Result<(), Self::Error> {
        let snapshot = record.rotation.snapshot();
        self.connection.execute(
            "INSERT INTO controller_signing_key_rotation (
                controller_id, state, old_fingerprint, new_fingerprint, requested_at,
                validated_at, activated_at, old_key_verifies_until, retired_at, failed_at,
                updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(controller_id) DO UPDATE SET
                state = excluded.state,
                old_fingerprint = excluded.old_fingerprint,
                new_fingerprint = excluded.new_fingerprint,
                requested_at = excluded.requested_at,
                validated_at = excluded.validated_at,
                activated_at = excluded.activated_at,
                old_key_verifies_until = excluded.old_key_verifies_until,
                retired_at = excluded.retired_at,
                failed_at = excluded.failed_at,
                updated_at = excluded.updated_at",
            params![
                record.controller_id,
                snapshot.state.as_str(),
                snapshot.old_fingerprint.as_str(),
                snapshot
                    .new_fingerprint
                    .as_ref()
                    .map(SigningKeyFingerprint::as_str),
                snapshot.requested_at.map(system_time_to_unix_secs),
                snapshot.validated_at.map(system_time_to_unix_secs),
                snapshot.activated_at.map(system_time_to_unix_secs),
                snapshot
                    .old_key_verifies_until
                    .map(system_time_to_unix_secs),
                snapshot.retired_at.map(system_time_to_unix_secs),
                snapshot.failed_at.map(system_time_to_unix_secs),
                system_time_to_unix_secs(record.updated_at),
            ],
        )?;
        Ok(())
    }

    fn load_signing_key_rotation(
        &self,
        controller_id: &str,
    ) -> Result<Option<SigningKeyRotationRecord>, Self::Error> {
        let raw = self
            .connection
            .query_row(
                "SELECT controller_id, state, old_fingerprint, new_fingerprint, requested_at,
                        validated_at, activated_at, old_key_verifies_until, retired_at,
                        failed_at, updated_at
                 FROM controller_signing_key_rotation
                 WHERE controller_id = ?1",
                params![controller_id],
                sqlite_row_to_signing_key_rotation_storage,
            )
            .optional()
            .map_err(StoreError::from)?;
        raw.map(signing_key_rotation_record_from_storage)
            .transpose()
    }
}

impl SigningKeyRotationRepository for &SqliteStore {
    type Error = StoreError;

    fn save_signing_key_rotation(
        &mut self,
        record: SigningKeyRotationRecord,
    ) -> Result<(), Self::Error> {
        let snapshot = record.rotation.snapshot();
        self.connection.execute(
            "INSERT INTO controller_signing_key_rotation (
                controller_id, state, old_fingerprint, new_fingerprint, requested_at,
                validated_at, activated_at, old_key_verifies_until, retired_at, failed_at,
                updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(controller_id) DO UPDATE SET
                state = excluded.state,
                old_fingerprint = excluded.old_fingerprint,
                new_fingerprint = excluded.new_fingerprint,
                requested_at = excluded.requested_at,
                validated_at = excluded.validated_at,
                activated_at = excluded.activated_at,
                old_key_verifies_until = excluded.old_key_verifies_until,
                retired_at = excluded.retired_at,
                failed_at = excluded.failed_at,
                updated_at = excluded.updated_at",
            params![
                record.controller_id,
                snapshot.state.as_str(),
                snapshot.old_fingerprint.as_str(),
                snapshot
                    .new_fingerprint
                    .as_ref()
                    .map(SigningKeyFingerprint::as_str),
                snapshot.requested_at.map(system_time_to_unix_secs),
                snapshot.validated_at.map(system_time_to_unix_secs),
                snapshot.activated_at.map(system_time_to_unix_secs),
                snapshot
                    .old_key_verifies_until
                    .map(system_time_to_unix_secs),
                snapshot.retired_at.map(system_time_to_unix_secs),
                snapshot.failed_at.map(system_time_to_unix_secs),
                system_time_to_unix_secs(record.updated_at),
            ],
        )?;
        Ok(())
    }

    fn load_signing_key_rotation(
        &self,
        controller_id: &str,
    ) -> Result<Option<SigningKeyRotationRecord>, Self::Error> {
        <SqliteStore as SigningKeyRotationRepository>::load_signing_key_rotation(
            *self,
            controller_id,
        )
    }
}

impl ControllerSigningStagedRolloutRepository for SqliteStore {
    type Error = StoreError;

    fn save_controller_signing_staged_rollout(
        &mut self,
        record: ControllerSigningStagedRolloutRecord,
    ) -> Result<(), Self::Error> {
        sqlite_save_controller_signing_staged_rollout(self, record)
    }

    fn load_controller_signing_staged_rollout(
        &self,
        controller_id: &str,
    ) -> Result<Option<ControllerSigningStagedRolloutRecord>, Self::Error> {
        sqlite_load_controller_signing_staged_rollout(self, controller_id)
    }
}

impl ControllerSigningStagedRolloutRepository for &SqliteStore {
    type Error = StoreError;

    fn save_controller_signing_staged_rollout(
        &mut self,
        record: ControllerSigningStagedRolloutRecord,
    ) -> Result<(), Self::Error> {
        sqlite_save_controller_signing_staged_rollout(self, record)
    }

    fn load_controller_signing_staged_rollout(
        &self,
        controller_id: &str,
    ) -> Result<Option<ControllerSigningStagedRolloutRecord>, Self::Error> {
        sqlite_load_controller_signing_staged_rollout(self, controller_id)
    }
}

impl AgentCertificateLifecycleRepository for SqliteStore {
    type Error = StoreError;

    fn save_agent_certificate_lifecycle(
        &mut self,
        record: AgentCertificateLifecycleRecord,
    ) -> Result<(), Self::Error> {
        sqlite_save_agent_certificate_lifecycle(self, record)
    }

    fn load_agent_certificate_lifecycle(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<AgentCertificateLifecycleRecord>, Self::Error> {
        sqlite_load_agent_certificate_lifecycle(self, agent_id)
    }
}

impl AgentCertificateLifecycleRepository for &SqliteStore {
    type Error = StoreError;

    fn save_agent_certificate_lifecycle(
        &mut self,
        record: AgentCertificateLifecycleRecord,
    ) -> Result<(), Self::Error> {
        sqlite_save_agent_certificate_lifecycle(self, record)
    }

    fn load_agent_certificate_lifecycle(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<AgentCertificateLifecycleRecord>, Self::Error> {
        sqlite_load_agent_certificate_lifecycle(self, agent_id)
    }
}

impl EnrollmentTokenRepository for SqliteStore {
    type Error = StoreError;

    fn insert_enrollment_token_hash(
        &mut self,
        id: &str,
        token_hash: &str,
        default_labels: &str,
        expires_at: SystemTime,
        max_uses: u32,
    ) -> Result<(), Self::Error> {
        SqliteStore::insert_enrollment_token_hash(
            self,
            id,
            token_hash,
            default_labels,
            expires_at,
            max_uses,
        )
    }

    fn list_enrollment_tokens(&self) -> Result<Vec<AppEnrollmentTokenRecord>, Self::Error> {
        Ok(SqliteStore::list_enrollment_tokens(self)?
            .into_iter()
            .map(|record| AppEnrollmentTokenRecord {
                id: record.id,
                default_labels: record.default_labels,
                expires_at: record.expires_at,
                max_uses: record.max_uses,
                used_count: record.used_count,
                revoked: record.revoked,
            })
            .collect())
    }

    fn revoke_enrollment_token(&mut self, id: &str) -> Result<bool, Self::Error> {
        SqliteStore::revoke_enrollment_token(self, id)
    }

    fn consume_enrollment_token_hash(
        &mut self,
        token_hash: &str,
        now: SystemTime,
    ) -> Result<AppEnrollmentTokenRecord, Self::Error> {
        let record = SqliteStore::consume_enrollment_token_hash(self, token_hash, now)?;
        Ok(AppEnrollmentTokenRecord {
            id: record.id,
            default_labels: record.default_labels,
            expires_at: record.expires_at,
            max_uses: record.max_uses,
            used_count: record.used_count,
            revoked: record.revoked,
        })
    }
}

impl ApprovalRepository for SqliteStore {
    type Error = StoreError;

    fn insert_approval_request(
        &mut self,
        request: AppApprovalRequestRecord,
    ) -> Result<(), Self::Error> {
        self.connection.execute(
            "INSERT INTO approval_requests (
                id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                request.id,
                request.job_id,
                request.requester,
                request.approver,
                request.reason,
                request.status,
                system_time_to_unix_secs(request.expires_at),
                system_time_to_unix_secs(request.created_at),
                request.decided_at.map(system_time_to_unix_secs),
            ],
        )?;
        Ok(())
    }

    fn find_approval_request(
        &self,
        approval_id: &str,
    ) -> Result<Option<AppApprovalRequestRecord>, Self::Error> {
        self.connection
            .query_row(
                "SELECT id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
                 FROM approval_requests
                 WHERE id = ?1",
                params![approval_id],
                row_to_app_approval_request_record,
            )
            .optional()
            .map_err(StoreError::from)
    }

    fn find_pending_approval_for_job(
        &self,
        job_id: &str,
    ) -> Result<Option<AppApprovalRequestRecord>, Self::Error> {
        self.connection
            .query_row(
                "SELECT id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
                 FROM approval_requests
                 WHERE job_id = ?1 AND status = 'pending'
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
                params![job_id],
                row_to_app_approval_request_record,
            )
            .optional()
            .map_err(StoreError::from)
    }

    fn list_approval_requests(
        &self,
        status: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AppApprovalRequestRecord>, Self::Error> {
        let limit = limit.clamp(1, 500) as i64;
        let mut statement = self.connection.prepare(
            "SELECT id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
             FROM approval_requests
             WHERE (?1 IS NULL OR status = ?1)
             ORDER BY created_at DESC, id DESC
             LIMIT ?2",
        )?;
        let rows = statement
            .query_map(params![status, limit], row_to_app_approval_request_record)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn update_approval_request(
        &mut self,
        request: AppApprovalRequestRecord,
    ) -> Result<bool, Self::Error> {
        let changed = self.connection.execute(
            "UPDATE approval_requests
             SET requester = ?2,
                 approver = ?3,
                 reason = ?4,
                 status = ?5,
                 expires_at = ?6,
                 created_at = ?7,
                 decided_at = ?8
             WHERE id = ?1",
            params![
                request.id,
                request.requester,
                request.approver,
                request.reason,
                request.status,
                system_time_to_unix_secs(request.expires_at),
                system_time_to_unix_secs(request.created_at),
                request.decided_at.map(system_time_to_unix_secs),
            ],
        )?;
        Ok(changed > 0)
    }

    fn update_job_status_for_approval(
        &mut self,
        job_id: &str,
        status: JobStatus,
    ) -> Result<bool, Self::Error> {
        self.update_job_status(job_id, status)
    }
}

impl JobRepository for SqliteStore {
    type Error = StoreError;

    fn save(&mut self, job: Job) -> Result<(), Self::Error> {
        self.save_job_record(&job)
    }
}

impl TaskAssignmentRepository for SqliteStore {
    type Error = StoreError;

    fn save_assignment(&mut self, envelope: TaskEnvelope) -> Result<(), Self::Error> {
        self.save_task_assignment_record(&envelope)
    }
}

impl DispatchAssignmentRepository for SqliteStore {
    type Error = StoreError;

    fn list_pending_assignments(
        &self,
        agent_id: Option<&AgentId>,
        job_id: Option<&JobId>,
        limit: usize,
    ) -> Result<Vec<AppPendingTaskAssignment>, Self::Error> {
        self.list_pending_dispatch_assignments(agent_id, job_id, limit)
    }

    fn find_dispatch_agent(&self, agent_id: &AgentId) -> Result<Option<Agent>, Self::Error> {
        self.find_agent_by_id(agent_id.as_str())
    }

    fn dispatch_gate(&self, job_id: &JobId) -> Result<AppJobDispatchGate, Self::Error> {
        let gate = self
            .job_dispatch_gate(job_id.as_str())?
            .unwrap_or(JobDispatchGateRecord {
                concurrency: 1,
                max_failures: None,
                active_count: 0,
                failure_count: 0,
            });
        Ok(AppJobDispatchGate {
            concurrency: gate.concurrency as usize,
            max_failures: gate.max_failures,
            active_count: gate.active_count,
            failure_count: gate.failure_count,
        })
    }

    fn latest_agent_capability_snapshot(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<AgentCapabilitySnapshot>, Self::Error> {
        SqliteStore::latest_agent_capability_snapshot(self, agent_id.as_str())
    }

    fn mark_assignment_rejected(
        &mut self,
        task_id: &TaskId,
        now: SystemTime,
        reason: &str,
    ) -> Result<(), Self::Error> {
        let job_id = self.find_task_assignment_job_id(task_id.as_str())?;
        let changed = self.update_active_task_assignment_status(
            task_id.as_str(),
            AssignmentStatus::Rejected,
            now,
            Some(reason),
        )?;
        if changed && let Some(job_id) = job_id {
            self.recompute_job_status_from_assignments(&job_id)?;
        }
        Ok(())
    }

    fn mark_assignment_dispatched(
        &mut self,
        task_id: &TaskId,
        now: SystemTime,
    ) -> Result<(), Self::Error> {
        self.update_task_assignment_status(
            task_id.as_str(),
            AssignmentStatus::Dispatched,
            now,
            None,
        )?;
        Ok(())
    }

    fn claim_assignment_for_dispatch(
        &mut self,
        task_id: &TaskId,
        now: SystemTime,
    ) -> Result<bool, Self::Error> {
        self.claim_task_assignment_for_dispatch(task_id.as_str(), now)
    }

    fn release_assignment_dispatch_claim(
        &mut self,
        task_id: &TaskId,
        _now: SystemTime,
        reason: &str,
    ) -> Result<(), Self::Error> {
        self.release_task_assignment_dispatch_claim(task_id.as_str(), reason)?;
        Ok(())
    }

    fn mark_job_running(&mut self, job_id: &JobId, _now: SystemTime) -> Result<(), Self::Error> {
        self.update_job_status(job_id.as_str(), JobStatus::Running)?;
        Ok(())
    }

    fn mark_job_expired(&mut self, job_id: &JobId, _now: SystemTime) -> Result<(), Self::Error> {
        self.update_job_status(job_id.as_str(), JobStatus::Expired)?;
        Ok(())
    }
}

impl AgentCapabilityRepository for SqliteStore {
    type Error = StoreError;

    fn save_agent_capability_snapshot(
        &mut self,
        agent_id: &AgentId,
        snapshot: AgentCapabilitySnapshot,
    ) -> Result<(), Self::Error> {
        SqliteStore::save_agent_capability_snapshot(self, agent_id.as_str(), snapshot)
    }

    fn latest_agent_capability_snapshot(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<AgentCapabilitySnapshot>, Self::Error> {
        SqliteStore::latest_agent_capability_snapshot(self, agent_id.as_str())
    }
}

impl CommandJobRepository for SqliteStore {
    fn save_command_job(
        &mut self,
        job: Job,
        task: &CommandTask,
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.save_command_job_record(&job, task)
    }

    fn save_command_job_with_assignments(
        &mut self,
        job: Job,
        task: &CommandTask,
        assignments: &[TaskEnvelope],
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.save_command_job_with_assignments_record(&job, task, assignments)
    }
}

impl DriftCheckJobRepository for SqliteStore {
    fn save_drift_check_job(
        &mut self,
        job: Job,
        task: &DriftCheckTask,
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.save_drift_check_job_record(&job, task)
    }

    fn save_drift_check_job_with_assignments(
        &mut self,
        job: Job,
        task: &DriftCheckTask,
        assignments: &[TaskEnvelope],
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.save_drift_check_job_with_assignments_record(&job, task, assignments)
    }
}

impl RunbookJobRepository for SqliteStore {
    fn save_runbook_job(
        &mut self,
        job: Job,
        task: &RunbookExecutionTask,
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.save_runbook_job_record(&job, task)
    }

    fn save_runbook_job_with_assignments(
        &mut self,
        job: Job,
        task: &RunbookExecutionTask,
        assignments: &[TaskEnvelope],
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.save_runbook_job_with_assignments_record(&job, task, assignments)
    }
}

impl JobOutputRepository for SqliteStore {
    type Error = StoreError;

    fn append_output_chunk(&mut self, chunk: JobOutputChunk) -> Result<(), Self::Error> {
        self.append_job_output_chunk_record(&chunk)
    }

    fn list_output_chunks(
        &self,
        job_id: &str,
        agent_id: &str,
    ) -> Result<Vec<JobOutputChunk>, Self::Error> {
        self.list_job_output_chunks(job_id, agent_id)
    }

    fn list_output_chunks_for_job(&self, job_id: &str) -> Result<Vec<JobOutputChunk>, Self::Error> {
        self.list_job_output_chunks_for_job(job_id)
    }
}

impl ArtifactMetadataRepository for SqliteStore {
    type Error = StoreError;

    fn save_rendered_artifact_metadata(
        &mut self,
        metadata: RenderedArtifactMetadata,
    ) -> Result<(), Self::Error> {
        self.save_rendered_artifact_metadata_record(&metadata)
    }

    fn list_rendered_artifacts_for_job(
        &self,
        job_id: &JobId,
    ) -> Result<Vec<RenderedArtifactMetadata>, Self::Error> {
        self.list_rendered_artifact_metadata_for_job(job_id.as_str())
    }
}

impl RemediationRequestRepository for SqliteStore {
    type Error = StoreError;

    fn save_remediation_request(
        &mut self,
        request: AppRemediationRequestRecord,
    ) -> Result<(), Self::Error> {
        self.save_remediation_request_record(&request)
    }

    fn find_remediation_request(
        &self,
        request_id: &str,
    ) -> Result<Option<AppRemediationRequestRecord>, Self::Error> {
        self.find_remediation_request_record(request_id)
    }

    fn find_remediation_request_by_job_id(
        &self,
        job_id: &str,
    ) -> Result<Option<AppRemediationRequestRecord>, Self::Error> {
        self.find_remediation_request_by_job_id_record(job_id)
    }

    fn list_remediation_requests(
        &self,
        agent_id: Option<&str>,
        policy_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AppRemediationRequestRecord>, Self::Error> {
        self.list_remediation_request_records(agent_id, policy_id, limit)
    }

    fn update_remediation_request_status(
        &mut self,
        request_id: &str,
        status: &str,
        job_id: Option<&str>,
        updated_at: SystemTime,
    ) -> Result<(), Self::Error> {
        self.update_remediation_request_status_record(request_id, status, job_id, updated_at)
    }
}

impl RemediationProposalRepository for SqliteStore {
    type Error = StoreError;

    fn save_remediation_proposal(
        &mut self,
        remediation: AppRemediationRequestRecord,
        audit: AuditEvent,
    ) -> Result<AppRemediationProposalSave, Self::Error> {
        self.save_remediation_proposal_record(&remediation, &audit)
    }
}

impl RemediationExecutionPersistenceRepository for SqliteStore {
    type Error = StoreError;

    fn persist_remediation_execution_transition(
        &mut self,
        input: AppRemediationExecutionPersistenceInput,
    ) -> Result<bool, Self::Error> {
        self.persist_remediation_execution_transition_record(&input)
    }
}

impl RemediationVerificationJobRepository for SqliteStore {
    fn find_remediation_verification_job(
        &self,
        remediation_id: &str,
    ) -> Result<Option<String>, <Self as RemediationRequestRepository>::Error> {
        self.find_remediation_verification_job_id(remediation_id)
    }

    fn save_remediation_verification_job(
        &mut self,
        input: AppRemediationVerificationJobPersistenceInput,
    ) -> Result<AppRemediationVerificationJobSave, <Self as RemediationRequestRepository>::Error>
    {
        self.save_remediation_verification_job_record(&input)
    }
}

impl RemediationVerificationResolutionRepository for SqliteStore {
    fn resolve_remediation_verification_evidence(
        &mut self,
        remediation: AppRemediationRequestRecord,
        origin_drift_report_id: DriftReportId,
        evidence_report_id: DriftReportId,
        verification_job_id: &str,
        verification_task_id: &str,
        audit: AuditEvent,
    ) -> Result<AppRemediationRequestRecord, <Self as RemediationRequestRepository>::Error> {
        self.resolve_remediation_verification_evidence_record(
            &remediation,
            &origin_drift_report_id,
            &evidence_report_id,
            verification_job_id,
            verification_task_id,
            &audit,
        )
    }
}

impl RemediationVerificationRecoveryRepository for SqliteStore {
    type Error = StoreError;

    fn list_pending_remediation_verification_recovery(
        &self,
        limit: usize,
    ) -> Result<Vec<AppRemediationRequestRecord>, Self::Error> {
        self.list_pending_remediation_verification_recovery_records(limit)
    }
}

impl VerifiedDriftProposalRepository for SqliteStore {
    type Error = StoreError;

    fn save_verified_drift_proposal(
        &mut self,
        input: AppPersistVerifiedDriftProposalInput,
    ) -> Result<AppPersistVerifiedDriftProposalOutput, Self::Error> {
        self.save_verified_drift_proposal_record(&input)
    }
}

impl JobQueryRepository for SqliteStore {
    type Error = StoreError;

    fn list_job_summaries(&self, limit: usize) -> Result<Vec<AppJobSummaryRecord>, Self::Error> {
        Ok(SqliteStore::list_job_summaries(self, limit)?
            .into_iter()
            .map(|record| AppJobSummaryRecord {
                id: record.id,
                status: record.status,
                risk: record.risk,
                command_program: record.command_program,
                command_args: record.command_args,
                selector_kind: record.selector_kind,
                selector_source: record.selector_source,
                strategy_concurrency: record.strategy_concurrency,
                strategy_max_failures: record.strategy_max_failures,
                target_count: record.target_count,
                target_agents: record
                    .target_agents
                    .into_iter()
                    .map(|target| AppJobTargetSummaryRecord {
                        agent_id: target.agent_id,
                        agent_name: target.agent_name,
                        status: target.status,
                        labels: target.labels,
                        task_id: target.task_id,
                        assignment_status: target.assignment_status,
                        last_error: target.last_error,
                    })
                    .collect(),
                created_at: record.created_at,
                expires_at: record.expires_at,
            })
            .collect())
    }

    fn find_job_summary(&self, job_id: &str) -> Result<Option<AppJobSummaryRecord>, Self::Error> {
        Ok(
            SqliteStore::find_job_summary(self, job_id)?.map(|record| AppJobSummaryRecord {
                id: record.id,
                status: record.status,
                risk: record.risk,
                command_program: record.command_program,
                command_args: record.command_args,
                selector_kind: record.selector_kind,
                selector_source: record.selector_source,
                strategy_concurrency: record.strategy_concurrency,
                strategy_max_failures: record.strategy_max_failures,
                target_count: record.target_count,
                target_agents: record
                    .target_agents
                    .into_iter()
                    .map(|target| AppJobTargetSummaryRecord {
                        agent_id: target.agent_id,
                        agent_name: target.agent_name,
                        status: target.status,
                        labels: target.labels,
                        task_id: target.task_id,
                        assignment_status: target.assignment_status,
                        last_error: target.last_error,
                    })
                    .collect(),
                created_at: record.created_at,
                expires_at: record.expires_at,
            }),
        )
    }
}

impl FactsRepository for SqliteStore {
    type Error = StoreError;

    fn insert_facts_snapshot(
        &mut self,
        agent_id: &str,
        body: &str,
        collected_at: SystemTime,
    ) -> Result<(), Self::Error> {
        SqliteStore::insert_facts_snapshot(self, agent_id, body, collected_at)
    }

    fn latest_facts_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<AppFactsSnapshotRecord>, Self::Error> {
        Ok(
            SqliteStore::latest_facts_snapshot(self, agent_id)?.map(|record| {
                AppFactsSnapshotRecord {
                    agent_id: record.agent_id,
                    body: record.body,
                    collected_at: record.collected_at,
                }
            }),
        )
    }

    fn list_facts_snapshots(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AppFactsSnapshotPageRecord>, Self::Error> {
        Ok(
            SqliteStore::list_facts_snapshots(self, agent_id, limit, before)?
                .into_iter()
                .map(|record| AppFactsSnapshotPageRecord {
                    agent_id: record.agent_id,
                    body: record.body,
                    collected_at: record.collected_at,
                    cursor: record.cursor,
                })
                .collect(),
        )
    }
}

impl MetricsRepository for SqliteStore {
    type Error = StoreError;

    fn insert_metrics_snapshot(
        &mut self,
        agent_id: &str,
        body: &str,
        collected_at: SystemTime,
    ) -> Result<(), Self::Error> {
        SqliteStore::insert_metrics_snapshot(self, agent_id, body, collected_at)
    }

    fn latest_metrics_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<AppMetricsSnapshotRecord>, Self::Error> {
        Ok(
            SqliteStore::latest_metrics_snapshot(self, agent_id)?.map(|record| {
                AppMetricsSnapshotRecord {
                    agent_id: record.agent_id,
                    body: record.body,
                    collected_at: record.collected_at,
                }
            }),
        )
    }

    fn list_metrics_snapshots(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AppMetricsSnapshotPageRecord>, Self::Error> {
        Ok(
            SqliteStore::list_metrics_snapshots(self, agent_id, limit, before)?
                .into_iter()
                .map(|record| AppMetricsSnapshotPageRecord {
                    agent_id: record.agent_id,
                    body: record.body,
                    collected_at: record.collected_at,
                    cursor: record.cursor,
                })
                .collect(),
        )
    }
}

impl AgentLogRepository for SqliteStore {
    type Error = StoreError;

    fn list_agent_log_chunks(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AppAgentLogChunkPageRecord>, Self::Error> {
        Ok(
            SqliteStore::list_agent_log_chunks_page(self, agent_id, limit, before)?
                .into_iter()
                .map(|record| AppAgentLogChunkPageRecord {
                    agent_id: record.agent_id,
                    line: record.line,
                    collected_at: record.collected_at,
                    cursor: record.cursor,
                })
                .collect(),
        )
    }
}

impl RetentionRepository for SqliteStore {
    type Error = StoreError;

    fn cleanup_retention(
        &mut self,
        cutoffs: RetentionCutoffs,
        dry_run: bool,
    ) -> Result<AppRetentionCleanupSummary, Self::Error> {
        let summary = SqliteStore::cleanup_retention_with_cutoffs(self, cutoffs, dry_run)?;
        Ok(AppRetentionCleanupSummary {
            job_output_chunks: summary.job_output_chunks,
            facts_snapshots: summary.facts_snapshots,
            metrics_snapshots: summary.metrics_snapshots,
            agent_log_chunks: summary.agent_log_chunks,
        })
    }
}

impl RetentionRepository for &SqliteStore {
    type Error = StoreError;

    fn cleanup_retention(
        &mut self,
        cutoffs: RetentionCutoffs,
        dry_run: bool,
    ) -> Result<AppRetentionCleanupSummary, Self::Error> {
        let summary = SqliteStore::cleanup_retention_with_cutoffs(self, cutoffs, dry_run)?;
        Ok(AppRetentionCleanupSummary {
            job_output_chunks: summary.job_output_chunks,
            facts_snapshots: summary.facts_snapshots,
            metrics_snapshots: summary.metrics_snapshots,
            agent_log_chunks: summary.agent_log_chunks,
        })
    }
}

impl DriftRepository for SqliteStore {
    type Error = StoreError;

    fn insert_drift_report(
        &mut self,
        agent_id: &str,
        report: &DriftReport,
        checked_at: SystemTime,
    ) -> Result<(), Self::Error> {
        SqliteStore::insert_drift_report(self, agent_id, report, checked_at)
    }

    fn insert_drift_report_with_provenance(
        &mut self,
        agent_id: &str,
        report: &DriftReport,
        provenance: &DriftReportProvenance,
        checked_at: SystemTime,
    ) -> Result<(), Self::Error> {
        SqliteStore::insert_drift_report_with_provenance(
            self, agent_id, report, provenance, checked_at,
        )
    }

    fn latest_drift_report(
        &self,
        agent_id: &str,
    ) -> Result<Option<AppDriftReportRecord>, Self::Error> {
        Ok(
            SqliteStore::latest_drift_report(self, agent_id)?.map(|record| AppDriftReportRecord {
                id: record.id,
                agent_id: record.agent_id,
                report: record.report,
                provenance: record.provenance,
                checked_at: record.checked_at,
            }),
        )
    }

    fn list_drift_reports(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AppDriftReportPageRecord>, Self::Error> {
        Ok(
            SqliteStore::list_drift_reports(self, agent_id, limit, before)?
                .into_iter()
                .map(|record| AppDriftReportPageRecord {
                    id: record.id,
                    agent_id: record.agent_id,
                    report: record.report,
                    provenance: record.provenance,
                    checked_at: record.checked_at,
                    cursor: record.cursor,
                })
                .collect(),
        )
    }
}

impl PolicyRepository for SqliteStore {
    type Error = StoreError;

    fn save_policy_source(
        &mut self,
        policy_id: &str,
        name: &str,
        version: u32,
        source: &str,
    ) -> Result<(), Self::Error> {
        SqliteStore::save_policy_source(self, policy_id, name, version, source)
    }

    fn list_policies(&self) -> Result<Vec<AppPolicyRecord>, Self::Error> {
        Ok(SqliteStore::list_policies(self)?
            .into_iter()
            .map(|record| AppPolicyRecord {
                id: record.id,
                name: record.name,
                version: record.version,
                source: record.source,
                created_at: record.created_at,
                updated_at: record.updated_at,
            })
            .collect())
    }

    fn find_policy(&self, policy_id: &str) -> Result<Option<AppPolicyRecord>, Self::Error> {
        Ok(
            SqliteStore::find_policy(self, policy_id)?.map(|record| AppPolicyRecord {
                id: record.id,
                name: record.name,
                version: record.version,
                source: record.source,
                created_at: record.created_at,
                updated_at: record.updated_at,
            }),
        )
    }

    fn assign_policy_to_agent(
        &mut self,
        policy_id: &str,
        agent_id: &str,
        assigned_at: SystemTime,
    ) -> Result<(), Self::Error> {
        SqliteStore::assign_policy_to_agent(self, policy_id, agent_id, assigned_at)
    }

    fn policies_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<AppPolicyAssignmentRecord>, Self::Error> {
        Ok(SqliteStore::policies_for_agent(self, agent_id)?
            .into_iter()
            .map(|record| AppPolicyAssignmentRecord {
                policy_id: record.policy_id,
                agent_id: record.agent_id,
                assigned_at: record.assigned_at,
            })
            .collect())
    }

    fn upsert_policy_schedule(
        &mut self,
        policy_id: &str,
        agent_id: &str,
        interval: Duration,
        next_due_at: SystemTime,
    ) -> Result<(), Self::Error> {
        SqliteStore::upsert_policy_schedule(self, policy_id, agent_id, interval, next_due_at)
    }

    fn due_scheduled_drift_checks(
        &self,
        now: SystemTime,
        limit: usize,
    ) -> Result<Vec<AppScheduledDriftRecord>, Self::Error> {
        Ok(SqliteStore::due_scheduled_drift_checks(self, now, limit)?
            .into_iter()
            .map(|record| AppScheduledDriftRecord {
                policy_id: record.policy_id,
                agent_id: record.agent_id,
                interval_seconds: record.interval_seconds,
                next_due_at: record.next_due_at,
                last_checked_at: record.last_checked_at,
            })
            .collect())
    }

    fn record_scheduled_drift_check(
        &mut self,
        policy_id: &str,
        agent_id: &str,
        checked_at: SystemTime,
    ) -> Result<(), Self::Error> {
        SqliteStore::record_scheduled_drift_check(self, policy_id, agent_id, checked_at)
    }

    fn acknowledge_latest_drift_report(
        &mut self,
        agent_id: &str,
        policy_name: &str,
        actor: &str,
        acknowledged_at: SystemTime,
    ) -> Result<bool, Self::Error> {
        SqliteStore::acknowledge_latest_drift_report(
            self,
            agent_id,
            policy_name,
            actor,
            acknowledged_at,
        )
    }

    fn mark_latest_drift_resolved(
        &mut self,
        agent_id: &str,
        policy_name: &str,
        job_id: &str,
        resolved_at: SystemTime,
    ) -> Result<bool, Self::Error> {
        SqliteStore::mark_latest_drift_resolved(self, agent_id, policy_name, job_id, resolved_at)
    }
}

fn row_to_facts_snapshot_page_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<FactsSnapshotPageRecord> {
    let id = row.get(0)?;
    let collected_at = unix_secs_to_system_time(row.get(3)?);
    Ok(FactsSnapshotPageRecord {
        agent_id: row.get(1)?,
        body: row.get(2)?,
        collected_at,
        cursor: SnapshotPageCursor {
            occurred_at: collected_at,
            row_id: id,
        },
    })
}

fn row_to_metrics_snapshot_page_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<MetricsSnapshotPageRecord> {
    let id = row.get(0)?;
    let collected_at = unix_secs_to_system_time(row.get(3)?);
    Ok(MetricsSnapshotPageRecord {
        agent_id: row.get(1)?,
        body: row.get(2)?,
        collected_at,
        cursor: SnapshotPageCursor {
            occurred_at: collected_at,
            row_id: id,
        },
    })
}

fn row_to_agent_log_chunk_page_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<AgentLogChunkPageRecord> {
    let id = row.get(0)?;
    let collected_at = unix_secs_to_system_time(row.get(3)?);
    Ok(AgentLogChunkPageRecord {
        agent_id: row.get(1)?,
        line: row.get(2)?,
        collected_at,
        cursor: SnapshotPageCursor {
            occurred_at: collected_at,
            row_id: id,
        },
    })
}

fn row_to_drift_report_page_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<DriftReportPageRecord> {
    let id = row.get(0)?;
    let checked_at = unix_secs_to_system_time(row.get(6)?);
    Ok(DriftReportPageRecord {
        id: DriftReportId::new(id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        agent_id: row.get(1)?,
        report: DriftReport {
            policy_name: row.get(2)?,
            status: parse_drift_status(&row.get::<_, String>(3)?),
            severity: parse_drift_severity(&row.get::<_, String>(7)?),
            acknowledgement: row_to_drift_acknowledgement(row, 8, 9, 10, 11)?,
            expected: row.get(4)?,
            actual: row.get(5)?,
        },
        provenance: row_to_drift_report_provenance(row, 12, 13, 14, 15, 16)?,
        checked_at,
        cursor: SnapshotPageCursor {
            occurred_at: checked_at,
            row_id: id,
        },
    })
}

fn row_to_drift_report_provenance(
    row: &rusqlite::Row<'_>,
    job_id_index: usize,
    task_id_index: usize,
    policy_id_index: usize,
    policy_version_index: usize,
    purpose_index: usize,
) -> rusqlite::Result<DriftReportProvenance> {
    let job_id = row.get::<_, Option<String>>(job_id_index)?;
    let task_id = row.get::<_, Option<String>>(task_id_index)?;
    let policy_id = row.get::<_, Option<String>>(policy_id_index)?;
    let policy_version = row.get::<_, Option<i64>>(policy_version_index)?;
    let purpose = row.get::<_, Option<String>>(purpose_index)?;

    let (Some(job_id), Some(task_id), Some(policy_id), Some(policy_version), Some(purpose)) =
        (job_id, task_id, policy_id, policy_version, purpose)
    else {
        return Ok(DriftReportProvenance::uncorrelated());
    };
    let (Ok(job_id), Ok(task_id), Some(purpose)) = (
        JobId::new(job_id),
        TaskId::new(task_id),
        DriftCheckPurpose::parse(&purpose),
    ) else {
        return Ok(DriftReportProvenance::uncorrelated());
    };
    if policy_version < 0 {
        return Ok(DriftReportProvenance::uncorrelated());
    }

    Ok(DriftReportProvenance::verified(
        job_id,
        task_id,
        policy_id,
        policy_version as u32,
        purpose,
    ))
}

fn map_drift_report_constraint(error: rusqlite::Error) -> StoreError {
    if matches!(
        &error,
        rusqlite::Error::SqliteFailure(code, _)
            if code.code == ErrorCode::ConstraintViolation
    ) {
        return StoreError::ConstraintViolation(
            "drift report correlation must be unique".to_owned(),
        );
    }
    StoreError::Sqlite(error)
}

fn row_to_policy_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<PolicyRecord> {
    Ok(PolicyRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        version: row.get::<_, i64>(2)?.max(0) as u32,
        source: row.get(3)?,
        created_at: unix_secs_to_system_time(row.get(4)?),
        updated_at: unix_secs_to_system_time(row.get(5)?),
    })
}

fn row_to_policy_assignment_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<PolicyAssignmentRecord> {
    Ok(PolicyAssignmentRecord {
        policy_id: row.get(0)?,
        agent_id: row.get(1)?,
        assigned_at: unix_secs_to_system_time(row.get(2)?),
    })
}

fn row_to_scheduled_drift_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ScheduledDriftRecord> {
    Ok(ScheduledDriftRecord {
        policy_id: row.get(0)?,
        agent_id: row.get(1)?,
        interval_seconds: row.get::<_, i64>(2)?.max(0) as u64,
        next_due_at: unix_secs_to_system_time(row.get(3)?),
        last_checked_at: row.get::<_, Option<i64>>(4)?.map(unix_secs_to_system_time),
    })
}

fn row_to_rendered_artifact_metadata(
    row: &rusqlite::Row<'_>,
) -> Result<RenderedArtifactMetadata, StoreError> {
    let id: String = row.get(0)?;
    let job_id: String = row.get(1)?;
    let agent_id: String = row.get(2)?;
    let task_id: String = row.get(3)?;
    let destination: String = row.get(4)?;
    let checksum: String = row.get(5)?;
    let size_bytes: i64 = row.get(6)?;
    let retention_class: String = row.get(7)?;
    let created_at: i64 = row.get(8)?;
    RenderedArtifactMetadata::new(
        ArtifactId::new(id).map_err(|error| StoreError::Domain(error.to_string()))?,
        JobId::new(job_id).map_err(|error| StoreError::Domain(error.to_string()))?,
        AgentId::new(agent_id).map_err(StoreError::from)?,
        TaskId::new(task_id).map_err(|error| StoreError::Domain(error.to_string()))?,
        destination,
        ArtifactChecksum::sha256(checksum)
            .map_err(|error| StoreError::Domain(error.to_string()))?,
        u64::try_from(size_bytes)
            .map_err(|_| StoreError::Domain("artifact size_bytes is negative".to_owned()))?,
        ArtifactRetentionClass::parse(&retention_class)
            .map_err(|error| StoreError::Domain(error.to_string()))?,
        unix_secs_to_system_time(created_at),
    )
    .map_err(|error| StoreError::Domain(error.to_string()))
}

fn row_to_remediation_request_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<AppRemediationRequestRecord> {
    Ok(AppRemediationRequestRecord {
        id: row.get(0)?,
        policy_id: row.get(1)?,
        policy_name: row.get(2)?,
        agent_id: row.get(3)?,
        runbook_ref: row.get(4)?,
        status: row.get(5)?,
        approval_required: row.get::<_, i64>(6)? != 0,
        risk_summary: row.get(7)?,
        job_id: row.get(8)?,
        origin_drift_report_id: row
            .get::<_, Option<i64>>(9)?
            .map(DriftReportId::new)
            .transpose()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        policy_version: row
            .get::<_, Option<i64>>(10)?
            .and_then(|value| u32::try_from(value).ok()),
        created_at: unix_secs_to_system_time(row.get(11)?),
        updated_at: unix_secs_to_system_time(row.get(12)?),
    })
}

fn row_to_app_approval_request_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<AppApprovalRequestRecord> {
    Ok(AppApprovalRequestRecord {
        id: row.get(0)?,
        job_id: row.get(1)?,
        requester: row.get(2)?,
        approver: row.get(3)?,
        reason: row.get(4)?,
        status: row.get(5)?,
        expires_at: unix_secs_to_system_time(row.get(6)?),
        created_at: unix_secs_to_system_time(row.get(7)?),
        decided_at: row.get::<_, Option<i64>>(8)?.map(unix_secs_to_system_time),
    })
}

#[cfg(feature = "postgres")]
fn postgres_row_to_app_approval_request_record(
    row: &postgres::Row,
) -> Result<AppApprovalRequestRecord, StoreError> {
    Ok(AppApprovalRequestRecord {
        id: row.get(0),
        job_id: row.get(1),
        requester: row.get(2),
        approver: row.get(3),
        reason: row.get(4),
        status: row.get(5),
        expires_at: unix_secs_to_system_time(row.get(6)),
        created_at: unix_secs_to_system_time(row.get(7)),
        decided_at: row.get::<_, Option<i64>>(8).map(unix_secs_to_system_time),
    })
}

fn row_to_audit(row: &rusqlite::Row<'_>) -> Result<AuditEvent, StoreError> {
    row_to_audit_at(row, 0)
}

fn row_to_audit_page_record(row: &rusqlite::Row<'_>) -> Result<AuditEventPageRecord, StoreError> {
    let row_id = row.get(0)?;
    if row_id <= 0 {
        return Err(StoreError::Domain(
            "audit event row id must be positive".to_owned(),
        ));
    }
    let event = row_to_audit_at(row, 1)?;
    Ok(AuditEventPageRecord {
        cursor: SnapshotPageCursor {
            occurred_at: event.occurred_at,
            row_id,
        },
        event,
    })
}

fn row_to_audit_at(row: &rusqlite::Row<'_>, offset: usize) -> Result<AuditEvent, StoreError> {
    let category: String = row.get(offset)?;
    let action: String = row.get(offset + 1)?;
    let actor: String = row.get(offset + 2)?;
    let target: String = row.get(offset + 3)?;
    let value_kind: String = row.get(offset + 4)?;
    let value_text: String = row.get(offset + 5)?;
    let occurred_at: i64 = row.get(offset + 6)?;

    let category = AuditCategory::parse(&category)
        .ok_or_else(|| StoreError::Domain(format!("unknown audit category: {category}")))?;

    Ok(AuditEvent {
        category,
        action,
        actor: AuditActor::new(actor),
        target: AuditTarget::new(target),
        value: decode_audit_value(&value_kind, &value_text),
        occurred_at: unix_secs_to_system_time(occurred_at),
    })
}

#[cfg(feature = "postgres")]
fn postgres_query_audit(
    store: &PostgresStore,
    category: Option<AuditCategory>,
    limit: usize,
) -> Result<Vec<AuditEvent>, StoreError> {
    let limit = limit.clamp(1, 500) as i64;
    let mut client = store.checkout_client()?;
    let rows = if let Some(category) = category {
        client
            .query(
                "SELECT category, action, actor, target, value_kind, value_text, occurred_at
                 FROM audit_events
                 WHERE category = $1
                 ORDER BY id DESC
                 LIMIT $2",
                &[&category.as_str(), &limit],
            )
            .map_err(|_| postgres_error("postgres audit query failed"))?
    } else {
        client
            .query(
                "SELECT category, action, actor, target, value_kind, value_text, occurred_at
                 FROM audit_events
                 ORDER BY id DESC
                 LIMIT $1",
                &[&limit],
            )
            .map_err(|_| postgres_error("postgres audit query failed"))?
    };

    rows.into_iter()
        .map(|row| postgres_row_to_audit(&row, 0))
        .collect()
}

#[cfg(feature = "postgres")]
fn postgres_query_audit_page(
    store: &PostgresStore,
    category: Option<AuditCategory>,
    limit: usize,
    before: Option<SnapshotPageCursor>,
) -> Result<Vec<AuditEventPageRecord>, StoreError> {
    let limit = limit.clamp(1, 500) as i64;
    let before_seconds = before.map(|cursor| system_time_to_unix_secs(cursor.occurred_at));
    let before_row_id = before.map(|cursor| cursor.row_id);
    let mut client = store.checkout_client()?;

    let rows = match (category, before_seconds, before_row_id) {
        (Some(category), Some(before_seconds), Some(before_row_id)) => client
            .query(
                "SELECT id, category, action, actor, target, value_kind, value_text, occurred_at
                 FROM audit_events
                 WHERE category = $1
                   AND (occurred_at < $2 OR (occurred_at = $2 AND id < $3))
                 ORDER BY occurred_at DESC, id DESC
                 LIMIT $4",
                &[&category.as_str(), &before_seconds, &before_row_id, &limit],
            )
            .map_err(|_| postgres_error("postgres audit export query failed"))?,
        (Some(category), None, None) => client
            .query(
                "SELECT id, category, action, actor, target, value_kind, value_text, occurred_at
                 FROM audit_events
                 WHERE category = $1
                 ORDER BY occurred_at DESC, id DESC
                 LIMIT $2",
                &[&category.as_str(), &limit],
            )
            .map_err(|_| postgres_error("postgres audit export query failed"))?,
        (None, Some(before_seconds), Some(before_row_id)) => client
            .query(
                "SELECT id, category, action, actor, target, value_kind, value_text, occurred_at
                 FROM audit_events
                 WHERE occurred_at < $1 OR (occurred_at = $1 AND id < $2)
                 ORDER BY occurred_at DESC, id DESC
                 LIMIT $3",
                &[&before_seconds, &before_row_id, &limit],
            )
            .map_err(|_| postgres_error("postgres audit export query failed"))?,
        (None, None, None) => client
            .query(
                "SELECT id, category, action, actor, target, value_kind, value_text, occurred_at
                 FROM audit_events
                 ORDER BY occurred_at DESC, id DESC
                 LIMIT $1",
                &[&limit],
            )
            .map_err(|_| postgres_error("postgres audit export query failed"))?,
        _ => return Err(StoreError::Domain("invalid audit page cursor".to_owned())),
    };

    rows.into_iter()
        .map(|row| postgres_row_to_audit_page_record(&row))
        .collect()
}

#[cfg(feature = "postgres")]
fn postgres_row_to_audit_page_record(
    row: &postgres::Row,
) -> Result<AuditEventPageRecord, StoreError> {
    let row_id: i64 = row.get(0);
    if row_id <= 0 {
        return Err(StoreError::Domain(
            "audit event row id must be positive".to_owned(),
        ));
    }
    let event = postgres_row_to_audit(row, 1)?;
    Ok(AuditEventPageRecord {
        cursor: SnapshotPageCursor {
            occurred_at: event.occurred_at,
            row_id,
        },
        event,
    })
}

#[cfg(feature = "postgres")]
fn postgres_row_to_audit(row: &postgres::Row, offset: usize) -> Result<AuditEvent, StoreError> {
    let category: String = row.get(offset);
    let action: String = row.get(offset + 1);
    let actor: String = row.get(offset + 2);
    let target: String = row.get(offset + 3);
    let value_kind: String = row.get(offset + 4);
    let value_text: String = row.get(offset + 5);
    let occurred_at: i64 = row.get(offset + 6);

    let category = AuditCategory::parse(&category)
        .ok_or_else(|| StoreError::Domain(format!("unknown audit category: {category}")))?;

    Ok(AuditEvent {
        category,
        action,
        actor: AuditActor::new(actor),
        target: AuditTarget::new(target),
        value: decode_audit_value(&value_kind, &value_text),
        occurred_at: unix_secs_to_system_time(occurred_at),
    })
}

#[cfg(feature = "postgres")]
fn postgres_list_pending_dispatch_assignments(
    store: &PostgresStore,
    agent_id: Option<&AgentId>,
    job_id: Option<&JobId>,
    limit: usize,
) -> Result<Vec<AppPendingTaskAssignment>, StoreError> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let agent_id = agent_id.map(AgentId::as_str);
    let job_id = job_id.map(JobId::as_str);
    let limit = limit.min(100) as i64;
    let mut client = store.checkout_client()?;
    let rows = client
        .query(
            "SELECT
                ta.job_id, ta.id, ta.agent_id, ta.nonce, ta.payload_hash,
                ta.signature, ta.issued_at, ta.expires_at,
                j.command_program, j.command_args_json, j.drift_policy_document,
                j.runbook_document, j.timeout_ms
             FROM task_assignments ta
             JOIN jobs j ON j.id = ta.job_id
             WHERE ta.status = 'queued'
               AND j.status IN ('queued', 'running')
               AND ($1::TEXT IS NULL OR ta.agent_id = $1)
               AND ($2::TEXT IS NULL OR ta.job_id = $2)
               AND (
                    j.command_program IS NOT NULL
                 OR j.drift_policy_document IS NOT NULL
                 OR j.runbook_document IS NOT NULL
               )
             ORDER BY ta.created_at, ta.id
             LIMIT $3",
            &[&agent_id, &job_id, &limit],
        )
        .map_err(|_| postgres_error("postgres pending assignment query failed"))?;

    rows.into_iter()
        .map(|row| postgres_row_to_pending_assignment(&row))
        .collect()
}

#[cfg(feature = "postgres")]
fn postgres_row_to_pending_assignment(
    row: &postgres::Row,
) -> Result<AppPendingTaskAssignment, StoreError> {
    let job_id: String = row.get(0);
    let task_id: String = row.get(1);
    let target_agent_id: String = row.get(2);
    let nonce: String = row.get(3);
    let payload_hash: String = row.get(4);
    let signature: String = row.get(5);
    let issued_at: i64 = row.get(6);
    let expires_at: i64 = row.get(7);
    let command_program: Option<String> = row.get(8);
    let command_args_json: String = row.get(9);
    let drift_policy_document: Option<String> = row.get(10);
    let runbook_document: Option<String> = row.get(11);
    let timeout_ms: i64 = row.get(12);

    let envelope = TaskEnvelope {
        job_id: JobId::new(job_id).map_err(|error| StoreError::Domain(error.to_string()))?,
        task_id: TaskId::new(task_id).map_err(|error| StoreError::Domain(error.to_string()))?,
        target_agent_id: AgentId::new(target_agent_id)
            .map_err(|error| StoreError::Domain(error.to_string()))?,
        issued_at: unix_secs_to_system_time(issued_at),
        expires_at: TaskExpiry::new(unix_secs_to_system_time(expires_at)),
        nonce: TaskNonce::new(nonce).map_err(|error| StoreError::Domain(error.to_string()))?,
        payload_hash,
        signature: Some(
            TaskSignature::new(signature).map_err(|error| StoreError::Domain(error.to_string()))?,
        ),
    };
    let timeout = Duration::from_millis(timeout_ms as u64);
    let task = if let Some(program) = command_program {
        TaskKind::Command(
            CommandTask::new(program, parse_command_args(&command_args_json)?, timeout)
                .map_err(|error| StoreError::Domain(error.to_string()))?,
        )
    } else if let Some(policy_document) = drift_policy_document {
        TaskKind::DriftCheck(
            DriftCheckTask::new(policy_document, timeout)
                .map_err(|error| StoreError::Domain(error.to_string()))?,
        )
    } else if let Some(runbook_document) = runbook_document {
        TaskKind::RunbookExecution(
            RunbookExecutionTask::new(runbook_document, timeout)
                .map_err(|error| StoreError::Domain(error.to_string()))?,
        )
    } else {
        return Err(StoreError::Domain(
            "pending assignment has no task payload".to_owned(),
        ));
    };

    Ok(AppPendingTaskAssignment { envelope, task })
}

#[cfg(feature = "postgres")]
fn postgres_job_dispatch_gate(
    store: &PostgresStore,
    job_id: &str,
) -> Result<Option<JobDispatchGateRecord>, StoreError> {
    let mut client = store.checkout_client()?;
    client
        .query_opt(
            "SELECT
                j.strategy_concurrency,
                j.strategy_max_failures,
                COALESCE(SUM(CASE WHEN ta.status IN ('dispatched', 'accepted', 'started') THEN 1 ELSE 0 END), 0) AS active_count,
                COALESCE(SUM(CASE WHEN ta.status IN ('failed', 'rejected', 'expired') THEN 1 ELSE 0 END), 0) AS failure_count
             FROM jobs j
             LEFT JOIN task_assignments ta ON ta.job_id = j.id
             WHERE j.id = $1
             GROUP BY j.id",
            &[&job_id],
        )
        .map(|row| {
            row.map(|row| {
                let concurrency = row.get::<_, i64>(0).max(1) as u32;
                let max_failures = row
                    .get::<_, Option<i64>>(1)
                    .and_then(|value| u32::try_from(value).ok())
                    .filter(|value| *value > 0);
                JobDispatchGateRecord {
                    concurrency,
                    max_failures,
                    active_count: row.get::<_, i64>(2).max(0) as usize,
                    failure_count: row.get::<_, i64>(3).max(0) as usize,
                }
            })
        })
        .map_err(|_| postgres_error("postgres job dispatch gate query failed"))
}

#[cfg(feature = "postgres")]
fn postgres_latest_agent_capability_snapshot(
    store: &PostgresStore,
    agent_id: &str,
) -> Result<Option<AgentCapabilitySnapshot>, StoreError> {
    let mut client = store.checkout_client()?;
    let Some(row) = client
        .query_opt(
            "SELECT privilege_level, package_manager, service_manager, capabilities_json, reported_at
             FROM agent_capability_snapshots
             WHERE agent_id = $1",
            &[&agent_id],
        )
        .map_err(|_| postgres_error("postgres agent capability query failed"))?
    else {
        return Ok(None);
    };
    let privilege_level: String = row.get(0);
    let package_manager: Option<String> = row.get(1);
    let service_manager: Option<String> = row.get(2);
    let capabilities_json: String = row.get(3);
    let reported_at: i64 = row.get(4);
    capability_snapshot_from_row(
        &privilege_level,
        package_manager.as_deref(),
        service_manager.as_deref(),
        &capabilities_json,
        reported_at,
    )
    .map(Some)
}

#[cfg(feature = "postgres")]
fn postgres_save_agent_capability_snapshot(
    store: &PostgresStore,
    agent_id: &str,
    snapshot: AgentCapabilitySnapshot,
) -> Result<(), StoreError> {
    let Some(profile) = snapshot.profile() else {
        return Err(StoreError::Domain(
            "capability snapshot profile is required".to_owned(),
        ));
    };
    let Some(reported_at) = snapshot.reported_at() else {
        return Err(StoreError::Domain(
            "capability snapshot reported_at is required".to_owned(),
        ));
    };
    let capabilities_json = serde_json::to_string(
        &profile
            .capabilities()
            .iter()
            .map(|capability| capability.as_str())
            .collect::<Vec<_>>(),
    )
    .map_err(|error| StoreError::Domain(error.to_string()))?;
    store
        .checkout_client()?
        .execute(
            "INSERT INTO agent_capability_snapshots (
                agent_id, privilege_level, package_manager, service_manager,
                capabilities_json, reported_at, updated_at
             )
             VALUES ($1, $2, $3, $4, $5, $6, EXTRACT(EPOCH FROM now())::BIGINT)
             ON CONFLICT(agent_id) DO UPDATE SET
                privilege_level = excluded.privilege_level,
                package_manager = excluded.package_manager,
                service_manager = excluded.service_manager,
                capabilities_json = excluded.capabilities_json,
                reported_at = excluded.reported_at,
                updated_at = excluded.updated_at",
            &[
                &agent_id,
                &profile.privilege().as_str(),
                &profile.package_manager().map(|manager| manager.as_str()),
                &profile.service_manager().map(|manager| manager.as_str()),
                &capabilities_json,
                &system_time_to_unix_secs(reported_at),
            ],
        )
        .map(|_| ())
        .map_err(|_| postgres_error("postgres capability snapshot upsert failed"))
}

#[cfg(feature = "postgres")]
fn postgres_insert_drift_report(
    store: &PostgresStore,
    agent_id: &str,
    report: &DriftReport,
    checked_at: SystemTime,
) -> Result<(), StoreError> {
    postgres_insert_drift_report_with_provenance(
        store,
        agent_id,
        report,
        &DriftReportProvenance::uncorrelated(),
        checked_at,
    )
}

#[cfg(feature = "postgres")]
fn postgres_insert_drift_report_with_provenance(
    store: &PostgresStore,
    agent_id: &str,
    report: &DriftReport,
    provenance: &DriftReportProvenance,
    checked_at: SystemTime,
) -> Result<(), StoreError> {
    store
        .checkout_client()?
        .execute(
            "INSERT INTO drift_reports (
                agent_id, policy_name, status, severity, expected, actual, checked_at,
                job_id, task_id, policy_id, policy_version, purpose
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
            &[
                &agent_id,
                &report.policy_name.as_str(),
                &drift_status_to_str(&report.status),
                &drift_severity_to_str(report.severity),
                &report.expected.as_str(),
                &report.actual.as_str(),
                &system_time_to_unix_secs(checked_at),
                &provenance.job_id.as_ref().map(JobId::as_str),
                &provenance.task_id.as_ref().map(TaskId::as_str),
                &provenance.policy_id.as_deref(),
                &provenance.policy_version.map(i64::from),
                &provenance.purpose.map(DriftCheckPurpose::as_str),
            ],
        )
        .map(|_| ())
        .map_err(|error| {
            postgres_constraint_or_context(error, "postgres drift report insert failed")
        })
}

#[cfg(feature = "postgres")]
fn postgres_latest_drift_report(
    store: &PostgresStore,
    agent_id: &str,
) -> Result<Option<AppDriftReportRecord>, StoreError> {
    store
        .checkout_client()?
        .query_opt(
            "SELECT id, agent_id, policy_name, status, expected, actual, checked_at,
                    severity, acknowledged_at, acknowledged_by, resolved_at, resolution_job_id,
                    job_id, task_id, policy_id, policy_version, purpose
             FROM drift_reports
             WHERE agent_id = $1
             ORDER BY checked_at DESC, id DESC
             LIMIT 1",
            &[&agent_id],
        )
        .map_err(|_| postgres_error("postgres latest drift report query failed"))?
        .map(|row| postgres_row_to_drift_report_record(&row))
        .transpose()
}

#[cfg(feature = "postgres")]
fn postgres_list_drift_reports(
    store: &PostgresStore,
    agent_id: &str,
    limit: usize,
    before: Option<SnapshotPageCursor>,
) -> Result<Vec<AppDriftReportPageRecord>, StoreError> {
    let limit = limit.clamp(1, 501) as i64;
    let mut client = store.checkout_client()?;
    let rows = if let Some(before) = before {
        let before_secs = system_time_to_unix_secs(before.occurred_at);
        client
            .query(
                "SELECT id, agent_id, policy_name, status, expected, actual, checked_at,
                        severity, acknowledged_at, acknowledged_by, resolved_at, resolution_job_id,
                        job_id, task_id, policy_id, policy_version, purpose
                 FROM drift_reports
                 WHERE agent_id = $1
                   AND (checked_at < $2 OR (checked_at = $2 AND id < $3))
                 ORDER BY checked_at DESC, id DESC
                 LIMIT $4",
                &[&agent_id, &before_secs, &before.row_id, &limit],
            )
            .map_err(|_| postgres_error("postgres drift report page query failed"))?
    } else {
        client
            .query(
                "SELECT id, agent_id, policy_name, status, expected, actual, checked_at,
                        severity, acknowledged_at, acknowledged_by, resolved_at, resolution_job_id,
                        job_id, task_id, policy_id, policy_version, purpose
                 FROM drift_reports
                 WHERE agent_id = $1
                 ORDER BY checked_at DESC, id DESC
                 LIMIT $2",
                &[&agent_id, &limit],
            )
            .map_err(|_| postgres_error("postgres drift report page query failed"))?
    };

    rows.into_iter()
        .map(|row| postgres_row_to_drift_report_page_record(&row))
        .collect()
}

#[cfg(feature = "postgres")]
fn postgres_row_to_drift_report_record(
    row: &postgres::Row,
) -> Result<AppDriftReportRecord, StoreError> {
    Ok(AppDriftReportRecord {
        id: DriftReportId::new(row.get(0))
            .map_err(|_| postgres_error("invalid drift report id"))?,
        agent_id: row.get(1),
        report: DriftReport {
            policy_name: row.get(2),
            status: parse_drift_status(&row.get::<_, String>(3)),
            severity: parse_drift_severity(&row.get::<_, String>(7)),
            acknowledgement: postgres_row_to_drift_acknowledgement(row, 8, 9, 10, 11),
            expected: row.get(4),
            actual: row.get(5),
        },
        provenance: postgres_row_to_drift_report_provenance(row, 12, 13, 14, 15, 16),
        checked_at: unix_secs_to_system_time(row.get(6)),
    })
}

#[cfg(feature = "postgres")]
fn postgres_row_to_drift_report_page_record(
    row: &postgres::Row,
) -> Result<AppDriftReportPageRecord, StoreError> {
    let id: i64 = row.get(0);
    let checked_at = unix_secs_to_system_time(row.get(6));
    Ok(AppDriftReportPageRecord {
        id: DriftReportId::new(id).map_err(|_| postgres_error("invalid drift report id"))?,
        agent_id: row.get(1),
        report: DriftReport {
            policy_name: row.get(2),
            status: parse_drift_status(&row.get::<_, String>(3)),
            severity: parse_drift_severity(&row.get::<_, String>(7)),
            acknowledgement: postgres_row_to_drift_acknowledgement(row, 8, 9, 10, 11),
            expected: row.get(4),
            actual: row.get(5),
        },
        provenance: postgres_row_to_drift_report_provenance(row, 12, 13, 14, 15, 16),
        checked_at,
        cursor: SnapshotPageCursor {
            occurred_at: checked_at,
            row_id: id,
        },
    })
}

#[cfg(feature = "postgres")]
fn postgres_row_to_drift_report_provenance(
    row: &postgres::Row,
    job_id_index: usize,
    task_id_index: usize,
    policy_id_index: usize,
    policy_version_index: usize,
    purpose_index: usize,
) -> DriftReportProvenance {
    let (Some(job_id), Some(task_id), Some(policy_id), Some(policy_version), Some(purpose)) = (
        row.get::<_, Option<String>>(job_id_index),
        row.get::<_, Option<String>>(task_id_index),
        row.get::<_, Option<String>>(policy_id_index),
        row.get::<_, Option<i64>>(policy_version_index),
        row.get::<_, Option<String>>(purpose_index),
    ) else {
        return DriftReportProvenance::uncorrelated();
    };
    let (Ok(job_id), Ok(task_id), Some(purpose)) = (
        JobId::new(job_id),
        TaskId::new(task_id),
        DriftCheckPurpose::parse(&purpose),
    ) else {
        return DriftReportProvenance::uncorrelated();
    };
    if policy_version < 0 {
        return DriftReportProvenance::uncorrelated();
    }
    DriftReportProvenance::verified(job_id, task_id, policy_id, policy_version as u32, purpose)
}

#[cfg(feature = "postgres")]
fn postgres_row_to_drift_acknowledgement(
    row: &postgres::Row,
    acknowledged_at_index: usize,
    acknowledged_by_index: usize,
    resolved_at_index: usize,
    resolution_job_id_index: usize,
) -> DriftAcknowledgement {
    let resolved_at = row.get::<_, Option<i64>>(resolved_at_index);
    let resolution_job_id = row.get::<_, Option<String>>(resolution_job_id_index);
    if let (Some(resolved_at), Some(job_id)) = (resolved_at, resolution_job_id) {
        return DriftAcknowledgement::Resolved {
            job_id,
            at: unix_secs_to_system_time(resolved_at),
        };
    }
    let acknowledged_at = row.get::<_, Option<i64>>(acknowledged_at_index);
    let acknowledged_by = row.get::<_, Option<String>>(acknowledged_by_index);
    if let (Some(acknowledged_at), Some(by)) = (acknowledged_at, acknowledged_by) {
        return DriftAcknowledgement::Acknowledged {
            by,
            at: unix_secs_to_system_time(acknowledged_at),
        };
    }
    DriftAcknowledgement::Open
}

#[cfg(feature = "postgres")]
fn postgres_save_policy_source(
    store: &PostgresStore,
    policy_id: &str,
    name: &str,
    version: u32,
    source: &str,
) -> Result<(), StoreError> {
    let version = i64::from(version);
    store
        .checkout_client()?
        .execute(
            "INSERT INTO policies (id, name, version, source, created_at, updated_at)
             VALUES (
                $1, $2, $3, $4,
                EXTRACT(EPOCH FROM now())::BIGINT,
                EXTRACT(EPOCH FROM now())::BIGINT
             )
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                version = excluded.version,
                source = excluded.source,
                updated_at = excluded.updated_at",
            &[&policy_id, &name, &version, &source],
        )
        .map(|_| ())
        .map_err(|_| postgres_error("postgres policy source upsert failed"))
}

#[cfg(feature = "postgres")]
fn postgres_list_policies(store: &PostgresStore) -> Result<Vec<AppPolicyRecord>, StoreError> {
    let rows = store
        .checkout_client()?
        .query(
            "SELECT id, name, version, source, created_at, updated_at
             FROM policies
             ORDER BY id",
            &[],
        )
        .map_err(|_| postgres_error("postgres policy list query failed"))?;
    rows.into_iter()
        .map(|row| Ok(postgres_row_to_policy_record(&row)))
        .collect()
}

#[cfg(feature = "postgres")]
fn postgres_find_policy(
    store: &PostgresStore,
    policy_id: &str,
) -> Result<Option<AppPolicyRecord>, StoreError> {
    store
        .checkout_client()?
        .query_opt(
            "SELECT id, name, version, source, created_at, updated_at
             FROM policies
             WHERE id = $1",
            &[&policy_id],
        )
        .map(|row| row.map(|row| postgres_row_to_policy_record(&row)))
        .map_err(|_| postgres_error("postgres policy find query failed"))
}

#[cfg(feature = "postgres")]
fn postgres_row_to_policy_record(row: &postgres::Row) -> AppPolicyRecord {
    AppPolicyRecord {
        id: row.get(0),
        name: row.get(1),
        version: row.get::<_, i64>(2).max(0) as u32,
        source: row.get(3),
        created_at: unix_secs_to_system_time(row.get(4)),
        updated_at: unix_secs_to_system_time(row.get(5)),
    }
}

#[cfg(feature = "postgres")]
fn postgres_assign_policy_to_agent(
    store: &PostgresStore,
    policy_id: &str,
    agent_id: &str,
    assigned_at: SystemTime,
) -> Result<(), StoreError> {
    store
        .checkout_client()?
        .execute(
            "INSERT INTO policy_assignments (policy_id, agent_id, assigned_at)
             VALUES ($1, $2, $3)
             ON CONFLICT(policy_id, agent_id) DO UPDATE SET
                assigned_at = excluded.assigned_at",
            &[
                &policy_id,
                &agent_id,
                &system_time_to_unix_secs(assigned_at),
            ],
        )
        .map(|_| ())
        .map_err(|_| postgres_error("postgres policy assignment upsert failed"))
}

#[cfg(feature = "postgres")]
fn postgres_policies_for_agent(
    store: &PostgresStore,
    agent_id: &str,
) -> Result<Vec<AppPolicyAssignmentRecord>, StoreError> {
    let rows = store
        .checkout_client()?
        .query(
            "SELECT policy_id, agent_id, assigned_at
             FROM policy_assignments
             WHERE agent_id = $1
             ORDER BY policy_id",
            &[&agent_id],
        )
        .map_err(|_| postgres_error("postgres policy assignment query failed"))?;
    Ok(rows
        .into_iter()
        .map(|row| AppPolicyAssignmentRecord {
            policy_id: row.get(0),
            agent_id: row.get(1),
            assigned_at: unix_secs_to_system_time(row.get(2)),
        })
        .collect())
}

#[cfg(feature = "postgres")]
fn postgres_upsert_policy_schedule(
    store: &PostgresStore,
    policy_id: &str,
    agent_id: &str,
    interval: Duration,
    next_due_at: SystemTime,
) -> Result<(), StoreError> {
    if interval.is_zero() {
        return Err(StoreError::Domain(
            "policy schedule interval must be positive".to_owned(),
        ));
    }
    let interval_seconds = interval.as_secs().max(1) as i64;
    store
        .checkout_client()?
        .execute(
            "INSERT INTO policy_drift_schedules (
                policy_id, agent_id, interval_seconds, next_due_at, last_checked_at
             ) VALUES ($1, $2, $3, $4, NULL)
             ON CONFLICT(policy_id, agent_id) DO UPDATE SET
                interval_seconds = excluded.interval_seconds,
                next_due_at = excluded.next_due_at",
            &[
                &policy_id,
                &agent_id,
                &interval_seconds,
                &system_time_to_unix_secs(next_due_at),
            ],
        )
        .map(|_| ())
        .map_err(|_| postgres_error("postgres policy schedule upsert failed"))
}

#[cfg(feature = "postgres")]
fn postgres_due_scheduled_drift_checks(
    store: &PostgresStore,
    now: SystemTime,
    limit: usize,
) -> Result<Vec<AppScheduledDriftRecord>, StoreError> {
    let limit = limit.clamp(1, 500) as i64;
    let rows = store
        .checkout_client()?
        .query(
            "SELECT policy_id, agent_id, interval_seconds, next_due_at, last_checked_at
             FROM policy_drift_schedules
             WHERE next_due_at <= $1
             ORDER BY next_due_at ASC
             LIMIT $2",
            &[&system_time_to_unix_secs(now), &limit],
        )
        .map_err(|_| postgres_error("postgres due drift schedule query failed"))?;
    Ok(rows
        .into_iter()
        .map(|row| AppScheduledDriftRecord {
            policy_id: row.get(0),
            agent_id: row.get(1),
            interval_seconds: row.get::<_, i64>(2).max(0) as u64,
            next_due_at: unix_secs_to_system_time(row.get(3)),
            last_checked_at: row.get::<_, Option<i64>>(4).map(unix_secs_to_system_time),
        })
        .collect())
}

#[cfg(feature = "postgres")]
fn postgres_record_scheduled_drift_check(
    store: &PostgresStore,
    policy_id: &str,
    agent_id: &str,
    checked_at: SystemTime,
) -> Result<(), StoreError> {
    let checked_at = system_time_to_unix_secs(checked_at);
    let changed = store
        .checkout_client()?
        .execute(
            "UPDATE policy_drift_schedules
             SET last_checked_at = $3,
                 next_due_at = $3 + interval_seconds
             WHERE policy_id = $1 AND agent_id = $2",
            &[&policy_id, &agent_id, &checked_at],
        )
        .map_err(|_| postgres_error("postgres policy schedule update failed"))?;
    if changed == 0 {
        return Err(StoreError::NotFound);
    }
    Ok(())
}

#[cfg(feature = "postgres")]
fn postgres_acknowledge_latest_drift_report(
    store: &PostgresStore,
    agent_id: &str,
    policy_name: &str,
    actor: &str,
    acknowledged_at: SystemTime,
) -> Result<bool, StoreError> {
    store
        .checkout_client()?
        .execute(
            "UPDATE drift_reports
             SET acknowledged_at = $3, acknowledged_by = $4
             WHERE id = (
                SELECT id FROM drift_reports
                WHERE agent_id = $1 AND policy_name = $2
                ORDER BY checked_at DESC, id DESC
                LIMIT 1
             )",
            &[
                &agent_id,
                &policy_name,
                &system_time_to_unix_secs(acknowledged_at),
                &actor,
            ],
        )
        .map(|changed| changed > 0)
        .map_err(|_| postgres_error("postgres drift acknowledgement update failed"))
}

#[cfg(feature = "postgres")]
fn postgres_mark_latest_drift_resolved(
    store: &PostgresStore,
    agent_id: &str,
    policy_name: &str,
    job_id: &str,
    resolved_at: SystemTime,
) -> Result<bool, StoreError> {
    store
        .checkout_client()?
        .execute(
            "UPDATE drift_reports
             SET resolved_at = $3, resolution_job_id = $4
             WHERE id = (
                SELECT id FROM drift_reports
                WHERE agent_id = $1 AND policy_name = $2
                ORDER BY checked_at DESC, id DESC
                LIMIT 1
             )",
            &[
                &agent_id,
                &policy_name,
                &system_time_to_unix_secs(resolved_at),
                &job_id,
            ],
        )
        .map(|changed| changed > 0)
        .map_err(|_| postgres_error("postgres drift resolution update failed"))
}

#[cfg(feature = "postgres")]
fn postgres_list_job_summaries_filtered(
    store: &PostgresStore,
    job_id: Option<&str>,
    limit: usize,
) -> Result<Vec<AppJobSummaryRecord>, StoreError> {
    let limit = limit.min(100) as i64;
    let rows = store.checkout_client()?
        .query(
            "SELECT
                j.id,
                j.status,
                j.risk,
                j.command_program,
                j.command_args_json,
                j.created_at,
                j.selector_kind,
                j.selector_source,
                j.strategy_concurrency,
                j.strategy_max_failures,
                COUNT(ta.id) AS target_count,
                COALESCE(STRING_AGG(
                    COALESCE(jt.agent_id, ta.agent_id, '') || CHR(30) ||
                    COALESCE(NULLIF(jt.agent_display_name, ''), a.name, jt.agent_id, ta.agent_id, '') || CHR(30) ||
                    COALESCE(NULLIF(jt.agent_status_snapshot, ''), jt.status, a.status, 'unknown') || CHR(30) ||
                    COALESCE(jt.labels_snapshot, '') || CHR(30) ||
                    COALESCE(ta.id, '') || CHR(30) ||
                    COALESCE(ta.status, '') || CHR(30) ||
                    COALESCE(ta.last_error, ''),
                    CHR(31)
                    ORDER BY COALESCE(jt.agent_id, ta.agent_id, ''), ta.id
                ), '') AS target_agents,
                MAX(ta.expires_at) AS expires_at
             FROM jobs j
             LEFT JOIN task_assignments ta ON ta.job_id = j.id
             LEFT JOIN job_targets jt ON jt.job_id = j.id AND jt.agent_id = ta.agent_id
             LEFT JOIN agents a ON a.id = ta.agent_id
             WHERE ($1::TEXT IS NULL OR j.id = $1)
             GROUP BY j.id
             ORDER BY j.created_at DESC, j.id DESC
             LIMIT $2",
            &[&job_id, &limit],
        )
        .map_err(|_| postgres_error("postgres job summary query failed"))?;

    rows.into_iter()
        .map(|row| postgres_row_to_job_summary(&row))
        .collect()
}

#[cfg(feature = "postgres")]
fn postgres_row_to_job_summary(row: &postgres::Row) -> Result<AppJobSummaryRecord, StoreError> {
    let command_args_json: String = row.get(4);
    let strategy_max_failures = row
        .get::<_, Option<i64>>(9)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0);
    let target_agents: String = row.get(11);
    Ok(AppJobSummaryRecord {
        id: row.get(0),
        status: row.get(1),
        risk: row.get(2),
        command_program: row.get(3),
        command_args: parse_command_args(&command_args_json)?,
        selector_kind: row.get(6),
        selector_source: row.get(7),
        strategy_concurrency: row.get::<_, i64>(8).max(1) as u32,
        strategy_max_failures,
        target_count: row.get::<_, i64>(10).max(0) as usize,
        target_agents: parse_job_target_summaries(&target_agents)
            .into_iter()
            .map(|target| AppJobTargetSummaryRecord {
                agent_id: target.agent_id,
                agent_name: target.agent_name,
                status: target.status,
                labels: target.labels,
                task_id: target.task_id,
                assignment_status: target.assignment_status,
                last_error: target.last_error,
            })
            .collect(),
        created_at: unix_secs_to_system_time(row.get(5)),
        expires_at: row.get::<_, Option<i64>>(12).map(unix_secs_to_system_time),
    })
}

#[cfg(feature = "postgres")]
fn postgres_save_rendered_artifact_metadata(
    store: &PostgresStore,
    metadata: &RenderedArtifactMetadata,
) -> Result<(), StoreError> {
    let size_bytes = i64::try_from(metadata.size_bytes)
        .map_err(|_| StoreError::Domain("artifact size_bytes exceeds postgres range".to_owned()))?;
    store
        .checkout_client()?
        .execute(
            "INSERT INTO rendered_artifacts (
                id, job_id, agent_id, task_id, destination, checksum_sha256,
                size_bytes, retention_class, created_at
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            &[
                &metadata.id.as_str(),
                &metadata.job_id.as_str(),
                &metadata.agent_id.as_str(),
                &metadata.task_id.as_str(),
                &metadata.destination.as_str(),
                &metadata.checksum.as_sha256(),
                &size_bytes,
                &metadata.retention_class.as_str(),
                &system_time_to_unix_secs(metadata.created_at),
            ],
        )
        .map(|_| ())
        .map_err(|error| {
            postgres_constraint_or_context(error, "postgres rendered artifact insert failed")
        })
}

#[cfg(feature = "postgres")]
fn postgres_list_rendered_artifacts_for_job(
    store: &PostgresStore,
    job_id: &str,
) -> Result<Vec<RenderedArtifactMetadata>, StoreError> {
    let rows = store
        .checkout_client()?
        .query(
            "SELECT id, job_id, agent_id, task_id, destination, checksum_sha256,
                    size_bytes, retention_class, created_at
             FROM rendered_artifacts
             WHERE job_id = $1
             ORDER BY created_at, id",
            &[&job_id],
        )
        .map_err(|_| postgres_error("postgres rendered artifact query failed"))?;
    rows.into_iter()
        .map(|row| postgres_row_to_rendered_artifact_metadata(&row))
        .collect()
}

#[cfg(feature = "postgres")]
fn postgres_row_to_rendered_artifact_metadata(
    row: &postgres::Row,
) -> Result<RenderedArtifactMetadata, StoreError> {
    RenderedArtifactMetadata::new(
        ArtifactId::new(row.get::<_, String>(0))
            .map_err(|error| StoreError::Domain(error.to_string()))?,
        JobId::new(row.get::<_, String>(1))
            .map_err(|error| StoreError::Domain(error.to_string()))?,
        AgentId::new(row.get::<_, String>(2))
            .map_err(|error| StoreError::Domain(error.to_string()))?,
        TaskId::new(row.get::<_, String>(3))
            .map_err(|error| StoreError::Domain(error.to_string()))?,
        row.get::<_, String>(4),
        ArtifactChecksum::sha256(row.get::<_, String>(5))
            .map_err(|error| StoreError::Domain(error.to_string()))?,
        row.get::<_, i64>(6).max(0) as u64,
        ArtifactRetentionClass::parse(&row.get::<_, String>(7))
            .map_err(|error| StoreError::Domain(error.to_string()))?,
        unix_secs_to_system_time(row.get(8)),
    )
    .map_err(|error| StoreError::Domain(error.to_string()))
}

#[cfg(feature = "postgres")]
fn postgres_cleanup_retention(
    store: &PostgresStore,
    cutoffs: RetentionCutoffs,
    dry_run: bool,
) -> Result<AppRetentionCleanupSummary, StoreError> {
    let job_output_cutoff = system_time_to_unix_secs(cutoffs.job_output);
    let facts_cutoff = system_time_to_unix_secs(cutoffs.facts);
    let metrics_cutoff = system_time_to_unix_secs(cutoffs.metrics);
    let agent_logs_cutoff = system_time_to_unix_secs(cutoffs.agent_logs);
    let summary = AppRetentionCleanupSummary {
        job_output_chunks: postgres_count_before(
            store,
            "job_output_chunks",
            "created_at",
            job_output_cutoff,
        )?,
        facts_snapshots: postgres_count_before(
            store,
            "facts_snapshots",
            "collected_at",
            facts_cutoff,
        )?,
        metrics_snapshots: postgres_count_before(
            store,
            "metrics_snapshots",
            "collected_at",
            metrics_cutoff,
        )?,
        agent_log_chunks: postgres_count_before(
            store,
            "agent_log_chunks",
            "collected_at",
            agent_logs_cutoff,
        )?,
    };
    if dry_run {
        return Ok(summary);
    }
    postgres_delete_before(store, "job_output_chunks", "created_at", job_output_cutoff)?;
    postgres_delete_before(store, "facts_snapshots", "collected_at", facts_cutoff)?;
    postgres_delete_before(store, "metrics_snapshots", "collected_at", metrics_cutoff)?;
    postgres_delete_before(store, "agent_log_chunks", "collected_at", agent_logs_cutoff)?;
    Ok(summary)
}

#[cfg(feature = "postgres")]
fn postgres_save_remediation_request(
    store: &PostgresStore,
    request: &AppRemediationRequestRecord,
) -> Result<(), StoreError> {
    store
        .checkout_client()?
        .execute(
            "INSERT INTO remediation_requests (
                id, policy_id, policy_name, agent_id, runbook_ref, status,
                approval_required, risk_summary, job_id, origin_drift_report_id, policy_version,
                created_at, updated_at
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
            &[
                &request.id,
                &request.policy_id,
                &request.policy_name,
                &request.agent_id,
                &request.runbook_ref,
                &request.status,
                &request.approval_required,
                &request.risk_summary,
                &request.job_id,
                &request.origin_drift_report_id.map(DriftReportId::as_i64),
                &request.policy_version.map(i64::from),
                &system_time_to_unix_secs(request.created_at),
                &system_time_to_unix_secs(request.updated_at),
            ],
        )
        .map(|_| ())
        .map_err(|error| {
            postgres_constraint_or_context(error, "postgres remediation request insert failed")
        })
}

#[cfg(feature = "postgres")]
fn postgres_save_remediation_proposal(
    store: &PostgresStore,
    request: &AppRemediationRequestRecord,
    audit: &AuditEvent,
) -> Result<AppRemediationProposalSave, StoreError> {
    let mut client = store.checkout_client()?;
    let mut transaction = client
        .transaction()
        .map_err(|_| postgres_error("postgres remediation proposal transaction failed"))?;
    let existing = transaction
        .query_opt(
            "SELECT id, policy_id, policy_name, agent_id, runbook_ref, status,
                    approval_required, risk_summary, job_id, origin_drift_report_id,
                    policy_version, created_at, updated_at
             FROM remediation_requests
             WHERE agent_id = $1 AND policy_id = $2
               AND origin_drift_report_id IS NOT NULL
               AND status NOT IN ('resolved', 'failed', 'rejected', 'expired', 'canceled')
             ORDER BY created_at, id
             LIMIT 1",
            &[&request.agent_id, &request.policy_id],
        )
        .map_err(|_| postgres_error("postgres remediation proposal lookup failed"))?;
    if let Some(row) = existing {
        let remediation = postgres_row_to_remediation_request_record(&row)?;
        transaction
            .commit()
            .map_err(|_| postgres_error("postgres remediation proposal commit failed"))?;
        return Ok(AppRemediationProposalSave {
            remediation,
            created: false,
        });
    }
    let insert = transaction.execute(
        "INSERT INTO remediation_requests (
                id, policy_id, policy_name, agent_id, runbook_ref, status,
                approval_required, risk_summary, job_id, origin_drift_report_id, policy_version,
                created_at, updated_at
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        &[
            &request.id,
            &request.policy_id,
            &request.policy_name,
            &request.agent_id,
            &request.runbook_ref,
            &request.status,
            &request.approval_required,
            &request.risk_summary,
            &request.job_id,
            &request.origin_drift_report_id.map(DriftReportId::as_i64),
            &request.policy_version.map(i64::from),
            &system_time_to_unix_secs(request.created_at),
            &system_time_to_unix_secs(request.updated_at),
        ],
    );
    if let Err(error) = insert {
        let _ = transaction.rollback();
        if let Some(remediation) =
            postgres_find_active_remediation_request(store, &request.agent_id, &request.policy_id)?
        {
            return Ok(AppRemediationProposalSave {
                remediation,
                created: false,
            });
        }
        return Err(postgres_constraint_or_context(
            error,
            "postgres remediation proposal insert failed",
        ));
    }
    let (value_kind, value_text) = encode_audit_value(&audit.value);
    transaction
        .execute(
            "INSERT INTO audit_events (
                category, action, actor, target, value_kind, value_text, occurred_at
             ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
            &[
                &audit.category.as_str(),
                &audit.action,
                &audit.actor.as_str(),
                &audit.target.as_str(),
                &value_kind,
                &value_text,
                &system_time_to_unix_secs(audit.occurred_at),
            ],
        )
        .map_err(|_| postgres_error("postgres remediation proposal audit insert failed"))?;
    transaction
        .commit()
        .map_err(|_| postgres_error("postgres remediation proposal commit failed"))?;
    Ok(AppRemediationProposalSave {
        remediation: request.clone(),
        created: true,
    })
}

#[cfg(feature = "postgres")]
fn postgres_save_verified_drift_proposal(
    store: &PostgresStore,
    input: &AppPersistVerifiedDriftProposalInput,
) -> Result<AppPersistVerifiedDriftProposalOutput, StoreError> {
    let job_id = input
        .provenance
        .job_id
        .as_ref()
        .ok_or_else(|| StoreError::Domain("verified drift requires job id".to_owned()))?;
    let task_id = input
        .provenance
        .task_id
        .as_ref()
        .ok_or_else(|| StoreError::Domain("verified drift requires task id".to_owned()))?;
    let mut client = store.checkout_client()?;
    let mut transaction = client
        .transaction()
        .map_err(|_| postgres_error("postgres verified drift proposal transaction failed"))?;
    let existing = transaction
        .query_opt(
            "SELECT id, agent_id, policy_name, status, expected, actual, checked_at,
                    severity, acknowledged_at, acknowledged_by, resolved_at, resolution_job_id,
                    job_id, task_id, policy_id, policy_version, purpose
             FROM drift_reports WHERE job_id = $1 AND task_id = $2",
            &[&job_id.as_str(), &task_id.as_str()],
        )
        .map_err(|_| postgres_error("postgres verified drift lookup failed"))?;
    if let Some(row) = existing {
        let report = postgres_row_to_drift_report_record(&row)?;
        let remediation = transaction
            .query_opt(
                "SELECT id, policy_id, policy_name, agent_id, runbook_ref, status,
                        approval_required, risk_summary, job_id, origin_drift_report_id,
                        policy_version, created_at, updated_at
                 FROM remediation_requests WHERE origin_drift_report_id = $1
                 ORDER BY created_at, id LIMIT 1",
                &[&report.id.as_i64()],
            )
            .map_err(|_| postgres_error("postgres verified remediation lookup failed"))?
            .map(|row| postgres_row_to_remediation_request_record(&row))
            .transpose()?
            .ok_or_else(|| {
                StoreError::Domain("verified drift has no remediation proposal".to_owned())
            })?;
        transaction
            .commit()
            .map_err(|_| postgres_error("postgres verified drift proposal commit failed"))?;
        return Ok(AppPersistVerifiedDriftProposalOutput {
            report,
            proposal: AppRemediationProposalSave {
                remediation,
                created: false,
            },
        });
    }
    let row = transaction
        .query_one(
            "INSERT INTO drift_reports (
                agent_id, policy_name, status, severity, expected, actual, checked_at,
                job_id, task_id, policy_id, policy_version, purpose
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) RETURNING id",
            &[
                &input.agent_id,
                &input.report.policy_name,
                &drift_status_to_str(&input.report.status),
                &drift_severity_to_str(input.report.severity),
                &input.report.expected,
                &input.report.actual,
                &system_time_to_unix_secs(input.checked_at),
                &job_id.as_str(),
                &task_id.as_str(),
                &input.provenance.policy_id,
                &input.provenance.policy_version.map(i64::from),
                &input.provenance.purpose.map(DriftCheckPurpose::as_str),
            ],
        )
        .map_err(|error| {
            postgres_constraint_or_context(error, "postgres verified drift insert failed")
        })?;
    let report_id =
        DriftReportId::new(row.get(0)).map_err(|error| StoreError::Domain(format!("{error:?}")))?;
    postgres_insert_audit_in_transaction(&mut transaction, &input.drift_audit)?;
    let mut remediation = input.remediation.clone();
    remediation.origin_drift_report_id = Some(report_id);
    let existing = transaction
        .query_opt(
            "SELECT id, policy_id, policy_name, agent_id, runbook_ref, status,
                    approval_required, risk_summary, job_id, origin_drift_report_id,
                    policy_version, created_at, updated_at
             FROM remediation_requests
             WHERE agent_id = $1 AND policy_id = $2
               AND origin_drift_report_id IS NOT NULL
               AND status NOT IN ('resolved', 'failed', 'rejected', 'expired', 'canceled')
             ORDER BY created_at, id LIMIT 1",
            &[&remediation.agent_id, &remediation.policy_id],
        )
        .map_err(|_| postgres_error("postgres active remediation lookup failed"))?;
    let proposal = if let Some(row) = existing {
        AppRemediationProposalSave {
            remediation: postgres_row_to_remediation_request_record(&row)?,
            created: false,
        }
    } else {
        postgres_insert_remediation_in_transaction(&mut transaction, &remediation)?;
        postgres_insert_audit_in_transaction(&mut transaction, &input.proposal_audit)?;
        AppRemediationProposalSave {
            remediation,
            created: true,
        }
    };
    transaction
        .commit()
        .map_err(|_| postgres_error("postgres verified drift proposal commit failed"))?;
    Ok(AppPersistVerifiedDriftProposalOutput {
        report: AppDriftReportRecord {
            id: report_id,
            agent_id: input.agent_id.clone(),
            report: input.report.clone(),
            provenance: input.provenance.clone(),
            checked_at: input.checked_at,
        },
        proposal,
    })
}

#[cfg(feature = "postgres")]
fn postgres_insert_audit_in_transaction(
    transaction: &mut postgres::Transaction<'_>,
    audit: &AuditEvent,
) -> Result<(), StoreError> {
    let (value_kind, value_text) = encode_audit_value(&audit.value);
    transaction
        .execute(
            "INSERT INTO audit_events (
                category, action, actor, target, value_kind, value_text, occurred_at
             ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
            &[
                &audit.category.as_str(),
                &audit.action,
                &audit.actor.as_str(),
                &audit.target.as_str(),
                &value_kind,
                &value_text,
                &system_time_to_unix_secs(audit.occurred_at),
            ],
        )
        .map(|_| ())
        .map_err(|_| postgres_error("postgres verified drift audit insert failed"))
}

#[cfg(feature = "postgres")]
fn postgres_insert_remediation_in_transaction(
    transaction: &mut postgres::Transaction<'_>,
    request: &AppRemediationRequestRecord,
) -> Result<(), StoreError> {
    transaction
        .execute(
            "INSERT INTO remediation_requests (
                id, policy_id, policy_name, agent_id, runbook_ref, status,
                approval_required, risk_summary, job_id, origin_drift_report_id, policy_version,
                created_at, updated_at
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
            &[
                &request.id,
                &request.policy_id,
                &request.policy_name,
                &request.agent_id,
                &request.runbook_ref,
                &request.status,
                &request.approval_required,
                &request.risk_summary,
                &request.job_id,
                &request.origin_drift_report_id.map(DriftReportId::as_i64),
                &request.policy_version.map(i64::from),
                &system_time_to_unix_secs(request.created_at),
                &system_time_to_unix_secs(request.updated_at),
            ],
        )
        .map(|_| ())
        .map_err(|error| {
            postgres_constraint_or_context(error, "postgres verified remediation insert failed")
        })
}

#[cfg(feature = "postgres")]
fn postgres_find_active_remediation_request(
    store: &PostgresStore,
    agent_id: &str,
    policy_id: &str,
) -> Result<Option<AppRemediationRequestRecord>, StoreError> {
    store
        .checkout_client()?
        .query_opt(
            "SELECT id, policy_id, policy_name, agent_id, runbook_ref, status,
                    approval_required, risk_summary, job_id, origin_drift_report_id,
                    policy_version, created_at, updated_at
             FROM remediation_requests
             WHERE agent_id = $1 AND policy_id = $2
               AND origin_drift_report_id IS NOT NULL
               AND status NOT IN ('resolved', 'failed', 'rejected', 'expired', 'canceled')
             ORDER BY created_at, id
             LIMIT 1",
            &[&agent_id, &policy_id],
        )
        .map_err(|_| postgres_error("postgres remediation proposal conflict lookup failed"))?
        .map(|row| postgres_row_to_remediation_request_record(&row))
        .transpose()
}

#[cfg(feature = "postgres")]
fn postgres_find_remediation_request(
    store: &PostgresStore,
    request_id: &str,
) -> Result<Option<AppRemediationRequestRecord>, StoreError> {
    let row = store
        .checkout_client()?
        .query_opt(
            "SELECT id, policy_id, policy_name, agent_id, runbook_ref, status,
                    approval_required, risk_summary, job_id, origin_drift_report_id, policy_version,
                    created_at, updated_at
             FROM remediation_requests
             WHERE id = $1",
            &[&request_id],
        )
        .map_err(|_| postgres_error("postgres remediation request query failed"))?;
    row.map(|row| postgres_row_to_remediation_request_record(&row))
        .transpose()
}

#[cfg(feature = "postgres")]
fn postgres_list_remediation_requests(
    store: &PostgresStore,
    agent_id: Option<&str>,
    policy_id: Option<&str>,
    limit: usize,
) -> Result<Vec<AppRemediationRequestRecord>, StoreError> {
    let limit = limit.clamp(1, 500) as i64;
    let rows = store
        .checkout_client()?
        .query(
            "SELECT id, policy_id, policy_name, agent_id, runbook_ref, status,
                    approval_required, risk_summary, job_id, origin_drift_report_id, policy_version,
                    created_at, updated_at
             FROM remediation_requests
             WHERE ($1::TEXT IS NULL OR agent_id = $1)
               AND ($2::TEXT IS NULL OR policy_id = $2)
             ORDER BY created_at, id
             LIMIT $3",
            &[&agent_id, &policy_id, &limit],
        )
        .map_err(|_| postgres_error("postgres remediation request list failed"))?;
    rows.into_iter()
        .map(|row| postgres_row_to_remediation_request_record(&row))
        .collect()
}

#[cfg(feature = "postgres")]
fn postgres_list_pending_remediation_verification_recovery(
    store: &PostgresStore,
    limit: usize,
) -> Result<Vec<AppRemediationRequestRecord>, StoreError> {
    let limit = limit.clamp(1, 100) as i64;
    let rows = store
        .checkout_client()?
        .query(
            "SELECT remediation_requests.id, policy_id, policy_name, agent_id, runbook_ref, status,
                    approval_required, risk_summary, job_id, origin_drift_report_id, policy_version,
                    remediation_requests.created_at, remediation_requests.updated_at
             FROM remediation_requests
             LEFT JOIN remediation_verification_jobs
               ON remediation_verification_jobs.remediation_id = remediation_requests.id
             WHERE remediation_requests.status = 'succeeded_pending_verify'
               AND remediation_verification_jobs.remediation_id IS NULL
             ORDER BY remediation_requests.created_at, remediation_requests.id
             LIMIT $1",
            &[&limit],
        )
        .map_err(|_| postgres_error("postgres remediation verification recovery list failed"))?;
    rows.into_iter()
        .map(|row| postgres_row_to_remediation_request_record(&row))
        .collect()
}

#[cfg(feature = "postgres")]
fn postgres_update_remediation_request_status(
    store: &PostgresStore,
    request_id: &str,
    status: &str,
    job_id: Option<&str>,
    updated_at: SystemTime,
) -> Result<(), StoreError> {
    let changed = store
        .checkout_client()?
        .execute(
            "UPDATE remediation_requests
             SET status = $2, job_id = $3, updated_at = $4
             WHERE id = $1",
            &[
                &request_id,
                &status,
                &job_id,
                &system_time_to_unix_secs(updated_at),
            ],
        )
        .map_err(|_| postgres_error("postgres remediation request update failed"))?;
    if changed == 0 {
        return Err(StoreError::NotFound);
    }
    Ok(())
}

#[cfg(feature = "postgres")]
fn postgres_row_to_remediation_request_record(
    row: &postgres::Row,
) -> Result<AppRemediationRequestRecord, StoreError> {
    Ok(AppRemediationRequestRecord {
        id: row.get(0),
        policy_id: row.get(1),
        policy_name: row.get(2),
        agent_id: row.get(3),
        runbook_ref: row.get(4),
        status: row.get(5),
        approval_required: row.get(6),
        risk_summary: row.get(7),
        job_id: row.get(8),
        origin_drift_report_id: row
            .get::<_, Option<i64>>(9)
            .map(DriftReportId::new)
            .transpose()
            .map_err(|_| postgres_error("invalid remediation origin id"))?,
        policy_version: row
            .get::<_, Option<i64>>(10)
            .and_then(|value| u32::try_from(value).ok()),
        created_at: unix_secs_to_system_time(row.get(11)),
        updated_at: unix_secs_to_system_time(row.get(12)),
    })
}

#[cfg(feature = "postgres")]
fn postgres_count_before(
    store: &PostgresStore,
    table: &'static str,
    column: &'static str,
    cutoff: i64,
) -> Result<usize, StoreError> {
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE {column} < $1");
    store
        .checkout_client()?
        .query_one(&sql, &[&cutoff])
        .map(|row| row.get::<_, i64>(0).max(0) as usize)
        .map_err(|_| postgres_error("postgres retention count failed"))
}

#[cfg(feature = "postgres")]
fn postgres_delete_before(
    store: &PostgresStore,
    table: &'static str,
    column: &'static str,
    cutoff: i64,
) -> Result<usize, StoreError> {
    let sql = format!("DELETE FROM {table} WHERE {column} < $1");
    store
        .checkout_client()?
        .execute(&sql, &[&cutoff])
        .map(|changed| changed as usize)
        .map_err(|_| postgres_error("postgres retention delete failed"))
}

#[cfg(feature = "postgres")]
fn postgres_update_job_status(
    store: &PostgresStore,
    job_id: &str,
    status: JobStatus,
) -> Result<bool, StoreError> {
    store
        .checkout_client()?
        .execute(
            "UPDATE jobs SET status = $2 WHERE id = $1",
            &[&job_id, &job_status_to_str(status)],
        )
        .map(|changed| changed > 0)
        .map_err(|_| postgres_error("postgres job status update failed"))
}

#[cfg(feature = "postgres")]
fn postgres_update_task_assignment_status(
    store: &PostgresStore,
    task_id: &str,
    status: AssignmentStatus,
    occurred_at: SystemTime,
    last_error: Option<&str>,
) -> Result<bool, StoreError> {
    let status_value = assignment_status_to_str(status);
    let occurred_at = system_time_to_unix_secs(occurred_at);
    let mut client = store.checkout_client()?;
    let result = match status {
        AssignmentStatus::Dispatched => client.execute(
            "UPDATE task_assignments
             SET status = $2, dispatched_at = $3
             WHERE id = $1",
            &[&task_id, &status_value, &occurred_at],
        ),
        AssignmentStatus::Accepted => client.execute(
            "UPDATE task_assignments
             SET status = $2, accepted_at = $3
             WHERE id = $1",
            &[&task_id, &status_value, &occurred_at],
        ),
        AssignmentStatus::Started => client.execute(
            "UPDATE task_assignments
             SET status = $2, started_at = $3
             WHERE id = $1",
            &[&task_id, &status_value, &occurred_at],
        ),
        AssignmentStatus::Succeeded
        | AssignmentStatus::Failed
        | AssignmentStatus::Rejected
        | AssignmentStatus::Canceled
        | AssignmentStatus::Expired => client.execute(
            "UPDATE task_assignments
             SET status = $2, completed_at = $3, last_error = COALESCE($4, last_error)
             WHERE id = $1",
            &[&task_id, &status_value, &occurred_at, &last_error],
        ),
        AssignmentStatus::Queued => client.execute(
            "UPDATE task_assignments
             SET status = $2, last_error = COALESCE($3, last_error)
             WHERE id = $1",
            &[&task_id, &status_value, &last_error],
        ),
    };
    result
        .map(|changed| changed > 0)
        .map_err(|_| postgres_error("postgres task assignment status update failed"))
}

#[cfg(feature = "postgres")]
fn postgres_update_active_task_assignment_status(
    store: &PostgresStore,
    task_id: &str,
    status: AssignmentStatus,
    occurred_at: SystemTime,
    last_error: Option<&str>,
) -> Result<bool, StoreError> {
    let Some(current_status) = postgres_find_task_assignment_status(store, task_id)? else {
        return Ok(false);
    };
    if assignment_status_value_is_terminal(&current_status) {
        return Ok(false);
    }
    postgres_update_task_assignment_status(store, task_id, status, occurred_at, last_error)
}

#[cfg(feature = "postgres")]
fn postgres_claim_task_assignment_for_dispatch(
    store: &PostgresStore,
    task_id: &str,
    occurred_at: SystemTime,
) -> Result<bool, StoreError> {
    store
        .checkout_client()?
        .execute(
            "UPDATE task_assignments
             SET status = 'dispatched', dispatched_at = $2, last_error = ''
             WHERE id = $1 AND status = 'queued'",
            &[&task_id, &system_time_to_unix_secs(occurred_at)],
        )
        .map(|changed| changed > 0)
        .map_err(|_| postgres_error("postgres task assignment dispatch claim failed"))
}

#[cfg(feature = "postgres")]
fn postgres_release_task_assignment_dispatch_claim(
    store: &PostgresStore,
    task_id: &str,
    reason: &str,
) -> Result<bool, StoreError> {
    store
        .checkout_client()?
        .execute(
            "UPDATE task_assignments
             SET status = 'queued', dispatched_at = NULL, last_error = $2
             WHERE id = $1 AND status = 'dispatched'",
            &[&task_id, &reason],
        )
        .map(|changed| changed > 0)
        .map_err(|_| postgres_error("postgres task assignment dispatch claim release failed"))
}

#[cfg(feature = "postgres")]
fn postgres_find_task_assignment_status(
    store: &PostgresStore,
    task_id: &str,
) -> Result<Option<String>, StoreError> {
    store
        .checkout_client()?
        .query_opt(
            "SELECT status FROM task_assignments WHERE id = $1",
            &[&task_id],
        )
        .map(|row| row.map(|row| row.get(0)))
        .map_err(|_| postgres_error("postgres task assignment status query failed"))
}

#[cfg(feature = "postgres")]
fn postgres_find_task_assignment_job_id(
    store: &PostgresStore,
    task_id: &str,
) -> Result<Option<String>, StoreError> {
    store
        .checkout_client()?
        .query_opt(
            "SELECT job_id FROM task_assignments WHERE id = $1",
            &[&task_id],
        )
        .map(|row| row.map(|row| row.get(0)))
        .map_err(|_| postgres_error("postgres task assignment job query failed"))
}

#[cfg(feature = "postgres")]
fn postgres_assignment_statuses_for_job(
    store: &PostgresStore,
    job_id: &str,
) -> Result<Vec<AssignmentStatus>, StoreError> {
    let mut client = store.checkout_client()?;
    let rows = client
        .query(
            "SELECT status
             FROM task_assignments
             WHERE job_id = $1
             ORDER BY created_at, id",
            &[&job_id],
        )
        .map_err(|_| postgres_error("postgres task assignment list failed"))?;
    rows.into_iter()
        .map(|row| {
            let status: String = row.get(0);
            AssignmentStatus::parse(&status).ok_or_else(|| {
                StoreError::Domain(format!("invalid task assignment status: {status}"))
            })
        })
        .collect()
}

#[cfg(feature = "postgres")]
fn postgres_job_strategy(
    store: &PostgresStore,
    job_id: &str,
) -> Result<Option<JobStrategyRecord>, StoreError> {
    store
        .checkout_client()?
        .query_opt(
            "SELECT strategy_concurrency, strategy_max_failures
             FROM jobs
             WHERE id = $1",
            &[&job_id],
        )
        .map(|row| {
            row.map(|row| {
                let concurrency = row.get::<_, i64>(0).max(1) as u32;
                let max_failures = row
                    .get::<_, Option<i64>>(1)
                    .and_then(|value| u32::try_from(value).ok())
                    .filter(|value| *value > 0);
                JobStrategyRecord {
                    concurrency,
                    max_failures,
                }
            })
        })
        .map_err(|_| postgres_error("postgres job strategy query failed"))
}

#[cfg(feature = "postgres")]
fn postgres_recompute_job_status_from_assignments(
    store: &PostgresStore,
    job_id: &str,
) -> Result<Option<JobStatus>, StoreError> {
    let strategy = postgres_job_strategy(store, job_id)?;
    let statuses = postgres_assignment_statuses_for_job(store, job_id)?;
    if statuses.is_empty() && strategy.is_none() {
        return Ok(None);
    }
    let status = aggregate_job_status(
        &statuses,
        strategy.and_then(|strategy| strategy.max_failures),
    );
    postgres_update_job_status(store, job_id, status)?;
    Ok(Some(status))
}

#[cfg(feature = "postgres")]
fn postgres_append_job_output_chunk(
    store: &PostgresStore,
    chunk: &JobOutputChunk,
) -> Result<(), StoreError> {
    let stream = output_stream_to_str(chunk.stream);
    let sequence = chunk.sequence as i64;
    let mut client = store.checkout_client()?;
    let result = client.execute(
        "INSERT INTO job_output_chunks (
            job_id, agent_id, stream, chunk_index, body
         ) VALUES ($1, $2, $3, $4, $5)",
        &[
            &chunk.job_id.as_str(),
            &chunk.agent_id.as_str(),
            &stream,
            &sequence,
            &chunk.body.as_str(),
        ],
    );

    match result {
        Ok(_) => Ok(()),
        Err(error) if postgres_is_unique_violation(&error) => {
            let existing_body = client
                .query_opt(
                    "SELECT body
                     FROM job_output_chunks
                     WHERE job_id = $1
                       AND agent_id = $2
                       AND stream = $3
                       AND chunk_index = $4",
                    &[
                        &chunk.job_id.as_str(),
                        &chunk.agent_id.as_str(),
                        &stream,
                        &sequence,
                    ],
                )
                .map_err(|_| postgres_error("postgres job output conflict query failed"))?
                .map(|row| row.get::<_, String>(0));
            if existing_body.as_deref() == Some(chunk.body.as_str()) {
                Ok(())
            } else {
                Err(StoreError::ConstraintViolation(
                    "postgres unique constraint violation".to_owned(),
                ))
            }
        }
        Err(error) => Err(postgres_constraint_or_context(
            error,
            "postgres job output insert failed",
        )),
    }
}

#[cfg(feature = "postgres")]
fn postgres_list_job_output_chunks(
    store: &PostgresStore,
    job_id: &str,
    agent_id: Option<&str>,
) -> Result<Vec<JobOutputChunk>, StoreError> {
    let mut client = store.checkout_client()?;
    let rows = if let Some(agent_id) = agent_id {
        client
            .query(
                "SELECT job_id, agent_id, stream, chunk_index, body
                 FROM job_output_chunks
                 WHERE job_id = $1 AND agent_id = $2
                 ORDER BY chunk_index",
                &[&job_id, &agent_id],
            )
            .map_err(|_| postgres_error("postgres job output query failed"))?
    } else {
        client
            .query(
                "SELECT job_id, agent_id, stream, chunk_index, body
                 FROM job_output_chunks
                 WHERE job_id = $1
                 ORDER BY agent_id, chunk_index, stream",
                &[&job_id],
            )
            .map_err(|_| postgres_error("postgres job output query failed"))?
    };

    rows.into_iter()
        .map(|row| {
            Ok(JobOutputChunk {
                job_id: row.get(0),
                agent_id: row.get(1),
                stream: parse_output_stream(&row.get::<_, String>(2)),
                sequence: row.get::<_, i64>(3).max(0) as u64,
                body: row.get(4),
            })
        })
        .collect()
}

#[cfg(feature = "postgres")]
fn postgres_insert_facts_snapshot(
    store: &PostgresStore,
    agent_id: &str,
    body: &str,
    collected_at: SystemTime,
) -> Result<(), StoreError> {
    store
        .checkout_client()?
        .execute(
            "INSERT INTO facts_snapshots (agent_id, body, collected_at)
             VALUES ($1, $2, $3)",
            &[&agent_id, &body, &system_time_to_unix_secs(collected_at)],
        )
        .map(|_| ())
        .map_err(|_| postgres_error("postgres facts snapshot insert failed"))
}

#[cfg(feature = "postgres")]
fn postgres_latest_facts_snapshot(
    store: &PostgresStore,
    agent_id: &str,
) -> Result<Option<AppFactsSnapshotRecord>, StoreError> {
    store
        .checkout_client()?
        .query_opt(
            "SELECT agent_id, body, collected_at
             FROM facts_snapshots
             WHERE agent_id = $1
             ORDER BY collected_at DESC, id DESC
             LIMIT 1",
            &[&agent_id],
        )
        .map(|row| {
            row.map(|row| AppFactsSnapshotRecord {
                agent_id: row.get(0),
                body: row.get(1),
                collected_at: unix_secs_to_system_time(row.get(2)),
            })
        })
        .map_err(|_| postgres_error("postgres facts snapshot query failed"))
}

#[cfg(feature = "postgres")]
fn postgres_list_facts_snapshots(
    store: &PostgresStore,
    agent_id: &str,
    limit: usize,
    before: Option<SnapshotPageCursor>,
) -> Result<Vec<AppFactsSnapshotPageRecord>, StoreError> {
    let limit = limit.clamp(1, 501) as i64;
    let mut client = store.checkout_client()?;
    let rows = if let Some(before) = before {
        let before_secs = system_time_to_unix_secs(before.occurred_at);
        client
            .query(
                "SELECT id, agent_id, body, collected_at
                 FROM facts_snapshots
                 WHERE agent_id = $1
                   AND (collected_at < $2 OR (collected_at = $2 AND id < $3))
                 ORDER BY collected_at DESC, id DESC
                 LIMIT $4",
                &[&agent_id, &before_secs, &before.row_id, &limit],
            )
            .map_err(|_| postgres_error("postgres facts snapshot page query failed"))?
    } else {
        client
            .query(
                "SELECT id, agent_id, body, collected_at
                 FROM facts_snapshots
                 WHERE agent_id = $1
                 ORDER BY collected_at DESC, id DESC
                 LIMIT $2",
                &[&agent_id, &limit],
            )
            .map_err(|_| postgres_error("postgres facts snapshot page query failed"))?
    };

    Ok(rows
        .into_iter()
        .map(|row| postgres_row_to_facts_snapshot_page_record(&row))
        .collect())
}

#[cfg(feature = "postgres")]
fn postgres_row_to_facts_snapshot_page_record(row: &postgres::Row) -> AppFactsSnapshotPageRecord {
    let id: i64 = row.get(0);
    let collected_at = unix_secs_to_system_time(row.get(3));
    AppFactsSnapshotPageRecord {
        agent_id: row.get(1),
        body: row.get(2),
        collected_at,
        cursor: SnapshotPageCursor {
            occurred_at: collected_at,
            row_id: id,
        },
    }
}

#[cfg(feature = "postgres")]
fn postgres_insert_metrics_snapshot(
    store: &PostgresStore,
    agent_id: &str,
    body: &str,
    collected_at: SystemTime,
) -> Result<(), StoreError> {
    store
        .checkout_client()?
        .execute(
            "INSERT INTO metrics_snapshots (agent_id, body, collected_at)
             VALUES ($1, $2, $3)",
            &[&agent_id, &body, &system_time_to_unix_secs(collected_at)],
        )
        .map(|_| ())
        .map_err(|_| postgres_error("postgres metrics snapshot insert failed"))
}

#[cfg(feature = "postgres")]
fn postgres_latest_metrics_snapshot(
    store: &PostgresStore,
    agent_id: &str,
) -> Result<Option<AppMetricsSnapshotRecord>, StoreError> {
    store
        .checkout_client()?
        .query_opt(
            "SELECT agent_id, body, collected_at
             FROM metrics_snapshots
             WHERE agent_id = $1
             ORDER BY collected_at DESC, id DESC
             LIMIT 1",
            &[&agent_id],
        )
        .map(|row| {
            row.map(|row| AppMetricsSnapshotRecord {
                agent_id: row.get(0),
                body: row.get(1),
                collected_at: unix_secs_to_system_time(row.get(2)),
            })
        })
        .map_err(|_| postgres_error("postgres metrics snapshot query failed"))
}

#[cfg(feature = "postgres")]
fn postgres_list_metrics_snapshots(
    store: &PostgresStore,
    agent_id: &str,
    limit: usize,
    before: Option<SnapshotPageCursor>,
) -> Result<Vec<AppMetricsSnapshotPageRecord>, StoreError> {
    let limit = limit.clamp(1, 501) as i64;
    let mut client = store.checkout_client()?;
    let rows = if let Some(before) = before {
        let before_secs = system_time_to_unix_secs(before.occurred_at);
        client
            .query(
                "SELECT id, agent_id, body, collected_at
                 FROM metrics_snapshots
                 WHERE agent_id = $1
                   AND (collected_at < $2 OR (collected_at = $2 AND id < $3))
                 ORDER BY collected_at DESC, id DESC
                 LIMIT $4",
                &[&agent_id, &before_secs, &before.row_id, &limit],
            )
            .map_err(|_| postgres_error("postgres metrics snapshot page query failed"))?
    } else {
        client
            .query(
                "SELECT id, agent_id, body, collected_at
                 FROM metrics_snapshots
                 WHERE agent_id = $1
                 ORDER BY collected_at DESC, id DESC
                 LIMIT $2",
                &[&agent_id, &limit],
            )
            .map_err(|_| postgres_error("postgres metrics snapshot page query failed"))?
    };

    Ok(rows
        .into_iter()
        .map(|row| postgres_row_to_metrics_snapshot_page_record(&row))
        .collect())
}

#[cfg(feature = "postgres")]
fn postgres_row_to_metrics_snapshot_page_record(
    row: &postgres::Row,
) -> AppMetricsSnapshotPageRecord {
    let id: i64 = row.get(0);
    let collected_at = unix_secs_to_system_time(row.get(3));
    AppMetricsSnapshotPageRecord {
        agent_id: row.get(1),
        body: row.get(2),
        collected_at,
        cursor: SnapshotPageCursor {
            occurred_at: collected_at,
            row_id: id,
        },
    }
}

#[cfg(feature = "postgres")]
fn postgres_list_agent_log_chunks(
    store: &PostgresStore,
    agent_id: &str,
    limit: usize,
    before: Option<SnapshotPageCursor>,
) -> Result<Vec<AppAgentLogChunkPageRecord>, StoreError> {
    let limit = limit.clamp(1, 501) as i64;
    let mut client = store.checkout_client()?;
    let rows = if let Some(before) = before {
        let before_secs = system_time_to_unix_secs(before.occurred_at);
        client
            .query(
                "SELECT id, agent_id, line, collected_at
                 FROM agent_log_chunks
                 WHERE agent_id = $1
                   AND (collected_at < $2 OR (collected_at = $2 AND id < $3))
                 ORDER BY collected_at DESC, id DESC
                 LIMIT $4",
                &[&agent_id, &before_secs, &before.row_id, &limit],
            )
            .map_err(|_| postgres_error("postgres agent log page query failed"))?
    } else {
        client
            .query(
                "SELECT id, agent_id, line, collected_at
                 FROM agent_log_chunks
                 WHERE agent_id = $1
                 ORDER BY collected_at DESC, id DESC
                 LIMIT $2",
                &[&agent_id, &limit],
            )
            .map_err(|_| postgres_error("postgres agent log page query failed"))?
    };

    Ok(rows
        .into_iter()
        .map(|row| postgres_row_to_agent_log_chunk_page_record(&row))
        .collect())
}

#[cfg(feature = "postgres")]
fn postgres_row_to_agent_log_chunk_page_record(row: &postgres::Row) -> AppAgentLogChunkPageRecord {
    let id: i64 = row.get(0);
    let collected_at = unix_secs_to_system_time(row.get(3));
    AppAgentLogChunkPageRecord {
        agent_id: row.get(1),
        line: row.get(2),
        collected_at,
        cursor: SnapshotPageCursor {
            occurred_at: collected_at,
            row_id: id,
        },
    }
}

fn status_to_str(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Pending => "pending",
        AgentStatus::Online => "online",
        AgentStatus::Busy => "busy",
        AgentStatus::Degraded => "degraded",
        AgentStatus::Offline => "offline",
        AgentStatus::Disabled => "disabled",
    }
}

fn parse_status(value: &str) -> AgentStatus {
    match value {
        "online" => AgentStatus::Online,
        "busy" => AgentStatus::Busy,
        "degraded" => AgentStatus::Degraded,
        "offline" => AgentStatus::Offline,
        "disabled" => AgentStatus::Disabled,
        _ => AgentStatus::Pending,
    }
}

fn job_status_to_str(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Draft => "draft",
        JobStatus::PendingApproval => "pending_approval",
        JobStatus::Queued => "queued",
        JobStatus::Running => "running",
        JobStatus::PartialSuccess => "partial_success",
        JobStatus::Success => "success",
        JobStatus::Failed => "failed",
        JobStatus::Canceled => "canceled",
        JobStatus::Expired => "expired",
    }
}

fn assignment_status_to_str(status: AssignmentStatus) -> &'static str {
    status.as_str()
}

fn assignment_status_value_is_terminal(value: &str) -> bool {
    matches!(
        value,
        "succeeded" | "failed" | "rejected" | "canceled" | "expired"
    )
}

fn task_risk_to_str(risk: fleet_domain::TaskRisk) -> &'static str {
    match risk {
        fleet_domain::TaskRisk::Low => "low",
        fleet_domain::TaskRisk::Medium => "medium",
        fleet_domain::TaskRisk::High => "high",
    }
}

fn approval_requirement_to_str(requirement: fleet_domain::ApprovalRequirement) -> &'static str {
    match requirement {
        fleet_domain::ApprovalRequirement::NotRequired => "not_required",
        fleet_domain::ApprovalRequirement::AdminConfirmation => "admin_confirmation",
        fleet_domain::ApprovalRequirement::ManualApproval => "manual_approval",
    }
}

fn output_stream_to_str(stream: JobOutputStream) -> &'static str {
    match stream {
        JobOutputStream::Stdout => "stdout",
        JobOutputStream::Stderr => "stderr",
    }
}

fn parse_output_stream(value: &str) -> JobOutputStream {
    match value {
        "stderr" => JobOutputStream::Stderr,
        _ => JobOutputStream::Stdout,
    }
}

#[derive(Debug, Clone)]
struct SigningKeyRotationStorageRow {
    controller_id: String,
    state: String,
    old_fingerprint: String,
    new_fingerprint: Option<String>,
    requested_at: Option<i64>,
    validated_at: Option<i64>,
    activated_at: Option<i64>,
    old_key_verifies_until: Option<i64>,
    retired_at: Option<i64>,
    failed_at: Option<i64>,
    updated_at: i64,
}

fn sqlite_row_to_signing_key_rotation_storage(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SigningKeyRotationStorageRow> {
    Ok(SigningKeyRotationStorageRow {
        controller_id: row.get(0)?,
        state: row.get(1)?,
        old_fingerprint: row.get(2)?,
        new_fingerprint: row.get(3)?,
        requested_at: row.get(4)?,
        validated_at: row.get(5)?,
        activated_at: row.get(6)?,
        old_key_verifies_until: row.get(7)?,
        retired_at: row.get(8)?,
        failed_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

#[cfg(feature = "postgres")]
fn postgres_row_to_signing_key_rotation_storage(
    row: &postgres::Row,
) -> SigningKeyRotationStorageRow {
    SigningKeyRotationStorageRow {
        controller_id: row.get(0),
        state: row.get(1),
        old_fingerprint: row.get(2),
        new_fingerprint: row.get(3),
        requested_at: row.get(4),
        validated_at: row.get(5),
        activated_at: row.get(6),
        old_key_verifies_until: row.get(7),
        retired_at: row.get(8),
        failed_at: row.get(9),
        updated_at: row.get(10),
    }
}

fn signing_key_rotation_record_from_storage(
    row: SigningKeyRotationStorageRow,
) -> Result<SigningKeyRotationRecord, StoreError> {
    let state = SigningKeyRotationState::parse(&row.state).ok_or_else(|| {
        StoreError::Domain("invalid signing key rotation state in store".to_owned())
    })?;
    let old_fingerprint = SigningKeyFingerprint::new(row.old_fingerprint)
        .map_err(|error| StoreError::Domain(error.to_string()))?;
    let new_fingerprint = row
        .new_fingerprint
        .map(SigningKeyFingerprint::new)
        .transpose()
        .map_err(|error| StoreError::Domain(error.to_string()))?;
    let rotation =
        ControllerSigningKeyRotation::from_snapshot(ControllerSigningKeyRotationSnapshot {
            state,
            old_fingerprint,
            new_fingerprint,
            requested_at: row.requested_at.map(unix_secs_to_system_time),
            validated_at: row.validated_at.map(unix_secs_to_system_time),
            activated_at: row.activated_at.map(unix_secs_to_system_time),
            old_key_verifies_until: row.old_key_verifies_until.map(unix_secs_to_system_time),
            retired_at: row.retired_at.map(unix_secs_to_system_time),
            failed_at: row.failed_at.map(unix_secs_to_system_time),
        })
        .map_err(|error| StoreError::Domain(error.to_string()))?;
    Ok(SigningKeyRotationRecord {
        controller_id: row.controller_id,
        rotation,
        updated_at: unix_secs_to_system_time(row.updated_at),
    })
}

#[derive(Debug, Clone)]
struct AgentCertificateLifecycleStorageRow {
    agent_id: String,
    state: String,
    current_serial: Option<String>,
    current_fingerprint: Option<String>,
    current_not_before: Option<i64>,
    current_not_after: Option<i64>,
    next_serial: Option<String>,
    next_fingerprint: Option<String>,
    next_not_before: Option<i64>,
    next_not_after: Option<i64>,
    grace_until: Option<i64>,
    revocation_reason: Option<String>,
    updated_at: i64,
}

#[derive(Debug, Clone)]
struct AgentCertificateStorageParts {
    serial: Option<String>,
    fingerprint: Option<String>,
    not_before: Option<i64>,
    not_after: Option<i64>,
}

fn agent_certificate_storage_parts(
    certificate: Option<&AgentCertificate>,
) -> AgentCertificateStorageParts {
    AgentCertificateStorageParts {
        serial: certificate.map(|certificate| certificate.serial().as_str().to_owned()),
        fingerprint: certificate.map(|certificate| certificate.fingerprint().as_str().to_owned()),
        not_before: certificate
            .map(|certificate| system_time_to_unix_secs(certificate.validity().not_before())),
        not_after: certificate
            .map(|certificate| system_time_to_unix_secs(certificate.validity().not_after())),
    }
}

fn sqlite_row_to_agent_certificate_lifecycle_storage(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<AgentCertificateLifecycleStorageRow> {
    Ok(AgentCertificateLifecycleStorageRow {
        agent_id: row.get(0)?,
        state: row.get(1)?,
        current_serial: row.get(2)?,
        current_fingerprint: row.get(3)?,
        current_not_before: row.get(4)?,
        current_not_after: row.get(5)?,
        next_serial: row.get(6)?,
        next_fingerprint: row.get(7)?,
        next_not_before: row.get(8)?,
        next_not_after: row.get(9)?,
        grace_until: row.get(10)?,
        revocation_reason: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

#[cfg(feature = "postgres")]
fn postgres_row_to_agent_certificate_lifecycle_storage(
    row: &postgres::Row,
) -> AgentCertificateLifecycleStorageRow {
    AgentCertificateLifecycleStorageRow {
        agent_id: row.get(0),
        state: row.get(1),
        current_serial: row.get(2),
        current_fingerprint: row.get(3),
        current_not_before: row.get(4),
        current_not_after: row.get(5),
        next_serial: row.get(6),
        next_fingerprint: row.get(7),
        next_not_before: row.get(8),
        next_not_after: row.get(9),
        grace_until: row.get(10),
        revocation_reason: row.get(11),
        updated_at: row.get(12),
    }
}

fn agent_certificate_lifecycle_record_from_storage(
    row: AgentCertificateLifecycleStorageRow,
) -> Result<AgentCertificateLifecycleRecord, StoreError> {
    let agent_id = AgentId::new(row.agent_id).map_err(StoreError::from)?;
    let state = AgentCertificateLifecycleState::parse(&row.state).ok_or_else(|| {
        StoreError::Domain("invalid agent certificate lifecycle state in store".to_owned())
    })?;
    let current_certificate = agent_certificate_from_storage(
        row.current_serial,
        row.current_fingerprint,
        row.current_not_before,
        row.current_not_after,
    )?;
    let next_certificate = agent_certificate_from_storage(
        row.next_serial,
        row.next_fingerprint,
        row.next_not_before,
        row.next_not_after,
    )?;
    let revocation_reason = row
        .revocation_reason
        .map(|value| {
            AgentCertificateRevocationReason::parse(&value).ok_or_else(|| {
                StoreError::Domain(
                    "invalid agent certificate revocation reason in store".to_owned(),
                )
            })
        })
        .transpose()?;
    let lifecycle = AgentCertificateLifecycle::from_snapshot(AgentCertificateLifecycleSnapshot {
        agent_id: agent_id.clone(),
        state,
        current_certificate,
        next_certificate,
        grace_until: row.grace_until.map(unix_secs_to_system_time),
        revocation_reason,
    })
    .map_err(|error| StoreError::Domain(error.to_string()))?;
    Ok(AgentCertificateLifecycleRecord {
        agent_id,
        lifecycle: lifecycle.snapshot(),
        updated_at: unix_secs_to_system_time(row.updated_at),
    })
}

fn agent_certificate_from_storage(
    serial: Option<String>,
    fingerprint: Option<String>,
    not_before: Option<i64>,
    not_after: Option<i64>,
) -> Result<Option<AgentCertificate>, StoreError> {
    match (serial, fingerprint, not_before, not_after) {
        (None, None, None, None) => Ok(None),
        (Some(serial), Some(fingerprint), Some(not_before), Some(not_after)) => {
            let certificate = AgentCertificate::new(
                AgentCertificateSerial::new(serial)
                    .map_err(|error| StoreError::Domain(error.to_string()))?,
                AgentCertificateFingerprint::new(fingerprint)
                    .map_err(|error| StoreError::Domain(error.to_string()))?,
                AgentCertificateValidity::new(
                    unix_secs_to_system_time(not_before),
                    unix_secs_to_system_time(not_after),
                )
                .map_err(|error| StoreError::Domain(error.to_string()))?,
            )
            .map_err(|error| StoreError::Domain(error.to_string()))?;
            Ok(Some(certificate))
        }
        _ => Err(StoreError::Domain(
            "incomplete agent certificate lifecycle certificate fields in store".to_owned(),
        )),
    }
}

fn capability_snapshot_from_row(
    privilege_level: &str,
    package_manager: Option<&str>,
    service_manager: Option<&str>,
    capabilities_json: &str,
    reported_at: i64,
) -> Result<AgentCapabilitySnapshot, StoreError> {
    let privilege = PrivilegeLevel::parse(privilege_level).ok_or_else(|| {
        StoreError::Domain(format!(
            "invalid capability privilege level: {privilege_level}"
        ))
    })?;
    let package_manager = package_manager
        .map(|value| {
            PackageManager::parse(value).ok_or_else(|| {
                StoreError::Domain(format!("invalid capability package manager: {value}"))
            })
        })
        .transpose()?;
    let service_manager = service_manager
        .map(|value| {
            ServiceManager::parse(value).ok_or_else(|| {
                StoreError::Domain(format!("invalid capability service manager: {value}"))
            })
        })
        .transpose()?;
    let capability_names: Vec<String> = serde_json::from_str(capabilities_json)
        .map_err(|error| StoreError::Domain(error.to_string()))?;
    let capabilities = capability_names
        .into_iter()
        .map(|value| {
            AgentCapability::parse(&value)
                .ok_or_else(|| StoreError::Domain(format!("invalid capability name: {value}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AgentCapabilitySnapshot::reported(
        AgentRuntimeProfile::new(privilege, package_manager, service_manager, capabilities),
        unix_secs_to_system_time(reported_at),
    ))
}

fn drift_status_to_str(status: &DriftStatus) -> &'static str {
    match status {
        DriftStatus::Compliant => "compliant",
        DriftStatus::Drifted => "drifted",
        DriftStatus::Unknown => "unknown",
    }
}

fn parse_drift_status(value: &str) -> DriftStatus {
    match value {
        "compliant" => DriftStatus::Compliant,
        "drifted" => DriftStatus::Drifted,
        _ => DriftStatus::Unknown,
    }
}

fn drift_severity_to_str(severity: DriftSeverity) -> &'static str {
    match severity {
        DriftSeverity::None => "none",
        DriftSeverity::Warning => "warning",
        DriftSeverity::Critical => "critical",
        DriftSeverity::Unknown => "unknown",
    }
}

fn parse_drift_severity(value: &str) -> DriftSeverity {
    match value {
        "none" => DriftSeverity::None,
        "warning" => DriftSeverity::Warning,
        "critical" => DriftSeverity::Critical,
        _ => DriftSeverity::Unknown,
    }
}

fn row_to_drift_acknowledgement(
    row: &rusqlite::Row<'_>,
    acknowledged_at_index: usize,
    acknowledged_by_index: usize,
    resolved_at_index: usize,
    resolution_job_id_index: usize,
) -> rusqlite::Result<DriftAcknowledgement> {
    let resolved_at = row.get::<_, Option<i64>>(resolved_at_index)?;
    let resolution_job_id = row.get::<_, Option<String>>(resolution_job_id_index)?;
    if let (Some(resolved_at), Some(job_id)) = (resolved_at, resolution_job_id) {
        return Ok(DriftAcknowledgement::Resolved {
            job_id,
            at: unix_secs_to_system_time(resolved_at),
        });
    }
    let acknowledged_at = row.get::<_, Option<i64>>(acknowledged_at_index)?;
    let acknowledged_by = row.get::<_, Option<String>>(acknowledged_by_index)?;
    if let (Some(acknowledged_at), Some(by)) = (acknowledged_at, acknowledged_by) {
        return Ok(DriftAcknowledgement::Acknowledged {
            by,
            at: unix_secs_to_system_time(acknowledged_at),
        });
    }
    Ok(DriftAcknowledgement::Open)
}

fn parse_command_args(value: &str) -> Result<Vec<String>, StoreError> {
    serde_json::from_str(value).map_err(|error| StoreError::Domain(error.to_string()))
}

fn parse_job_target_summaries(value: &str) -> Vec<JobTargetSummaryRecord> {
    if value.is_empty() {
        return Vec::new();
    }
    value
        .split('\u{1f}')
        .filter_map(|item| {
            let mut fields = item.split('\u{1e}');
            let agent_id = fields.next()?;
            let agent_name = fields.next().unwrap_or(agent_id);
            let status = fields.next().unwrap_or("unknown");
            let labels_snapshot = fields.next().unwrap_or("");
            let task_id = fields.next().filter(|value| !value.is_empty());
            let assignment_status = fields.next().filter(|value| !value.is_empty());
            let last_error = fields.next().unwrap_or("");
            if agent_id.is_empty() {
                return None;
            }
            Some(JobTargetSummaryRecord {
                agent_id: agent_id.to_owned(),
                agent_name: agent_name.to_owned(),
                status: status.to_owned(),
                labels: decode_labels(labels_snapshot)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|label| (label.key().to_owned(), label.value().to_owned()))
                    .collect(),
                task_id: task_id.map(str::to_owned),
                assignment_status: assignment_status.map(str::to_owned),
                last_error: last_error.to_owned(),
            })
        })
        .collect()
}

fn encode_labels(labels: &[AgentLabel]) -> String {
    labels
        .iter()
        .map(|label| format!("{}={}", label.key(), label.value()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn decode_labels(value: &str) -> Result<Vec<AgentLabel>, StoreError> {
    value
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| StoreError::Domain(format!("invalid label storage: {line}")))?;
            AgentLabel::new(key, value).map_err(StoreError::from)
        })
        .collect()
}

fn encode_audit_value(value: &AuditValue) -> (&'static str, String) {
    match value {
        AuditValue::Plain(value) => ("plain", value.clone()),
        AuditValue::SecretRef(value) => ("secret_ref", value.clone()),
        AuditValue::Redacted => ("redacted", String::new()),
    }
}

fn decode_audit_value(kind: &str, value: &str) -> AuditValue {
    match kind {
        "plain" => AuditValue::Plain(value.to_owned()),
        "secret_ref" => AuditValue::SecretRef(value.to_owned()),
        _ => AuditValue::Redacted,
    }
}

#[derive(Debug, Clone)]
struct ControllerSigningStagedRolloutStorageRow {
    controller_id: String,
    state: String,
    target_ids: String,
    batch_size: i64,
    max_failures: i64,
    ack_timeout_seconds: i64,
    acknowledged_agent_ids: String,
    unavailable_agent_ids: String,
    failed_agent_ids: String,
    in_flight_attempts: String,
    failure_reason_code: Option<String>,
    current_fingerprint: String,
    previous_fingerprint: Option<String>,
    updated_at: i64,
}

fn sqlite_save_controller_signing_staged_rollout(
    store: &SqliteStore,
    record: ControllerSigningStagedRolloutRecord,
) -> Result<(), StoreError> {
    let storage = controller_signing_staged_rollout_record_to_storage(record)?;
    store.connection.execute(
        "INSERT INTO controller_signing_staged_rollout (
            controller_id, state, target_ids, batch_size, max_failures,
            ack_timeout_seconds, acknowledged_agent_ids, unavailable_agent_ids,
            failed_agent_ids, in_flight_attempts, failure_reason_code,
            current_fingerprint, previous_fingerprint, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT(controller_id) DO UPDATE SET
            state = excluded.state,
            target_ids = excluded.target_ids,
            batch_size = excluded.batch_size,
            max_failures = excluded.max_failures,
            ack_timeout_seconds = excluded.ack_timeout_seconds,
            acknowledged_agent_ids = excluded.acknowledged_agent_ids,
            unavailable_agent_ids = excluded.unavailable_agent_ids,
            failed_agent_ids = excluded.failed_agent_ids,
            in_flight_attempts = excluded.in_flight_attempts,
            failure_reason_code = excluded.failure_reason_code,
            current_fingerprint = excluded.current_fingerprint,
            previous_fingerprint = excluded.previous_fingerprint,
            updated_at = excluded.updated_at",
        params![
            storage.controller_id,
            storage.state,
            storage.target_ids,
            storage.batch_size,
            storage.max_failures,
            storage.ack_timeout_seconds,
            storage.acknowledged_agent_ids,
            storage.unavailable_agent_ids,
            storage.failed_agent_ids,
            storage.in_flight_attempts,
            storage.failure_reason_code,
            storage.current_fingerprint,
            storage.previous_fingerprint,
            storage.updated_at,
        ],
    )?;
    Ok(())
}

fn sqlite_load_controller_signing_staged_rollout(
    store: &SqliteStore,
    controller_id: &str,
) -> Result<Option<ControllerSigningStagedRolloutRecord>, StoreError> {
    store
        .connection
        .query_row(
            "SELECT controller_id, state, target_ids, batch_size, max_failures,
                    ack_timeout_seconds, acknowledged_agent_ids, unavailable_agent_ids,
                    failed_agent_ids, in_flight_attempts, failure_reason_code,
                    current_fingerprint, previous_fingerprint, updated_at
             FROM controller_signing_staged_rollout
             WHERE controller_id = ?1",
            params![controller_id],
            sqlite_row_to_controller_signing_staged_rollout_storage,
        )
        .optional()
        .map_err(StoreError::from)?
        .map(controller_signing_staged_rollout_record_from_storage)
        .transpose()
}

fn sqlite_save_agent_certificate_lifecycle(
    store: &SqliteStore,
    record: AgentCertificateLifecycleRecord,
) -> Result<(), StoreError> {
    let snapshot = record.lifecycle;
    let current = agent_certificate_storage_parts(snapshot.current_certificate.as_ref());
    let next = agent_certificate_storage_parts(snapshot.next_certificate.as_ref());
    store.connection.execute(
        "INSERT INTO agent_certificate_lifecycle (
            agent_id, state, current_serial, current_fingerprint,
            current_not_before, current_not_after, next_serial, next_fingerprint,
            next_not_before, next_not_after, grace_until, revocation_reason, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(agent_id) DO UPDATE SET
            state = excluded.state,
            current_serial = excluded.current_serial,
            current_fingerprint = excluded.current_fingerprint,
            current_not_before = excluded.current_not_before,
            current_not_after = excluded.current_not_after,
            next_serial = excluded.next_serial,
            next_fingerprint = excluded.next_fingerprint,
            next_not_before = excluded.next_not_before,
            next_not_after = excluded.next_not_after,
            grace_until = excluded.grace_until,
            revocation_reason = excluded.revocation_reason,
            updated_at = excluded.updated_at",
        params![
            record.agent_id.as_str(),
            snapshot.state.as_str(),
            current.serial,
            current.fingerprint,
            current.not_before,
            current.not_after,
            next.serial,
            next.fingerprint,
            next.not_before,
            next.not_after,
            snapshot.grace_until.map(system_time_to_unix_secs),
            snapshot
                .revocation_reason
                .map(AgentCertificateRevocationReason::as_str),
            system_time_to_unix_secs(record.updated_at),
        ],
    )?;
    Ok(())
}

fn sqlite_load_agent_certificate_lifecycle(
    store: &SqliteStore,
    agent_id: &AgentId,
) -> Result<Option<AgentCertificateLifecycleRecord>, StoreError> {
    store
        .connection
        .query_row(
            "SELECT agent_id, state, current_serial, current_fingerprint,
                    current_not_before, current_not_after, next_serial, next_fingerprint,
                    next_not_before, next_not_after, grace_until, revocation_reason, updated_at
             FROM agent_certificate_lifecycle
             WHERE agent_id = ?1",
            params![agent_id.as_str()],
            sqlite_row_to_agent_certificate_lifecycle_storage,
        )
        .optional()
        .map_err(StoreError::from)?
        .map(agent_certificate_lifecycle_record_from_storage)
        .transpose()
}

fn sqlite_row_to_controller_signing_staged_rollout_storage(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ControllerSigningStagedRolloutStorageRow> {
    Ok(ControllerSigningStagedRolloutStorageRow {
        controller_id: row.get(0)?,
        state: row.get(1)?,
        target_ids: row.get(2)?,
        batch_size: row.get(3)?,
        max_failures: row.get(4)?,
        ack_timeout_seconds: row.get(5)?,
        acknowledged_agent_ids: row.get(6)?,
        unavailable_agent_ids: row.get(7)?,
        failed_agent_ids: row.get(8)?,
        in_flight_attempts: row.get(9)?,
        failure_reason_code: row.get(10)?,
        current_fingerprint: row.get(11)?,
        previous_fingerprint: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

#[cfg(feature = "postgres")]
fn postgres_row_to_controller_signing_staged_rollout_storage(
    row: &postgres::Row,
) -> ControllerSigningStagedRolloutStorageRow {
    ControllerSigningStagedRolloutStorageRow {
        controller_id: row.get(0),
        state: row.get(1),
        target_ids: row.get(2),
        batch_size: row.get(3),
        max_failures: row.get(4),
        ack_timeout_seconds: row.get(5),
        acknowledged_agent_ids: row.get(6),
        unavailable_agent_ids: row.get(7),
        failed_agent_ids: row.get(8),
        in_flight_attempts: row.get(9),
        failure_reason_code: row.get(10),
        current_fingerprint: row.get(11),
        previous_fingerprint: row.get(12),
        updated_at: row.get(13),
    }
}

fn controller_signing_staged_rollout_record_to_storage(
    record: ControllerSigningStagedRolloutRecord,
) -> Result<ControllerSigningStagedRolloutStorageRow, StoreError> {
    if record.controller_id.trim().is_empty() || record.current_fingerprint.trim().is_empty() {
        return Err(StoreError::Domain(
            "invalid controller signing staged rollout record".to_owned(),
        ));
    }
    if record
        .previous_fingerprint
        .as_deref()
        .is_some_and(|fingerprint| fingerprint.trim().is_empty())
    {
        return Err(StoreError::Domain(
            "invalid controller signing staged rollout previous fingerprint".to_owned(),
        ));
    }
    let snapshot = record.rollout.snapshot();
    let batch_size = i64::try_from(snapshot.config.batch_size)
        .map_err(|_| StoreError::Domain("staged rollout batch size is too large".to_owned()))?;
    let max_failures = i64::try_from(snapshot.config.max_failures)
        .map_err(|_| StoreError::Domain("staged rollout max failures is too large".to_owned()))?;
    let ack_timeout_seconds = i64::try_from(snapshot.config.ack_timeout.as_secs())
        .map_err(|_| StoreError::Domain("staged rollout timeout is too large".to_owned()))?;
    Ok(ControllerSigningStagedRolloutStorageRow {
        controller_id: record.controller_id,
        state: snapshot.state.as_str().to_owned(),
        target_ids: json_string_vec(&snapshot.target_ids)?,
        batch_size,
        max_failures,
        ack_timeout_seconds,
        acknowledged_agent_ids: json_string_vec(&snapshot.acknowledged_agent_ids)?,
        unavailable_agent_ids: json_string_vec(&snapshot.unavailable_agent_ids)?,
        failed_agent_ids: json_string_vec(&snapshot.failed_agent_ids)?,
        in_flight_attempts: staged_rollout_attempts_to_json(&snapshot.in_flight)?,
        failure_reason_code: snapshot.failure_reason_code,
        current_fingerprint: record.current_fingerprint,
        previous_fingerprint: record.previous_fingerprint,
        updated_at: system_time_to_unix_secs(record.updated_at),
    })
}

fn controller_signing_staged_rollout_record_from_storage(
    row: ControllerSigningStagedRolloutStorageRow,
) -> Result<ControllerSigningStagedRolloutRecord, StoreError> {
    let state = fleet_domain::ControllerSigningStagedRolloutState::parse(&row.state)
        .ok_or_else(|| StoreError::Domain("invalid staged rollout state in store".to_owned()))?;
    let batch_size = usize::try_from(row.batch_size)
        .map_err(|_| StoreError::Domain("invalid staged rollout batch size".to_owned()))?;
    let max_failures = usize::try_from(row.max_failures)
        .map_err(|_| StoreError::Domain("invalid staged rollout max failures".to_owned()))?;
    let ack_timeout_seconds = u64::try_from(row.ack_timeout_seconds)
        .map_err(|_| StoreError::Domain("invalid staged rollout ack timeout".to_owned()))?;
    let snapshot = fleet_domain::ControllerSigningStagedRolloutSnapshot {
        state,
        target_ids: json_vec_string(&row.target_ids)?,
        config: fleet_domain::ControllerSigningStagedRolloutConfig {
            batch_size,
            max_failures,
            ack_timeout: Duration::from_secs(ack_timeout_seconds),
        },
        acknowledged_agent_ids: json_vec_string(&row.acknowledged_agent_ids)?,
        unavailable_agent_ids: json_vec_string(&row.unavailable_agent_ids)?,
        failed_agent_ids: json_vec_string(&row.failed_agent_ids)?,
        in_flight: staged_rollout_attempts_from_json(&row.in_flight_attempts)?,
        failure_reason_code: row.failure_reason_code,
    };
    let rollout = fleet_domain::ControllerSigningStagedRollout::from_snapshot(snapshot)
        .map_err(|error| StoreError::Domain(error.to_string()))?;
    Ok(ControllerSigningStagedRolloutRecord {
        controller_id: row.controller_id,
        current_fingerprint: row.current_fingerprint,
        previous_fingerprint: row.previous_fingerprint,
        rollout,
        updated_at: unix_secs_to_system_time(row.updated_at),
    })
}

fn json_string_vec(values: &[String]) -> Result<String, StoreError> {
    serde_json::to_string(values).map_err(|error| StoreError::Domain(error.to_string()))
}

fn json_vec_string(value: &str) -> Result<Vec<String>, StoreError> {
    serde_json::from_str(value).map_err(|error| StoreError::Domain(error.to_string()))
}

fn staged_rollout_attempts_to_json(
    attempts: &[fleet_domain::ControllerSigningStagedRolloutAttemptSnapshot],
) -> Result<String, StoreError> {
    let values = attempts
        .iter()
        .map(|attempt| {
            serde_json::json!({
                "agent_id": attempt.agent_id,
                "dispatched_at": system_time_to_unix_secs(attempt.dispatched_at),
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&values).map_err(|error| StoreError::Domain(error.to_string()))
}

fn staged_rollout_attempts_from_json(
    value: &str,
) -> Result<Vec<fleet_domain::ControllerSigningStagedRolloutAttemptSnapshot>, StoreError> {
    let values: Vec<serde_json::Value> =
        serde_json::from_str(value).map_err(|error| StoreError::Domain(error.to_string()))?;
    values
        .into_iter()
        .map(|value| {
            let agent_id = value
                .get("agent_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    StoreError::Domain("invalid staged rollout attempt agent_id".to_owned())
                })?;
            let dispatched_at = value
                .get("dispatched_at")
                .and_then(serde_json::Value::as_i64)
                .ok_or_else(|| {
                    StoreError::Domain("invalid staged rollout attempt dispatched_at".to_owned())
                })?;
            Ok(
                fleet_domain::ControllerSigningStagedRolloutAttemptSnapshot {
                    agent_id: agent_id.to_owned(),
                    dispatched_at: unix_secs_to_system_time(dispatched_at),
                },
            )
        })
        .collect()
}

fn system_time_to_unix_secs(value: SystemTime) -> i64 {
    value
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn unix_secs_to_system_time(value: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(value.max(0) as u64)
}

pub fn schema_sql() -> &'static str {
    SCHEMA_SQL
}

pub fn store_layer_ready() -> bool {
    fleet_application::application_layer_name() == fleet_domain::DOMAIN_LAYER
}

const SCHEMA_SQL: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_migrations (
    name TEXT PRIMARY KEY,
    version INTEGER NOT NULL,
    applied_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS controller_identity (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    public_key TEXT NOT NULL,
    public_fingerprint TEXT NOT NULL,
    private_key_path TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS controller_signing_key_rotation (
    controller_id TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    old_fingerprint TEXT NOT NULL,
    new_fingerprint TEXT,
    requested_at INTEGER,
    validated_at INTEGER,
    activated_at INTEGER,
    old_key_verifies_until INTEGER,
    retired_at INTEGER,
    failed_at INTEGER,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS controller_signing_staged_rollout (
    controller_id TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    target_ids TEXT NOT NULL,
    batch_size INTEGER NOT NULL,
    max_failures INTEGER NOT NULL,
    ack_timeout_seconds INTEGER NOT NULL,
    acknowledged_agent_ids TEXT NOT NULL,
    unavailable_agent_ids TEXT NOT NULL,
    failed_agent_ids TEXT NOT NULL,
    in_flight_attempts TEXT NOT NULL,
    failure_reason_code TEXT,
    current_fingerprint TEXT NOT NULL,
    previous_fingerprint TEXT,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS agent_certificate_lifecycle (
    agent_id TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    current_serial TEXT,
    current_fingerprint TEXT,
    current_not_before INTEGER,
    current_not_after INTEGER,
    next_serial TEXT,
    next_fingerprint TEXT,
    next_not_before INTEGER,
    next_not_after INTEGER,
    grace_until INTEGER,
    revocation_reason TEXT,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS admin_tokens (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    token_hash TEXT NOT NULL,
    actor_id TEXT NOT NULL DEFAULT 'bootstrap-admin',
    role TEXT NOT NULL DEFAULT 'owner',
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS agents (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    public_key TEXT NOT NULL,
    fingerprint TEXT NOT NULL UNIQUE,
    labels TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL,
    last_seen_at INTEGER,
    pinned_controller TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS agent_identities (
    agent_id TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
    public_key TEXT NOT NULL,
    fingerprint TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS enrollment_tokens (
    id TEXT PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    default_labels TEXT NOT NULL DEFAULT '',
    expires_at INTEGER NOT NULL,
    max_uses INTEGER NOT NULL,
    used_count INTEGER NOT NULL DEFAULT 0,
    revoked_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS jobs (
    id TEXT PRIMARY KEY,
    status TEXT NOT NULL,
    risk TEXT NOT NULL,
    approval_requirement TEXT NOT NULL,
    timeout_ms INTEGER NOT NULL,
    command_program TEXT,
    command_args_json TEXT NOT NULL DEFAULT '[]',
    command_max_output_bytes INTEGER NOT NULL DEFAULT 1048576,
    drift_policy_document TEXT,
    drift_policy_id TEXT,
    drift_policy_version INTEGER,
    drift_purpose TEXT,
    runbook_document TEXT,
    selector_kind TEXT NOT NULL DEFAULT 'explicit_ids',
    selector_source TEXT NOT NULL DEFAULT '',
    strategy_concurrency INTEGER NOT NULL DEFAULT 1,
    strategy_max_failures INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS job_targets (
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    agent_display_name TEXT NOT NULL DEFAULT '',
    agent_status_snapshot TEXT NOT NULL DEFAULT 'unknown',
    labels_snapshot TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (job_id, agent_id)
);

CREATE TABLE IF NOT EXISTS task_assignments (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'queued',
    nonce TEXT NOT NULL UNIQUE,
    payload_hash TEXT NOT NULL,
    signature TEXT NOT NULL,
    issued_at INTEGER NOT NULL DEFAULT (unixepoch()),
    expires_at INTEGER NOT NULL,
    dispatched_at INTEGER,
    accepted_at INTEGER,
    started_at INTEGER,
    completed_at INTEGER,
    last_error TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS job_output_chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    stream TEXT NOT NULL,
    chunk_index INTEGER NOT NULL,
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(job_id, agent_id, stream, chunk_index)
);

CREATE TABLE IF NOT EXISTS rendered_artifacts (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL,
    destination TEXT NOT NULL,
    checksum_sha256 TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK(size_bytes > 0),
    retention_class TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS remediation_requests (
    id TEXT PRIMARY KEY,
    policy_id TEXT NOT NULL,
    policy_name TEXT NOT NULL,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    runbook_ref TEXT NOT NULL,
    status TEXT NOT NULL,
    approval_required INTEGER NOT NULL,
    risk_summary TEXT NOT NULL,
    job_id TEXT,
    origin_drift_report_id INTEGER,
    policy_version INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS remediation_requests_active_policy_unique_idx
    ON remediation_requests (agent_id, policy_id)
    WHERE origin_drift_report_id IS NOT NULL
      AND status NOT IN ('resolved', 'failed', 'rejected', 'expired', 'canceled');

CREATE TABLE IF NOT EXISTS approval_decisions (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    actor TEXT NOT NULL,
    decision TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS approval_requests (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL,
    requester TEXT NOT NULL,
    approver TEXT,
    reason TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    decided_at INTEGER
);

CREATE TABLE IF NOT EXISTS audit_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    category TEXT NOT NULL,
    action TEXT NOT NULL,
    actor TEXT NOT NULL,
    target TEXT NOT NULL,
    value_kind TEXT NOT NULL,
    value_text TEXT NOT NULL DEFAULT '',
    occurred_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS facts_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    body TEXT NOT NULL,
    collected_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS metrics_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    body TEXT NOT NULL,
    collected_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS agent_capability_snapshots (
    agent_id TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
    privilege_level TEXT NOT NULL,
    package_manager TEXT,
    service_manager TEXT,
    capabilities_json TEXT NOT NULL,
    reported_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS drift_reports (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    policy_name TEXT NOT NULL,
    status TEXT NOT NULL,
    severity TEXT NOT NULL DEFAULT 'unknown',
    expected TEXT NOT NULL,
    actual TEXT NOT NULL,
    checked_at INTEGER NOT NULL,
    acknowledged_at INTEGER,
    acknowledged_by TEXT,
    resolved_at INTEGER,
    resolution_job_id TEXT
);

CREATE TABLE IF NOT EXISTS policies (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    version INTEGER NOT NULL,
    source TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS policy_assignments (
    policy_id TEXT NOT NULL REFERENCES policies(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    assigned_at INTEGER NOT NULL,
    PRIMARY KEY (policy_id, agent_id)
);

CREATE TABLE IF NOT EXISTS policy_drift_schedules (
    policy_id TEXT NOT NULL REFERENCES policies(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    interval_seconds INTEGER NOT NULL,
    next_due_at INTEGER NOT NULL,
    last_checked_at INTEGER,
    PRIMARY KEY (policy_id, agent_id)
);

CREATE TABLE IF NOT EXISTS agent_log_chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    line TEXT NOT NULL,
    collected_at INTEGER NOT NULL
);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn agent() -> Agent {
        agent_with_id("a1", "web-01", "0123456789abcdef")
    }

    fn agent_with_id(id: &str, name: &str, fingerprint: &str) -> Agent {
        let mut agent = Agent::new(
            AgentId::new(id).unwrap(),
            AgentName::new(name).unwrap(),
            AgentIdentity {
                public_key: AgentPublicKey::new("pk").unwrap(),
                fingerprint: AgentFingerprint::new(fingerprint).unwrap(),
            },
        );
        agent.set_labels(vec![AgentLabel::new("role", "web").unwrap()]);
        agent.pin_controller(ControllerPublicKey::new("controller-pk").unwrap());
        agent
    }

    #[test]
    fn memory_repo_stores_and_finds_agent() {
        let mut repo = MemoryAgentRepository::default();
        repo.save(agent()).unwrap();
        assert!(
            repo.find_by_id(&AgentId::new("a1").unwrap())
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn memory_repo_rejects_duplicate_agent() {
        let mut repo = MemoryAgentRepository::default();
        repo.save(agent()).unwrap();
        assert_eq!(repo.save(agent()), Err(StoreError::DuplicateAgent));
    }

    #[test]
    fn migration_is_repeatable() {
        let store = SqliteStore::in_memory().unwrap();
        store.migrate().unwrap();
        store.migrate().unwrap();
        assert_eq!(
            store.current_schema_version().unwrap(),
            Some(CURRENT_SCHEMA_VERSION)
        );
    }

    #[test]
    fn empty_database_initialization_records_schema_version() {
        let store = SqliteStore::in_memory().unwrap();

        assert_eq!(
            store.current_schema_version().unwrap(),
            Some(CURRENT_SCHEMA_VERSION)
        );
        assert!(store.has_column("schema_migrations", "version").unwrap());
        assert!(store.has_column("agents", "pinned_controller").unwrap());
        assert!(store.has_column("task_assignments", "nonce").unwrap());
        assert!(store.has_column("task_assignments", "status").unwrap());
        assert!(store.has_column("task_assignments", "accepted_at").unwrap());
        assert!(store.has_column("admin_tokens", "actor_id").unwrap());
        assert!(store.has_column("admin_tokens", "role").unwrap());
        assert!(
            store
                .has_column("rendered_artifacts", "checksum_sha256")
                .unwrap()
        );
        assert!(!store.has_column("rendered_artifacts", "body").unwrap());
        assert!(
            !store
                .has_column("rendered_artifacts", "template_body")
                .unwrap()
        );
        assert!(
            store
                .has_column("controller_signing_key_rotation", "state")
                .unwrap()
        );
        assert!(
            store
                .has_column("controller_signing_key_rotation", "old_fingerprint")
                .unwrap()
        );
        assert!(
            !store
                .has_column("controller_signing_key_rotation", "private_key")
                .unwrap()
        );
        assert!(
            !store
                .has_column("controller_signing_key_rotation", "private_key_path")
                .unwrap()
        );
        assert!(
            !store
                .has_column("controller_signing_key_rotation", "key_material")
                .unwrap()
        );
        assert!(
            store
                .has_column("controller_signing_staged_rollout", "state")
                .unwrap()
        );
        assert!(
            store
                .has_column("controller_signing_staged_rollout", "in_flight_attempts")
                .unwrap()
        );
        assert!(
            store
                .has_column("controller_signing_staged_rollout", "current_fingerprint")
                .unwrap()
        );
        for forbidden in [
            "private_key",
            "private_key_path",
            "public_key_body",
            "local_key_path",
            "admin_token",
            "websocket_handle",
        ] {
            assert!(
                !store
                    .has_column("controller_signing_staged_rollout", forbidden)
                    .unwrap(),
                "staged rollout schema must not include {forbidden}"
            );
        }
        assert!(
            store
                .has_column("agent_certificate_lifecycle", "state")
                .unwrap()
        );
        assert!(
            store
                .has_column("agent_certificate_lifecycle", "current_fingerprint")
                .unwrap()
        );
        assert!(
            store
                .has_column("agent_certificate_lifecycle", "next_fingerprint")
                .unwrap()
        );
        for forbidden in [
            "private_key",
            "private_key_path",
            "certificate_body",
            "pem_body",
            "ca_path",
            "websocket_handle",
            "runtime_env",
        ] {
            assert!(
                !store
                    .has_column("agent_certificate_lifecycle", forbidden)
                    .unwrap(),
                "agent certificate lifecycle schema must not include {forbidden}"
            );
        }
    }

    #[test]
    fn migration_from_versioned_previous_fixture_adds_columns_without_losing_rows() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(include_str!("../fixtures/sqlite/schema_v8_jobs_only.sql"))
            .unwrap();

        let store = SqliteStore { connection };
        assert_eq!(store.current_schema_version().unwrap(), Some(8));

        store.migrate().unwrap();

        assert!(store.has_column("jobs", "drift_policy_document").unwrap());
        assert!(store.has_column("jobs", "drift_policy_id").unwrap());
        assert!(store.has_column("jobs", "drift_policy_version").unwrap());
        assert!(store.has_column("jobs", "drift_purpose").unwrap());
        assert!(store.has_column("jobs", "runbook_document").unwrap());
        assert!(
            store
                .has_column("remediation_requests", "origin_drift_report_id")
                .unwrap()
        );
        assert!(
            store
                .has_column("remediation_requests", "policy_version")
                .unwrap()
        );
        assert!(
            store
                .has_column("controller_signing_key_rotation", "state")
                .unwrap()
        );
        assert_eq!(
            store.current_schema_version().unwrap(),
            Some(CURRENT_SCHEMA_VERSION)
        );
        let job_count: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE id = 'legacy-job'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(job_count, 1);
    }

    #[test]
    fn migration_preserves_legacy_drift_report_without_provenance() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "
                CREATE TABLE schema_migrations (
                    name TEXT PRIMARY KEY,
                    version INTEGER NOT NULL,
                    applied_at INTEGER NOT NULL DEFAULT 0
                );
                INSERT INTO schema_migrations (name, version, applied_at)
                VALUES ('fleet_store', 15, 1710000000);
                CREATE TABLE drift_reports (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    agent_id TEXT NOT NULL,
                    policy_name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    severity TEXT NOT NULL DEFAULT 'unknown',
                    expected TEXT NOT NULL,
                    actual TEXT NOT NULL,
                    checked_at INTEGER NOT NULL,
                    acknowledged_at INTEGER,
                    acknowledged_by TEXT,
                    resolved_at INTEGER,
                    resolution_job_id TEXT
                );
                INSERT INTO drift_reports (
                    agent_id, policy_name, status, severity, expected, actual, checked_at
                ) VALUES (
                    'legacy-agent', 'legacy-policy', 'drifted', 'warning',
                    'expected', 'actual', 1710000000
                );
                ",
            )
            .unwrap();

        let store = SqliteStore { connection };
        store.migrate().unwrap();

        let record = store.latest_drift_report("legacy-agent").unwrap().unwrap();
        assert_eq!(record.report.policy_name, "legacy-policy");
        assert_eq!(record.provenance, DriftReportProvenance::uncorrelated());
        assert!(!record.provenance.is_automation_eligible());
    }

    #[test]
    fn correlated_drift_report_persists_provenance_and_rejects_duplicate_correlation() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let report = DriftReport::drifted("nginx-running", "expected", "actual");
        let provenance = DriftReportProvenance::verified(
            JobId::new("job-drift").unwrap(),
            TaskId::new("task-drift").unwrap(),
            "policy-nginx",
            7,
            DriftCheckPurpose::Evaluation,
        );

        store
            .insert_drift_report_with_provenance(
                "a1",
                &report,
                &provenance,
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();

        let record = store.latest_drift_report("a1").unwrap().unwrap();
        assert!(record.id.as_i64() > 0);
        assert_eq!(record.provenance, provenance);
        assert!(record.provenance.is_automation_eligible());

        assert!(matches!(
            store.insert_drift_report_with_provenance(
                "a1",
                &report,
                &provenance,
                SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            ),
            Err(StoreError::ConstraintViolation(_))
        ));
    }

    #[test]
    fn migration_rebuilds_approval_requests_to_allow_reserved_remediation_job_ids() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(include_str!("../fixtures/sqlite/schema_v8_jobs_only.sql"))
            .unwrap();
        connection
            .execute_batch(
                "
                CREATE TABLE approval_requests (
                    id TEXT PRIMARY KEY,
                    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
                    requester TEXT NOT NULL,
                    approver TEXT,
                    reason TEXT NOT NULL DEFAULT '',
                    status TEXT NOT NULL,
                    expires_at INTEGER NOT NULL,
                    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
                    decided_at INTEGER
                );
                INSERT INTO approval_requests (
                    id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
                ) VALUES (
                    'legacy-approval', 'legacy-job', 'operator', NULL, 'legacy', 'pending', 1710000600, 1710000000, NULL
                );
                ",
            )
            .unwrap();

        let mut store = SqliteStore { connection };
        store.migrate().unwrap();
        assert_eq!(
            store
                .find_approval_request("legacy-approval")
                .unwrap()
                .unwrap()
                .job_id,
            "legacy-job"
        );

        <SqliteStore as ApprovalRepository>::insert_approval_request(
            &mut store,
            AppApprovalRequestRecord {
                id: "reserved-remediation-approval".to_owned(),
                job_id: "reserved-remediation-job".to_owned(),
                requester: "operator".to_owned(),
                approver: None,
                reason: "reserved remediation job id".to_owned(),
                status: "pending".to_owned(),
                expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(60),
                created_at: SystemTime::UNIX_EPOCH,
                decided_at: None,
            },
        )
        .unwrap();
    }

    #[test]
    fn schema_does_not_store_raw_enrollment_token() {
        let store = SqliteStore::in_memory().unwrap();
        assert!(store.has_column("enrollment_tokens", "token_hash").unwrap());
        assert!(!store.has_column("enrollment_tokens", "token").unwrap());
        assert!(!store.has_column("enrollment_tokens", "raw_token").unwrap());
    }

    #[test]
    fn schema_contains_mvp_command_and_inventory_columns() {
        let store = SqliteStore::in_memory().unwrap();

        assert!(store.has_column("jobs", "command_program").unwrap());
        assert!(store.has_column("jobs", "command_args_json").unwrap());
        assert!(store.has_column("jobs", "timeout_ms").unwrap());
        assert!(store.has_column("jobs", "drift_policy_document").unwrap());
        assert!(store.has_column("jobs", "runbook_document").unwrap());
        assert!(store.has_column("jobs", "selector_kind").unwrap());
        assert!(store.has_column("jobs", "selector_source").unwrap());
        assert!(store.has_column("jobs", "strategy_concurrency").unwrap());
        assert!(store.has_column("jobs", "strategy_max_failures").unwrap());
        assert!(
            store
                .has_column("job_targets", "agent_display_name")
                .unwrap()
        );
        assert!(
            store
                .has_column("job_targets", "agent_status_snapshot")
                .unwrap()
        );
        assert!(store.has_column("job_targets", "labels_snapshot").unwrap());
        assert!(store.has_column("task_assignments", "issued_at").unwrap());
        assert!(
            store
                .has_column("job_output_chunks", "chunk_index")
                .unwrap()
        );
        assert!(store.has_column("facts_snapshots", "body").unwrap());
        assert!(store.has_column("facts_snapshots", "collected_at").unwrap());
        assert!(
            store
                .has_column("agent_capability_snapshots", "privilege_level")
                .unwrap()
        );
        assert!(
            store
                .has_column("agent_capability_snapshots", "capabilities_json")
                .unwrap()
        );
        assert!(
            store
                .has_column("agent_capability_snapshots", "reported_at")
                .unwrap()
        );
        assert!(store.has_column("agent_log_chunks", "line").unwrap());
        assert!(
            store
                .has_column("agent_log_chunks", "collected_at")
                .unwrap()
        );
        assert!(store.has_column("agents", "labels").unwrap());
        assert!(store.has_column("agents", "status").unwrap());
    }

    #[test]
    fn sqlite_store_implements_application_repository_contracts() {
        let mut store = SqliteStore::in_memory().unwrap();
        assert_application_repository_contracts(&mut store);
    }

    #[test]
    fn sqlite_store_passes_shared_repository_contract_harness() {
        let mut store = SqliteStore::in_memory().unwrap();
        assert_application_repository_contracts(&mut store);
    }

    #[test]
    fn sqlite_store_passes_bootstrap_repository_contract_harness() {
        let mut store = SqliteStore::in_memory().unwrap();
        assert_bootstrap_repository_contracts(&mut store);
    }

    #[test]
    fn sqlite_store_passes_enrollment_token_repository_contract_harness() {
        let mut store = SqliteStore::in_memory().unwrap();
        assert_enrollment_token_repository_contracts(&mut store);
    }

    #[test]
    fn sqlite_store_passes_audit_repository_contract_harness() {
        let mut store = SqliteStore::in_memory().unwrap();
        assert_audit_repository_contracts(&mut store);
    }

    #[test]
    fn sqlite_store_passes_approval_repository_contract_harness() {
        let mut store = SqliteStore::in_memory().unwrap();
        assert_approval_repository_contracts(&mut store);
    }

    #[test]
    fn sqlite_store_passes_job_assignment_repository_contract_harness() {
        let mut store = SqliteStore::in_memory().unwrap();
        assert_job_assignment_repository_contracts(&mut store);
    }

    #[test]
    fn sqlite_store_passes_typed_job_repository_contract_harness() {
        let mut store = SqliteStore::in_memory().unwrap();
        assert_typed_job_repository_contracts(&mut store);
    }

    #[test]
    fn sqlite_store_rolls_back_command_job_assignment_transaction() {
        let mut store = SqliteStore::in_memory().unwrap();
        assert_typed_job_repository_contracts(&mut store);
    }

    #[test]
    fn sqlite_store_passes_dispatch_assignment_repository_contract_harness() {
        let mut store = SqliteStore::in_memory().unwrap();
        assert_dispatch_assignment_repository_contracts(&mut store);
    }

    #[test]
    fn sqlite_store_passes_output_telemetry_repository_contract_harness() {
        let mut store = SqliteStore::in_memory().unwrap();
        assert_output_telemetry_repository_contracts(&mut store, |store, agent_id, line, at| {
            store.insert_agent_log_chunk(agent_id, line, at).unwrap();
        });
    }

    #[test]
    fn sqlite_store_passes_drift_policy_capability_repository_contract_harness() {
        let mut store = SqliteStore::in_memory().unwrap();
        assert_drift_policy_capability_repository_contracts(&mut store);
    }

    #[test]
    fn sqlite_store_passes_query_artifact_retention_repository_contract_harness() {
        let mut store = SqliteStore::in_memory().unwrap();
        assert_query_artifact_retention_repository_contracts(
            &mut store,
            |store, agent_id, line, at| {
                store.insert_agent_log_chunk(agent_id, line, at).unwrap();
            },
        );
    }

    #[test]
    fn sqlite_store_passes_remediation_request_repository_contract_harness() {
        let mut store = SqliteStore::in_memory().unwrap();
        assert_remediation_request_repository_contracts(&mut store);
    }

    #[test]
    fn migration_state_transitions_follow_phase3_gate() {
        let state = StoreMigrationState::NotStarted
            .transition(StoreMigrationEvent::CheckSchema)
            .unwrap()
            .transition(StoreMigrationEvent::Plan)
            .unwrap()
            .transition(StoreMigrationEvent::Apply)
            .unwrap()
            .transition(StoreMigrationEvent::Verify)
            .unwrap();

        assert_eq!(state, StoreMigrationState::MigrationVerified);
        assert_eq!(
            StoreMigrationState::MigrationVerified
                .transition(StoreMigrationEvent::Apply)
                .unwrap_err(),
            StoreError::Domain(
                "invalid migration transition: MigrationVerified -> Apply".to_owned()
            )
        );
        assert_eq!(
            StoreMigrationState::MigrationPlanned
                .transition(StoreMigrationEvent::Fail)
                .unwrap(),
            StoreMigrationState::Failed
        );
        assert_eq!(
            StoreMigrationState::Failed
                .transition(StoreMigrationEvent::Verify)
                .unwrap_err(),
            StoreError::Domain("invalid migration transition: Failed -> Verify".to_owned())
        );
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_store_exposes_explicit_connection_entrypoints() {
        let connect: fn(&str) -> Result<PostgresStore, StoreError> = PostgresStore::connect;
        let connect_with_settings: fn(
            &PostgresStoreConnectSettings<'_>,
        ) -> Result<PostgresStore, StoreError> = PostgresStore::connect_with_settings;
        let migrate: fn(&mut PostgresStore) -> Result<(), StoreError> = PostgresStore::migrate;
        let schema_version: fn(&mut PostgresStore) -> Result<Option<i64>, StoreError> =
            PostgresStore::schema_version;

        let settings = PostgresStoreConnectSettings::new(
            "postgresql://fleet:secret@db.example.com/fleet",
            PostgresStoreSslMode::Disable,
            Duration::from_secs(7),
        )
        .unwrap();

        assert_eq!(
            settings.url(),
            "postgresql://fleet:secret@db.example.com/fleet"
        );
        assert_eq!(settings.ssl_mode(), PostgresStoreSslMode::Disable);
        assert_eq!(settings.connect_timeout(), Duration::from_secs(7));
        assert_eq!(
            settings.pool_max_connections(),
            PostgresStoreConnectSettings::DEFAULT_POOL_MAX_CONNECTIONS
        );
        assert_eq!(
            settings.pool_checkout_timeout(),
            PostgresStoreConnectSettings::DEFAULT_POOL_CHECKOUT_TIMEOUT
        );
        assert!(
            PostgresStoreConnectSettings::new(
                "postgresql://fleet:secret@db.example.com/fleet",
                PostgresStoreSslMode::Disable,
                Duration::ZERO,
            )
            .is_err()
        );
        assert!(
            PostgresStoreConnectSettings::with_pool_settings(
                "postgresql://fleet:secret@db.example.com/fleet",
                PostgresStoreSslMode::Disable,
                Duration::from_secs(7),
                0,
                Duration::from_secs(1),
            )
            .is_err()
        );
        assert!(
            PostgresStoreConnectSettings::with_pool_settings(
                "postgresql://fleet:secret@db.example.com/fleet",
                PostgresStoreSslMode::Disable,
                Duration::from_secs(7),
                1,
                Duration::ZERO,
            )
            .is_err()
        );

        let _ = (connect, connect_with_settings, migrate, schema_version);
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_store_checkout_failure_is_redacted() {
        let store = PostgresStore::empty_pool_for_test(Duration::from_secs(1));

        let error = match store.checkout_client() {
            Ok(_) => panic!("empty test pool should fail checkout"),
            Err(error) => error,
        };
        let message = format!("{error:?}");

        assert!(message.contains("postgres client checkout failed"));
        assert!(!message.contains("secret"));
        assert!(!message.contains("db.example.com"));
        assert!(!message.contains("fleet:"));
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_tls_adapter_failure_is_redacted() {
        let message = format!("{:?}", postgres_tls_adapter_error());

        assert!(message.contains("postgres TLS adapter initialization failed"));
        assert!(!message.contains("secret"));
        assert!(!message.contains("db.example.com"));
        assert!(!message.contains("fleet:"));
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_store_selects_tls_adapter_for_sslmode_require() {
        let settings = PostgresStoreConnectSettings::new(
            "postgresql://fleet:secret@db.example.com/fleet",
            PostgresStoreSslMode::Require,
            Duration::from_secs(1),
        )
        .unwrap();

        assert_eq!(
            postgres_connection_security(&settings),
            PostgresConnectionSecurity::TlsRequired
        );
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_store_selects_notls_for_sslmode_disable() {
        let settings = PostgresStoreConnectSettings::new(
            "postgresql://fleet:secret@db.example.com/fleet",
            PostgresStoreSslMode::Disable,
            Duration::from_secs(1),
        )
        .unwrap();

        assert_eq!(
            postgres_connection_security(&settings),
            PostgresConnectionSecurity::NoTls
        );
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_store_selects_tls_preferred_for_sslmode_prefer() {
        let settings = PostgresStoreConnectSettings::new(
            "postgresql://fleet:secret@db.example.com/fleet",
            PostgresStoreSslMode::Prefer,
            Duration::from_secs(1),
        )
        .unwrap();

        assert_eq!(
            postgres_connection_security(&settings),
            PostgresConnectionSecurity::TlsPreferred
        );
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_store_implements_bootstrap_repository_traits() {
        fn assert_traits<S>()
        where
            S: AgentRepository<Error = StoreError>
                + AgentIdentityRepository<Error = StoreError>
                + AdminTokenRepository<Error = StoreError>
                + ControllerIdentityRepository<Error = StoreError>,
        {
        }

        assert_traits::<PostgresStore>();
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_store_implements_enrollment_token_repository_trait() {
        fn assert_trait<S>()
        where
            S: EnrollmentTokenRepository<Error = StoreError>,
        {
        }

        assert_trait::<PostgresStore>();
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_store_implements_audit_repository_traits() {
        fn assert_traits<S>()
        where
            S: AuditRepository<Error = StoreError>,
        {
        }

        assert_traits::<PostgresStore>();
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_store_implements_approval_repository_trait() {
        fn assert_trait<S>()
        where
            S: ApprovalRepository<Error = StoreError>,
        {
        }

        assert_trait::<PostgresStore>();
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_store_implements_job_assignment_repository_traits() {
        fn assert_traits<S>()
        where
            S: AgentRepository<Error = StoreError>
                + JobRepository<Error = StoreError>
                + TaskAssignmentRepository<Error = StoreError>,
        {
        }

        assert_traits::<PostgresStore>();
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_store_implements_typed_job_repository_traits() {
        fn assert_traits<S>()
        where
            S: CommandJobRepository
                + DriftCheckJobRepository
                + RunbookJobRepository
                + TaskAssignmentRepository<Error = StoreError>
                + ApprovalRepository<Error = StoreError>,
        {
        }

        assert_traits::<PostgresStore>();
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_store_implements_dispatch_assignment_repository_trait() {
        fn assert_trait<S>()
        where
            S: DispatchAssignmentRepository<Error = StoreError>,
        {
        }

        assert_trait::<PostgresStore>();
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_store_implements_output_telemetry_repository_traits() {
        fn assert_traits<S>()
        where
            S: JobOutputRepository<Error = StoreError>
                + FactsRepository<Error = StoreError>
                + MetricsRepository<Error = StoreError>
                + AgentLogRepository<Error = StoreError>,
        {
        }

        assert_traits::<PostgresStore>();
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_store_implements_drift_policy_capability_repository_traits() {
        fn assert_traits<S>()
        where
            S: AgentRepository<Error = StoreError>
                + AgentCapabilityRepository<Error = StoreError>
                + DriftRepository<Error = StoreError>
                + PolicyRepository<Error = StoreError>,
        {
        }

        assert_traits::<PostgresStore>();
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_store_implements_query_artifact_retention_repository_traits() {
        fn assert_traits<S>()
        where
            S: JobQueryRepository<Error = StoreError>
                + ArtifactMetadataRepository<Error = StoreError>
                + RetentionRepository<Error = StoreError>,
        {
        }

        assert_traits::<PostgresStore>();
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_store_implements_remediation_request_repository_trait() {
        fn assert_trait<S>()
        where
            S: AgentRepository<Error = StoreError>
                + RemediationRequestRepository<Error = StoreError>,
        {
        }

        assert_trait::<PostgresStore>();
    }

    #[cfg(feature = "postgres")]
    #[test]
    #[ignore = "requires explicit FLEET_TEST_POSTGRES_URL and a disposable Postgres database"]
    fn postgres_migration_records_current_schema_version() {
        let Ok(url) = std::env::var("FLEET_TEST_POSTGRES_URL") else {
            return;
        };
        let mut store = PostgresStore::connect(&url).unwrap();

        store.migrate().unwrap();

        assert_eq!(
            store.schema_version().unwrap(),
            Some(CURRENT_SCHEMA_VERSION)
        );
    }

    #[cfg(feature = "postgres")]
    #[test]
    #[ignore = "requires explicit FLEET_TEST_POSTGRES_URL and a disposable Postgres database"]
    fn postgres_bootstrap_repositories_roundtrip() {
        let Ok(url) = std::env::var("FLEET_TEST_POSTGRES_URL") else {
            return;
        };
        let mut store = PostgresStore::connect(&url).unwrap();
        store.migrate().unwrap();
        let raw_token_columns: i64 = store
            .checkout_client()
            .unwrap()
            .query_one(
                "SELECT COUNT(*)
                 FROM information_schema.columns
                 WHERE table_name = 'enrollment_tokens'
                   AND column_name IN ('token', 'raw_token')",
                &[],
            )
            .unwrap()
            .get(0);
        assert_eq!(raw_token_columns, 0);
        store
            .checkout_client()
            .unwrap()
            .batch_execute(
                "DELETE FROM agent_identities WHERE agent_id = 'a1';
                 DELETE FROM agents WHERE id = 'a1' OR fingerprint = '0123456789abcdef';
                 DELETE FROM controller_identity WHERE id = 1;
                 DELETE FROM admin_tokens WHERE id = 1;",
            )
            .unwrap();

        assert_bootstrap_repository_contracts(&mut store);
    }

    #[cfg(feature = "postgres")]
    #[test]
    #[ignore = "requires explicit FLEET_TEST_POSTGRES_URL and a disposable Postgres database"]
    fn postgres_enrollment_token_repository_roundtrip() {
        let Ok(url) = std::env::var("FLEET_TEST_POSTGRES_URL") else {
            return;
        };
        let mut store = PostgresStore::connect(&url).unwrap();
        store.migrate().unwrap();
        store.checkout_client().unwrap()
            .batch_execute(
                "DELETE FROM enrollment_tokens
                 WHERE id IN ('et-contract', 'et-revoked', 'et-expired', 'et-exhausted')
                    OR token_hash IN ('hash-contract', 'hash-revoked', 'hash-expired', 'hash-exhausted');",
            )
            .unwrap();

        assert_enrollment_token_repository_contracts(&mut store);
    }

    #[cfg(feature = "postgres")]
    #[test]
    #[ignore = "requires explicit FLEET_TEST_POSTGRES_URL and a disposable Postgres database"]
    fn postgres_audit_repository_roundtrip() {
        let Ok(url) = std::env::var("FLEET_TEST_POSTGRES_URL") else {
            return;
        };
        let mut store = PostgresStore::connect(&url).unwrap();
        store.migrate().unwrap();
        store.checkout_client().unwrap()
            .batch_execute(
                "DELETE FROM audit_events
                 WHERE target IN ('agent-audit-contract', 'job-audit-contract', 'controller-audit-contract')
                    OR action IN ('invalid_signature_contract', 'job_created_contract', 'insecure_http_transport_contract');",
            )
            .unwrap();

        assert_audit_repository_contracts(&mut store);
    }

    #[cfg(feature = "postgres")]
    #[test]
    #[ignore = "requires explicit FLEET_TEST_POSTGRES_URL and a disposable Postgres database"]
    fn postgres_approval_repository_roundtrip() {
        let Ok(url) = std::env::var("FLEET_TEST_POSTGRES_URL") else {
            return;
        };
        let mut store = PostgresStore::connect(&url).unwrap();
        store.migrate().unwrap();
        store.checkout_client().unwrap()
            .batch_execute(
                "DELETE FROM approval_requests
                 WHERE id IN ('approval-contract', 'approval-other', 'approval-missing')
                    OR job_id IN ('job-approval-contract', 'job-approval-other', 'reserved-remediation-job');
                 DELETE FROM jobs
                 WHERE id IN ('job-approval-contract', 'job-approval-other', 'reserved-remediation-job');",
            )
            .unwrap();

        assert_approval_repository_contracts(&mut store);
    }

    #[cfg(feature = "postgres")]
    #[test]
    #[ignore = "requires explicit FLEET_TEST_POSTGRES_URL and a disposable Postgres database"]
    fn postgres_job_assignment_repository_roundtrip() {
        let Ok(url) = std::env::var("FLEET_TEST_POSTGRES_URL") else {
            return;
        };
        let mut store = PostgresStore::connect(&url).unwrap();
        store.migrate().unwrap();
        store
            .checkout_client()
            .unwrap()
            .batch_execute(
                "DELETE FROM task_assignments
                 WHERE id IN ('task-assignment-contract', 'task-unsigned-contract')
                    OR nonce IN ('nonce-assignment-contract', 'nonce-unsigned-contract');
                 DELETE FROM job_targets
                 WHERE job_id = 'job-assignment-contract' OR agent_id = 'a1';
                 DELETE FROM jobs
                 WHERE id = 'job-assignment-contract';
                 DELETE FROM agent_identities WHERE agent_id = 'a1';
                 DELETE FROM agents WHERE id = 'a1' OR fingerprint = '0123456789abcdef';",
            )
            .unwrap();

        assert_job_assignment_repository_contracts(&mut store);
    }

    #[cfg(feature = "postgres")]
    #[test]
    #[ignore = "requires explicit FLEET_TEST_POSTGRES_URL and a disposable Postgres database"]
    fn postgres_typed_job_repository_roundtrip() {
        let Ok(url) = std::env::var("FLEET_TEST_POSTGRES_URL") else {
            return;
        };
        let mut store = PostgresStore::connect(&url).unwrap();
        store.migrate().unwrap();
        store
            .checkout_client()
            .unwrap()
            .batch_execute(
                "DELETE FROM task_assignments
                 WHERE id IN (
                    'task-command-atomic-first',
                    'task-command-atomic-second',
                    'task-command-atomic-retry'
                 )
                    OR nonce IN (
                    'nonce-command-atomic-duplicate',
                    'nonce-command-atomic-retry'
                 );
                 DELETE FROM job_targets
                 WHERE job_id = 'job-command-atomic-contract' OR agent_id = 'a1';
                 DELETE FROM jobs
                 WHERE id IN (
                    'job-command-contract',
                    'job-runbook-contract',
                    'job-drift-contract',
                    'job-command-atomic-contract'
                 );
                 DELETE FROM agent_identities WHERE agent_id = 'a1';
                 DELETE FROM agents WHERE id = 'a1' OR fingerprint = '0123456789abcdef';",
            )
            .unwrap();

        assert_typed_job_repository_contracts(&mut store);
    }

    #[cfg(feature = "postgres")]
    #[test]
    #[ignore = "requires explicit FLEET_TEST_POSTGRES_URL and a disposable Postgres database"]
    fn postgres_dispatch_assignment_repository_roundtrip() {
        let Ok(url) = std::env::var("FLEET_TEST_POSTGRES_URL") else {
            return;
        };
        let mut store = PostgresStore::connect(&url).unwrap();
        store.migrate().unwrap();
        store
            .checkout_client()
            .unwrap()
            .batch_execute(
                "DELETE FROM task_assignments
                 WHERE id IN (
                    'task-dispatch-command-contract',
                    'task-dispatch-drift-contract',
                    'task-dispatch-runbook-contract'
                 )
                    OR nonce IN (
                    'nonce-dispatch-command-contract',
                    'nonce-dispatch-drift-contract',
                    'nonce-dispatch-runbook-contract'
                 );
                 DELETE FROM job_targets
                 WHERE job_id IN (
                    'job-dispatch-command-contract',
                    'job-dispatch-drift-contract',
                    'job-dispatch-runbook-contract'
                 )
                    OR agent_id = 'a1';
                 DELETE FROM jobs
                 WHERE id IN (
                    'job-dispatch-command-contract',
                    'job-dispatch-drift-contract',
                    'job-dispatch-runbook-contract'
                 );
                 DELETE FROM agent_capability_snapshots WHERE agent_id = 'a1';
                 DELETE FROM agent_identities WHERE agent_id = 'a1';
                 DELETE FROM agents WHERE id = 'a1' OR fingerprint = '0123456789abcdef';",
            )
            .unwrap();

        assert_dispatch_assignment_repository_contracts(&mut store);
    }

    #[cfg(feature = "postgres")]
    #[test]
    #[ignore = "requires explicit FLEET_TEST_POSTGRES_URL and a disposable Postgres database"]
    fn postgres_output_telemetry_repository_roundtrip() {
        let Ok(url) = std::env::var("FLEET_TEST_POSTGRES_URL") else {
            return;
        };
        let mut store = PostgresStore::connect(&url).unwrap();
        store.migrate().unwrap();
        store
            .checkout_client()
            .unwrap()
            .batch_execute(
                "DELETE FROM agent_log_chunks WHERE agent_id = 'a1';
                 DELETE FROM metrics_snapshots WHERE agent_id = 'a1';
                 DELETE FROM facts_snapshots WHERE agent_id = 'a1';
                 DELETE FROM job_output_chunks WHERE job_id = 'job-output-telemetry-contract';
                 DELETE FROM jobs WHERE id = 'job-output-telemetry-contract';
                 DELETE FROM agent_identities WHERE agent_id = 'a1';
                 DELETE FROM agents WHERE id = 'a1' OR fingerprint = '0123456789abcdef';",
            )
            .unwrap();

        assert_output_telemetry_repository_contracts(&mut store, |store, agent_id, line, at| {
            store
                .checkout_client()
                .unwrap()
                .execute(
                    "INSERT INTO agent_log_chunks (agent_id, line, collected_at)
                     VALUES ($1, $2, $3)",
                    &[&agent_id, &line, &system_time_to_unix_secs(at)],
                )
                .unwrap();
        });
    }

    #[cfg(feature = "postgres")]
    #[test]
    #[ignore = "requires explicit FLEET_TEST_POSTGRES_URL and a disposable Postgres database"]
    fn postgres_drift_policy_capability_repository_roundtrip() {
        let Ok(url) = std::env::var("FLEET_TEST_POSTGRES_URL") else {
            return;
        };
        let mut store = PostgresStore::connect(&url).unwrap();
        store.migrate().unwrap();
        store.checkout_client().unwrap()
            .batch_execute(
                "DELETE FROM policy_drift_schedules WHERE policy_id IN ('policy-contract', 'policy-other') OR agent_id = 'a1';
                 DELETE FROM policy_assignments WHERE policy_id IN ('policy-contract', 'policy-other') OR agent_id = 'a1';
                 DELETE FROM policies WHERE id IN ('policy-contract', 'policy-other');
                 DELETE FROM drift_reports WHERE agent_id = 'a1';
                 DELETE FROM agent_capability_snapshots WHERE agent_id = 'a1';
                 DELETE FROM agent_identities WHERE agent_id = 'a1';
                 DELETE FROM agents WHERE id = 'a1' OR fingerprint = '0123456789abcdef';",
            )
            .unwrap();

        assert_drift_policy_capability_repository_contracts(&mut store);
    }

    #[cfg(feature = "postgres")]
    #[test]
    #[ignore = "requires explicit FLEET_TEST_POSTGRES_URL and a disposable Postgres database"]
    fn postgres_query_artifact_retention_repository_roundtrip() {
        let Ok(url) = std::env::var("FLEET_TEST_POSTGRES_URL") else {
            return;
        };
        let mut store = PostgresStore::connect(&url).unwrap();
        store.migrate().unwrap();
        store.checkout_client().unwrap()
            .batch_execute(
                "DELETE FROM rendered_artifacts WHERE job_id = 'job-query-artifact-retention-contract';
                 DELETE FROM agent_log_chunks WHERE agent_id = 'a1';
                 DELETE FROM metrics_snapshots WHERE agent_id = 'a1';
                 DELETE FROM facts_snapshots WHERE agent_id = 'a1';
                 DELETE FROM job_output_chunks WHERE job_id = 'job-query-artifact-retention-contract';
                 DELETE FROM task_assignments
                 WHERE job_id = 'job-query-artifact-retention-contract'
                    OR id = 'task-query-artifact-retention-contract'
                    OR nonce = 'nonce-query-artifact-retention-contract';
                 DELETE FROM job_targets WHERE job_id = 'job-query-artifact-retention-contract' OR agent_id = 'a1';
                 DELETE FROM jobs WHERE id = 'job-query-artifact-retention-contract';
                 DELETE FROM audit_events WHERE target = 'retention-contract';
                 DELETE FROM agent_identities WHERE agent_id = 'a1';
                 DELETE FROM agents WHERE id = 'a1' OR fingerprint = '0123456789abcdef';",
            )
            .unwrap();

        assert_query_artifact_retention_repository_contracts(
            &mut store,
            |store, agent_id, line, at| {
                store
                    .checkout_client()
                    .unwrap()
                    .execute(
                        "INSERT INTO agent_log_chunks (agent_id, line, collected_at)
                         VALUES ($1, $2, $3)",
                        &[&agent_id, &line, &system_time_to_unix_secs(at)],
                    )
                    .unwrap();
            },
        );
    }

    #[cfg(feature = "postgres")]
    #[test]
    #[ignore = "requires explicit FLEET_TEST_POSTGRES_URL and a disposable Postgres database"]
    fn postgres_remediation_request_repository_roundtrip() {
        let Ok(url) = std::env::var("FLEET_TEST_POSTGRES_URL") else {
            return;
        };
        let mut store = PostgresStore::connect(&url).unwrap();
        store.migrate().unwrap();
        let raw_body_columns: i64 = store.checkout_client().unwrap()
            .query_one(
                "SELECT COUNT(*)
                 FROM information_schema.columns
                 WHERE table_name = 'remediation_requests'
                   AND column_name IN ('runbook_body', 'rendered_body', 'command_output', 'secret_value')",
                &[],
            )
            .unwrap()
            .get(0);
        assert_eq!(raw_body_columns, 0);
        store
            .checkout_client()
            .unwrap()
            .batch_execute(
                "DELETE FROM remediation_requests
                 WHERE id IN (
                    'remediation-contract-1',
                    'remediation-contract-2',
                    'remediation-other'
                 )
                    OR agent_id IN ('a1', 'a2')
                    OR policy_id IN ('policy-remediation-contract', 'policy-remediation-other');
                 DELETE FROM agent_identities WHERE agent_id IN ('a1', 'a2');
                 DELETE FROM agents
                 WHERE id IN ('a1', 'a2')
                    OR fingerprint IN ('0123456789abcdef', 'fedcba9876543210');",
            )
            .unwrap();

        assert_remediation_request_repository_contracts(&mut store);
    }

    fn assert_application_repository_contracts<S>(store: &mut S)
    where
        S: AgentRepository<Error = StoreError>
            + AgentIdentityRepository<Error = StoreError>
            + ControllerIdentityRepository<Error = StoreError>
            + AdminTokenRepository<Error = StoreError>
            + EnrollmentTokenRepository<Error = StoreError>
            + JobRepository<Error = StoreError>
            + ApprovalRepository<Error = StoreError>
            + FactsRepository<Error = StoreError>
            + MetricsRepository<Error = StoreError>
            + DriftRepository<Error = StoreError>
            + PolicyRepository<Error = StoreError>
            + AgentCapabilityRepository<Error = StoreError>,
    {
        <S as AgentRepository>::save(store, agent()).unwrap();

        let identity = <S as AgentIdentityRepository>::find_agent_identity(store, "a1")
            .unwrap()
            .unwrap();
        assert_eq!(identity.fingerprint, "0123456789abcdef");

        <S as ControllerIdentityRepository>::save_controller_identity_metadata(
            store,
            ControllerIdentityMetadata {
                public_key: "controller-pk".to_owned(),
                public_fingerprint: "controller-fp".to_owned(),
                private_key_path: "/var/lib/fleet/controller_private.key".to_owned(),
                created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            },
        )
        .unwrap();
        assert_eq!(
            <S as ControllerIdentityRepository>::controller_identity_metadata(store)
                .unwrap()
                .unwrap()
                .public_fingerprint,
            "controller-fp"
        );

        <S as AdminTokenRepository>::insert_admin_token_hash(store, "admin-hash-contract").unwrap();
        assert!(
            <S as AdminTokenRepository>::verify_admin_token_hash(store, "admin-hash-contract")
                .unwrap()
        );
        assert_eq!(
            <S as AdminTokenRepository>::find_admin_token_record(store, "admin-hash-contract")
                .unwrap(),
            Some(AppAdminTokenRecord {
                actor_id: "bootstrap-admin".to_owned(),
                role: "owner".to_owned(),
            })
        );

        <S as EnrollmentTokenRepository>::insert_enrollment_token_hash(
            store,
            "et-contract",
            "hash-contract",
            "role=web",
            SystemTime::UNIX_EPOCH + Duration::from_secs(60),
            1,
        )
        .unwrap();
        assert_eq!(
            <S as EnrollmentTokenRepository>::list_enrollment_tokens(store)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            <S as EnrollmentTokenRepository>::consume_enrollment_token_hash(
                store,
                "hash-contract",
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap()
            .default_labels,
            "role=web"
        );

        let job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-contract").unwrap(),
            fleet_domain::TaskRisk::Low,
            fleet_domain::ApprovalRequirement::NotRequired,
            Duration::from_secs(30),
        );
        <S as JobRepository>::save(store, job).unwrap();
        <S as ApprovalRepository>::insert_approval_request(
            store,
            AppApprovalRequestRecord {
                id: "approval-contract".to_owned(),
                job_id: "job-contract".to_owned(),
                requester: "operator".to_owned(),
                approver: None,
                reason: "test".to_owned(),
                status: "pending".to_owned(),
                expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(60),
                created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(2),
                decided_at: None,
            },
        )
        .unwrap();
        assert_eq!(
            <S as ApprovalRepository>::list_approval_requests(store, Some("pending"), 10).unwrap()
                [0]
            .job_id,
            "job-contract"
        );
        <S as ApprovalRepository>::insert_approval_request(
            store,
            AppApprovalRequestRecord {
                id: "approval-reserved-remediation".to_owned(),
                job_id: "reserved-remediation-job".to_owned(),
                requester: "operator".to_owned(),
                approver: None,
                reason: "remediation approval before job creation".to_owned(),
                status: "pending".to_owned(),
                expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(60),
                created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(3),
                decided_at: None,
            },
        )
        .unwrap();
        assert_eq!(
            <S as ApprovalRepository>::find_approval_request(
                store,
                "approval-reserved-remediation"
            )
            .unwrap()
            .unwrap()
            .job_id,
            "reserved-remediation-job"
        );

        <S as FactsRepository>::insert_facts_snapshot(
            store,
            "a1",
            "{\"os\":\"linux\"}",
            SystemTime::UNIX_EPOCH + Duration::from_secs(3),
        )
        .unwrap();
        assert!(
            <S as FactsRepository>::latest_facts_snapshot(store, "a1")
                .unwrap()
                .is_some()
        );

        <S as MetricsRepository>::insert_metrics_snapshot(
            store,
            "a1",
            "{\"cpu\":{\"logical_count\":2}}",
            SystemTime::UNIX_EPOCH + Duration::from_secs(4),
        )
        .unwrap();
        assert!(
            <S as MetricsRepository>::latest_metrics_snapshot(store, "a1")
                .unwrap()
                .is_some()
        );

        <S as DriftRepository>::insert_drift_report(
            store,
            "a1",
            &DriftReport {
                policy_name: "contract".to_owned(),
                status: DriftStatus::Compliant,
                severity: DriftSeverity::None,
                acknowledgement: DriftAcknowledgement::Open,
                expected: "expected".to_owned(),
                actual: "actual".to_owned(),
            },
            SystemTime::UNIX_EPOCH + Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(
            <S as DriftRepository>::latest_drift_report(store, "a1")
                .unwrap()
                .unwrap()
                .report
                .policy_name,
            "contract"
        );

        <S as PolicyRepository>::save_policy_source(
            store,
            "policy-contract",
            "contract-policy",
            1,
            "kind: Policy",
        )
        .unwrap();
        assert_eq!(
            <S as PolicyRepository>::find_policy(store, "policy-contract")
                .unwrap()
                .unwrap()
                .name,
            "contract-policy"
        );
        <S as PolicyRepository>::assign_policy_to_agent(
            store,
            "policy-contract",
            "a1",
            SystemTime::UNIX_EPOCH + Duration::from_secs(6),
        )
        .unwrap();
        assert_eq!(
            <S as PolicyRepository>::policies_for_agent(store, "a1").unwrap()[0].policy_id,
            "policy-contract"
        );
        <S as PolicyRepository>::upsert_policy_schedule(
            store,
            "policy-contract",
            "a1",
            Duration::from_secs(300),
            SystemTime::UNIX_EPOCH + Duration::from_secs(300),
        )
        .unwrap();
        assert_eq!(
            <S as PolicyRepository>::due_scheduled_drift_checks(
                store,
                SystemTime::UNIX_EPOCH + Duration::from_secs(300),
                10
            )
            .unwrap()[0]
                .policy_id,
            "policy-contract"
        );
        <S as PolicyRepository>::record_scheduled_drift_check(
            store,
            "policy-contract",
            "a1",
            SystemTime::UNIX_EPOCH + Duration::from_secs(300),
        )
        .unwrap();
        assert!(
            <S as PolicyRepository>::due_scheduled_drift_checks(
                store,
                SystemTime::UNIX_EPOCH + Duration::from_secs(599),
                10
            )
            .unwrap()
            .is_empty()
        );

        <S as AgentCapabilityRepository>::save_agent_capability_snapshot(
            store,
            &AgentId::new("a1").unwrap(),
            AgentCapabilitySnapshot::reported(
                AgentRuntimeProfile::new(
                    PrivilegeLevel::SudoAvailable,
                    Some(PackageManager::Apt),
                    Some(ServiceManager::Systemd),
                    vec![
                        AgentCapability::PersistentSession,
                        AgentCapability::CommandExecution,
                    ],
                ),
                SystemTime::UNIX_EPOCH + Duration::from_secs(7),
            ),
        )
        .unwrap();
        let capability_snapshot =
            <S as AgentCapabilityRepository>::latest_agent_capability_snapshot(
                store,
                &AgentId::new("a1").unwrap(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            capability_snapshot
                .evaluate(fleet_domain::RuntimePrimitive::Command)
                .status,
            fleet_domain::CapabilitySnapshotStatus::Compatible
        );
    }

    fn assert_bootstrap_repository_contracts<S>(store: &mut S)
    where
        S: AgentRepository<Error = StoreError>
            + AgentIdentityRepository<Error = StoreError>
            + ControllerIdentityRepository<Error = StoreError>
            + AdminTokenRepository<Error = StoreError>,
    {
        <S as AgentRepository>::save(store, agent()).unwrap();

        let found = <S as AgentRepository>::find_by_id(store, &AgentId::new("a1").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(found.name().as_str(), "web-01");
        assert_eq!(found.labels()[0].key(), "role");
        assert_eq!(
            <S as AgentRepository>::list(store).unwrap()[0]
                .id()
                .as_str(),
            "a1"
        );
        assert_eq!(
            <S as AgentIdentityRepository>::find_agent_identity(store, "a1")
                .unwrap()
                .unwrap()
                .fingerprint,
            "0123456789abcdef"
        );

        <S as ControllerIdentityRepository>::save_controller_identity_metadata(
            store,
            ControllerIdentityMetadata {
                public_key: "controller-pk".to_owned(),
                public_fingerprint: "controller-fp".to_owned(),
                private_key_path: "/var/lib/fleet/controller_private.key".to_owned(),
                created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            },
        )
        .unwrap();
        assert_eq!(
            <S as ControllerIdentityRepository>::controller_identity_metadata(store)
                .unwrap()
                .unwrap()
                .public_fingerprint,
            "controller-fp"
        );

        <S as AdminTokenRepository>::insert_admin_token_hash(store, "admin-hash-bootstrap")
            .unwrap();
        assert!(<S as AdminTokenRepository>::admin_token_exists(store).unwrap());
        assert!(
            <S as AdminTokenRepository>::verify_admin_token_hash(store, "admin-hash-bootstrap")
                .unwrap()
        );
        assert_eq!(
            <S as AdminTokenRepository>::find_admin_token_record(store, "admin-hash-bootstrap")
                .unwrap(),
            Some(AppAdminTokenRecord {
                actor_id: "bootstrap-admin".to_owned(),
                role: "owner".to_owned(),
            })
        );
    }

    fn assert_enrollment_token_repository_contracts<S>(store: &mut S)
    where
        S: EnrollmentTokenRepository<Error = StoreError>,
    {
        <S as EnrollmentTokenRepository>::insert_enrollment_token_hash(
            store,
            "et-contract",
            "hash-contract",
            "role=web",
            SystemTime::UNIX_EPOCH + Duration::from_secs(60),
            2,
        )
        .unwrap();
        assert_eq!(
            <S as EnrollmentTokenRepository>::list_enrollment_tokens(store)
                .unwrap()
                .len(),
            1
        );
        let consumed = <S as EnrollmentTokenRepository>::consume_enrollment_token_hash(
            store,
            "hash-contract",
            SystemTime::UNIX_EPOCH + Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(consumed.id, "et-contract");
        assert_eq!(consumed.default_labels, "role=web");
        assert_eq!(consumed.used_count, 0);
        assert_eq!(
            <S as EnrollmentTokenRepository>::list_enrollment_tokens(store).unwrap()[0].used_count,
            1
        );

        assert!(
            <S as EnrollmentTokenRepository>::revoke_enrollment_token(store, "et-contract")
                .unwrap()
        );
        assert!(
            !<S as EnrollmentTokenRepository>::revoke_enrollment_token(store, "et-contract")
                .unwrap()
        );
        assert!(matches!(
            <S as EnrollmentTokenRepository>::consume_enrollment_token_hash(
                store,
                "hash-contract",
                SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            ),
            Err(StoreError::Domain(message)) if message.contains("revoked")
        ));

        <S as EnrollmentTokenRepository>::insert_enrollment_token_hash(
            store,
            "et-expired",
            "hash-expired",
            "",
            SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            1,
        )
        .unwrap();
        assert!(matches!(
            <S as EnrollmentTokenRepository>::consume_enrollment_token_hash(
                store,
                "hash-expired",
                SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            ),
            Err(StoreError::Domain(message)) if message.contains("expired")
        ));

        <S as EnrollmentTokenRepository>::insert_enrollment_token_hash(
            store,
            "et-exhausted",
            "hash-exhausted",
            "",
            SystemTime::UNIX_EPOCH + Duration::from_secs(60),
            1,
        )
        .unwrap();
        <S as EnrollmentTokenRepository>::consume_enrollment_token_hash(
            store,
            "hash-exhausted",
            SystemTime::UNIX_EPOCH + Duration::from_secs(2),
        )
        .unwrap();
        assert!(matches!(
            <S as EnrollmentTokenRepository>::consume_enrollment_token_hash(
                store,
                "hash-exhausted",
                SystemTime::UNIX_EPOCH + Duration::from_secs(3),
            ),
            Err(StoreError::Domain(message)) if message.contains("max uses")
        ));
    }

    fn assert_audit_repository_contracts<S>(store: &mut S)
    where
        S: AuditRepository<Error = StoreError>,
    {
        <S as AuditWriter>::write(
            store,
            AuditEvent {
                category: AuditCategory::Security,
                action: "invalid_signature_contract".to_owned(),
                actor: AuditActor::new("system"),
                target: AuditTarget::new("agent-audit-contract"),
                value: AuditValue::Redacted,
                occurred_at: SystemTime::UNIX_EPOCH + Duration::from_secs(3),
            },
        )
        .unwrap();
        <S as AuditWriter>::write(
            store,
            AuditEvent {
                category: AuditCategory::Job,
                action: "job_created_contract".to_owned(),
                actor: AuditActor::new("operator"),
                target: AuditTarget::new("job-audit-contract"),
                value: AuditValue::Plain("target_count=1".to_owned()),
                occurred_at: SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            },
        )
        .unwrap();
        <S as AuditWriter>::write(
            store,
            AuditEvent {
                category: AuditCategory::Security,
                action: "insecure_http_transport_contract".to_owned(),
                actor: AuditActor::new("controller"),
                target: AuditTarget::new("controller-audit-contract"),
                value: AuditValue::SecretRef("secret-ref:transport-warning".to_owned()),
                occurred_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            },
        )
        .unwrap();

        let latest = <S as AuditRepository>::list(store, 10).unwrap();
        assert!(latest.len() >= 3);
        assert_eq!(latest[0].action, "insecure_http_transport_contract");
        assert!(!latest[0].contains_secret_plaintext());

        let security =
            <S as AuditRepository>::list_by_category(store, AuditCategory::Security, 10).unwrap();
        assert_eq!(security.len(), 2);
        assert_eq!(security[0].action, "insecure_http_transport_contract");
        assert!(
            security
                .iter()
                .all(|event| event.category == AuditCategory::Security)
        );
        assert!(
            security
                .iter()
                .all(|event| !event.contains_secret_plaintext())
        );

        let first_page =
            <S as AuditRepository>::export_page(store, Some(AuditCategory::Security), 1, None)
                .unwrap();
        assert_eq!(first_page.len(), 1);
        assert_eq!(first_page[0].event.action, "invalid_signature_contract");
        assert!(first_page[0].cursor.row_id > 0);
        assert!(!first_page[0].event.contains_secret_plaintext());

        let second_page = <S as AuditRepository>::export_page(
            store,
            Some(AuditCategory::Security),
            1,
            Some(first_page[0].cursor),
        )
        .unwrap();
        assert_eq!(second_page.len(), 1);
        assert_eq!(
            second_page[0].event.action,
            "insecure_http_transport_contract"
        );
        assert!(second_page[0].cursor.row_id > 0);
    }

    fn assert_approval_repository_contracts<S>(store: &mut S)
    where
        S: ApprovalRepository<Error = StoreError>,
    {
        <S as ApprovalRepository>::insert_approval_request(
            store,
            AppApprovalRequestRecord {
                id: "approval-other".to_owned(),
                job_id: "job-approval-other".to_owned(),
                requester: "operator".to_owned(),
                approver: None,
                reason: "older request".to_owned(),
                status: "pending".to_owned(),
                expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(60),
                created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
                decided_at: None,
            },
        )
        .unwrap();
        <S as ApprovalRepository>::insert_approval_request(
            store,
            AppApprovalRequestRecord {
                id: "approval-contract".to_owned(),
                job_id: "job-approval-contract".to_owned(),
                requester: "operator".to_owned(),
                approver: None,
                reason: "reserved remediation job may not exist yet".to_owned(),
                status: "pending".to_owned(),
                expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(60),
                created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(2),
                decided_at: None,
            },
        )
        .unwrap();

        let found =
            <S as ApprovalRepository>::find_approval_request(store, "approval-contract").unwrap();
        assert_eq!(
            found.as_ref().map(|record| record.job_id.as_str()),
            Some("job-approval-contract")
        );

        let pending = <S as ApprovalRepository>::find_pending_approval_for_job(
            store,
            "job-approval-contract",
        )
        .unwrap();
        assert_eq!(
            pending.as_ref().map(|record| record.id.as_str()),
            Some("approval-contract")
        );

        let pending_requests =
            <S as ApprovalRepository>::list_approval_requests(store, Some("pending"), 10).unwrap();
        assert_eq!(pending_requests.len(), 2);
        assert_eq!(pending_requests[0].id, "approval-contract");
        assert_eq!(pending_requests[1].id, "approval-other");

        let mut approved = pending.unwrap();
        approved.status = "approved".to_owned();
        approved.approver = Some("admin".to_owned());
        approved.decided_at = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(3));
        assert!(<S as ApprovalRepository>::update_approval_request(store, approved).unwrap());

        assert!(
            <S as ApprovalRepository>::find_pending_approval_for_job(
                store,
                "job-approval-contract"
            )
            .unwrap()
            .is_none()
        );
        let approved = <S as ApprovalRepository>::find_approval_request(store, "approval-contract")
            .unwrap()
            .unwrap();
        assert_eq!(approved.status, "approved");
        assert_eq!(approved.approver.as_deref(), Some("admin"));
        assert!(approved.decided_at.is_some());

        assert!(
            !<S as ApprovalRepository>::update_approval_request(
                store,
                AppApprovalRequestRecord {
                    id: "approval-missing".to_owned(),
                    job_id: "job-approval-missing".to_owned(),
                    requester: "operator".to_owned(),
                    approver: None,
                    reason: "missing request".to_owned(),
                    status: "rejected".to_owned(),
                    expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(60),
                    created_at: SystemTime::UNIX_EPOCH,
                    decided_at: None,
                },
            )
            .unwrap()
        );

        assert!(
            !<S as ApprovalRepository>::update_job_status_for_approval(
                store,
                "job-approval-missing",
                JobStatus::Failed,
            )
            .unwrap()
        );
    }

    fn assert_job_assignment_repository_contracts<S>(store: &mut S)
    where
        S: AgentRepository<Error = StoreError>
            + JobRepository<Error = StoreError>
            + TaskAssignmentRepository<Error = StoreError>,
    {
        <S as AgentRepository>::save(store, agent()).unwrap();

        let job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-assignment-contract").unwrap(),
            fleet_domain::TaskRisk::Low,
            fleet_domain::ApprovalRequirement::NotRequired,
            Duration::from_secs(30),
        );
        <S as JobRepository>::save(store, job).unwrap();
        let duplicate_job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-assignment-contract").unwrap(),
            fleet_domain::TaskRisk::Low,
            fleet_domain::ApprovalRequirement::NotRequired,
            Duration::from_secs(30),
        );
        assert!(<S as JobRepository>::save(store, duplicate_job).is_err());

        let envelope = task_envelope_for_job(
            "job-assignment-contract",
            "a1",
            "nonce-assignment-contract",
            "task-assignment-contract",
        );
        <S as TaskAssignmentRepository>::save_assignment(store, envelope.clone()).unwrap();
        assert!(<S as TaskAssignmentRepository>::save_assignment(store, envelope).is_err());

        let mut unsigned = task_envelope_for_job(
            "job-assignment-contract",
            "a1",
            "nonce-unsigned-contract",
            "task-unsigned-contract",
        );
        unsigned.signature = None;
        assert!(matches!(
            <S as TaskAssignmentRepository>::save_assignment(store, unsigned),
            Err(StoreError::Domain(message)) if message.contains("signed")
        ));
    }

    fn assert_typed_job_repository_contracts<S>(store: &mut S)
    where
        S: AgentRepository<Error = StoreError>
            + CommandJobRepository
            + DriftCheckJobRepository
            + RunbookJobRepository
            + TaskAssignmentRepository<Error = StoreError>
            + ApprovalRepository<Error = StoreError>
            + DispatchAssignmentRepository<Error = StoreError>,
    {
        <S as AgentRepository>::save(store, agent()).unwrap();

        let command =
            CommandTask::new("echo", vec!["hello".to_owned()], Duration::from_secs(30)).unwrap();
        let command_job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-command-contract").unwrap(),
            command.risk(),
            fleet_domain::ApprovalRequirement::NotRequired,
            command.timeout(),
        );
        <S as CommandJobRepository>::save_command_job(store, command_job, &command).unwrap();
        let duplicate_command_job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-command-contract").unwrap(),
            command.risk(),
            fleet_domain::ApprovalRequirement::NotRequired,
            command.timeout(),
        );
        assert!(
            <S as CommandJobRepository>::save_command_job(store, duplicate_command_job, &command)
                .is_err()
        );

        let runbook = RunbookExecutionTask::new(
            "kind: Runbook\nmetadata:\n  name: contract\n",
            Duration::from_secs(30),
        )
        .unwrap();
        let runbook_job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-runbook-contract").unwrap(),
            runbook.risk(),
            fleet_domain::ApprovalRequirement::ManualApproval,
            runbook.timeout(),
        );
        <S as RunbookJobRepository>::save_runbook_job(store, runbook_job, &runbook).unwrap();

        let drift = DriftCheckTask::new(
            "kind: Policy\nmetadata:\n  name: contract\n",
            Duration::from_secs(30),
        )
        .unwrap();
        let drift_job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-drift-contract").unwrap(),
            drift.risk(),
            fleet_domain::ApprovalRequirement::NotRequired,
            drift.timeout(),
        );
        <S as DriftCheckJobRepository>::save_drift_check_job(store, drift_job, &drift).unwrap();

        let mut atomic_job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-command-atomic-contract").unwrap(),
            command.risk(),
            fleet_domain::ApprovalRequirement::NotRequired,
            command.timeout(),
        );
        atomic_job.queue(true).unwrap();
        let first_assignment = task_envelope_for_job(
            "job-command-atomic-contract",
            "a1",
            "nonce-command-atomic-duplicate",
            "task-command-atomic-first",
        );
        let duplicate_nonce_assignment = task_envelope_for_job(
            "job-command-atomic-contract",
            "a1",
            "nonce-command-atomic-duplicate",
            "task-command-atomic-second",
        );
        let error = <S as CommandJobRepository>::save_command_job_with_assignments(
            store,
            atomic_job,
            &command,
            &[first_assignment, duplicate_nonce_assignment],
        )
        .expect_err("duplicate assignment nonce should fail the atomic job bundle");
        assert!(matches!(
            error,
            StoreError::ConstraintViolation(_) | StoreError::Postgres(_) | StoreError::Sqlite(_)
        ));

        let mut retry_job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-command-atomic-contract").unwrap(),
            command.risk(),
            fleet_domain::ApprovalRequirement::NotRequired,
            command.timeout(),
        );
        retry_job.queue(true).unwrap();
        let retry_assignment = task_envelope_for_job(
            "job-command-atomic-contract",
            "a1",
            "nonce-command-atomic-retry",
            "task-command-atomic-retry",
        );
        <S as CommandJobRepository>::save_command_job_with_assignments(
            store,
            retry_job,
            &command,
            &[retry_assignment],
        )
        .unwrap();
        let pending = <S as DispatchAssignmentRepository>::list_pending_assignments(
            store,
            None,
            Some(&fleet_domain::JobId::new("job-command-atomic-contract").unwrap()),
            10,
        )
        .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].envelope.task_id.as_str(),
            "task-command-atomic-retry"
        );
    }

    fn assert_dispatch_assignment_repository_contracts<S>(store: &mut S)
    where
        S: AgentRepository<Error = StoreError>
            + CommandJobRepository
            + DriftCheckJobRepository
            + RunbookJobRepository
            + TaskAssignmentRepository<Error = StoreError>
            + ApprovalRepository<Error = StoreError>
            + DispatchAssignmentRepository<Error = StoreError>,
    {
        <S as AgentRepository>::save(store, agent()).unwrap();

        let command =
            CommandTask::new("echo", vec!["hello".to_owned()], Duration::from_secs(30)).unwrap();
        let mut command_job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-dispatch-command-contract").unwrap(),
            command.risk(),
            fleet_domain::ApprovalRequirement::NotRequired,
            command.timeout(),
        );
        command_job.queue(true).unwrap();
        <S as CommandJobRepository>::save_command_job(store, command_job, &command).unwrap();
        <S as TaskAssignmentRepository>::save_assignment(
            store,
            task_envelope_for_job(
                "job-dispatch-command-contract",
                "a1",
                "nonce-dispatch-command-contract",
                "task-dispatch-command-contract",
            ),
        )
        .unwrap();

        let drift = DriftCheckTask::new(
            "kind: Policy\nmetadata:\n  name: dispatch\n",
            Duration::from_secs(30),
        )
        .unwrap();
        let mut drift_job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-dispatch-drift-contract").unwrap(),
            drift.risk(),
            fleet_domain::ApprovalRequirement::NotRequired,
            drift.timeout(),
        );
        drift_job.queue(true).unwrap();
        <S as DriftCheckJobRepository>::save_drift_check_job(store, drift_job, &drift).unwrap();
        <S as TaskAssignmentRepository>::save_assignment(
            store,
            task_envelope_for_job(
                "job-dispatch-drift-contract",
                "a1",
                "nonce-dispatch-drift-contract",
                "task-dispatch-drift-contract",
            ),
        )
        .unwrap();

        let runbook = RunbookExecutionTask::new(
            "kind: Runbook\nmetadata:\n  name: dispatch\n",
            Duration::from_secs(30),
        )
        .unwrap();
        let mut runbook_job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-dispatch-runbook-contract").unwrap(),
            runbook.risk(),
            fleet_domain::ApprovalRequirement::ManualApproval,
            runbook.timeout(),
        );
        runbook_job.queue(true).unwrap();
        <S as RunbookJobRepository>::save_runbook_job(store, runbook_job, &runbook).unwrap();
        <S as TaskAssignmentRepository>::save_assignment(
            store,
            task_envelope_for_job(
                "job-dispatch-runbook-contract",
                "a1",
                "nonce-dispatch-runbook-contract",
                "task-dispatch-runbook-contract",
            ),
        )
        .unwrap();

        let all = <S as DispatchAssignmentRepository>::list_pending_assignments(
            store,
            Some(&AgentId::new("a1").unwrap()),
            None,
            10,
        )
        .unwrap();
        assert_eq!(all.len(), 3);
        assert!(matches!(all[0].task, TaskKind::Command(_)));
        assert!(matches!(all[1].task, TaskKind::DriftCheck(_)));
        assert!(matches!(all[2].task, TaskKind::RunbookExecution(_)));

        let limited =
            <S as DispatchAssignmentRepository>::list_pending_assignments(store, None, None, 2)
                .unwrap();
        assert_eq!(limited.len(), 2);
        assert!(
            <S as DispatchAssignmentRepository>::list_pending_assignments(store, None, None, 0)
                .unwrap()
                .is_empty()
        );

        let command_job_id = fleet_domain::JobId::new("job-dispatch-command-contract").unwrap();
        let command_only = <S as DispatchAssignmentRepository>::list_pending_assignments(
            store,
            None,
            Some(&command_job_id),
            10,
        )
        .unwrap();
        assert_eq!(command_only.len(), 1);
        assert!(matches!(command_only[0].task, TaskKind::Command(_)));

        let agent = <S as DispatchAssignmentRepository>::find_dispatch_agent(
            store,
            &AgentId::new("a1").unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(agent.id().as_str(), "a1");
        assert!(
            <S as DispatchAssignmentRepository>::find_dispatch_agent(
                store,
                &AgentId::new("missing-agent").unwrap(),
            )
            .unwrap()
            .is_none()
        );
        assert!(
            <S as DispatchAssignmentRepository>::latest_agent_capability_snapshot(
                store,
                &AgentId::new("a1").unwrap(),
            )
            .unwrap()
            .is_none()
        );

        let missing_gate = <S as DispatchAssignmentRepository>::dispatch_gate(
            store,
            &fleet_domain::JobId::new("missing-job").unwrap(),
        )
        .unwrap();
        assert_eq!(missing_gate.concurrency, 1);
        assert_eq!(missing_gate.active_count, 0);
        assert_eq!(missing_gate.failure_count, 0);

        let runbook_task_id = fleet_domain::TaskId::new("task-dispatch-runbook-contract").unwrap();
        let runbook_job_id = fleet_domain::JobId::new("job-dispatch-runbook-contract").unwrap();
        assert!(
            <S as DispatchAssignmentRepository>::claim_assignment_for_dispatch(
                store,
                &runbook_task_id,
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap()
        );
        assert!(
            !<S as DispatchAssignmentRepository>::claim_assignment_for_dispatch(
                store,
                &runbook_task_id,
                SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            )
            .unwrap()
        );
        assert!(
            <S as DispatchAssignmentRepository>::list_pending_assignments(
                store,
                None,
                Some(&runbook_job_id),
                10,
            )
            .unwrap()
            .is_empty()
        );
        <S as DispatchAssignmentRepository>::release_assignment_dispatch_claim(
            store,
            &runbook_task_id,
            SystemTime::UNIX_EPOCH + Duration::from_secs(3),
            "send failed",
        )
        .unwrap();
        assert_eq!(
            <S as DispatchAssignmentRepository>::list_pending_assignments(
                store,
                None,
                Some(&runbook_job_id),
                10,
            )
            .unwrap()
            .len(),
            1
        );

        let command_task_id = fleet_domain::TaskId::new("task-dispatch-command-contract").unwrap();
        <S as DispatchAssignmentRepository>::mark_assignment_dispatched(
            store,
            &command_task_id,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1),
        )
        .unwrap();
        let command_gate =
            <S as DispatchAssignmentRepository>::dispatch_gate(store, &command_job_id).unwrap();
        assert_eq!(command_gate.active_count, 1);
        assert!(
            <S as DispatchAssignmentRepository>::list_pending_assignments(
                store,
                None,
                Some(&command_job_id),
                10,
            )
            .unwrap()
            .is_empty()
        );
        <S as DispatchAssignmentRepository>::mark_job_running(
            store,
            &command_job_id,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1),
        )
        .unwrap();

        let drift_task_id = fleet_domain::TaskId::new("task-dispatch-drift-contract").unwrap();
        <S as DispatchAssignmentRepository>::mark_assignment_rejected(
            store,
            &drift_task_id,
            SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            "capability unsupported",
        )
        .unwrap();
        let drift_job_id = fleet_domain::JobId::new("job-dispatch-drift-contract").unwrap();
        let drift_gate =
            <S as DispatchAssignmentRepository>::dispatch_gate(store, &drift_job_id).unwrap();
        assert_eq!(drift_gate.failure_count, 1);
        assert!(
            <S as DispatchAssignmentRepository>::list_pending_assignments(
                store,
                None,
                Some(&drift_job_id),
                10,
            )
            .unwrap()
            .is_empty()
        );
        <S as DispatchAssignmentRepository>::mark_assignment_rejected(
            store,
            &drift_task_id,
            SystemTime::UNIX_EPOCH + Duration::from_secs(3),
            "second rejection is a no-op",
        )
        .unwrap();
        <S as DispatchAssignmentRepository>::release_assignment_dispatch_claim(
            store,
            &drift_task_id,
            SystemTime::UNIX_EPOCH + Duration::from_secs(3),
            "terminal assignment must not requeue",
        )
        .unwrap();
        let drift_gate =
            <S as DispatchAssignmentRepository>::dispatch_gate(store, &drift_job_id).unwrap();
        assert_eq!(drift_gate.failure_count, 1);
        assert!(
            <S as DispatchAssignmentRepository>::list_pending_assignments(
                store,
                None,
                Some(&drift_job_id),
                10,
            )
            .unwrap()
            .is_empty()
        );

        let runbook_job_id = fleet_domain::JobId::new("job-dispatch-runbook-contract").unwrap();
        <S as DispatchAssignmentRepository>::mark_job_expired(
            store,
            &runbook_job_id,
            SystemTime::UNIX_EPOCH + Duration::from_secs(4),
        )
        .unwrap();
        assert!(
            <S as DispatchAssignmentRepository>::list_pending_assignments(
                store,
                None,
                Some(&runbook_job_id),
                10,
            )
            .unwrap()
            .is_empty()
        );
    }

    fn assert_output_telemetry_repository_contracts<S, F>(store: &mut S, mut insert_log: F)
    where
        S: AgentRepository<Error = StoreError>
            + JobRepository<Error = StoreError>
            + JobOutputRepository<Error = StoreError>
            + FactsRepository<Error = StoreError>
            + MetricsRepository<Error = StoreError>
            + AgentLogRepository<Error = StoreError>,
        F: FnMut(&mut S, &str, &str, SystemTime),
    {
        <S as AgentRepository>::save(store, agent()).unwrap();
        <S as JobRepository>::save(
            store,
            fleet_domain::Job::new(
                fleet_domain::JobId::new("job-output-telemetry-contract").unwrap(),
                fleet_domain::TaskRisk::Low,
                fleet_domain::ApprovalRequirement::NotRequired,
                Duration::from_secs(30),
            ),
        )
        .unwrap();

        let second = JobOutputChunk {
            job_id: "job-output-telemetry-contract".to_owned(),
            agent_id: "a1".to_owned(),
            stream: JobOutputStream::Stdout,
            sequence: 1,
            body: "second".to_owned(),
        };
        let first = JobOutputChunk {
            sequence: 0,
            body: "first".to_owned(),
            ..second.clone()
        };
        <S as JobOutputRepository>::append_output_chunk(store, second).unwrap();
        <S as JobOutputRepository>::append_output_chunk(store, first.clone()).unwrap();
        <S as JobOutputRepository>::append_output_chunk(store, first.clone()).unwrap();

        let chunks = <S as JobOutputRepository>::list_output_chunks(
            store,
            "job-output-telemetry-contract",
            "a1",
        )
        .unwrap();
        assert_eq!(
            chunks
                .iter()
                .map(|chunk| chunk.body.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        let job_chunks = <S as JobOutputRepository>::list_output_chunks_for_job(
            store,
            "job-output-telemetry-contract",
        )
        .unwrap();
        assert_eq!(job_chunks.len(), 2);

        let conflict = JobOutputChunk {
            body: "changed".to_owned(),
            ..first
        };
        assert!(matches!(
            <S as JobOutputRepository>::append_output_chunk(store, conflict),
            Err(StoreError::ConstraintViolation(_))
        ));

        for body in [
            "{\"seq\":1,\"kind\":\"facts\"}",
            "{\"seq\":2,\"kind\":\"facts\"}",
            "{\"seq\":3,\"kind\":\"facts\"}",
        ] {
            <S as FactsRepository>::insert_facts_snapshot(
                store,
                "a1",
                body,
                SystemTime::UNIX_EPOCH + Duration::from_secs(10),
            )
            .unwrap();
        }
        let latest_facts = <S as FactsRepository>::latest_facts_snapshot(store, "a1")
            .unwrap()
            .unwrap();
        assert_eq!(latest_facts.body, "{\"seq\":3,\"kind\":\"facts\"}");
        let first_facts_page =
            <S as FactsRepository>::list_facts_snapshots(store, "a1", 2, None).unwrap();
        assert_eq!(
            first_facts_page
                .iter()
                .map(|record| record.body.as_str())
                .collect::<Vec<_>>(),
            vec![
                "{\"seq\":3,\"kind\":\"facts\"}",
                "{\"seq\":2,\"kind\":\"facts\"}"
            ]
        );
        let second_facts_page = <S as FactsRepository>::list_facts_snapshots(
            store,
            "a1",
            2,
            Some(first_facts_page[1].cursor),
        )
        .unwrap();
        assert_eq!(second_facts_page.len(), 1);
        assert_eq!(second_facts_page[0].body, "{\"seq\":1,\"kind\":\"facts\"}");

        for body in [
            "{\"seq\":1,\"kind\":\"metrics\"}",
            "{\"seq\":2,\"kind\":\"metrics\"}",
            "{\"seq\":3,\"kind\":\"metrics\"}",
        ] {
            <S as MetricsRepository>::insert_metrics_snapshot(
                store,
                "a1",
                body,
                SystemTime::UNIX_EPOCH + Duration::from_secs(20),
            )
            .unwrap();
        }
        let latest_metrics = <S as MetricsRepository>::latest_metrics_snapshot(store, "a1")
            .unwrap()
            .unwrap();
        assert_eq!(latest_metrics.body, "{\"seq\":3,\"kind\":\"metrics\"}");
        let first_metrics_page =
            <S as MetricsRepository>::list_metrics_snapshots(store, "a1", 2, None).unwrap();
        assert_eq!(
            first_metrics_page
                .iter()
                .map(|record| record.body.as_str())
                .collect::<Vec<_>>(),
            vec![
                "{\"seq\":3,\"kind\":\"metrics\"}",
                "{\"seq\":2,\"kind\":\"metrics\"}"
            ]
        );
        let second_metrics_page = <S as MetricsRepository>::list_metrics_snapshots(
            store,
            "a1",
            2,
            Some(first_metrics_page[1].cursor),
        )
        .unwrap();
        assert_eq!(second_metrics_page.len(), 1);
        assert_eq!(
            second_metrics_page[0].body,
            "{\"seq\":1,\"kind\":\"metrics\"}"
        );

        for line in [
            "level=info event=agent_log_uploaded sequence=1",
            "level=info event=agent_log_uploaded sequence=2",
            "level=info event=agent_log_uploaded sequence=3",
        ] {
            insert_log(
                store,
                "a1",
                line,
                SystemTime::UNIX_EPOCH + Duration::from_secs(30),
            );
        }
        let first_log_page =
            <S as AgentLogRepository>::list_agent_log_chunks(store, "a1", 2, None).unwrap();
        assert_eq!(
            first_log_page
                .iter()
                .map(|record| record.line.as_str())
                .collect::<Vec<_>>(),
            vec![
                "level=info event=agent_log_uploaded sequence=3",
                "level=info event=agent_log_uploaded sequence=2"
            ]
        );
        let second_log_page = <S as AgentLogRepository>::list_agent_log_chunks(
            store,
            "a1",
            2,
            Some(first_log_page[1].cursor),
        )
        .unwrap();
        assert_eq!(second_log_page.len(), 1);
        assert!(second_log_page[0].line.contains("sequence=1"));
    }

    fn assert_drift_policy_capability_repository_contracts<S>(store: &mut S)
    where
        S: AgentRepository<Error = StoreError>
            + AgentCapabilityRepository<Error = StoreError>
            + DriftRepository<Error = StoreError>
            + PolicyRepository<Error = StoreError>,
    {
        <S as AgentRepository>::save(store, agent()).unwrap();

        assert!(
            !<S as PolicyRepository>::acknowledge_latest_drift_report(
                store,
                "a1",
                "nginx-running",
                "admin",
                SystemTime::UNIX_EPOCH,
            )
            .unwrap()
        );
        assert!(
            !<S as PolicyRepository>::mark_latest_drift_resolved(
                store,
                "a1",
                "nginx-running",
                "job-remediate",
                SystemTime::UNIX_EPOCH,
            )
            .unwrap()
        );

        for (actual, status, severity) in [
            ("actual-1", DriftStatus::Unknown, DriftSeverity::Unknown),
            ("actual-2", DriftStatus::Compliant, DriftSeverity::None),
            ("actual-3", DriftStatus::Drifted, DriftSeverity::Warning),
        ] {
            <S as DriftRepository>::insert_drift_report(
                store,
                "a1",
                &DriftReport {
                    policy_name: "nginx-running".to_owned(),
                    status,
                    severity,
                    acknowledgement: DriftAcknowledgement::Open,
                    expected: "service nginx running".to_owned(),
                    actual: actual.to_owned(),
                },
                SystemTime::UNIX_EPOCH + Duration::from_secs(10),
            )
            .unwrap();
        }

        let latest = <S as DriftRepository>::latest_drift_report(store, "a1")
            .unwrap()
            .unwrap();
        assert_eq!(latest.report.actual, "actual-3");
        assert_eq!(latest.report.status, DriftStatus::Drifted);
        assert_eq!(latest.report.severity, DriftSeverity::Warning);

        let first_drift_page =
            <S as DriftRepository>::list_drift_reports(store, "a1", 2, None).unwrap();
        assert_eq!(
            first_drift_page
                .iter()
                .map(|record| record.report.actual.as_str())
                .collect::<Vec<_>>(),
            vec!["actual-3", "actual-2"]
        );
        let second_drift_page = <S as DriftRepository>::list_drift_reports(
            store,
            "a1",
            2,
            Some(first_drift_page[1].cursor),
        )
        .unwrap();
        assert_eq!(second_drift_page.len(), 1);
        assert_eq!(second_drift_page[0].report.actual, "actual-1");

        assert!(
            <S as PolicyRepository>::acknowledge_latest_drift_report(
                store,
                "a1",
                "nginx-running",
                "admin",
                SystemTime::UNIX_EPOCH + Duration::from_secs(11),
            )
            .unwrap()
        );
        let acknowledged = <S as DriftRepository>::latest_drift_report(store, "a1")
            .unwrap()
            .unwrap();
        assert!(matches!(
            acknowledged.report.acknowledgement,
            DriftAcknowledgement::Acknowledged { ref by, .. } if by == "admin"
        ));

        assert!(
            <S as PolicyRepository>::mark_latest_drift_resolved(
                store,
                "a1",
                "nginx-running",
                "job-remediate",
                SystemTime::UNIX_EPOCH + Duration::from_secs(12),
            )
            .unwrap()
        );
        let resolved = <S as DriftRepository>::latest_drift_report(store, "a1")
            .unwrap()
            .unwrap();
        assert!(matches!(
            resolved.report.acknowledgement,
            DriftAcknowledgement::Resolved { ref job_id, .. } if job_id == "job-remediate"
        ));

        <S as PolicyRepository>::save_policy_source(
            store,
            "policy-contract",
            "nginx-running",
            1,
            "kind: Policy\nmetadata:\n  name: nginx-running\n",
        )
        .unwrap();
        <S as PolicyRepository>::save_policy_source(
            store,
            "policy-other",
            "ssh-running",
            1,
            "kind: Policy\nmetadata:\n  name: ssh-running\n",
        )
        .unwrap();
        <S as PolicyRepository>::save_policy_source(
            store,
            "policy-contract",
            "nginx-running",
            2,
            "kind: Policy\nmetadata:\n  name: nginx-running\nspec:\n  version: 2\n",
        )
        .unwrap();

        let policies = <S as PolicyRepository>::list_policies(store).unwrap();
        assert_eq!(
            policies
                .iter()
                .map(|policy| policy.id.as_str())
                .collect::<Vec<_>>(),
            vec!["policy-contract", "policy-other"]
        );
        let policy = <S as PolicyRepository>::find_policy(store, "policy-contract")
            .unwrap()
            .unwrap();
        assert_eq!(policy.version, 2);
        assert!(policy.source.contains("version: 2"));

        <S as PolicyRepository>::assign_policy_to_agent(
            store,
            "policy-contract",
            "a1",
            SystemTime::UNIX_EPOCH + Duration::from_secs(20),
        )
        .unwrap();
        let assignments = <S as PolicyRepository>::policies_for_agent(store, "a1").unwrap();
        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].policy_id, "policy-contract");

        assert!(matches!(
            <S as PolicyRepository>::upsert_policy_schedule(
                store,
                "policy-contract",
                "a1",
                Duration::ZERO,
                SystemTime::UNIX_EPOCH + Duration::from_secs(300),
            ),
            Err(StoreError::Domain(message)) if message.contains("positive")
        ));
        <S as PolicyRepository>::upsert_policy_schedule(
            store,
            "policy-contract",
            "a1",
            Duration::from_secs(300),
            SystemTime::UNIX_EPOCH + Duration::from_secs(300),
        )
        .unwrap();
        assert!(
            <S as PolicyRepository>::due_scheduled_drift_checks(
                store,
                SystemTime::UNIX_EPOCH + Duration::from_secs(299),
                10,
            )
            .unwrap()
            .is_empty()
        );
        let due = <S as PolicyRepository>::due_scheduled_drift_checks(
            store,
            SystemTime::UNIX_EPOCH + Duration::from_secs(300),
            10,
        )
        .unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].policy_id, "policy-contract");
        assert_eq!(due[0].last_checked_at, None);

        <S as PolicyRepository>::record_scheduled_drift_check(
            store,
            "policy-contract",
            "a1",
            SystemTime::UNIX_EPOCH + Duration::from_secs(300),
        )
        .unwrap();
        assert!(
            <S as PolicyRepository>::due_scheduled_drift_checks(
                store,
                SystemTime::UNIX_EPOCH + Duration::from_secs(599),
                10,
            )
            .unwrap()
            .is_empty()
        );
        let due_again = <S as PolicyRepository>::due_scheduled_drift_checks(
            store,
            SystemTime::UNIX_EPOCH + Duration::from_secs(600),
            10,
        )
        .unwrap();
        assert_eq!(due_again.len(), 1);
        assert_eq!(
            due_again[0].last_checked_at,
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(300))
        );

        let agent_id = AgentId::new("a1").unwrap();
        assert!(matches!(
            <S as AgentCapabilityRepository>::save_agent_capability_snapshot(
                store,
                &agent_id,
                AgentCapabilitySnapshot::unknown(),
            ),
            Err(StoreError::Domain(message)) if message.contains("profile")
        ));
        <S as AgentCapabilityRepository>::save_agent_capability_snapshot(
            store,
            &agent_id,
            AgentCapabilitySnapshot::reported(
                AgentRuntimeProfile::new(
                    PrivilegeLevel::SudoAvailable,
                    Some(PackageManager::Apt),
                    Some(ServiceManager::Systemd),
                    vec![
                        AgentCapability::PersistentSession,
                        AgentCapability::CommandExecution,
                    ],
                ),
                SystemTime::UNIX_EPOCH + Duration::from_secs(30),
            ),
        )
        .unwrap();
        let compatible =
            <S as AgentCapabilityRepository>::latest_agent_capability_snapshot(store, &agent_id)
                .unwrap()
                .unwrap();
        assert_eq!(
            compatible
                .evaluate(fleet_domain::RuntimePrimitive::Command)
                .status,
            fleet_domain::CapabilitySnapshotStatus::Compatible
        );

        <S as AgentCapabilityRepository>::save_agent_capability_snapshot(
            store,
            &agent_id,
            AgentCapabilitySnapshot::reported(
                AgentRuntimeProfile::new(PrivilegeLevel::Unprivileged, None, None, Vec::new()),
                SystemTime::UNIX_EPOCH + Duration::from_secs(31),
            ),
        )
        .unwrap();
        let unsupported =
            <S as AgentCapabilityRepository>::latest_agent_capability_snapshot(store, &agent_id)
                .unwrap()
                .unwrap();
        assert_eq!(
            unsupported
                .evaluate(fleet_domain::RuntimePrimitive::Command)
                .status,
            fleet_domain::CapabilitySnapshotStatus::Unsupported
        );
    }

    fn assert_query_artifact_retention_repository_contracts<S, F>(store: &mut S, mut insert_log: F)
    where
        S: AgentRepository<Error = StoreError>
            + CommandJobRepository
            + TaskAssignmentRepository<Error = StoreError>
            + ApprovalRepository<Error = StoreError>
            + JobQueryRepository<Error = StoreError>
            + ArtifactMetadataRepository<Error = StoreError>
            + JobOutputRepository<Error = StoreError>
            + FactsRepository<Error = StoreError>
            + MetricsRepository<Error = StoreError>
            + AgentLogRepository<Error = StoreError>
            + AuditRepository<Error = StoreError>
            + RetentionRepository<Error = StoreError>,
        F: FnMut(&mut S, &str, &str, SystemTime),
    {
        <S as AgentRepository>::save(store, agent()).unwrap();

        let command =
            CommandTask::new("echo", vec!["hello".to_owned()], Duration::from_secs(30)).unwrap();
        let mut job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-query-artifact-retention-contract").unwrap(),
            command.risk(),
            fleet_domain::ApprovalRequirement::NotRequired,
            command.timeout(),
        );
        job.queue(true).unwrap();
        <S as CommandJobRepository>::save_command_job(store, job, &command).unwrap();
        <S as TaskAssignmentRepository>::save_assignment(
            store,
            task_envelope_for_job(
                "job-query-artifact-retention-contract",
                "a1",
                "nonce-query-artifact-retention-contract",
                "task-query-artifact-retention-contract",
            ),
        )
        .unwrap();

        let summary = <S as JobQueryRepository>::find_job_summary(
            store,
            "job-query-artifact-retention-contract",
        )
        .unwrap()
        .unwrap();
        assert_eq!(summary.id, "job-query-artifact-retention-contract");
        assert_eq!(summary.command_program.as_deref(), Some("echo"));
        assert_eq!(summary.command_args, vec!["hello".to_owned()]);
        assert_eq!(summary.selector_kind, "explicit_ids");
        assert_eq!(summary.strategy_concurrency, 1);
        assert_eq!(summary.strategy_max_failures, None);
        assert_eq!(summary.target_count, 1);
        assert_eq!(summary.target_agents.len(), 1);
        assert_eq!(summary.target_agents[0].agent_id, "a1");
        assert_eq!(
            summary.target_agents[0].task_id.as_deref(),
            Some("task-query-artifact-retention-contract")
        );
        assert_eq!(
            summary.target_agents[0].assignment_status.as_deref(),
            Some("queued")
        );
        assert_eq!(
            summary.target_agents[0].labels,
            vec![("role".to_owned(), "web".to_owned())]
        );
        assert!(summary.expires_at.is_some());
        assert!(
            <S as JobQueryRepository>::list_job_summaries(store, 10)
                .unwrap()
                .iter()
                .any(|record| record.id == "job-query-artifact-retention-contract")
        );

        let metadata = RenderedArtifactMetadata::new(
            ArtifactId::new("artifact-query-contract").unwrap(),
            JobId::new("job-query-artifact-retention-contract").unwrap(),
            AgentId::new("a1").unwrap(),
            TaskId::new("task-query-artifact-retention-contract").unwrap(),
            "/etc/fleet/rendered.conf",
            ArtifactChecksum::sha256(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap(),
            42,
            ArtifactRetentionClass::RenderedTemplate,
            SystemTime::UNIX_EPOCH + Duration::from_secs(40),
        )
        .unwrap();
        <S as ArtifactMetadataRepository>::save_rendered_artifact_metadata(store, metadata.clone())
            .unwrap();
        assert!(matches!(
            <S as ArtifactMetadataRepository>::save_rendered_artifact_metadata(store, metadata),
            Err(StoreError::ConstraintViolation(_))
        ));
        let artifacts = <S as ArtifactMetadataRepository>::list_rendered_artifacts_for_job(
            store,
            &JobId::new("job-query-artifact-retention-contract").unwrap(),
        )
        .unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].id.as_str(), "artifact-query-contract");
        assert_eq!(
            artifacts[0].checksum.as_sha256(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );

        <S as JobOutputRepository>::append_output_chunk(
            store,
            JobOutputChunk {
                job_id: "job-query-artifact-retention-contract".to_owned(),
                agent_id: "a1".to_owned(),
                stream: JobOutputStream::Stdout,
                sequence: 0,
                body: "retention output".to_owned(),
            },
        )
        .unwrap();
        <S as FactsRepository>::insert_facts_snapshot(
            store,
            "a1",
            "{\"retention\":\"facts\"}",
            SystemTime::UNIX_EPOCH + Duration::from_secs(41),
        )
        .unwrap();
        <S as MetricsRepository>::insert_metrics_snapshot(
            store,
            "a1",
            "{\"retention\":\"metrics\"}",
            SystemTime::UNIX_EPOCH + Duration::from_secs(42),
        )
        .unwrap();
        insert_log(
            store,
            "a1",
            "level=info event=retention_contract",
            SystemTime::UNIX_EPOCH + Duration::from_secs(43),
        );
        <S as AuditWriter>::write(
            store,
            AuditEvent {
                category: AuditCategory::Security,
                action: "retention_contract".to_owned(),
                actor: AuditActor::new("test"),
                target: AuditTarget::new("retention-contract"),
                value: AuditValue::Plain("kept".to_owned()),
                occurred_at: SystemTime::UNIX_EPOCH + Duration::from_secs(44),
            },
        )
        .unwrap();

        let future_cutoffs = RetentionCutoffs {
            job_output: SystemTime::UNIX_EPOCH + Duration::from_secs(4_102_444_800),
            facts: SystemTime::UNIX_EPOCH + Duration::from_secs(4_102_444_800),
            metrics: SystemTime::UNIX_EPOCH + Duration::from_secs(4_102_444_800),
            agent_logs: SystemTime::UNIX_EPOCH + Duration::from_secs(4_102_444_800),
        };
        let dry_run =
            <S as RetentionRepository>::cleanup_retention(store, future_cutoffs, true).unwrap();
        assert_eq!(dry_run.job_output_chunks, 1);
        assert_eq!(dry_run.facts_snapshots, 1);
        assert_eq!(dry_run.metrics_snapshots, 1);
        assert_eq!(dry_run.agent_log_chunks, 1);
        assert_eq!(
            <S as JobOutputRepository>::list_output_chunks(
                store,
                "job-query-artifact-retention-contract",
                "a1",
            )
            .unwrap()
            .len(),
            1
        );
        assert!(
            <S as FactsRepository>::latest_facts_snapshot(store, "a1")
                .unwrap()
                .is_some()
        );
        assert!(
            <S as MetricsRepository>::latest_metrics_snapshot(store, "a1")
                .unwrap()
                .is_some()
        );
        assert_eq!(
            <S as AgentLogRepository>::list_agent_log_chunks(store, "a1", 10, None)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(<S as AuditRepository>::list(store, 10).unwrap().len(), 1);

        let cleanup =
            <S as RetentionRepository>::cleanup_retention(store, future_cutoffs, false).unwrap();
        assert_eq!(cleanup, dry_run);
        assert!(
            <S as JobOutputRepository>::list_output_chunks(
                store,
                "job-query-artifact-retention-contract",
                "a1",
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            <S as FactsRepository>::latest_facts_snapshot(store, "a1")
                .unwrap()
                .is_none()
        );
        assert!(
            <S as MetricsRepository>::latest_metrics_snapshot(store, "a1")
                .unwrap()
                .is_none()
        );
        assert!(
            <S as AgentLogRepository>::list_agent_log_chunks(store, "a1", 10, None)
                .unwrap()
                .is_empty()
        );
        assert_eq!(<S as AuditRepository>::list(store, 10).unwrap().len(), 1);
    }

    fn assert_remediation_request_repository_contracts<S>(store: &mut S)
    where
        S: AgentRepository<Error = StoreError> + RemediationRequestRepository<Error = StoreError>,
    {
        <S as AgentRepository>::save(store, agent()).unwrap();
        <S as AgentRepository>::save(store, agent_with_id("a2", "agent-2", "fedcba9876543210"))
            .unwrap();

        let first = remediation_request_record(
            "remediation-contract-1",
            "a1",
            "policy-remediation-contract",
        );
        let second = remediation_request_record(
            "remediation-contract-2",
            "a1",
            "policy-remediation-contract",
        );
        let mut other =
            remediation_request_record("remediation-other", "a2", "policy-remediation-other");
        other.created_at = SystemTime::UNIX_EPOCH + Duration::from_secs(5);
        other.updated_at = other.created_at;

        <S as RemediationRequestRepository>::save_remediation_request(store, second.clone())
            .unwrap();
        <S as RemediationRequestRepository>::save_remediation_request(store, first.clone())
            .unwrap();
        <S as RemediationRequestRepository>::save_remediation_request(store, other.clone())
            .unwrap();
        assert!(matches!(
            <S as RemediationRequestRepository>::save_remediation_request(store, first.clone()),
            Err(StoreError::ConstraintViolation(_))
        ));

        assert_eq!(
            <S as RemediationRequestRepository>::find_remediation_request(
                store,
                "remediation-contract-1"
            )
            .unwrap(),
            Some(first.clone())
        );
        assert!(
            <S as RemediationRequestRepository>::find_remediation_request(store, "missing")
                .unwrap()
                .is_none()
        );

        let limited = <S as RemediationRequestRepository>::list_remediation_requests(
            store,
            Some("a1"),
            Some("policy-remediation-contract"),
            0,
        )
        .unwrap();
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].id, "remediation-contract-1");

        let filtered = <S as RemediationRequestRepository>::list_remediation_requests(
            store,
            Some("a1"),
            Some("policy-remediation-contract"),
            10,
        )
        .unwrap();
        assert_eq!(
            filtered
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>(),
            vec!["remediation-contract-1", "remediation-contract-2"]
        );

        let other_only = <S as RemediationRequestRepository>::list_remediation_requests(
            store,
            Some("a2"),
            Some("policy-remediation-other"),
            10,
        )
        .unwrap();
        assert_eq!(other_only, vec![other]);

        <S as RemediationRequestRepository>::update_remediation_request_status(
            store,
            "remediation-contract-1",
            "pending_approval",
            None,
            SystemTime::UNIX_EPOCH + Duration::from_secs(20),
        )
        .unwrap();
        let pending = <S as RemediationRequestRepository>::find_remediation_request(
            store,
            "remediation-contract-1",
        )
        .unwrap()
        .unwrap();
        assert_eq!(pending.status, "pending_approval");
        assert_eq!(pending.job_id, None);
        assert_eq!(
            pending.updated_at,
            SystemTime::UNIX_EPOCH + Duration::from_secs(20)
        );
        assert_eq!(pending.runbook_ref, first.runbook_ref);
        assert_eq!(pending.risk_summary, first.risk_summary);

        <S as RemediationRequestRepository>::update_remediation_request_status(
            store,
            "remediation-contract-1",
            "job_created",
            Some("job-remediation-contract"),
            SystemTime::UNIX_EPOCH + Duration::from_secs(30),
        )
        .unwrap();
        let job_created = <S as RemediationRequestRepository>::find_remediation_request(
            store,
            "remediation-contract-1",
        )
        .unwrap()
        .unwrap();
        assert_eq!(job_created.status, "job_created");
        assert_eq!(
            job_created.job_id.as_deref(),
            Some("job-remediation-contract")
        );
        assert_eq!(
            job_created.updated_at,
            SystemTime::UNIX_EPOCH + Duration::from_secs(30)
        );

        assert!(matches!(
            <S as RemediationRequestRepository>::update_remediation_request_status(
                store,
                "missing",
                "failed",
                None,
                SystemTime::UNIX_EPOCH,
            ),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn task_assignment_nonce_is_unique() {
        let store = SqliteStore::in_memory().unwrap();
        let result = store.connection.execute(
            "INSERT INTO task_assignments (
                id, job_id, agent_id, nonce, payload_hash, signature, expires_at
             ) VALUES ('t1', 'missing-job', 'missing-agent', 'nonce-1', 'hash', 'sig', 1)",
            [],
        );
        assert!(result.is_err());

        let unique_index_exists = store
            .connection
            .prepare("SELECT 1 FROM pragma_index_list('task_assignments') WHERE [unique] = 1")
            .unwrap()
            .exists([])
            .unwrap();
        assert!(unique_index_exists);
    }
    #[test]
    fn sqlite_repo_stores_and_finds_agent() {
        let mut store = SqliteStore::in_memory().unwrap();
        AgentRepository::save(&mut store, agent()).unwrap();

        let found = store
            .find_by_id(&AgentId::new("a1").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(found.name().as_str(), "web-01");
        assert_eq!(found.labels()[0].key(), "role");
    }

    #[test]
    fn sqlite_repo_returns_none_for_missing_records() {
        let store = SqliteStore::in_memory().unwrap();

        assert!(store.find_agent_by_id("missing-agent").unwrap().is_none());
        assert!(
            store
                .latest_facts_snapshot("missing-agent")
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .find_job_status_value("missing-job")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn sqlite_repo_rejects_duplicate_agent() {
        let mut store = SqliteStore::in_memory().unwrap();
        AgentRepository::save(&mut store, agent()).unwrap();
        assert_eq!(
            AgentRepository::save(&mut store, agent()),
            Err(StoreError::DuplicateAgent)
        );
    }

    #[test]
    fn updates_agent_labels() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let labels = vec![
            AgentLabel::new("role", "api").unwrap(),
            AgentLabel::new("env", "prod").unwrap(),
        ];

        assert!(store.update_agent_labels("a1", &labels).unwrap());
        let agent = store.find_agent_by_id("a1").unwrap().unwrap();

        assert_eq!(agent.labels()[0].value(), "api");
        assert_eq!(agent.labels()[1].key(), "env");
    }

    #[test]
    fn revoked_agent_key_disables_agent_identity() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        assert!(store.find_agent_identity("a1").unwrap().is_some());

        assert!(store.revoke_agent_key("a1").unwrap());
        assert!(store.find_agent_identity("a1").unwrap().is_none());
        assert!(
            !store
                .mark_agent_online("a1", SystemTime::UNIX_EPOCH + Duration::from_secs(5))
                .unwrap()
        );
        let agent = store.find_agent_by_id("a1").unwrap().unwrap();

        assert_eq!(agent.status(), AgentStatus::Disabled);
    }

    #[test]
    fn stores_latest_facts_snapshot() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .insert_facts_snapshot(
                "a1",
                "{\"os\":\"linux\"}",
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();
        store
            .insert_facts_snapshot(
                "a1",
                "{\"os\":\"macos\"}",
                SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            )
            .unwrap();

        let snapshot = store.latest_facts_snapshot("a1").unwrap().unwrap();

        assert_eq!(snapshot.agent_id, "a1");
        assert_eq!(snapshot.body, "{\"os\":\"macos\"}");
    }

    #[test]
    fn stores_agent_log_chunks() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .insert_agent_log_chunk(
                "a1",
                "level=info event=agent_heartbeat_completed",
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();
        store
            .insert_agent_log_chunk(
                "a1",
                "level=info event=agent_heartbeat_completed sequence=2",
                SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            )
            .unwrap();

        let chunks = store.list_agent_log_chunks("a1", 10).unwrap();

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].agent_id, "a1");
        assert!(chunks[0].line.contains("sequence=2"));
        assert_eq!(
            chunks[0].collected_at,
            SystemTime::UNIX_EPOCH + Duration::from_secs(2)
        );
    }

    #[test]
    fn pages_agent_log_chunks_before_cursor() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        for (seconds, line) in [
            (1, "level=info event=agent_log_uploaded sequence=1"),
            (2, "level=info event=agent_log_uploaded sequence=2"),
            (3, "level=info event=agent_log_uploaded sequence=3"),
        ] {
            store
                .insert_agent_log_chunk(
                    "a1",
                    line,
                    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds),
                )
                .unwrap();
        }

        let first_page = store.list_agent_log_chunks_page("a1", 2, None).unwrap();
        assert_eq!(
            first_page
                .iter()
                .map(|record| record.line.as_str())
                .collect::<Vec<_>>(),
            vec![
                "level=info event=agent_log_uploaded sequence=3",
                "level=info event=agent_log_uploaded sequence=2"
            ]
        );

        let second_page = store
            .list_agent_log_chunks_page("a1", 2, Some(first_page[1].cursor))
            .unwrap();
        assert_eq!(second_page.len(), 1);
        assert!(second_page[0].line.contains("sequence=1"));
    }

    #[test]
    fn pages_facts_snapshots_with_stable_cursor() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        for body in ["{\"seq\":1}", "{\"seq\":2}", "{\"seq\":3}"] {
            store
                .insert_facts_snapshot("a1", body, SystemTime::UNIX_EPOCH + Duration::from_secs(1))
                .unwrap();
        }

        let first_page = store.list_facts_snapshots("a1", 2, None).unwrap();
        assert_eq!(
            first_page
                .iter()
                .map(|record| record.body.as_str())
                .collect::<Vec<_>>(),
            vec!["{\"seq\":3}", "{\"seq\":2}"]
        );

        let second_page = store
            .list_facts_snapshots("a1", 2, Some(first_page[1].cursor))
            .unwrap();
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].body, "{\"seq\":1}");
    }

    #[test]
    fn stores_latest_metrics_snapshot() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .insert_metrics_snapshot(
                "a1",
                "{\"cpu\":{\"logical_count\":2}}",
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();
        store
            .insert_metrics_snapshot(
                "a1",
                "{\"cpu\":{\"logical_count\":4}}",
                SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            )
            .unwrap();

        let snapshot = store.latest_metrics_snapshot("a1").unwrap().unwrap();

        assert_eq!(snapshot.agent_id, "a1");
        assert_eq!(snapshot.body, "{\"cpu\":{\"logical_count\":4}}");
    }

    #[test]
    fn pages_metrics_snapshots_before_cursor() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        for (seconds, body) in [
            (1, "{\"cpu\":{\"logical_count\":1}}"),
            (2, "{\"cpu\":{\"logical_count\":2}}"),
            (3, "{\"cpu\":{\"logical_count\":3}}"),
        ] {
            store
                .insert_metrics_snapshot(
                    "a1",
                    body,
                    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds),
                )
                .unwrap();
        }

        let first_page = store.list_metrics_snapshots("a1", 2, None).unwrap();
        assert_eq!(
            first_page
                .iter()
                .map(|record| record.body.as_str())
                .collect::<Vec<_>>(),
            vec![
                "{\"cpu\":{\"logical_count\":3}}",
                "{\"cpu\":{\"logical_count\":2}}"
            ]
        );

        let second_page = store
            .list_metrics_snapshots("a1", 2, Some(first_page[1].cursor))
            .unwrap();
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].body, "{\"cpu\":{\"logical_count\":1}}");
    }

    #[test]
    fn stores_latest_drift_report() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .insert_drift_report(
                "a1",
                &DriftReport {
                    policy_name: "nginx-running".to_owned(),
                    status: DriftStatus::Unknown,
                    severity: DriftSeverity::Unknown,
                    acknowledgement: DriftAcknowledgement::Open,
                    expected: "service nginx running".to_owned(),
                    actual: "unknown".to_owned(),
                },
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();
        store
            .insert_drift_report(
                "a1",
                &DriftReport {
                    policy_name: "nginx-running".to_owned(),
                    status: DriftStatus::Drifted,
                    severity: DriftSeverity::Warning,
                    acknowledgement: DriftAcknowledgement::Open,
                    expected: "service nginx running".to_owned(),
                    actual: "stopped".to_owned(),
                },
                SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            )
            .unwrap();

        let record = store.latest_drift_report("a1").unwrap().unwrap();

        assert_eq!(record.agent_id, "a1");
        assert_eq!(record.report.status, DriftStatus::Drifted);
        assert_eq!(record.report.actual, "stopped");
    }

    #[test]
    fn pages_drift_reports_before_cursor() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        for (seconds, status, actual) in [
            (1, DriftStatus::Unknown, "unknown"),
            (2, DriftStatus::Compliant, "running"),
            (3, DriftStatus::Drifted, "stopped"),
        ] {
            store
                .insert_drift_report(
                    "a1",
                    &DriftReport {
                        policy_name: "nginx-running".to_owned(),
                        severity: DriftSeverity::for_status(status.clone()),
                        acknowledgement: DriftAcknowledgement::Open,
                        status,
                        expected: "service nginx running".to_owned(),
                        actual: actual.to_owned(),
                    },
                    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds),
                )
                .unwrap();
        }

        let first_page = store.list_drift_reports("a1", 2, None).unwrap();
        assert_eq!(
            first_page
                .iter()
                .map(|record| record.report.actual.as_str())
                .collect::<Vec<_>>(),
            vec!["stopped", "running"]
        );

        let second_page = store
            .list_drift_reports("a1", 2, Some(first_page[1].cursor))
            .unwrap();
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].report.actual, "unknown");
    }

    #[test]
    fn stores_policy_assignment_schedule_and_drift_acknowledgement_state() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .save_policy_source("policy-1", "nginx-running", 1, "kind: Policy")
            .unwrap();

        assert_eq!(store.list_policies().unwrap().len(), 1);
        assert!(store.find_policy("policy-1").unwrap().is_some());

        store
            .assign_policy_to_agent(
                "policy-1",
                "a1",
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(
            store.assigned_policy_ids_for_agent("a1").unwrap(),
            vec!["policy-1".to_owned()]
        );

        store
            .upsert_policy_schedule(
                "policy-1",
                "a1",
                Duration::from_secs(300),
                SystemTime::UNIX_EPOCH + Duration::from_secs(300),
            )
            .unwrap();
        assert!(
            store
                .due_scheduled_drift_checks(SystemTime::UNIX_EPOCH + Duration::from_secs(299), 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .due_scheduled_drift_checks(SystemTime::UNIX_EPOCH + Duration::from_secs(300), 10)
                .unwrap()
                .len(),
            1
        );
        store
            .record_scheduled_drift_check(
                "policy-1",
                "a1",
                SystemTime::UNIX_EPOCH + Duration::from_secs(300),
            )
            .unwrap();
        assert!(
            store
                .due_scheduled_drift_checks(SystemTime::UNIX_EPOCH + Duration::from_secs(599), 10)
                .unwrap()
                .is_empty()
        );

        store
            .insert_drift_report(
                "a1",
                &DriftReport {
                    policy_name: "nginx-running".to_owned(),
                    status: DriftStatus::Drifted,
                    severity: DriftSeverity::Warning,
                    acknowledgement: DriftAcknowledgement::Open,
                    expected: "service nginx running".to_owned(),
                    actual: "stopped".to_owned(),
                },
                SystemTime::UNIX_EPOCH + Duration::from_secs(301),
            )
            .unwrap();

        assert!(
            store
                .acknowledge_latest_drift_report(
                    "a1",
                    "nginx-running",
                    "admin",
                    SystemTime::UNIX_EPOCH + Duration::from_secs(302),
                )
                .unwrap()
        );
        let acknowledged = store.latest_drift_report("a1").unwrap().unwrap();
        assert!(matches!(
            acknowledged.report.acknowledgement,
            DriftAcknowledgement::Acknowledged { ref by, .. } if by == "admin"
        ));

        assert!(
            store
                .mark_latest_drift_resolved(
                    "a1",
                    "nginx-running",
                    "job-remediate",
                    SystemTime::UNIX_EPOCH + Duration::from_secs(303),
                )
                .unwrap()
        );
        let resolved = store.latest_drift_report("a1").unwrap().unwrap();
        assert!(matches!(
            resolved.report.acknowledgement,
            DriftAcknowledgement::Resolved { ref job_id, .. } if job_id == "job-remediate"
        ));
    }

    #[test]
    fn retention_cleanup_dry_run_does_not_delete() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        seed_retention_rows(&store);

        let summary = store
            .cleanup_retention(SystemTime::UNIX_EPOCH + Duration::from_secs(100), true)
            .unwrap();

        assert_eq!(
            summary,
            RetentionCleanupSummary {
                job_output_chunks: 1,
                facts_snapshots: 1,
                metrics_snapshots: 1,
                agent_log_chunks: 1,
            }
        );
        assert_eq!(row_count(&store, "job_output_chunks"), 2);
        assert_eq!(row_count(&store, "facts_snapshots"), 2);
        assert_eq!(row_count(&store, "metrics_snapshots"), 2);
        assert_eq!(row_count(&store, "agent_log_chunks"), 2);
    }

    #[test]
    fn retention_cleanup_deletes_only_old_rows() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        seed_retention_rows(&store);

        let summary = store
            .cleanup_retention(SystemTime::UNIX_EPOCH + Duration::from_secs(100), false)
            .unwrap();

        assert_eq!(summary.total(), 4);
        assert_eq!(row_count(&store, "job_output_chunks"), 1);
        assert_eq!(row_count(&store, "facts_snapshots"), 1);
        assert_eq!(row_count(&store, "metrics_snapshots"), 1);
        assert_eq!(row_count(&store, "agent_log_chunks"), 1);
    }

    #[test]
    fn retention_cleanup_applies_artifact_specific_cutoffs() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        seed_retention_rows(&store);

        let summary = store
            .cleanup_retention_with_cutoffs(
                RetentionCutoffs {
                    job_output: SystemTime::UNIX_EPOCH + Duration::from_secs(100),
                    facts: SystemTime::UNIX_EPOCH,
                    metrics: SystemTime::UNIX_EPOCH + Duration::from_secs(100),
                    agent_logs: SystemTime::UNIX_EPOCH,
                },
                false,
            )
            .unwrap();

        assert_eq!(
            summary,
            RetentionCleanupSummary {
                job_output_chunks: 1,
                facts_snapshots: 0,
                metrics_snapshots: 1,
                agent_log_chunks: 0,
            }
        );
        assert_eq!(row_count(&store, "job_output_chunks"), 1);
        assert_eq!(row_count(&store, "facts_snapshots"), 2);
        assert_eq!(row_count(&store, "metrics_snapshots"), 1);
        assert_eq!(row_count(&store, "agent_log_chunks"), 2);
    }

    #[test]
    fn retention_cleanup_does_not_delete_audit_events() {
        let mut store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        seed_retention_rows(&store);
        store
            .write(AuditEvent::security("retention_guard", "store"))
            .unwrap();

        store
            .cleanup_retention(SystemTime::UNIX_EPOCH + Duration::from_secs(100), false)
            .unwrap();

        assert_eq!(AuditRepository::list(&store, 10).unwrap().len(), 1);
        assert_eq!(row_count(&store, "audit_events"), 1);
    }

    #[test]
    fn stores_job_and_task_assignment() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-1").unwrap(),
            fleet_domain::TaskRisk::High,
            fleet_domain::ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        store.save_job_record(&job).unwrap();

        store
            .save_task_assignment_record(&task_envelope("nonce-1", "task-1"))
            .unwrap();

        let assignment_exists = store
            .connection
            .prepare("SELECT 1 FROM task_assignments WHERE id = 'task-1'")
            .unwrap()
            .exists([])
            .unwrap();
        assert!(assignment_exists);
        assert_eq!(
            store.find_task_assignment_status("task-1").unwrap(),
            Some("queued".to_owned())
        );
    }

    #[test]
    fn task_assignment_status_transitions_are_persisted() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let mut job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-1").unwrap(),
            fleet_domain::TaskRisk::High,
            fleet_domain::ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        job.queue(true).unwrap();
        let command = CommandTask::new("uptime", Vec::new(), Duration::from_secs(30)).unwrap();
        store.save_command_job_record(&job, &command).unwrap();
        store
            .save_task_assignment_record(&task_envelope("nonce-1", "task-1"))
            .unwrap();

        store
            .update_task_assignment_status(
                "task-1",
                AssignmentStatus::Dispatched,
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
                None,
            )
            .unwrap();

        assert_eq!(
            store.find_task_assignment_status("task-1").unwrap(),
            Some("dispatched".to_owned())
        );
        assert!(
            store
                .list_pending_dispatch_assignments(
                    Some(&AgentId::new("a1").unwrap()),
                    Some(&fleet_domain::JobId::new("job-1").unwrap()),
                    10,
                )
                .unwrap()
                .is_empty()
        );

        store
            .update_task_assignment_status(
                "task-1",
                AssignmentStatus::Accepted,
                SystemTime::UNIX_EPOCH + Duration::from_secs(2),
                None,
            )
            .unwrap();
        store
            .update_task_assignment_status(
                "task-1",
                AssignmentStatus::Started,
                SystemTime::UNIX_EPOCH + Duration::from_secs(3),
                None,
            )
            .unwrap();
        store
            .update_task_assignment_status(
                "task-1",
                AssignmentStatus::Failed,
                SystemTime::UNIX_EPOCH + Duration::from_secs(4),
                Some("exit_code=1"),
            )
            .unwrap();

        assert_eq!(
            store.find_task_assignment_status("task-1").unwrap(),
            Some("failed".to_owned())
        );
    }

    #[test]
    fn active_assignment_update_does_not_override_terminal_status() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let mut job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-1").unwrap(),
            fleet_domain::TaskRisk::High,
            fleet_domain::ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        job.queue(true).unwrap();
        let command = CommandTask::new("uptime", Vec::new(), Duration::from_secs(30)).unwrap();
        store.save_command_job_record(&job, &command).unwrap();
        store
            .save_task_assignment_record(&task_envelope("nonce-1", "task-1"))
            .unwrap();

        store
            .update_task_assignment_status(
                "task-1",
                AssignmentStatus::Canceled,
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
                Some("operator requested cancel"),
            )
            .unwrap();
        let changed = store
            .update_active_task_assignment_status(
                "task-1",
                AssignmentStatus::Succeeded,
                SystemTime::UNIX_EPOCH + Duration::from_secs(2),
                Some("exit_code=0"),
            )
            .unwrap();
        let state = store
            .find_task_assignment_state_for_job("job-1")
            .unwrap()
            .unwrap();

        assert!(!changed);
        assert_eq!(state.task_id, "task-1");
        assert_eq!(state.agent_id, "a1");
        assert_eq!(state.status, "canceled");
    }

    #[test]
    fn pending_command_assignments_include_command_payload() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let mut job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-1").unwrap(),
            fleet_domain::TaskRisk::High,
            fleet_domain::ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        job.queue(true).unwrap();
        let command =
            CommandTask::new("echo", vec!["hello".to_owned()], Duration::from_secs(30)).unwrap();
        store.save_command_job_record(&job, &command).unwrap();
        store
            .save_task_assignment_record(&task_envelope("nonce-1", "task-1"))
            .unwrap();

        let assignments = store
            .list_pending_command_assignments_for_agent("a1")
            .unwrap();

        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].command.program(), "echo");
        assert_eq!(assignments[0].command.args(), ["hello"]);
        assert_eq!(assignments[0].envelope.task_id.as_str(), "task-1");
    }

    #[test]
    fn pending_runbook_assignments_include_runbook_payload() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let mut job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-1").unwrap(),
            fleet_domain::TaskRisk::High,
            fleet_domain::ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        job.queue(true).unwrap();
        let runbook = RunbookExecutionTask::new(
            "apiVersion: fleet.sponzey.dev/v1alpha1\nkind: Runbook",
            Duration::from_secs(30),
        )
        .unwrap();
        store.save_runbook_job_record(&job, &runbook).unwrap();
        store
            .save_task_assignment_record(&task_envelope("nonce-runbook", "task-runbook"))
            .unwrap();

        let assignments = store
            .list_pending_runbook_assignments_for_agent("a1")
            .unwrap();

        assert_eq!(assignments.len(), 1);
        assert!(
            assignments[0]
                .runbook
                .runbook_document()
                .contains("kind: Runbook")
        );
        assert_eq!(assignments[0].envelope.task_id.as_str(), "task-runbook");
    }

    #[test]
    fn pending_dispatch_assignments_are_fifo_across_task_types() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let mut command_job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-command").unwrap(),
            fleet_domain::TaskRisk::High,
            fleet_domain::ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        command_job.queue(true).unwrap();
        let command =
            CommandTask::new("echo", vec!["hello".to_owned()], Duration::from_secs(30)).unwrap();
        store
            .save_command_job_record(&command_job, &command)
            .unwrap();
        store
            .save_task_assignment_record(&task_envelope_for_job(
                "job-command",
                "a1",
                "nonce-command",
                "task-command",
            ))
            .unwrap();
        let mut runbook_job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-runbook").unwrap(),
            fleet_domain::TaskRisk::High,
            fleet_domain::ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        runbook_job.queue(true).unwrap();
        let runbook = RunbookExecutionTask::new("kind: Runbook", Duration::from_secs(30)).unwrap();
        store
            .save_runbook_job_record(&runbook_job, &runbook)
            .unwrap();
        store
            .save_task_assignment_record(&task_envelope_for_job(
                "job-runbook",
                "a1",
                "nonce-runbook-fifo",
                "task-runbook-fifo",
            ))
            .unwrap();
        store
            .connection
            .execute(
                "UPDATE task_assignments
                 SET created_at = CASE id
                   WHEN 'task-runbook-fifo' THEN 1
                   WHEN 'task-command' THEN 2
                   ELSE created_at
                 END",
                [],
            )
            .unwrap();

        let assignments = store
            .list_pending_dispatch_assignments(Some(&AgentId::new("a1").unwrap()), None, 10)
            .unwrap();

        assert_eq!(assignments.len(), 2);
        assert_eq!(
            assignments[0].envelope.task_id.as_str(),
            "task-runbook-fifo"
        );
        assert!(matches!(assignments[0].task, TaskKind::RunbookExecution(_)));
        assert_eq!(assignments[1].envelope.task_id.as_str(), "task-command");
        assert!(matches!(assignments[1].task, TaskKind::Command(_)));
    }

    #[test]
    fn pending_dispatch_assignments_filter_by_agent_and_job() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .save_agent(agent_with_id("a2", "web-02", "1123456789abcdef"))
            .unwrap();
        for (job_id, agent_id, nonce, task_id) in [
            ("job-1", "a1", "nonce-1", "task-1"),
            ("job-2", "a2", "nonce-2", "task-2"),
        ] {
            let mut job = fleet_domain::Job::new(
                fleet_domain::JobId::new(job_id).unwrap(),
                fleet_domain::TaskRisk::High,
                fleet_domain::ApprovalRequirement::AdminConfirmation,
                Duration::from_secs(30),
            );
            job.queue(true).unwrap();
            let command = CommandTask::new("uptime", Vec::new(), Duration::from_secs(30)).unwrap();
            store.save_command_job_record(&job, &command).unwrap();
            store
                .save_task_assignment_record(&task_envelope_for_job(
                    job_id, agent_id, nonce, task_id,
                ))
                .unwrap();
        }

        let assignments = store
            .list_pending_dispatch_assignments(
                Some(&AgentId::new("a2").unwrap()),
                Some(&fleet_domain::JobId::new("job-2").unwrap()),
                10,
            )
            .unwrap();
        let mismatched = store
            .list_pending_dispatch_assignments(
                Some(&AgentId::new("a2").unwrap()),
                Some(&fleet_domain::JobId::new("job-1").unwrap()),
                10,
            )
            .unwrap();

        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].envelope.task_id.as_str(), "task-2");
        assert!(mismatched.is_empty());
    }

    #[test]
    fn lists_recent_job_summaries() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let mut job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-1").unwrap(),
            fleet_domain::TaskRisk::High,
            fleet_domain::ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        job.queue(true).unwrap();
        let command =
            CommandTask::new("uptime", vec!["-a".to_owned()], Duration::from_secs(30)).unwrap();
        store.save_command_job_record(&job, &command).unwrap();
        store
            .save_task_assignment_record(&task_envelope("nonce-1", "task-1"))
            .unwrap();

        let summaries = store.list_job_summaries(10).unwrap();

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, "job-1");
        assert_eq!(summaries[0].status, "queued");
        assert_eq!(summaries[0].command_program.as_deref(), Some("uptime"));
        assert_eq!(summaries[0].command_args, vec!["-a"]);
        assert_eq!(summaries[0].target_count, 1);
        assert_eq!(summaries[0].selector_kind, "explicit_ids");
        assert_eq!(summaries[0].strategy_concurrency, 1);
        assert_eq!(summaries[0].strategy_max_failures, None);
        assert_eq!(summaries[0].target_agents[0].agent_id, "a1");
        assert_eq!(summaries[0].target_agents[0].agent_name, "web-01");
        assert_eq!(summaries[0].target_agents[0].status, "pending");
        assert_eq!(
            summaries[0].target_agents[0].task_id.as_deref(),
            Some("task-1")
        );
        assert_eq!(
            summaries[0].target_agents[0].assignment_status.as_deref(),
            Some("queued")
        );
        assert_eq!(
            summaries[0].target_agents[0].labels,
            vec![("role".to_owned(), "web".to_owned())]
        );
    }

    #[test]
    fn job_target_snapshot_survives_later_agent_label_and_status_changes() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let mut job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-1").unwrap(),
            fleet_domain::TaskRisk::High,
            fleet_domain::ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        job.queue(true).unwrap();
        let command = CommandTask::new("uptime", Vec::new(), Duration::from_secs(30)).unwrap();
        store.save_command_job_record(&job, &command).unwrap();
        store
            .update_job_selector_snapshot("job-1", "selector", "label:role=web")
            .unwrap();
        store.update_job_strategy("job-1", 2, Some(1)).unwrap();
        store
            .save_task_assignment_record(&task_envelope("nonce-1", "task-1"))
            .unwrap();

        store
            .update_agent_labels("a1", &[AgentLabel::new("role", "db").unwrap()])
            .unwrap();
        store.revoke_agent_key("a1").unwrap();

        let summary = store.find_job_summary("job-1").unwrap().unwrap();

        assert_eq!(summary.selector_kind, "selector");
        assert_eq!(summary.selector_source, "label:role=web");
        assert_eq!(summary.strategy_concurrency, 2);
        assert_eq!(summary.strategy_max_failures, Some(1));
        assert_eq!(summary.target_agents[0].status, "pending");
        assert_eq!(
            summary.target_agents[0].labels,
            vec![("role".to_owned(), "web".to_owned())]
        );
    }

    #[test]
    fn job_output_chunks_are_stored_in_order() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .save_job_record(&fleet_domain::Job::new(
                fleet_domain::JobId::new("job-1").unwrap(),
                fleet_domain::TaskRisk::Low,
                fleet_domain::ApprovalRequirement::NotRequired,
                Duration::from_secs(30),
            ))
            .unwrap();
        store
            .append_job_output_chunk_record(&JobOutputChunk {
                job_id: "job-1".to_owned(),
                agent_id: "a1".to_owned(),
                stream: JobOutputStream::Stdout,
                sequence: 1,
                body: "second".to_owned(),
            })
            .unwrap();
        store
            .append_job_output_chunk_record(&JobOutputChunk {
                job_id: "job-1".to_owned(),
                agent_id: "a1".to_owned(),
                stream: JobOutputStream::Stdout,
                sequence: 0,
                body: "first".to_owned(),
            })
            .unwrap();

        let chunks = store.list_job_output_chunks("job-1", "a1").unwrap();

        assert_eq!(chunks[0].body, "first");
        assert_eq!(chunks[1].body, "second");
    }

    #[test]
    fn duplicate_job_output_chunks_with_same_body_are_idempotent() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .save_job_record(&fleet_domain::Job::new(
                fleet_domain::JobId::new("job-1").unwrap(),
                fleet_domain::TaskRisk::Low,
                fleet_domain::ApprovalRequirement::NotRequired,
                Duration::from_secs(30),
            ))
            .unwrap();
        let chunk = JobOutputChunk {
            job_id: "job-1".to_owned(),
            agent_id: "a1".to_owned(),
            stream: JobOutputStream::Stdout,
            sequence: 0,
            body: "first".to_owned(),
        };
        store.append_job_output_chunk_record(&chunk).unwrap();

        store.append_job_output_chunk_record(&chunk).unwrap();

        let chunks = store.list_job_output_chunks("job-1", "a1").unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].body, "first");
    }

    #[test]
    fn duplicate_job_output_chunks_with_different_body_are_constraint_violation() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .save_job_record(&fleet_domain::Job::new(
                fleet_domain::JobId::new("job-1").unwrap(),
                fleet_domain::TaskRisk::Low,
                fleet_domain::ApprovalRequirement::NotRequired,
                Duration::from_secs(30),
            ))
            .unwrap();
        let chunk = JobOutputChunk {
            job_id: "job-1".to_owned(),
            agent_id: "a1".to_owned(),
            stream: JobOutputStream::Stdout,
            sequence: 0,
            body: "first".to_owned(),
        };
        store.append_job_output_chunk_record(&chunk).unwrap();
        let conflicting = JobOutputChunk {
            body: "changed".to_owned(),
            ..chunk
        };

        assert!(matches!(
            store.append_job_output_chunk_record(&conflicting),
            Err(StoreError::ConstraintViolation(_))
        ));
    }

    #[test]
    fn rendered_artifact_metadata_is_stored_without_rendered_body() {
        let mut store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .save_job_record(&fleet_domain::Job::new(
                fleet_domain::JobId::new("job-1").unwrap(),
                fleet_domain::TaskRisk::Low,
                fleet_domain::ApprovalRequirement::NotRequired,
                Duration::from_secs(30),
            ))
            .unwrap();
        let metadata = rendered_artifact_metadata("artifact-1", "job-1", "a1", "task-1");

        <SqliteStore as fleet_application::ArtifactMetadataRepository>::save_rendered_artifact_metadata(
            &mut store,
            metadata.clone(),
        )
        .unwrap();

        let records =
            <SqliteStore as fleet_application::ArtifactMetadataRepository>::list_rendered_artifacts_for_job(
                &store,
                &fleet_domain::JobId::new("job-1").unwrap(),
            )
            .unwrap();
        assert_eq!(records, vec![metadata]);
        assert!(
            store
                .has_column("rendered_artifacts", "checksum_sha256")
                .unwrap()
        );
        assert!(
            !store
                .has_column("rendered_artifacts", "rendered_body")
                .unwrap()
        );
        assert!(
            !store
                .has_column("rendered_artifacts", "template_body")
                .unwrap()
        );
    }

    #[test]
    fn signing_key_rotation_repository_roundtrips_without_private_material() {
        let mut store = SqliteStore::in_memory().unwrap();
        let record = signing_key_rotation_record();

        <SqliteStore as SigningKeyRotationRepository>::save_signing_key_rotation(
            &mut store,
            record.clone(),
        )
        .unwrap();

        let loaded = <SqliteStore as SigningKeyRotationRepository>::load_signing_key_rotation(
            &store,
            "controller-default",
        )
        .unwrap()
        .expect("rotation state should load");

        assert_eq!(loaded.controller_id, record.controller_id);
        assert_eq!(loaded.rotation.state(), record.rotation.state());
        assert_eq!(
            loaded.rotation.old_fingerprint().as_str(),
            "old-signing-fingerprint"
        );
        assert_eq!(
            loaded.rotation.new_fingerprint().unwrap().as_str(),
            "new-signing-fingerprint"
        );
        assert!(!format!("{loaded:?}").contains("PRIVATE KEY"));
        assert!(!format!("{loaded:?}").contains("private_key"));
    }

    #[test]
    fn signing_key_rotation_old_key_verification_window_survives_store_roundtrip() {
        let mut store = SqliteStore::in_memory().unwrap();
        <SqliteStore as SigningKeyRotationRepository>::save_signing_key_rotation(
            &mut store,
            signing_key_rotation_record(),
        )
        .unwrap();

        let loaded = <SqliteStore as SigningKeyRotationRepository>::load_signing_key_rotation(
            &store,
            "controller-default",
        )
        .unwrap()
        .unwrap();

        assert!(loaded.rotation.can_verify_signature_from(
            &SigningKeyFingerprint::new("old-signing-fingerprint").unwrap(),
            UNIX_EPOCH + Duration::from_secs(19),
            UNIX_EPOCH + Duration::from_secs(40),
        ));
        assert!(!loaded.rotation.can_verify_signature_from(
            &SigningKeyFingerprint::new("old-signing-fingerprint").unwrap(),
            UNIX_EPOCH + Duration::from_secs(19),
            UNIX_EPOCH + Duration::from_secs(41),
        ));
    }

    #[test]
    fn staged_rollout_repository_roundtrips_without_material_or_handles() {
        let mut store = SqliteStore::in_memory().unwrap();
        let record = staged_rollout_record();

        <SqliteStore as ControllerSigningStagedRolloutRepository>::save_controller_signing_staged_rollout(
            &mut store,
            record.clone(),
        )
        .unwrap();

        let loaded = <SqliteStore as ControllerSigningStagedRolloutRepository>::load_controller_signing_staged_rollout(
            &store,
            "controller-default",
        )
        .unwrap()
        .expect("staged rollout state should load");
        let snapshot = loaded.rollout.snapshot();
        let debug = format!("{loaded:?}");

        assert_eq!(loaded.controller_id, record.controller_id);
        assert_eq!(loaded.current_fingerprint, "new-signing-fingerprint");
        assert_eq!(
            loaded.previous_fingerprint.as_deref(),
            Some("old-signing-fingerprint")
        );
        assert_eq!(loaded.updated_at, UNIX_EPOCH + Duration::from_secs(13));
        assert_eq!(snapshot, record.rollout.snapshot());
        assert_eq!(
            snapshot.state,
            fleet_domain::ControllerSigningStagedRolloutState::WaitingForAck
        );
        assert_eq!(snapshot.in_flight[0].agent_id, "agent-a");
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
                "staged rollout store record must not expose {forbidden}"
            );
        }
    }

    #[test]
    fn agent_certificate_lifecycle_repository_roundtrips_public_state_only() {
        let mut store = SqliteStore::in_memory().unwrap();
        let record = agent_certificate_lifecycle_record();

        <SqliteStore as AgentCertificateLifecycleRepository>::save_agent_certificate_lifecycle(
            &mut store,
            record.clone(),
        )
        .unwrap();

        let loaded =
            <SqliteStore as AgentCertificateLifecycleRepository>::load_agent_certificate_lifecycle(
                &store,
                &AgentId::new("agent-1").unwrap(),
            )
            .unwrap()
            .expect("agent certificate lifecycle should load");
        let debug = format!("{loaded:?}");

        assert_eq!(loaded.agent_id, record.agent_id);
        assert_eq!(
            loaded.lifecycle.state,
            AgentCertificateLifecycleState::DualCertificateActive
        );
        assert_eq!(
            loaded
                .lifecycle
                .current_certificate
                .as_ref()
                .unwrap()
                .serial()
                .as_str(),
            "serial-1"
        );
        assert_eq!(
            loaded
                .lifecycle
                .next_certificate
                .as_ref()
                .unwrap()
                .fingerprint()
                .as_str(),
            "fedcba9876543210"
        );
        for forbidden in [
            "PRIVATE KEY",
            "BEGIN CERTIFICATE",
            "certificate_body",
            "private_key",
            "/etc/fleet",
            "CA_PATH",
            "websocket_handle",
        ] {
            assert!(
                !debug.contains(forbidden),
                "agent certificate lifecycle record must not expose {forbidden}"
            );
        }
    }

    #[test]
    fn duplicate_rendered_artifact_id_is_rejected() {
        let mut store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .save_job_record(&fleet_domain::Job::new(
                fleet_domain::JobId::new("job-1").unwrap(),
                fleet_domain::TaskRisk::Low,
                fleet_domain::ApprovalRequirement::NotRequired,
                Duration::from_secs(30),
            ))
            .unwrap();
        let metadata = rendered_artifact_metadata("artifact-1", "job-1", "a1", "task-1");

        <SqliteStore as fleet_application::ArtifactMetadataRepository>::save_rendered_artifact_metadata(
            &mut store,
            metadata.clone(),
        )
        .unwrap();

        assert!(matches!(
            <SqliteStore as fleet_application::ArtifactMetadataRepository>::save_rendered_artifact_metadata(
                &mut store,
                metadata,
            ),
            Err(StoreError::ConstraintViolation(_))
        ));
    }

    #[test]
    fn local_artifact_store_writes_reads_and_verifies_checksum() {
        let root = artifact_test_root("local-artifact-store-writes");
        let _ = std::fs::remove_dir_all(&root);
        let mut store = LocalArtifactStore::new(&root).unwrap();
        let bytes = b"rendered config\n".to_vec();
        let checksum = test_artifact_checksum(&bytes);
        let id = ArtifactId::new("artifact-1").unwrap();

        let record = store
            .put(ArtifactStorePut {
                id: id.clone(),
                retention_class: ArtifactRetentionClass::RenderedTemplate,
                expected_checksum: checksum.clone(),
                bytes: bytes.clone(),
            })
            .unwrap();

        assert_eq!(record.id, id);
        assert_eq!(record.checksum, checksum);
        assert_eq!(record.size_bytes, bytes.len() as u64);
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

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn local_artifact_store_reports_corrupt_checksum_without_rewriting() {
        let root = artifact_test_root("local-artifact-store-corrupt");
        let _ = std::fs::remove_dir_all(&root);
        let mut store = LocalArtifactStore::new(&root).unwrap();
        let id = ArtifactId::new("artifact-1").unwrap();
        let bytes = b"good body".to_vec();
        let checksum = test_artifact_checksum(&bytes);
        store
            .put(ArtifactStorePut {
                id: id.clone(),
                retention_class: ArtifactRetentionClass::RenderedTemplate,
                expected_checksum: checksum.clone(),
                bytes,
            })
            .unwrap();

        let path = store
            .object_path(&id, ArtifactRetentionClass::RenderedTemplate)
            .unwrap();
        std::fs::write(path, b"tampered body").unwrap();
        let verification = store
            .verify(&id, ArtifactRetentionClass::RenderedTemplate, &checksum)
            .unwrap();

        assert!(matches!(
            verification,
            ArtifactVerification::Corrupt { expected, actual }
                if expected == checksum && actual != checksum
        ));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn local_artifact_store_rejects_path_traversal_ids() {
        let root = artifact_test_root("local-artifact-store-traversal");
        let _ = std::fs::remove_dir_all(&root);
        let mut store = LocalArtifactStore::new(&root).unwrap();
        let bytes = b"body".to_vec();
        let checksum = test_artifact_checksum(&bytes);

        for raw_id in ["../secret", "/tmp/secret", "nested/secret", r"..\secret"] {
            let id = ArtifactId::new(raw_id).unwrap();
            let error = store
                .put(ArtifactStorePut {
                    id,
                    retention_class: ArtifactRetentionClass::RenderedTemplate,
                    expected_checksum: checksum.clone(),
                    bytes: bytes.clone(),
                })
                .unwrap_err();
            assert!(matches!(error, StoreError::Domain(_)));
        }

        assert!(!root.join("secret.blob").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn local_artifact_store_delete_is_idempotent_and_root_scoped() {
        let root = artifact_test_root("local-artifact-store-delete");
        let _ = std::fs::remove_dir_all(&root);
        let mut store = LocalArtifactStore::new(&root).unwrap();
        let id = ArtifactId::new("artifact-delete").unwrap();
        let bytes = b"delete me".to_vec();
        let checksum = test_artifact_checksum(&bytes);
        store
            .put(ArtifactStorePut {
                id: id.clone(),
                retention_class: ArtifactRetentionClass::RenderedTemplate,
                expected_checksum: checksum,
                bytes,
            })
            .unwrap();

        assert_eq!(
            store
                .delete(&id, ArtifactRetentionClass::RenderedTemplate)
                .unwrap(),
            ArtifactDeleteOutcome::Deleted
        );
        assert_eq!(
            store
                .delete(&id, ArtifactRetentionClass::RenderedTemplate)
                .unwrap(),
            ArtifactDeleteOutcome::Missing
        );
        assert!(root.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn remediation_request_metadata_is_stored_without_payload_bodies() {
        let mut store = SqliteStore::in_memory().unwrap();
        store
            .save_agent(agent_with_id("agent-1", "agent-1", "1111111111111111"))
            .unwrap();
        let request = remediation_request_record("rem-1", "agent-1", "nginx-running");

        <SqliteStore as RemediationRequestRepository>::save_remediation_request(
            &mut store,
            request.clone(),
        )
        .unwrap();

        let found = <SqliteStore as RemediationRequestRepository>::find_remediation_request(
            &store, "rem-1",
        )
        .unwrap()
        .unwrap();
        assert_eq!(found, request);
        assert!(
            !store
                .has_column("remediation_requests", "runbook_body")
                .unwrap()
        );
        assert!(
            !store
                .has_column("remediation_requests", "rendered_body")
                .unwrap()
        );
        assert!(
            !store
                .has_column("remediation_requests", "command_output")
                .unwrap()
        );
        assert!(
            !store
                .has_column("remediation_requests", "secret_value")
                .unwrap()
        );
    }

    #[test]
    fn remediation_verification_job_is_atomic_and_idempotent() {
        let mut store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let request = remediation_request_record("remediation-verify", "a1", "nginx-running");
        <SqliteStore as RemediationRequestRepository>::save_remediation_request(
            &mut store, request,
        )
        .unwrap();
        let task = DriftCheckTask::new(
            "apiVersion: fleet.sponzey.dev/v1alpha1",
            Duration::from_secs(30),
        )
        .unwrap();
        let mut job = Job::new(
            JobId::new("job-remediation-verify").unwrap(),
            task.risk(),
            fleet_domain::ApprovalRequirement::NotRequired,
            Duration::from_secs(30),
        );
        job.queue(false).unwrap();
        let input = AppRemediationVerificationJobPersistenceInput {
            remediation_id: "remediation-verify".to_owned(),
            job,
            task,
            assignment: task_envelope_for_job(
                "job-remediation-verify",
                "a1",
                "nonce-remediation-verify",
                "task-remediation-verify",
            ),
            provenance: DriftJobProvenance::remediation_verification("nginx-running", 1),
            audit: AuditEvent {
                category: AuditCategory::Policy,
                action: "remediation_verification_created".to_owned(),
                actor: AuditActor::new("controller"),
                target: AuditTarget::new("a1"),
                value: AuditValue::Plain(
                    "remediation_id=remediation-verify,policy_id=nginx-running".to_owned(),
                ),
                occurred_at: SystemTime::UNIX_EPOCH,
            },
        };

        let first = store
            .save_remediation_verification_job_record(&input)
            .unwrap();
        let duplicate = store
            .save_remediation_verification_job_record(&input)
            .unwrap();

        assert!(first.created);
        assert!(!duplicate.created);
        assert_eq!(first.job_id, "job-remediation-verify");
        assert_eq!(duplicate.job_id, first.job_id);
        assert_eq!(
            store
                .find_remediation_verification_job_id("remediation-verify")
                .unwrap(),
            Some(first.job_id),
        );
        assert_eq!(row_count(&store, "jobs"), 1);
        assert_eq!(row_count(&store, "task_assignments"), 1);
        assert_eq!(row_count(&store, "audit_events"), 1);
    }

    #[test]
    fn remediation_verification_job_rolls_back_when_assignment_insert_fails() {
        let mut store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let existing_job = Job::new(
            JobId::new("job-existing").unwrap(),
            fleet_domain::TaskRisk::Low,
            fleet_domain::ApprovalRequirement::NotRequired,
            Duration::from_secs(30),
        );
        store.save_job_record(&existing_job).unwrap();
        store
            .save_task_assignment_record(&task_envelope_for_job(
                "job-existing",
                "a1",
                "nonce-existing",
                "task-conflict",
            ))
            .unwrap();
        <SqliteStore as RemediationRequestRepository>::save_remediation_request(
            &mut store,
            remediation_request_record("remediation-rollback", "a1", "nginx-running"),
        )
        .unwrap();
        let task = DriftCheckTask::new(
            "apiVersion: fleet.sponzey.dev/v1alpha1",
            Duration::from_secs(30),
        )
        .unwrap();
        let mut job = Job::new(
            JobId::new("job-remediation-rollback").unwrap(),
            task.risk(),
            fleet_domain::ApprovalRequirement::NotRequired,
            Duration::from_secs(30),
        );
        job.queue(false).unwrap();

        let result = store.save_remediation_verification_job_record(
            &AppRemediationVerificationJobPersistenceInput {
                remediation_id: "remediation-rollback".to_owned(),
                job,
                task,
                assignment: task_envelope_for_job(
                    "job-remediation-rollback",
                    "a1",
                    "nonce-rollback",
                    "task-conflict",
                ),
                provenance: DriftJobProvenance::remediation_verification("nginx-running", 1),
                audit: AuditEvent {
                    category: AuditCategory::Policy,
                    action: "remediation_verification_created".to_owned(),
                    actor: AuditActor::new("controller"),
                    target: AuditTarget::new("a1"),
                    value: AuditValue::Redacted,
                    occurred_at: SystemTime::UNIX_EPOCH,
                },
            },
        );

        assert!(matches!(result, Err(StoreError::ConstraintViolation(_))));
        assert_eq!(row_count(&store, "jobs"), 1);
        assert_eq!(row_count(&store, "task_assignments"), 1);
        assert_eq!(row_count(&store, "audit_events"), 0);
        assert_eq!(
            store
                .find_remediation_verification_job_id("remediation-rollback")
                .unwrap(),
            None
        );
    }

    #[test]
    fn remediation_verification_resolution_requires_persisted_evidence_and_is_atomic() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let origin_report = DriftReport {
            policy_name: "nginx-running".to_owned(),
            status: DriftStatus::Drifted,
            severity: DriftSeverity::Warning,
            acknowledgement: DriftAcknowledgement::Open,
            expected: "running".to_owned(),
            actual: "stopped".to_owned(),
        };
        store
            .insert_drift_report("a1", &origin_report, SystemTime::UNIX_EPOCH)
            .unwrap();
        let origin_drift_report_id = store.latest_drift_report("a1").unwrap().unwrap().id;
        let mut remediation =
            remediation_request_record("remediation-resolution", "a1", "nginx-running");
        remediation.status = "succeeded_pending_verify".to_owned();
        remediation.job_id = Some("job-remediation".to_owned());
        remediation.origin_drift_report_id = Some(origin_drift_report_id);
        remediation.policy_version = Some(1);
        store.save_remediation_request_record(&remediation).unwrap();
        let verification_job = Job::new(
            JobId::new("job-verification-resolution").unwrap(),
            fleet_domain::TaskRisk::Low,
            fleet_domain::ApprovalRequirement::NotRequired,
            Duration::from_secs(30),
        );
        store.save_job_record(&verification_job).unwrap();
        store
            .save_remediation_verification_job(
                "remediation-resolution",
                "job-verification-resolution",
                SystemTime::UNIX_EPOCH,
            )
            .unwrap();
        let evidence_report = DriftReport {
            policy_name: "nginx-running".to_owned(),
            status: DriftStatus::Compliant,
            severity: DriftSeverity::for_status(DriftStatus::Compliant),
            acknowledgement: DriftAcknowledgement::Open,
            expected: "running".to_owned(),
            actual: "running".to_owned(),
        };
        store
            .insert_drift_report_with_provenance(
                "a1",
                &evidence_report,
                &DriftReportProvenance::verified(
                    JobId::new("job-verification-resolution").unwrap(),
                    TaskId::new("task-verification-resolution").unwrap(),
                    "nginx-running",
                    1,
                    DriftCheckPurpose::RemediationVerification,
                ),
                SystemTime::UNIX_EPOCH + Duration::from_secs(20),
            )
            .unwrap();
        let evidence_report_id = store.latest_drift_report("a1").unwrap().unwrap().id;
        let resolved = AppRemediationRequestRecord {
            status: "resolved".to_owned(),
            updated_at: SystemTime::UNIX_EPOCH + Duration::from_secs(20),
            ..remediation.clone()
        };
        let audit = AuditEvent {
            category: AuditCategory::Policy,
            action: "remediation_resolved_by_verification".to_owned(),
            actor: AuditActor::new("controller"),
            target: AuditTarget::new("a1"),
            value: AuditValue::Plain("remediation_id=remediation-resolution".to_owned()),
            occurred_at: SystemTime::UNIX_EPOCH + Duration::from_secs(20),
        };

        let missing_evidence = store.resolve_remediation_verification_evidence_record(
            &resolved,
            &origin_drift_report_id,
            &DriftReportId::new(999).unwrap(),
            "job-verification-resolution",
            "task-verification-resolution",
            &audit,
        );
        assert!(matches!(missing_evidence, Err(StoreError::NotFound)));
        assert_eq!(
            store
                .find_remediation_request_record("remediation-resolution")
                .unwrap()
                .unwrap()
                .status,
            "succeeded_pending_verify"
        );
        assert_eq!(row_count(&store, "audit_events"), 0);

        store
            .resolve_remediation_verification_evidence_record(
                &resolved,
                &origin_drift_report_id,
                &evidence_report_id,
                "job-verification-resolution",
                "task-verification-resolution",
                &audit,
            )
            .unwrap();
        assert_eq!(
            store
                .find_remediation_request_record("remediation-resolution")
                .unwrap()
                .unwrap()
                .status,
            "resolved"
        );
        assert_eq!(row_count(&store, "audit_events"), 1);
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT resolution_job_id FROM drift_reports WHERE id = ?1",
                    params![origin_drift_report_id.as_i64()],
                    |row| row.get::<_, Option<String>>(0),
                )
                .unwrap(),
            Some("job-verification-resolution".to_owned())
        );
    }

    #[test]
    fn verification_recovery_list_is_bounded_and_omits_existing_correlations() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        for remediation_id in ["remediation-1", "remediation-2", "remediation-3"] {
            let mut request = remediation_request_record(remediation_id, "a1", "nginx-running");
            request.status = "succeeded_pending_verify".to_owned();
            store.save_remediation_request_record(&request).unwrap();
        }
        let job = Job::new(
            JobId::new("job-existing-verification").unwrap(),
            fleet_domain::TaskRisk::Low,
            fleet_domain::ApprovalRequirement::NotRequired,
            Duration::from_secs(30),
        );
        store.save_job_record(&job).unwrap();
        store
            .connection
            .execute(
                "INSERT INTO remediation_verification_jobs (remediation_id, job_id, created_at)
                 VALUES (?1, ?2, ?3)",
                params!["remediation-1", "job-existing-verification", 0_i64],
            )
            .unwrap();

        let records = store
            .list_pending_remediation_verification_recovery_records(1)
            .unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "remediation-2");
    }

    #[test]
    fn remediation_requests_list_in_deterministic_order_and_update_status() {
        let mut store = SqliteStore::in_memory().unwrap();
        store
            .save_agent(agent_with_id("agent-1", "agent-1", "1111111111111111"))
            .unwrap();
        store
            .save_agent(agent_with_id("agent-2", "agent-2", "2222222222222222"))
            .unwrap();
        <SqliteStore as RemediationRequestRepository>::save_remediation_request(
            &mut store,
            remediation_request_record("rem-2", "agent-1", "nginx-running"),
        )
        .unwrap();
        <SqliteStore as RemediationRequestRepository>::save_remediation_request(
            &mut store,
            remediation_request_record("rem-1", "agent-1", "nginx-running"),
        )
        .unwrap();
        <SqliteStore as RemediationRequestRepository>::save_remediation_request(
            &mut store,
            remediation_request_record("rem-other", "agent-2", "ssh-running"),
        )
        .unwrap();

        let records = <SqliteStore as RemediationRequestRepository>::list_remediation_requests(
            &store,
            Some("agent-1"),
            Some("nginx-running"),
            10,
        )
        .unwrap();
        assert_eq!(
            records
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>(),
            vec!["rem-1", "rem-2"]
        );

        <SqliteStore as RemediationRequestRepository>::update_remediation_request_status(
            &mut store,
            "rem-1",
            "job_created",
            Some("job-1"),
            SystemTime::UNIX_EPOCH + Duration::from_secs(10),
        )
        .unwrap();
        let updated = <SqliteStore as RemediationRequestRepository>::find_remediation_request(
            &store, "rem-1",
        )
        .unwrap()
        .unwrap();
        assert_eq!(updated.status, "job_created");
        assert_eq!(updated.job_id.as_deref(), Some("job-1"));

        assert!(matches!(
            <SqliteStore as RemediationRequestRepository>::update_remediation_request_status(
                &mut store,
                "missing",
                "failed",
                None,
                SystemTime::UNIX_EPOCH,
            ),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn remediation_execution_transition_rolls_back_assignment_and_audit_when_request_is_missing() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .save_agent(agent_with_id("agent-1", "agent-1", "1111111111111111"))
            .unwrap();
        let job = Job::new(
            JobId::new("job-execution").unwrap(),
            fleet_domain::TaskRisk::Low,
            fleet_domain::ApprovalRequirement::NotRequired,
            Duration::from_secs(60),
        );
        store.save_job_record(&job).unwrap();
        store
            .save_task_assignment_record(&task_envelope_for_job(
                "job-execution",
                "agent-1",
                "nonce-execution",
                "task-execution",
            ))
            .unwrap();
        let mut missing = remediation_request_record("missing-remediation", "agent-1", "policy-1");
        missing.status = "running".to_owned();
        missing.job_id = Some("job-execution".to_owned());
        let input = AppRemediationExecutionPersistenceInput {
            task_id: "task-execution".to_owned(),
            assignment_status: "started".to_owned(),
            assignment_last_error: None,
            occurred_at: SystemTime::UNIX_EPOCH,
            remediation: Some(missing),
            remediation_audit: Some(AuditEvent {
                category: AuditCategory::Policy,
                action: "remediation_job_running".to_owned(),
                actor: AuditActor::new("agent-1".to_owned()),
                target: AuditTarget::new("agent-1".to_owned()),
                value: AuditValue::Plain("remediation_id=missing-remediation".to_owned()),
                occurred_at: SystemTime::UNIX_EPOCH,
            }),
        };

        assert!(
            store
                .persist_remediation_execution_transition_record(&input)
                .is_err()
        );
        assert_eq!(
            store.find_task_assignment_status("task-execution").unwrap(),
            Some("queued".to_owned())
        );
        assert_eq!(row_count(&store, "audit_events"), 0);
    }

    #[test]
    fn remediation_proposal_transaction_is_idempotent_and_allows_terminal_new_episode() {
        let mut store = SqliteStore::in_memory().unwrap();
        store
            .save_agent(agent_with_id("agent-1", "agent-1", "1111111111111111"))
            .unwrap();
        let mut first = remediation_request_record("rem-1", "agent-1", "nginx-running");
        first.origin_drift_report_id = Some(DriftReportId::new(1).unwrap());
        first.policy_version = Some(1);
        let audit = AuditEvent {
            category: AuditCategory::Policy,
            action: "remediation_requested".to_owned(),
            actor: AuditActor::new("operator-1"),
            target: AuditTarget::new("agent-1"),
            value: AuditValue::Plain("remediation_id=rem-1,policy_id=nginx-running".to_owned()),
            occurred_at: SystemTime::UNIX_EPOCH,
        };

        let created = <SqliteStore as RemediationProposalRepository>::save_remediation_proposal(
            &mut store,
            first.clone(),
            audit.clone(),
        )
        .unwrap();
        let duplicate = <SqliteStore as RemediationProposalRepository>::save_remediation_proposal(
            &mut store,
            first.clone(),
            audit.clone(),
        )
        .unwrap();

        assert!(created.created);
        assert!(!duplicate.created);
        assert_eq!(duplicate.remediation.id, "rem-1");
        assert_eq!(row_count(&store, "remediation_requests"), 1);
        assert_eq!(
            store
                .list_audit_events_by_category(AuditCategory::Policy, 10)
                .unwrap()
                .len(),
            1
        );

        let mut active_conflict = first.clone();
        active_conflict.id = "rem-conflict".to_owned();
        active_conflict.origin_drift_report_id = Some(DriftReportId::new(99).unwrap());
        let active_conflict =
            <SqliteStore as RemediationProposalRepository>::save_remediation_proposal(
                &mut store,
                active_conflict,
                audit.clone(),
            )
            .unwrap();
        assert!(!active_conflict.created);
        assert_eq!(active_conflict.remediation.id, "rem-1");
        assert_eq!(row_count(&store, "remediation_requests"), 1);
        assert_eq!(
            store
                .list_audit_events_by_category(AuditCategory::Policy, 10)
                .unwrap()
                .len(),
            1
        );

        store
            .update_remediation_request_status_record(
                "rem-1",
                "resolved",
                None,
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();
        let mut next = first;
        next.id = "rem-2".to_owned();
        next.origin_drift_report_id = Some(DriftReportId::new(2).unwrap());
        let next = <SqliteStore as RemediationProposalRepository>::save_remediation_proposal(
            &mut store, next, audit,
        )
        .unwrap();
        assert!(next.created);
        assert_eq!(row_count(&store, "remediation_requests"), 2);
    }

    #[test]
    fn remediation_proposal_failure_rolls_back_request_and_audit() {
        let mut store = SqliteStore::in_memory().unwrap();
        let mut request =
            remediation_request_record("rem-missing", "missing-agent", "nginx-running");
        request.origin_drift_report_id = Some(DriftReportId::new(1).unwrap());
        request.policy_version = Some(1);
        let audit = AuditEvent::security("remediation_requested", "missing-agent");

        assert!(
            <SqliteStore as RemediationProposalRepository>::save_remediation_proposal(
                &mut store, request, audit,
            )
            .is_err()
        );
        assert_eq!(row_count(&store, "remediation_requests"), 0);
        assert!(
            store
                .list_audit_events_by_category(AuditCategory::Security, 10)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn verified_drift_proposal_failure_rolls_back_report_remediation_and_audits() {
        let mut store = SqliteStore::in_memory().unwrap();
        let input = AppPersistVerifiedDriftProposalInput {
            agent_id: "missing-agent".to_owned(),
            report: DriftReport::drifted("nginx-running", "running", "stopped"),
            provenance: DriftReportProvenance::verified(
                JobId::new("job-drift").unwrap(),
                TaskId::new("task-drift").unwrap(),
                "nginx-running",
                1,
                DriftCheckPurpose::Evaluation,
            ),
            remediation: remediation_request_record("rem-1", "missing-agent", "nginx-running"),
            drift_audit: AuditEvent {
                category: AuditCategory::Drift,
                action: "drift_report_received".to_owned(),
                actor: AuditActor::new("agent"),
                target: AuditTarget::new("missing-agent"),
                value: AuditValue::Plain("policy_name=nginx-running,status=drifted".to_owned()),
                occurred_at: SystemTime::UNIX_EPOCH,
            },
            proposal_audit: AuditEvent::security("remediation_requested", "missing-agent"),
            checked_at: SystemTime::UNIX_EPOCH,
        };

        assert!(
            <SqliteStore as VerifiedDriftProposalRepository>::save_verified_drift_proposal(
                &mut store, input,
            )
            .is_err()
        );
        assert_eq!(row_count(&store, "drift_reports"), 0);
        assert_eq!(row_count(&store, "remediation_requests"), 0);
        assert_eq!(row_count(&store, "audit_events"), 0);
    }

    #[test]
    fn consumes_valid_enrollment_token_once() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_enrollment_token_hash(
                "et-1",
                "hash-1",
                "role=web",
                SystemTime::UNIX_EPOCH + Duration::from_secs(60),
                1,
            )
            .unwrap();

        let record = store
            .consume_enrollment_token_hash("hash-1", SystemTime::UNIX_EPOCH)
            .unwrap();

        assert_eq!(record.default_labels, "role=web");
        assert_eq!(
            store.consume_enrollment_token_hash("hash-1", SystemTime::UNIX_EPOCH),
            Err(StoreError::Domain(
                "enrollment token max uses exceeded".to_owned()
            ))
        );
    }

    fn task_envelope(nonce: &str, task_id: &str) -> TaskEnvelope {
        task_envelope_for_job("job-1", "a1", nonce, task_id)
    }

    fn task_envelope_for_job(
        job_id: &str,
        agent_id: &str,
        nonce: &str,
        task_id: &str,
    ) -> TaskEnvelope {
        TaskEnvelope {
            job_id: fleet_domain::JobId::new(job_id).unwrap(),
            task_id: fleet_domain::TaskId::new(task_id).unwrap(),
            target_agent_id: AgentId::new(agent_id).unwrap(),
            issued_at: SystemTime::UNIX_EPOCH,
            expires_at: fleet_domain::TaskExpiry::new(
                SystemTime::UNIX_EPOCH + Duration::from_secs(60),
            ),
            nonce: fleet_domain::TaskNonce::new(nonce).unwrap(),
            payload_hash: "hash".to_owned(),
            signature: Some(fleet_domain::TaskSignature::new("sig").unwrap()),
        }
    }

    fn seed_retention_rows(store: &SqliteStore) {
        let job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-1").unwrap(),
            fleet_domain::TaskRisk::Low,
            fleet_domain::ApprovalRequirement::NotRequired,
            Duration::from_secs(30),
        );
        store.save_job_record(&job).unwrap();
        store
            .append_job_output_chunk_record(&JobOutputChunk {
                job_id: "job-1".to_owned(),
                agent_id: "a1".to_owned(),
                stream: JobOutputStream::Stdout,
                sequence: 0,
                body: "old".to_owned(),
            })
            .unwrap();
        store
            .append_job_output_chunk_record(&JobOutputChunk {
                job_id: "job-1".to_owned(),
                agent_id: "a1".to_owned(),
                stream: JobOutputStream::Stdout,
                sequence: 1,
                body: "recent".to_owned(),
            })
            .unwrap();
        store
            .connection
            .execute(
                "UPDATE job_output_chunks SET created_at = CASE chunk_index WHEN 0 THEN 1 ELSE 200 END",
                [],
            )
            .unwrap();
        store
            .insert_facts_snapshot(
                "a1",
                "{\"old\":true}",
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();
        store
            .insert_facts_snapshot(
                "a1",
                "{\"recent\":true}",
                SystemTime::UNIX_EPOCH + Duration::from_secs(200),
            )
            .unwrap();
        store
            .insert_metrics_snapshot(
                "a1",
                "{\"old\":true}",
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();
        store
            .insert_metrics_snapshot(
                "a1",
                "{\"recent\":true}",
                SystemTime::UNIX_EPOCH + Duration::from_secs(200),
            )
            .unwrap();
        store
            .insert_agent_log_chunk(
                "a1",
                "level=info event=old_agent_log",
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();
        store
            .insert_agent_log_chunk(
                "a1",
                "level=info event=recent_agent_log",
                SystemTime::UNIX_EPOCH + Duration::from_secs(200),
            )
            .unwrap();
    }

    fn rendered_artifact_metadata(
        id: &str,
        job_id: &str,
        agent_id: &str,
        task_id: &str,
    ) -> fleet_domain::RenderedArtifactMetadata {
        fleet_domain::RenderedArtifactMetadata::new(
            fleet_domain::ArtifactId::new(id).unwrap(),
            fleet_domain::JobId::new(job_id).unwrap(),
            fleet_domain::AgentId::new(agent_id).unwrap(),
            fleet_domain::TaskId::new(task_id).unwrap(),
            "/etc/app.conf",
            fleet_domain::ArtifactChecksum::sha256(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap(),
            42,
            fleet_domain::ArtifactRetentionClass::RenderedTemplate,
            SystemTime::UNIX_EPOCH + Duration::from_secs(10),
        )
        .unwrap()
    }

    fn signing_key_rotation_record() -> SigningKeyRotationRecord {
        let mut rotation = ControllerSigningKeyRotation::steady(
            SigningKeyFingerprint::new("old-signing-fingerprint").unwrap(),
        );
        rotation
            .request_rotation(
                SigningKeyFingerprint::new("new-signing-fingerprint").unwrap(),
                SystemTime::UNIX_EPOCH + Duration::from_secs(10),
                SystemTime::UNIX_EPOCH + Duration::from_secs(40),
            )
            .unwrap();
        rotation
            .validate_new_material(SystemTime::UNIX_EPOCH + Duration::from_secs(12))
            .unwrap();
        rotation
            .activate_dual_trust(SystemTime::UNIX_EPOCH + Duration::from_secs(20))
            .unwrap();
        SigningKeyRotationRecord {
            controller_id: "controller-default".to_owned(),
            rotation,
            updated_at: SystemTime::UNIX_EPOCH + Duration::from_secs(21),
        }
    }

    fn staged_rollout_record() -> ControllerSigningStagedRolloutRecord {
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
                SystemTime::UNIX_EPOCH + Duration::from_secs(12),
            )
            .unwrap();
        rollout
            .batch_dispatched(
                &plan.agent_ids,
                SystemTime::UNIX_EPOCH + Duration::from_secs(12),
            )
            .unwrap();
        ControllerSigningStagedRolloutRecord {
            controller_id: "controller-default".to_owned(),
            current_fingerprint: "new-signing-fingerprint".to_owned(),
            previous_fingerprint: Some("old-signing-fingerprint".to_owned()),
            rollout,
            updated_at: SystemTime::UNIX_EPOCH + Duration::from_secs(13),
        }
    }

    fn agent_certificate_lifecycle_record() -> AgentCertificateLifecycleRecord {
        let agent_id = AgentId::new("agent-1").unwrap();
        let mut lifecycle = AgentCertificateLifecycle::new(agent_id.clone());
        lifecycle
            .request_issuance(UNIX_EPOCH + Duration::from_secs(10))
            .unwrap();
        lifecycle
            .issue(
                agent_certificate("serial-1", "0123456789abcdef", 10, 110),
                UNIX_EPOCH + Duration::from_secs(11),
            )
            .unwrap();
        lifecycle
            .request_renewal(
                UNIX_EPOCH + Duration::from_secs(80),
                &fleet_domain::AgentCertificateRenewalPolicy::new(
                    Duration::from_secs(30),
                    Duration::from_secs(10),
                )
                .unwrap(),
            )
            .unwrap();
        lifecycle
            .activate_renewal(
                agent_certificate("serial-2", "fedcba9876543210", 80, 200),
                UNIX_EPOCH + Duration::from_secs(81),
                &fleet_domain::AgentCertificateRenewalPolicy::new(
                    Duration::from_secs(30),
                    Duration::from_secs(10),
                )
                .unwrap(),
            )
            .unwrap();
        AgentCertificateLifecycleRecord {
            agent_id,
            lifecycle: lifecycle.snapshot(),
            updated_at: UNIX_EPOCH + Duration::from_secs(82),
        }
    }

    fn agent_certificate(
        serial: &str,
        fingerprint: &str,
        not_before: u64,
        not_after: u64,
    ) -> AgentCertificate {
        AgentCertificate::new(
            AgentCertificateSerial::new(serial).unwrap(),
            AgentCertificateFingerprint::new(fingerprint).unwrap(),
            AgentCertificateValidity::new(
                UNIX_EPOCH + Duration::from_secs(not_before),
                UNIX_EPOCH + Duration::from_secs(not_after),
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn artifact_test_root(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("fleet-{name}-{}-{unique}", std::process::id()))
    }

    fn test_artifact_checksum(bytes: &[u8]) -> ArtifactChecksum {
        artifact_sha256(bytes).unwrap()
    }

    fn remediation_request_record(
        id: &str,
        agent_id: &str,
        policy_id: &str,
    ) -> AppRemediationRequestRecord {
        AppRemediationRequestRecord {
            id: id.to_owned(),
            policy_id: policy_id.to_owned(),
            policy_name: policy_id.to_owned(),
            agent_id: agent_id.to_owned(),
            runbook_ref: "runbooks/remediate.yml".to_owned(),
            status: "proposed".to_owned(),
            approval_required: true,
            risk_summary: "drifted policy requires approved remediation".to_owned(),
            job_id: None,
            origin_drift_report_id: None,
            policy_version: None,
            created_at: SystemTime::UNIX_EPOCH,
            updated_at: SystemTime::UNIX_EPOCH,
        }
    }

    fn row_count(store: &SqliteStore, table: &str) -> usize {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        let count: i64 = store
            .connection
            .query_row(&sql, [], |row| row.get(0))
            .unwrap();
        count as usize
    }

    #[test]
    fn rejects_expired_enrollment_token() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_enrollment_token_hash("et-1", "hash-1", "", SystemTime::UNIX_EPOCH, 1)
            .unwrap();

        assert_eq!(
            store.consume_enrollment_token_hash("hash-1", SystemTime::UNIX_EPOCH),
            Err(StoreError::Domain("enrollment token is expired".to_owned()))
        );
    }

    #[test]
    fn rejects_revoked_enrollment_token() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_enrollment_token_hash(
                "et-1",
                "hash-1",
                "",
                SystemTime::UNIX_EPOCH + Duration::from_secs(60),
                1,
            )
            .unwrap();
        assert!(store.revoke_enrollment_token("et-1").unwrap());

        assert_eq!(
            store.consume_enrollment_token_hash("hash-1", SystemTime::UNIX_EPOCH),
            Err(StoreError::Domain("enrollment token is revoked".to_owned()))
        );
    }

    #[test]
    fn marks_agent_online() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();

        assert!(
            store
                .mark_agent_online("a1", SystemTime::UNIX_EPOCH + Duration::from_secs(5))
                .unwrap()
        );

        let found = store
            .find_by_id(&AgentId::new("a1").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(found.status(), AgentStatus::Online);
        assert_eq!(
            store.find_agent_fingerprint("a1").unwrap().as_deref(),
            Some("0123456789abcdef")
        );
    }

    #[test]
    fn marks_agent_degraded_without_touching_disabled_agents() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();

        assert!(
            store
                .mark_agent_degraded("a1", SystemTime::UNIX_EPOCH + Duration::from_secs(5))
                .unwrap()
        );
        let found = store.find_agent_by_id("a1").unwrap().unwrap();
        assert_eq!(found.status(), AgentStatus::Degraded);

        let disabled_store = SqliteStore::in_memory().unwrap();
        let mut disabled = agent();
        disabled.disable();
        disabled_store.save_agent(disabled).unwrap();
        assert!(
            !disabled_store
                .mark_agent_degraded("a1", SystemTime::UNIX_EPOCH + Duration::from_secs(10))
                .unwrap()
        );
        let found = disabled_store.find_agent_by_id("a1").unwrap().unwrap();
        assert_eq!(found.status(), AgentStatus::Disabled);
    }

    #[test]
    fn stale_online_agents_transition_offline() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .mark_agent_online("a1", SystemTime::UNIX_EPOCH + Duration::from_secs(10))
            .unwrap();

        let changed = store
            .mark_stale_agents_offline(
                SystemTime::UNIX_EPOCH + Duration::from_secs(20),
                SystemTime::UNIX_EPOCH + Duration::from_secs(30),
            )
            .unwrap();

        let found = store.find_agent_by_id("a1").unwrap().unwrap();
        assert_eq!(changed, 1);
        assert_eq!(found.status(), AgentStatus::Offline);
    }

    #[test]
    fn stale_degraded_agents_transition_offline() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .mark_agent_degraded("a1", SystemTime::UNIX_EPOCH + Duration::from_secs(10))
            .unwrap();

        let changed = store
            .mark_stale_agents_offline(
                SystemTime::UNIX_EPOCH + Duration::from_secs(20),
                SystemTime::UNIX_EPOCH + Duration::from_secs(30),
            )
            .unwrap();

        let found = store.find_agent_by_id("a1").unwrap().unwrap();
        assert_eq!(changed, 1);
        assert_eq!(found.status(), AgentStatus::Offline);
    }

    #[test]
    fn recent_online_agents_remain_online_during_offline_sweep() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .mark_agent_online("a1", SystemTime::UNIX_EPOCH + Duration::from_secs(30))
            .unwrap();

        let changed = store
            .mark_stale_agents_offline(
                SystemTime::UNIX_EPOCH + Duration::from_secs(20),
                SystemTime::UNIX_EPOCH + Duration::from_secs(40),
            )
            .unwrap();

        let found = store.find_agent_by_id("a1").unwrap().unwrap();
        assert_eq!(changed, 0);
        assert_eq!(found.status(), AgentStatus::Online);
    }

    #[test]
    fn audit_repository_is_append_only_and_queryable() {
        let mut store = SqliteStore::in_memory().unwrap();
        store
            .write(AuditEvent::security("invalid_signature", "agent-1"))
            .unwrap();
        store
            .write(AuditEvent {
                category: AuditCategory::Agent,
                action: "online".to_owned(),
                actor: AuditActor::new("system"),
                target: AuditTarget::new("agent-1"),
                value: AuditValue::Plain("status=online".to_owned()),
                occurred_at: SystemTime::UNIX_EPOCH,
            })
            .unwrap();

        assert_eq!(AuditRepository::list(&store, 10).unwrap().len(), 2);
        let security = store.list_by_category(AuditCategory::Security, 10).unwrap();
        assert_eq!(security.len(), 1);
        assert!(!security[0].contains_secret_plaintext());
        assert_eq!(
            store
                .audit_count_by_category(AuditCategory::Security)
                .unwrap(),
            1
        );
    }

    #[test]
    fn audit_export_filters_by_category_and_pages_with_cursor() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .write_audit_event(AuditEvent {
                category: AuditCategory::Security,
                action: "invalid_signature".to_owned(),
                actor: AuditActor::new("system"),
                target: AuditTarget::new("agent-1"),
                value: AuditValue::SecretRef("token=raw-secret".to_owned()),
                occurred_at: SystemTime::UNIX_EPOCH + Duration::from_secs(3),
            })
            .unwrap();
        store
            .write_audit_event(AuditEvent {
                category: AuditCategory::Job,
                action: "job_created".to_owned(),
                actor: AuditActor::new("operator"),
                target: AuditTarget::new("job-1"),
                value: AuditValue::Plain("target_count=1".to_owned()),
                occurred_at: SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            })
            .unwrap();
        store
            .write_audit_event(AuditEvent {
                category: AuditCategory::Security,
                action: "insecure_http_transport_enabled".to_owned(),
                actor: AuditActor::new("controller"),
                target: AuditTarget::new("http://127.0.0.1:7700"),
                value: AuditValue::Plain("http_without_tls".to_owned()),
                occurred_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            })
            .unwrap();

        let first_page = store
            .export_audit_events(Some(AuditCategory::Security), 1, None)
            .unwrap();
        assert_eq!(first_page.len(), 1);
        assert_eq!(first_page[0].event.action, "invalid_signature");
        assert_eq!(first_page[0].cursor.row_id, 1);
        assert!(!first_page[0].event.contains_secret_plaintext());

        let second_page = store
            .export_audit_events(Some(AuditCategory::Security), 1, Some(first_page[0].cursor))
            .unwrap();
        assert_eq!(second_page.len(), 1);
        assert_eq!(
            second_page[0].event.action,
            "insecure_http_transport_enabled"
        );
    }
}
