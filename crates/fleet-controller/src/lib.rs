use axum::{
    Router,
    body::Bytes,
    extract::{
        State,
        ws::{Message as AxumWsMessage, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response as AxumResponse},
    routing::get,
};
use fleet_application::{
    ActivateSigningKeyRotation, ActivateSigningKeyRotationInput,
    AgentCertificateLifecycleRepository, AgentCertificateLifecycleUseCaseError,
    ApprovalRepository as AppApprovalRepository, ApprovalRequestRecord as AppApprovalRequestRecord,
    ApprovalUseCaseError, ApproveApprovalInput, ApproveApprovalRequest,
    ApproveRemediationRunbookJob, ApproveRemediationRunbookJobError,
    ApproveRemediationRunbookJobInput, ArtifactStore, ControllerSigningRotationStatus,
    ControllerSigningRotationStatusInput, ControllerSigningStagedRolloutRecord,
    ControllerSigningStagedRolloutRepository, FailSigningKeyRotation, FailSigningKeyRotationInput,
    QueryControllerSigningRotationStatus, RejectApprovalInput, RejectApprovalRequest,
};
use fleet_application::{
    AdminTokenRepository, AgentInventoryRepository, AgentLogRepository, AuthenticateAdminToken,
    CommandJobRepository, CreateCommandJob, CreateCommandJobError, CreateCommandJobInput,
    CreateDriftCheckJob, CreateDriftCheckJobError, CreateDriftCheckJobInput, CreateEnrollmentToken,
    CreateEnrollmentTokenInput, CreateRunbookJob, CreateRunbookJobError, CreateRunbookJobInput,
    DispatchAssignmentRepository, DispatchPendingAssignments, DispatchPendingAssignmentsInput,
    DispatchPendingAssignmentsOutput, DriftRepository, EnrollmentTokenRepository,
    EnrollmentTokenUseCaseError, EnsureAdminToken, ExpireApprovalRequests, ExpireApprovalsInput,
    ExportAuditEvents, FactsRepository, GetInventoryAgent, GetJobSummary, GetLatestDrift,
    GetLatestFacts, GetLatestMetrics, JobDispatchGate, JobOutputChunk, JobOutputRepository,
    JobOutputStream, JobQueryRepository, JobRepository, ListAgentLogChunks, ListApprovalRequests,
    ListAuditEvents, ListDriftReports, ListDueScheduledDrift, ListEnrollmentTokens,
    ListFactsSnapshots, ListInventoryAgents, ListJobOutputForJob, ListJobSummaries,
    ListMetricsSnapshots, MarkRemediationJobRunning, MarkRemediationJobRunningInput,
    MetricsRepository, PendingAssignmentDispatcher, PendingTaskAssignment,
    PolicyRepository as AppPolicyRepository, PreviewSelector, RecordRemediationJobResult,
    RecordRemediationJobResultInput, RemediationApprovalRequestError, RemediationJobResultStatus,
    RemediationRequestRecord, RemediationRequestRepository, RemediationResultUseCaseError,
    RequestAgentCertificateIssuance, RequestAgentCertificateIssuanceInput,
    RequestRemediationApproval, RequestRemediationApprovalInput, RequestSigningKeyRotation,
    RequestSigningKeyRotationInput, RetireSigningKeyRotation, RetireSigningKeyRotationInput,
    RevokeAgentKey, RevokeAgentKeyError, RevokeAgentKeyInput, RevokeEnrollmentToken,
    RevokeEnrollmentTokenInput, RunDueScheduledDrift, RunDueScheduledDriftError,
    RunDueScheduledDriftInput, RunRetentionCleanup, RunRetentionCleanupError,
    RunRetentionCleanupInput, RunbookJobRepository, SavePolicy, SavePolicyInput,
    SchedulePolicyDrift, SchedulePolicyDriftInput, SelectorPreviewInput, SigningKeyRotationRecord,
    SigningKeyRotationRepository, SigningKeyRotationUseCaseError, SnapshotPageCursor,
    TaskAssignmentRepository, TaskEnvelopeSigner, UpdateAgentLabels, UpdateAgentLabelsError,
    UpdateAgentLabelsInput, ValidateSigningKeyRotation, ValidateSigningKeyRotationInput,
    VerifyRemediationResolution, VerifyRemediationResolutionInput,
    select_controller_signing_fingerprint, select_dispatch_targets,
};
use fleet_application::{AssignPolicyToAgent, AssignPolicyToAgentInput, RetentionPolicy};
use fleet_application::{
    DisabledSecretProvider, DisabledSecretProviderError, ResolvedSecret, SecretProvider,
    SecretProviderError, StaticSecretProvider, StaticSecretProviderError,
};
use fleet_core::{
    AgentClientCertificateTrust, ArtifactStoreBackend, ArtifactStoreSettings,
    ControllerTrustSettings, DatabaseBackend, DatabaseSettings, SecretProviderBackend,
    SecretProviderSettings,
};
use fleet_domain::{
    Agent, AgentCapability, AgentCapabilitySnapshot, AgentFingerprint, AgentId, AgentIdentity,
    AgentLabel, AgentName, AgentPublicKey, AgentRuntimeProfile, AgentStatus, ArtifactChecksum,
    ArtifactId, ArtifactRetentionClass, AssignmentStatus, AuditActor, AuditCategory, AuditEvent,
    AuditTarget, AuditValue, ControllerPublicKey, DriftAcknowledgement, DriftReport, DriftSeverity,
    DriftStatus, Job, JobId, JobStatus, PackageManager, PrivilegeLevel, RenderedArtifactMetadata,
    Selector, ServiceManager, TaskEnvelope, TaskId,
};
use fleet_store::{LocalArtifactStore, SqliteStore};
#[cfg(feature = "postgres")]
use fleet_store::{PostgresStore, PostgresStoreConnectSettings, PostgresStoreSslMode};
use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::io::BufReader;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const ADMIN_INDEX_HTML: &str = include_str!("../../../web-admin/index.html");
const ADMIN_STYLES_CSS: &str = include_str!("../../../web-admin/styles.css");
const ADMIN_APP_JS: &str = include_str!("../../../web-admin/app.js");
const ADMIN_API_CLIENT_JS: &str = include_str!("../../../web-admin/api-client.js");
const ADMIN_API_SCHEMA_JSON: &str = include_str!("../../../web-admin/api.schema.json");
const OPENAPI_JSON: &str = include_str!("../../../docs/openapi.json");
const SWAGGER_UI_HTML: &str = include_str!("../../../docs/swagger-ui.html");
const AGENT_OFFLINE_AFTER: Duration = Duration::from_secs(90);
const AGENT_RECENTLY_SEEN_AFTER: Duration = Duration::from_secs(90);
const HEARTBEAT_BOUND_IDLE_CLOSE_AFTER: Duration = Duration::from_millis(75);
const AGENT_SESSION_OUTBOUND_QUEUE_CAPACITY: usize = 64;
const AGENT_LOG_CHUNK_MAX_BYTES: usize = 4096;
const SCHEDULED_DRIFT_WORKER_INTERVAL: Duration = Duration::from_secs(30);
const SCHEDULED_DRIFT_WORKER_GRACE: Duration = Duration::from_secs(60);
const SCHEDULED_DRIFT_WORKER_LIMIT: usize = 100;
const SCHEDULED_DRIFT_JOB_TIMEOUT: Duration = Duration::from_secs(30);
const SCHEDULED_DRIFT_JOB_EXPIRES_IN: Duration = Duration::from_secs(300);
const CONTROLLER_SIGNING_STAGED_ROLLOUT_WORKER_INTERVAL: Duration = Duration::from_secs(30);
const RETENTION_WORKER_INTERVAL: Duration = Duration::from_secs(3_600);
const DEFAULT_MAX_ARTIFACT_BODY_BYTES: usize = 1024 * 1024;
const DEFAULT_CONTROLLER_ID: &str = "controller-default";
const CONTROLLER_SIGNING_VALIDATION_CHALLENGE: &str = "controller-signing-rotation-validation";
const AGENT_CLIENT_CERTIFICATE_MTLS_UNSUPPORTED: &str = "agent client certificate mTLS enforcement is not implemented; remove --agent-client-ca-cert until controller mTLS enforcement is available";

#[derive(Debug, Clone)]
pub struct ControllerServerConfig {
    pub host: String,
    pub port: u16,
    pub external_url: Option<String>,
    pub tls_cert_path: Option<PathBuf>,
    pub tls_key_path: Option<PathBuf>,
    pub agent_client_ca_cert_path: Option<PathBuf>,
    pub data_dir: PathBuf,
    pub database: Option<DatabaseSettings>,
    pub secret_provider: Option<SecretProviderSettings>,
}

#[derive(Clone)]
struct ControllerAppState {
    store: Arc<Mutex<ControllerStore>>,
    artifact_store: Arc<Mutex<LocalArtifactStore>>,
    identity: Arc<ControllerIdentity>,
    metadata: Arc<ControllerRuntimeMetadata>,
    sessions: Arc<Mutex<AgentSessionRegistry>>,
}

enum ControllerStore {
    Sqlite(SqliteStore),
    #[cfg(feature = "postgres")]
    Postgres(Box<std::cell::RefCell<PostgresStore>>),
}

#[derive(Debug, Clone)]
enum ControllerSecretProvider {
    Disabled(DisabledSecretProvider),
    StaticTest(StaticSecretProvider),
}

impl ControllerSecretProvider {
    fn mode(&self) -> &'static str {
        match self {
            Self::Disabled(_) => "disabled",
            Self::StaticTest(_) => "static-test",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ControllerSecretProviderConstructionError {
    StaticTestFixtureSourceRequired,
}

impl Display for ControllerSecretProviderConstructionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaticTestFixtureSourceRequired => formatter.write_str(
                "static test secret provider requires an explicit fixture source at bootstrap",
            ),
        }
    }
}

impl std::error::Error for ControllerSecretProviderConstructionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControllerSecretProviderRuntimeError {
    Disabled(DisabledSecretProviderError),
    StaticTest(StaticSecretProviderError),
}

impl Display for ControllerSecretProviderRuntimeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled(error) => Display::fmt(error, formatter),
            Self::StaticTest(error) => Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for ControllerSecretProviderRuntimeError {}

impl SecretProvider for ControllerSecretProvider {
    type Error = ControllerSecretProviderRuntimeError;

    fn resolve_secret(
        &self,
        reference: &fleet_domain::SecretRef,
    ) -> Result<ResolvedSecret, SecretProviderError<Self::Error>> {
        match self {
            Self::Disabled(provider) => provider.resolve_secret(reference).map_err(|error| {
                map_secret_provider_error(error, ControllerSecretProviderRuntimeError::Disabled)
            }),
            Self::StaticTest(provider) => provider.resolve_secret(reference).map_err(|error| {
                map_secret_provider_error(error, ControllerSecretProviderRuntimeError::StaticTest)
            }),
        }
    }
}

fn map_secret_provider_error<E>(
    error: SecretProviderError<E>,
    map_source: impl FnOnce(E) -> ControllerSecretProviderRuntimeError,
) -> SecretProviderError<ControllerSecretProviderRuntimeError> {
    match error {
        SecretProviderError::NotFound { reference } => SecretProviderError::NotFound { reference },
        SecretProviderError::Denied { reference } => SecretProviderError::Denied { reference },
        SecretProviderError::Provider { reference, source } => SecretProviderError::Provider {
            reference,
            source: map_source(source),
        },
    }
}

#[allow(dead_code)]
impl ControllerStore {
    fn sqlite(store: SqliteStore) -> Self {
        Self::Sqlite(store)
    }

    #[cfg(feature = "postgres")]
    fn postgres(store: PostgresStore) -> Self {
        Self::Postgres(Box::new(std::cell::RefCell::new(store)))
    }

    fn backend_name(&self) -> &'static str {
        match self {
            Self::Sqlite(_) => "sqlite",
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => Self::postgres_backend_name(),
        }
    }

    fn load_signing_key_rotation(
        &self,
        controller_id: &str,
    ) -> Result<Option<SigningKeyRotationRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => {
                SigningKeyRotationRepository::load_signing_key_rotation(store, controller_id)
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => SigningKeyRotationRepository::load_signing_key_rotation(
                &*store.borrow(),
                controller_id,
            ),
        }
    }

    #[cfg(feature = "postgres")]
    fn postgres_backend_name() -> &'static str {
        "postgres"
    }
}

#[derive(Clone, Copy)]
enum ControllerStoreRef<'a> {
    Sqlite(&'a SqliteStore),
    #[cfg(feature = "postgres")]
    Postgres(&'a std::cell::RefCell<PostgresStore>),
}

impl<'a> From<&'a ControllerStore> for ControllerStoreRef<'a> {
    fn from(store: &'a ControllerStore) -> Self {
        match store {
            ControllerStore::Sqlite(sqlite) => Self::Sqlite(sqlite),
            #[cfg(feature = "postgres")]
            ControllerStore::Postgres(postgres) => Self::Postgres(postgres),
        }
    }
}

impl<'a> From<&'a SqliteStore> for ControllerStoreRef<'a> {
    fn from(store: &'a SqliteStore) -> Self {
        Self::Sqlite(store)
    }
}

impl<'a> From<&'a std::sync::MutexGuard<'_, ControllerStore>> for ControllerStoreRef<'a> {
    fn from(store: &'a std::sync::MutexGuard<'_, ControllerStore>) -> Self {
        (&**store).into()
    }
}

impl SigningKeyRotationRepository for ControllerStoreRef<'_> {
    type Error = fleet_store::StoreError;

    fn save_signing_key_rotation(
        &mut self,
        record: SigningKeyRotationRecord,
    ) -> Result<(), Self::Error> {
        match self {
            Self::Sqlite(store) => {
                let mut store = *store;
                SigningKeyRotationRepository::save_signing_key_rotation(&mut store, record)
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => SigningKeyRotationRepository::save_signing_key_rotation(
                &mut *store.borrow_mut(),
                record,
            ),
        }
    }

    fn load_signing_key_rotation(
        &self,
        controller_id: &str,
    ) -> Result<Option<SigningKeyRotationRecord>, Self::Error> {
        match self {
            Self::Sqlite(store) => {
                SigningKeyRotationRepository::load_signing_key_rotation(*store, controller_id)
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => SigningKeyRotationRepository::load_signing_key_rotation(
                &*store.borrow(),
                controller_id,
            ),
        }
    }
}

impl ControllerSigningStagedRolloutRepository for ControllerStoreRef<'_> {
    type Error = fleet_store::StoreError;

    fn save_controller_signing_staged_rollout(
        &mut self,
        record: ControllerSigningStagedRolloutRecord,
    ) -> Result<(), Self::Error> {
        match self {
            Self::Sqlite(store) => {
                let mut store = *store;
                ControllerSigningStagedRolloutRepository::save_controller_signing_staged_rollout(
                    &mut store, record,
                )
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => {
                ControllerSigningStagedRolloutRepository::save_controller_signing_staged_rollout(
                    &mut *store.borrow_mut(),
                    record,
                )
            }
        }
    }

    fn load_controller_signing_staged_rollout(
        &self,
        controller_id: &str,
    ) -> Result<Option<ControllerSigningStagedRolloutRecord>, Self::Error> {
        match self {
            Self::Sqlite(store) => {
                ControllerSigningStagedRolloutRepository::load_controller_signing_staged_rollout(
                    *store,
                    controller_id,
                )
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => {
                ControllerSigningStagedRolloutRepository::load_controller_signing_staged_rollout(
                    &*store.borrow(),
                    controller_id,
                )
            }
        }
    }
}

#[allow(dead_code)]
impl ControllerStore {
    fn store_ref(&self) -> ControllerStoreRef<'_> {
        self.into()
    }

    fn mark_agent_online(
        &self,
        agent_id: &str,
        at: SystemTime,
    ) -> Result<bool, fleet_store::StoreError> {
        self.store_ref().mark_agent_online(agent_id, at)
    }

    fn mark_agent_degraded(
        &self,
        agent_id: &str,
        at: SystemTime,
    ) -> Result<bool, fleet_store::StoreError> {
        self.store_ref().mark_agent_degraded(agent_id, at)
    }

    fn mark_stale_agents_offline(
        &self,
        cutoff: SystemTime,
        now: SystemTime,
    ) -> Result<usize, fleet_store::StoreError> {
        self.store_ref().mark_stale_agents_offline(cutoff, now)
    }

    fn write_audit_event(&self, event: AuditEvent) -> Result<(), fleet_store::StoreError> {
        self.store_ref().write_audit_event(event)
    }

    fn find_agent_identity(
        &self,
        agent_id: &str,
    ) -> Result<Option<(String, String)>, fleet_store::StoreError> {
        self.store_ref().find_agent_identity(agent_id)
    }

    fn update_active_task_assignment_status(
        &self,
        task_id: &str,
        status: AssignmentStatus,
        occurred_at: SystemTime,
        last_error: Option<&str>,
    ) -> Result<bool, fleet_store::StoreError> {
        self.store_ref().update_active_task_assignment_status(
            task_id,
            status,
            occurred_at,
            last_error,
        )
    }

    fn update_task_assignment_status(
        &self,
        task_id: &str,
        status: AssignmentStatus,
        occurred_at: SystemTime,
        last_error: Option<&str>,
    ) -> Result<bool, fleet_store::StoreError> {
        self.store_ref()
            .update_task_assignment_status(task_id, status, occurred_at, last_error)
    }

    fn find_task_assignment_status(
        &self,
        task_id: &str,
    ) -> Result<Option<String>, fleet_store::StoreError> {
        self.store_ref().find_task_assignment_status(task_id)
    }

    fn find_job_status_value(
        &self,
        job_id: &str,
    ) -> Result<Option<String>, fleet_store::StoreError> {
        self.store_ref().find_job_status_value(job_id)
    }

    fn list_task_assignment_summaries_for_job(
        &self,
        job_id: &str,
    ) -> Result<Vec<fleet_store::TaskAssignmentSummaryRecord>, fleet_store::StoreError> {
        self.store_ref()
            .list_task_assignment_summaries_for_job(job_id)
    }

    fn recompute_job_status_from_assignments(
        &self,
        job_id: &str,
    ) -> Result<Option<JobStatus>, fleet_store::StoreError> {
        self.store_ref()
            .recompute_job_status_from_assignments(job_id)
    }

    fn insert_facts_snapshot(
        &self,
        agent_id: &str,
        body: &str,
        collected_at: SystemTime,
    ) -> Result<(), fleet_store::StoreError> {
        self.store_ref()
            .insert_facts_snapshot(agent_id, body, collected_at)
    }

    fn insert_metrics_snapshot(
        &self,
        agent_id: &str,
        body: &str,
        collected_at: SystemTime,
    ) -> Result<(), fleet_store::StoreError> {
        self.store_ref()
            .insert_metrics_snapshot(agent_id, body, collected_at)
    }

    fn save_agent_capability_snapshot(
        &self,
        agent_id: &AgentId,
        snapshot: AgentCapabilitySnapshot,
    ) -> Result<(), fleet_store::StoreError> {
        self.store_ref()
            .save_agent_capability_snapshot(agent_id, snapshot)
    }

    fn latest_agent_capability_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<AgentCapabilitySnapshot>, fleet_store::StoreError> {
        self.store_ref().latest_agent_capability_snapshot(agent_id)
    }

    fn insert_agent_log_chunk(
        &self,
        agent_id: &str,
        line: &str,
        collected_at: SystemTime,
    ) -> Result<(), fleet_store::StoreError> {
        self.store_ref()
            .insert_agent_log_chunk(agent_id, line, collected_at)
    }

    fn insert_drift_report(
        &self,
        agent_id: &str,
        report: &DriftReport,
        checked_at: SystemTime,
    ) -> Result<(), fleet_store::StoreError> {
        self.store_ref()
            .insert_drift_report(agent_id, report, checked_at)
    }

    fn append_job_output_chunk_record(
        &self,
        chunk: &JobOutputChunk,
    ) -> Result<(), fleet_store::StoreError> {
        self.store_ref().append_job_output_chunk_record(chunk)
    }

    fn cancel_queued_assignments_after_max_failures(
        &self,
        job_id: &str,
        occurred_at: SystemTime,
        reason: &str,
    ) -> Result<usize, fleet_store::StoreError> {
        self.store_ref()
            .cancel_queued_assignments_after_max_failures(job_id, occurred_at, reason)
    }

    fn save_rendered_artifact_metadata_record(
        &self,
        metadata: &RenderedArtifactMetadata,
    ) -> Result<(), fleet_store::StoreError> {
        self.store_ref()
            .save_rendered_artifact_metadata_record(metadata)
    }

    fn find_policy(
        &self,
        policy_id: &str,
    ) -> Result<Option<fleet_store::PolicyRecord>, fleet_store::StoreError> {
        self.store_ref().find_policy(policy_id)
    }

    fn list_remediation_request_records(
        &self,
        agent_id: Option<&str>,
        policy_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RemediationRequestRecord>, fleet_store::StoreError> {
        self.store_ref()
            .list_remediation_request_records(agent_id, policy_id, limit)
    }

    fn assigned_policy_ids_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<String>, fleet_store::StoreError> {
        self.store_ref().assigned_policy_ids_for_agent(agent_id)
    }

    fn consume_enrollment_token_hash(
        &self,
        token_hash: &str,
        now: SystemTime,
    ) -> Result<fleet_application::EnrollmentTokenRecord, fleet_store::StoreError> {
        self.store_ref()
            .consume_enrollment_token_hash(token_hash, now)
    }

    fn save_agent(&self, agent: Agent) -> Result<(), fleet_store::StoreError> {
        self.store_ref().save_agent(agent)
    }
}

#[allow(dead_code)]
impl ControllerStoreRef<'_> {
    #[cfg(feature = "postgres")]
    fn with_postgres<T>(
        &self,
        operation: impl FnOnce(&mut PostgresStore) -> Result<T, fleet_store::StoreError>,
    ) -> Result<T, fleet_store::StoreError> {
        match self {
            Self::Postgres(postgres) => operation(&mut postgres.borrow_mut()),
            Self::Sqlite(_) => unreachable!("with_postgres called for sqlite store"),
        }
    }

    fn admin_token_exists(&self) -> Result<bool, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.admin_token_exists(),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| store.admin_token_exists()),
        }
    }

    fn insert_admin_token_hash(&self, token_hash: &str) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.insert_admin_token_hash(token_hash),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.insert_admin_token_hash(token_hash))
            }
        }
    }

    fn verify_admin_token_hash(&self, token_hash: &str) -> Result<bool, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.verify_admin_token_hash(token_hash),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.verify_admin_token_hash(token_hash))
            }
        }
    }

    fn find_admin_token_record(
        &self,
        token_hash: &str,
    ) -> Result<Option<fleet_application::AdminTokenRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.find_admin_token_record(token_hash),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.find_admin_token_record(token_hash))
            }
        }
    }

    fn save_agent(&self, agent: Agent) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.save_agent(agent),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| store.save_agent(agent)),
        }
    }

    fn list_agents(&self) -> Result<Vec<Agent>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.list_agents(),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| store.list_agents()),
        }
    }

    fn find_agent_by_id(&self, agent_id: &str) -> Result<Option<Agent>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.find_agent_by_id(agent_id),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| store.find_agent_by_id(agent_id)),
        }
    }

    fn update_agent_labels(
        &self,
        agent_id: &str,
        labels: &[AgentLabel],
    ) -> Result<bool, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.update_agent_labels(agent_id, labels),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.update_agent_labels(agent_id, labels))
            }
        }
    }

    fn revoke_agent_key(&self, agent_id: &str) -> Result<bool, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.revoke_agent_key(agent_id),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| store.revoke_agent_key(agent_id)),
        }
    }

    fn find_agent_identity(
        &self,
        agent_id: &str,
    ) -> Result<Option<(String, String)>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.find_agent_identity(agent_id),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| store.find_agent_identity(agent_id)),
        }
    }

    fn mark_agent_online(
        &self,
        agent_id: &str,
        at: SystemTime,
    ) -> Result<bool, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.mark_agent_online(agent_id, at),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| store.mark_agent_online(agent_id, at)),
        }
    }

    fn mark_agent_degraded(
        &self,
        agent_id: &str,
        at: SystemTime,
    ) -> Result<bool, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.mark_agent_degraded(agent_id, at),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.mark_agent_degraded(agent_id, at))
            }
        }
    }

    fn mark_stale_agents_offline(
        &self,
        cutoff: SystemTime,
        now: SystemTime,
    ) -> Result<usize, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.mark_stale_agents_offline(cutoff, now),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.mark_stale_agents_offline(cutoff, now))
            }
        }
    }

    fn write_audit_event(&self, event: AuditEvent) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.write_audit_event(event),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| fleet_application::AuditWriter::write(store, event))
            }
        }
    }

    fn list_audit_events(&self, limit: usize) -> Result<Vec<AuditEvent>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.list_audit_events(limit),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| fleet_application::AuditRepository::list(store, limit))
            }
        }
    }

    fn list_audit_events_by_category(
        &self,
        category: AuditCategory,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.list_audit_events_by_category(category, limit),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                fleet_application::AuditRepository::list_by_category(store, category, limit)
            }),
        }
    }

    fn export_audit_events(
        &self,
        category: Option<AuditCategory>,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<fleet_application::AuditEventPageRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.export_audit_events(category, limit, before),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                fleet_application::AuditRepository::export_page(store, category, limit, before)
            }),
        }
    }

    fn insert_enrollment_token_hash(
        &self,
        id: &str,
        token_hash: &str,
        default_labels: &str,
        expires_at: SystemTime,
        max_uses: u32,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.insert_enrollment_token_hash(
                id,
                token_hash,
                default_labels,
                expires_at,
                max_uses,
            ),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                store.insert_enrollment_token_hash(
                    id,
                    token_hash,
                    default_labels,
                    expires_at,
                    max_uses,
                )
            }),
        }
    }

    fn list_enrollment_tokens(
        &self,
    ) -> Result<Vec<fleet_application::EnrollmentTokenRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => Ok(store
                .list_enrollment_tokens()?
                .into_iter()
                .map(app_enrollment_record_from_store)
                .collect()),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| store.list_enrollment_tokens()),
        }
    }

    fn revoke_enrollment_token(&self, id: &str) -> Result<bool, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.revoke_enrollment_token(id),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| store.revoke_enrollment_token(id)),
        }
    }

    fn consume_enrollment_token_hash(
        &self,
        token_hash: &str,
        now: SystemTime,
    ) -> Result<fleet_application::EnrollmentTokenRecord, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => Ok(app_enrollment_record_from_store(
                store.consume_enrollment_token_hash(token_hash, now)?,
            )),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.consume_enrollment_token_hash(token_hash, now))
            }
        }
    }

    fn insert_facts_snapshot(
        &self,
        agent_id: &str,
        body: &str,
        collected_at: SystemTime,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.insert_facts_snapshot(agent_id, body, collected_at),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self
                .with_postgres(|store| store.insert_facts_snapshot(agent_id, body, collected_at)),
        }
    }

    fn latest_facts_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<fleet_application::FactsSnapshotRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => Ok(store.latest_facts_snapshot(agent_id)?.map(|record| {
                fleet_application::FactsSnapshotRecord {
                    agent_id: record.agent_id,
                    body: record.body,
                    collected_at: record.collected_at,
                }
            })),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| store.latest_facts_snapshot(agent_id)),
        }
    }

    fn list_facts_snapshots(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<fleet_application::FactsSnapshotPageRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => Ok(store
                .list_facts_snapshots(agent_id, limit, before)?
                .into_iter()
                .map(|record| fleet_application::FactsSnapshotPageRecord {
                    agent_id: record.agent_id,
                    body: record.body,
                    collected_at: record.collected_at,
                    cursor: record.cursor,
                })
                .collect()),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.list_facts_snapshots(agent_id, limit, before))
            }
        }
    }

    fn insert_metrics_snapshot(
        &self,
        agent_id: &str,
        body: &str,
        collected_at: SystemTime,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.insert_metrics_snapshot(agent_id, body, collected_at),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self
                .with_postgres(|store| store.insert_metrics_snapshot(agent_id, body, collected_at)),
        }
    }

    fn latest_metrics_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<fleet_application::MetricsSnapshotRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => Ok(store.latest_metrics_snapshot(agent_id)?.map(|record| {
                fleet_application::MetricsSnapshotRecord {
                    agent_id: record.agent_id,
                    body: record.body,
                    collected_at: record.collected_at,
                }
            })),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.latest_metrics_snapshot(agent_id))
            }
        }
    }

    fn list_metrics_snapshots(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<fleet_application::MetricsSnapshotPageRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => Ok(store
                .list_metrics_snapshots(agent_id, limit, before)?
                .into_iter()
                .map(|record| fleet_application::MetricsSnapshotPageRecord {
                    agent_id: record.agent_id,
                    body: record.body,
                    collected_at: record.collected_at,
                    cursor: record.cursor,
                })
                .collect()),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.list_metrics_snapshots(agent_id, limit, before))
            }
        }
    }

    fn insert_agent_log_chunk(
        &self,
        agent_id: &str,
        line: &str,
        collected_at: SystemTime,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.insert_agent_log_chunk(agent_id, line, collected_at),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self
                .with_postgres(|store| store.insert_agent_log_chunk(agent_id, line, collected_at)),
        }
    }

    fn list_agent_log_chunks_page(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<fleet_application::AgentLogChunkPageRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => Ok(store
                .list_agent_log_chunks_page(agent_id, limit, before)?
                .into_iter()
                .map(|record| fleet_application::AgentLogChunkPageRecord {
                    agent_id: record.agent_id,
                    line: record.line,
                    collected_at: record.collected_at,
                    cursor: record.cursor,
                })
                .collect()),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.list_agent_log_chunks(agent_id, limit, before))
            }
        }
    }

    fn insert_drift_report(
        &self,
        agent_id: &str,
        report: &DriftReport,
        checked_at: SystemTime,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.insert_drift_report(agent_id, report, checked_at),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.insert_drift_report(agent_id, report, checked_at))
            }
        }
    }

    fn latest_drift_report(
        &self,
        agent_id: &str,
    ) -> Result<Option<fleet_application::DriftReportRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => Ok(store.latest_drift_report(agent_id)?.map(|record| {
                fleet_application::DriftReportRecord {
                    agent_id: record.agent_id,
                    report: record.report,
                    checked_at: record.checked_at,
                }
            })),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| store.latest_drift_report(agent_id)),
        }
    }

    fn list_drift_reports(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<fleet_application::DriftReportPageRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => Ok(store
                .list_drift_reports(agent_id, limit, before)?
                .into_iter()
                .map(|record| fleet_application::DriftReportPageRecord {
                    agent_id: record.agent_id,
                    report: record.report,
                    checked_at: record.checked_at,
                    cursor: record.cursor,
                })
                .collect()),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.list_drift_reports(agent_id, limit, before))
            }
        }
    }

    fn save_agent_capability_snapshot(
        &self,
        agent_id: &AgentId,
        snapshot: AgentCapabilitySnapshot,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => {
                SqliteStore::save_agent_capability_snapshot(store, agent_id.as_str(), snapshot)
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                fleet_application::AgentCapabilityRepository::save_agent_capability_snapshot(
                    store, agent_id, snapshot,
                )
            }),
        }
    }

    fn latest_agent_capability_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<AgentCapabilitySnapshot>, fleet_store::StoreError> {
        let agent_id = AgentId::new(agent_id).map_err(fleet_store::StoreError::from)?;
        match self {
            Self::Sqlite(store) => {
                SqliteStore::latest_agent_capability_snapshot(store, agent_id.as_str())
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.latest_agent_capability_snapshot(&agent_id))
            }
        }
    }

    fn save_policy_source(
        &self,
        policy_id: &str,
        name: &str,
        version: u32,
        source: &str,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.save_policy_source(policy_id, name, version, source),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self
                .with_postgres(|store| store.save_policy_source(policy_id, name, version, source)),
        }
    }

    fn list_policies(
        &self,
    ) -> Result<Vec<fleet_application::PolicyRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => Ok(store
                .list_policies()?
                .into_iter()
                .map(app_policy_record_from_store)
                .collect()),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| store.list_policies()),
        }
    }

    fn find_policy(
        &self,
        policy_id: &str,
    ) -> Result<Option<fleet_store::PolicyRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.find_policy(policy_id),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                Ok(store
                    .find_policy(policy_id)?
                    .map(store_policy_record_from_app))
            }),
        }
    }

    fn assign_policy_to_agent(
        &self,
        policy_id: &str,
        agent_id: &str,
        assigned_at: SystemTime,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.assign_policy_to_agent(policy_id, agent_id, assigned_at),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                store.assign_policy_to_agent(policy_id, agent_id, assigned_at)
            }),
        }
    }

    fn policies_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<fleet_application::PolicyAssignmentRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => Ok(store
                .policies_for_agent(agent_id)?
                .into_iter()
                .map(|record| fleet_application::PolicyAssignmentRecord {
                    policy_id: record.policy_id,
                    agent_id: record.agent_id,
                    assigned_at: record.assigned_at,
                })
                .collect()),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| store.policies_for_agent(agent_id)),
        }
    }

    fn assigned_policy_ids_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<String>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.assigned_policy_ids_for_agent(agent_id),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.assigned_policy_ids_for_agent(agent_id))
            }
        }
    }

    fn upsert_policy_schedule(
        &self,
        policy_id: &str,
        agent_id: &str,
        interval: Duration,
        next_due_at: SystemTime,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => {
                store.upsert_policy_schedule(policy_id, agent_id, interval, next_due_at)
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                store.upsert_policy_schedule(policy_id, agent_id, interval, next_due_at)
            }),
        }
    }

    fn due_scheduled_drift_checks(
        &self,
        now: SystemTime,
        limit: usize,
    ) -> Result<Vec<fleet_application::ScheduledDriftRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => Ok(store
                .due_scheduled_drift_checks(now, limit)?
                .into_iter()
                .map(|record| fleet_application::ScheduledDriftRecord {
                    policy_id: record.policy_id,
                    agent_id: record.agent_id,
                    interval_seconds: record.interval_seconds,
                    next_due_at: record.next_due_at,
                    last_checked_at: record.last_checked_at,
                })
                .collect()),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.due_scheduled_drift_checks(now, limit))
            }
        }
    }

    fn record_scheduled_drift_check(
        &self,
        policy_id: &str,
        agent_id: &str,
        checked_at: SystemTime,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => {
                store.record_scheduled_drift_check(policy_id, agent_id, checked_at)
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                store.record_scheduled_drift_check(policy_id, agent_id, checked_at)
            }),
        }
    }

    fn acknowledge_latest_drift_report(
        &self,
        agent_id: &str,
        policy_name: &str,
        actor: &str,
        acknowledged_at: SystemTime,
    ) -> Result<bool, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => {
                store.acknowledge_latest_drift_report(agent_id, policy_name, actor, acknowledged_at)
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                store.acknowledge_latest_drift_report(agent_id, policy_name, actor, acknowledged_at)
            }),
        }
    }

    fn mark_latest_drift_resolved(
        &self,
        agent_id: &str,
        policy_name: &str,
        job_id: &str,
        resolved_at: SystemTime,
    ) -> Result<bool, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => {
                store.mark_latest_drift_resolved(agent_id, policy_name, job_id, resolved_at)
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                store.mark_latest_drift_resolved(agent_id, policy_name, job_id, resolved_at)
            }),
        }
    }

    fn append_job_output_chunk_record(
        &self,
        chunk: &JobOutputChunk,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.append_job_output_chunk_record(chunk),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.append_job_output_chunk_record(chunk))
            }
        }
    }

    fn list_job_output_chunks(
        &self,
        job_id: &str,
        agent_id: &str,
    ) -> Result<Vec<JobOutputChunk>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.list_job_output_chunks(job_id, agent_id),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.list_output_chunks(job_id, agent_id))
            }
        }
    }

    fn list_job_output_chunks_for_job(
        &self,
        job_id: &str,
    ) -> Result<Vec<JobOutputChunk>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.list_job_output_chunks_for_job(job_id),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.list_output_chunks_for_job(job_id))
            }
        }
    }

    fn save_job_record(&self, job: &Job) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.save_job_record(job),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| store.save(job.clone())),
        }
    }

    fn save_task_assignment_record(
        &self,
        envelope: &TaskEnvelope,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.save_task_assignment_record(envelope),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.save_assignment(envelope.clone()))
            }
        }
    }

    fn save_command_job_record(
        &self,
        job: &Job,
        task: &fleet_domain::CommandTask,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.save_command_job_record(job, task),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.save_command_job(job.clone(), task))
            }
        }
    }

    fn save_command_job_with_assignments_record(
        &self,
        job: &Job,
        task: &fleet_domain::CommandTask,
        assignments: &[TaskEnvelope],
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => {
                store.save_command_job_with_assignments_record(job, task, assignments)
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                store.save_command_job_with_assignments(job.clone(), task, assignments)
            }),
        }
    }

    fn save_drift_check_job_record(
        &self,
        job: &Job,
        task: &fleet_domain::DriftCheckTask,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.save_drift_check_job_record(job, task),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                fleet_application::DriftCheckJobRepository::save_drift_check_job(
                    store,
                    job.clone(),
                    task,
                )
            }),
        }
    }

    fn save_drift_check_job_with_assignments_record(
        &self,
        job: &Job,
        task: &fleet_domain::DriftCheckTask,
        assignments: &[TaskEnvelope],
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => {
                store.save_drift_check_job_with_assignments_record(job, task, assignments)
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                fleet_application::DriftCheckJobRepository::save_drift_check_job_with_assignments(
                    store,
                    job.clone(),
                    task,
                    assignments,
                )
            }),
        }
    }

    fn save_runbook_job_record(
        &self,
        job: &Job,
        task: &fleet_domain::RunbookExecutionTask,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.save_runbook_job_record(job, task),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.save_runbook_job(job.clone(), task))
            }
        }
    }

    fn save_runbook_job_with_assignments_record(
        &self,
        job: &Job,
        task: &fleet_domain::RunbookExecutionTask,
        assignments: &[TaskEnvelope],
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => {
                store.save_runbook_job_with_assignments_record(job, task, assignments)
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                store.save_runbook_job_with_assignments(job.clone(), task, assignments)
            }),
        }
    }

    fn list_pending_dispatch_assignments(
        &self,
        agent_id: Option<&AgentId>,
        job_id: Option<&JobId>,
        limit: usize,
    ) -> Result<Vec<PendingTaskAssignment>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.list_pending_dispatch_assignments(agent_id, job_id, limit),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.list_pending_assignments(agent_id, job_id, limit))
            }
        }
    }

    fn job_dispatch_gate(
        &self,
        job_id: &str,
    ) -> Result<Option<fleet_store::JobDispatchGateRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.job_dispatch_gate(job_id),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                let job_id = JobId::new(job_id)
                    .map_err(|error| fleet_store::StoreError::Domain(error.to_string()))?;
                let gate = store.dispatch_gate(&job_id)?;
                Ok(Some(fleet_store::JobDispatchGateRecord {
                    concurrency: gate.concurrency as u32,
                    max_failures: gate.max_failures,
                    active_count: gate.active_count,
                    failure_count: gate.failure_count,
                }))
            }),
        }
    }

    fn find_task_assignment_job_id(
        &self,
        task_id: &str,
    ) -> Result<Option<String>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.find_task_assignment_job_id(task_id),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.find_task_assignment_job_id(task_id))
            }
        }
    }

    fn update_active_task_assignment_status(
        &self,
        task_id: &str,
        status: AssignmentStatus,
        occurred_at: SystemTime,
        last_error: Option<&str>,
    ) -> Result<bool, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => {
                store.update_active_task_assignment_status(task_id, status, occurred_at, last_error)
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                store.update_active_task_assignment_status(task_id, status, occurred_at, last_error)
            }),
        }
    }

    fn claim_task_assignment_for_dispatch(
        &self,
        task_id: &str,
        occurred_at: SystemTime,
    ) -> Result<bool, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.claim_task_assignment_for_dispatch(task_id, occurred_at),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                fleet_application::DispatchAssignmentRepository::claim_assignment_for_dispatch(
                    store,
                    &TaskId::new(task_id.to_owned())
                        .map_err(|error| fleet_store::StoreError::Domain(error.to_string()))?,
                    occurred_at,
                )
            }),
        }
    }

    fn release_task_assignment_dispatch_claim(
        &self,
        task_id: &str,
        _occurred_at: SystemTime,
        reason: &str,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => {
                store.release_task_assignment_dispatch_claim(task_id, reason)?;
                Ok(())
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                fleet_application::DispatchAssignmentRepository::release_assignment_dispatch_claim(
                    store,
                    &TaskId::new(task_id.to_owned())
                        .map_err(|error| fleet_store::StoreError::Domain(error.to_string()))?,
                    _occurred_at,
                    reason,
                )
            }),
        }
    }

    fn update_task_assignment_status(
        &self,
        task_id: &str,
        status: AssignmentStatus,
        occurred_at: SystemTime,
        last_error: Option<&str>,
    ) -> Result<bool, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => {
                store.update_task_assignment_status(task_id, status, occurred_at, last_error)
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                store.update_task_assignment_status(task_id, status, occurred_at, last_error)
            }),
        }
    }

    fn find_task_assignment_status(
        &self,
        task_id: &str,
    ) -> Result<Option<String>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.find_task_assignment_status(task_id),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.find_task_assignment_status(task_id))
            }
        }
    }

    fn list_task_assignment_summaries_for_job(
        &self,
        job_id: &str,
    ) -> Result<Vec<fleet_store::TaskAssignmentSummaryRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.list_task_assignment_summaries_for_job(job_id),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.list_task_assignment_summaries_for_job(job_id))
            }
        }
    }

    fn recompute_job_status_from_assignments(
        &self,
        job_id: &str,
    ) -> Result<Option<JobStatus>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.recompute_job_status_from_assignments(job_id),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.recompute_job_status_from_assignments(job_id))
            }
        }
    }

    fn cancel_queued_assignments_after_max_failures(
        &self,
        job_id: &str,
        occurred_at: SystemTime,
        reason: &str,
    ) -> Result<usize, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => {
                store.cancel_queued_assignments_after_max_failures(job_id, occurred_at, reason)
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                store.cancel_queued_assignments_after_max_failures(job_id, occurred_at, reason)
            }),
        }
    }

    fn find_job_status_value(
        &self,
        job_id: &str,
    ) -> Result<Option<String>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.find_job_status_value(job_id),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| store.find_job_status_value(job_id)),
        }
    }

    fn update_job_status(
        &self,
        job_id: &str,
        status: JobStatus,
    ) -> Result<bool, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.update_job_status(job_id, status),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.update_job_status(job_id, status))
            }
        }
    }

    fn update_job_strategy(
        &self,
        job_id: &str,
        concurrency: u32,
        max_failures: Option<u32>,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store
                .update_job_strategy(job_id, concurrency, max_failures)
                .map(|_| ()),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                store.update_job_strategy(job_id, concurrency, max_failures)
            }),
        }
    }

    fn update_job_selector_snapshot(
        &self,
        job_id: &str,
        selector_kind: &str,
        selector_source: &str,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store
                .update_job_selector_snapshot(job_id, selector_kind, selector_source)
                .map(|_| ()),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                store.update_job_selector_snapshot(job_id, selector_kind, selector_source)
            }),
        }
    }

    fn insert_approval_request(
        &self,
        request: AppApprovalRequestRecord,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.insert_approval_request(request),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| store.insert_approval_request(request)),
        }
    }

    fn find_approval_request(
        &self,
        approval_id: &str,
    ) -> Result<Option<AppApprovalRequestRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.find_approval_request(approval_id),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.find_approval_request(approval_id))
            }
        }
    }

    fn find_pending_approval_for_job(
        &self,
        job_id: &str,
    ) -> Result<Option<AppApprovalRequestRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.find_pending_approval_for_job(job_id),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.find_pending_approval_for_job(job_id))
            }
        }
    }

    fn list_approval_requests(
        &self,
        status: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AppApprovalRequestRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.list_approval_requests(status, limit),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.list_approval_requests(status, limit))
            }
        }
    }

    fn update_approval_request(
        &self,
        request: AppApprovalRequestRecord,
    ) -> Result<bool, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.update_approval_request(request),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| store.update_approval_request(request)),
        }
    }

    fn save_remediation_request_record(
        &self,
        request: &RemediationRequestRecord,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.save_remediation_request_record(request),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.save_remediation_request(request.clone()))
            }
        }
    }

    fn find_remediation_request_record(
        &self,
        request_id: &str,
    ) -> Result<Option<RemediationRequestRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.find_remediation_request_record(request_id),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.find_remediation_request(request_id))
            }
        }
    }

    fn list_remediation_request_records(
        &self,
        agent_id: Option<&str>,
        policy_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RemediationRequestRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => {
                store.list_remediation_request_records(agent_id, policy_id, limit)
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self
                .with_postgres(|store| store.list_remediation_requests(agent_id, policy_id, limit)),
        }
    }

    fn update_remediation_request_status_record(
        &self,
        request_id: &str,
        status: &str,
        job_id: Option<&str>,
        updated_at: SystemTime,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store
                .update_remediation_request_status_record(request_id, status, job_id, updated_at),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                store.update_remediation_request_status(request_id, status, job_id, updated_at)
            }),
        }
    }

    fn list_job_summaries(
        &self,
        limit: usize,
    ) -> Result<Vec<fleet_application::JobSummaryRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => Ok(store
                .list_job_summaries(limit)?
                .into_iter()
                .map(app_job_summary_from_store)
                .collect()),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| store.list_job_summaries(limit)),
        }
    }

    fn find_job_summary(
        &self,
        job_id: &str,
    ) -> Result<Option<fleet_application::JobSummaryRecord>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => Ok(store
                .find_job_summary(job_id)?
                .map(app_job_summary_from_store)),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| store.find_job_summary(job_id)),
        }
    }

    fn save_rendered_artifact_metadata_record(
        &self,
        metadata: &RenderedArtifactMetadata,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => store.save_rendered_artifact_metadata_record(metadata),
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => {
                self.with_postgres(|store| store.save_rendered_artifact_metadata_record(metadata))
            }
        }
    }

    fn list_rendered_artifacts_for_job(
        &self,
        job_id: &JobId,
    ) -> Result<Vec<RenderedArtifactMetadata>, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => {
                fleet_application::ArtifactMetadataRepository::list_rendered_artifacts_for_job(
                    *store, job_id,
                )
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                fleet_application::ArtifactMetadataRepository::list_rendered_artifacts_for_job(
                    store, job_id,
                )
            }),
        }
    }

    fn cleanup_retention(
        &self,
        cutoffs: fleet_application::RetentionCutoffs,
        dry_run: bool,
    ) -> Result<fleet_application::RetentionCleanupSummary, fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => {
                let mut repo = *store;
                fleet_application::RetentionRepository::cleanup_retention(
                    &mut repo, cutoffs, dry_run,
                )
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                fleet_application::RetentionRepository::cleanup_retention(store, cutoffs, dry_run)
            }),
        }
    }

    fn save_agent_certificate_lifecycle(
        &self,
        record: fleet_application::AgentCertificateLifecycleRecord,
    ) -> Result<(), fleet_store::StoreError> {
        match self {
            Self::Sqlite(store) => {
                let mut repo = *store;
                AgentCertificateLifecycleRepository::save_agent_certificate_lifecycle(
                    &mut repo, record,
                )
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                AgentCertificateLifecycleRepository::save_agent_certificate_lifecycle(store, record)
            }),
        }
    }

    fn load_agent_certificate_lifecycle(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<fleet_application::AgentCertificateLifecycleRecord>, fleet_store::StoreError>
    {
        match self {
            Self::Sqlite(store) => {
                AgentCertificateLifecycleRepository::load_agent_certificate_lifecycle(
                    *store, agent_id,
                )
            }
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => self.with_postgres(|store| {
                AgentCertificateLifecycleRepository::load_agent_certificate_lifecycle(
                    store, agent_id,
                )
            }),
        }
    }
}

fn app_enrollment_record_from_store(
    record: fleet_store::EnrollmentTokenRecord,
) -> fleet_application::EnrollmentTokenRecord {
    fleet_application::EnrollmentTokenRecord {
        id: record.id,
        default_labels: record.default_labels,
        expires_at: record.expires_at,
        max_uses: record.max_uses,
        used_count: record.used_count,
        revoked: record.revoked,
    }
}

#[allow(dead_code)]
fn app_policy_record_from_store(
    record: fleet_store::PolicyRecord,
) -> fleet_application::PolicyRecord {
    fleet_application::PolicyRecord {
        id: record.id,
        name: record.name,
        version: record.version,
        source: record.source,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

#[allow(dead_code)]
fn store_policy_record_from_app(
    record: fleet_application::PolicyRecord,
) -> fleet_store::PolicyRecord {
    fleet_store::PolicyRecord {
        id: record.id,
        name: record.name,
        version: record.version,
        source: record.source,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn app_job_summary_from_store(
    record: fleet_store::JobSummaryRecord,
) -> fleet_application::JobSummaryRecord {
    fleet_application::JobSummaryRecord {
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
            .map(|target| fleet_application::JobTargetSummaryRecord {
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
    }
}

#[derive(Debug, Clone, Default)]
struct ControllerRuntimeMetadata {
    external_url: Option<String>,
    tls_enabled: bool,
    controller_signing_public_key_path: Option<PathBuf>,
    controller_signing_private_key_path: Option<PathBuf>,
    tls_cert_path: Option<PathBuf>,
    tls_key_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdminRole {
    Owner,
    Admin,
    Operator,
    Viewer,
}

impl AdminRole {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "operator" => Some(Self::Operator),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }

    fn allows(self, permission: AdminPermission) -> bool {
        match self {
            Self::Owner | Self::Admin => true,
            Self::Operator => matches!(
                permission,
                AdminPermission::AgentRead
                    | AdminPermission::ApprovalRead
                    | AdminPermission::JobRead
                    | AdminPermission::JobCreate
                    | AdminPermission::JobApprove
                    | AdminPermission::JobCancel
                    | AdminPermission::AuditRead
                    | AdminPermission::PolicyRead
            ),
            Self::Viewer => matches!(
                permission,
                AdminPermission::AgentRead
                    | AdminPermission::ApprovalRead
                    | AdminPermission::JobRead
                    | AdminPermission::AuditRead
                    | AdminPermission::PolicyRead
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdminPermission {
    AgentRead,
    AgentWrite,
    AgentRevoke,
    ApprovalRead,
    JobRead,
    JobCreate,
    JobApprove,
    JobCancel,
    EnrollmentTokenRead,
    EnrollmentTokenCreate,
    EnrollmentTokenRevoke,
    AuditRead,
    PolicyRead,
    PolicyWrite,
    SigningRotationWrite,
}

impl AdminPermission {
    fn as_str(self) -> &'static str {
        match self {
            Self::AgentRead => "agent_read",
            Self::AgentWrite => "agent_write",
            Self::AgentRevoke => "agent_revoke",
            Self::ApprovalRead => "approval_read",
            Self::JobRead => "job_read",
            Self::JobCreate => "job_create",
            Self::JobApprove => "job_approve",
            Self::JobCancel => "job_cancel",
            Self::EnrollmentTokenRead => "enrollment_token_read",
            Self::EnrollmentTokenCreate => "enrollment_token_create",
            Self::EnrollmentTokenRevoke => "enrollment_token_revoke",
            Self::AuditRead => "audit_read",
            Self::PolicyRead => "policy_read",
            Self::PolicyWrite => "policy_write",
            Self::SigningRotationWrite => "signing_rotation_write",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AdminRequestContext {
    actor_id: String,
    role: AdminRole,
}

impl AdminRequestContext {
    fn allows(&self, permission: AdminPermission) -> bool {
        self.role.allows(permission)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSessionCloseReason {
    NormalShutdown,
    IdleTimeout,
    HeartbeatTimeout,
    HandlerEnded,
    ReplacedByNewSession,
    Revoked,
    AuthFailed,
    ProtocolError,
    WriteFailure,
    WriteQueueOverflow,
    StoreError,
}

impl AgentSessionCloseReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::NormalShutdown => "normal_shutdown",
            Self::IdleTimeout => "idle_timeout",
            Self::HeartbeatTimeout => "heartbeat_timeout",
            Self::HandlerEnded => "handler_ended",
            Self::ReplacedByNewSession => "replaced_by_new_session",
            Self::Revoked => "agent_revoked",
            Self::AuthFailed => "auth_failed",
            Self::ProtocolError => "protocol_error",
            Self::WriteFailure => "write_failure",
            Self::WriteQueueOverflow => "write_queue_overflow",
            Self::StoreError => "store_error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSessionOutboundMessage {
    Wire(Box<fleet_protocol::WireMessage>),
    Close { reason: AgentSessionCloseReason },
}

pub type AgentSessionOutboundSender = mpsc::Sender<AgentSessionOutboundMessage>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSessionSendError {
    NotConnected,
    QueueFull,
    Closed,
}

impl Display for AgentSessionSendError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConnected => write!(formatter, "agent session is not connected"),
            Self::QueueFull => write!(formatter, "agent session outbound queue is full"),
            Self::Closed => write!(formatter, "agent session outbound queue is closed"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentControllerSigningTrustAck {
    pub accepted: bool,
    pub current_fingerprint: Option<String>,
    pub entries_count: usize,
    pub reason_code: Option<String>,
    pub acknowledged_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCertificateLifecycleRuntimeAck {
    pub accepted: bool,
    pub state: fleet_protocol::AgentCertificateLifecycleStateWire,
    pub current_fingerprint: Option<String>,
    pub reason_code: Option<String>,
    pub acknowledged_at: SystemTime,
}

#[derive(Clone)]
pub struct AgentSessionHandle {
    agent_id: String,
    connection_id: String,
    connected_at: SystemTime,
    last_seen_at: SystemTime,
    capabilities: Vec<String>,
    outbound_sender: AgentSessionOutboundSender,
    queue_capacity: Option<usize>,
    controller_signing_trust_ack: Option<AgentControllerSigningTrustAck>,
    agent_certificate_lifecycle_ack: Option<AgentCertificateLifecycleRuntimeAck>,
}

impl AgentSessionHandle {
    pub fn new(
        agent_id: impl Into<String>,
        connection_id: impl Into<String>,
        connected_at: SystemTime,
        capabilities: Vec<String>,
        outbound_sender: AgentSessionOutboundSender,
        queue_capacity: Option<usize>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            connection_id: connection_id.into(),
            connected_at,
            last_seen_at: connected_at,
            capabilities,
            outbound_sender,
            queue_capacity,
            controller_signing_trust_ack: None,
            agent_certificate_lifecycle_ack: None,
        }
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    pub fn connection_id(&self) -> &str {
        &self.connection_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSessionSummary {
    pub agent_id: String,
    pub connected: bool,
    pub connection_id: String,
    pub connected_at_ms: u64,
    pub last_session_seen_at_ms: u64,
    pub capabilities: Vec<String>,
    pub queue_depth: usize,
    pub queue_capacity: Option<usize>,
    pub controller_signing_trust_accepted: Option<bool>,
    pub controller_signing_trust_current_fingerprint_prefix: Option<String>,
    pub controller_signing_trust_entries_count: Option<usize>,
    pub controller_signing_trust_reason_code: Option<String>,
    pub controller_signing_trust_acknowledged_at_ms: Option<u64>,
    pub agent_certificate_lifecycle_accepted: Option<bool>,
    pub agent_certificate_lifecycle_state: Option<String>,
    pub agent_certificate_lifecycle_current_fingerprint_prefix: Option<String>,
    pub agent_certificate_lifecycle_reason_code: Option<String>,
    pub agent_certificate_lifecycle_acknowledged_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSessionReplacement {
    pub agent_id: String,
    pub old_connection_id: String,
    pub new_connection_id: String,
    pub close_reason: AgentSessionCloseReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentSessionRegisterOutcome {
    pub replaced: Option<AgentSessionReplacement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSessionEnded {
    pub agent_id: String,
    pub connection_id: String,
    pub close_reason: AgentSessionCloseReason,
}

#[derive(Default)]
pub struct AgentSessionRegistry {
    sessions: BTreeMap<String, AgentSessionHandle>,
}

impl AgentSessionRegistry {
    pub fn register(&mut self, handle: AgentSessionHandle) -> AgentSessionRegisterOutcome {
        let replaced = self
            .sessions
            .insert(handle.agent_id.clone(), handle.clone());
        let replacement = replaced.map(|old| {
            let _ = old
                .outbound_sender
                .try_send(AgentSessionOutboundMessage::Close {
                    reason: AgentSessionCloseReason::ReplacedByNewSession,
                });
            AgentSessionReplacement {
                agent_id: handle.agent_id.clone(),
                old_connection_id: old.connection_id,
                new_connection_id: handle.connection_id.clone(),
                close_reason: AgentSessionCloseReason::ReplacedByNewSession,
            }
        });

        AgentSessionRegisterOutcome {
            replaced: replacement,
        }
    }

    pub fn get(&self, agent_id: &str) -> Option<AgentSessionHandle> {
        self.sessions.get(agent_id).cloned()
    }

    pub fn has_active_session(&self, agent_id: &str) -> bool {
        self.sessions.contains_key(agent_id)
    }

    pub fn try_send(
        &self,
        agent_id: &str,
        message: fleet_protocol::WireMessage,
    ) -> Result<(), AgentSessionSendError> {
        let Some(handle) = self.sessions.get(agent_id) else {
            return Err(AgentSessionSendError::NotConnected);
        };
        handle
            .outbound_sender
            .try_send(AgentSessionOutboundMessage::Wire(Box::new(message)))
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => AgentSessionSendError::QueueFull,
                mpsc::error::TrySendError::Closed(_) => AgentSessionSendError::Closed,
            })
    }

    pub fn mark_seen(&mut self, agent_id: &str, connection_id: &str, seen_at: SystemTime) -> bool {
        let Some(handle) = self.sessions.get_mut(agent_id) else {
            return false;
        };
        if handle.connection_id != connection_id {
            return false;
        }
        handle.last_seen_at = seen_at;
        true
    }

    pub fn record_controller_signing_trust_ack(
        &mut self,
        agent_id: &str,
        connection_id: &str,
        ack: AgentControllerSigningTrustAck,
    ) -> bool {
        let Some(handle) = self.sessions.get_mut(agent_id) else {
            return false;
        };
        if handle.connection_id != connection_id {
            return false;
        }
        handle.controller_signing_trust_ack = Some(ack);
        true
    }

    pub fn record_agent_certificate_lifecycle_ack(
        &mut self,
        agent_id: &str,
        connection_id: &str,
        ack: AgentCertificateLifecycleRuntimeAck,
    ) -> bool {
        let Some(handle) = self.sessions.get_mut(agent_id) else {
            return false;
        };
        if handle.connection_id != connection_id {
            return false;
        }
        handle.agent_certificate_lifecycle_ack = Some(ack);
        true
    }

    pub fn controller_signing_trust_is_current(
        &self,
        agent_id: &str,
        current_fingerprint: &str,
    ) -> bool {
        self.sessions
            .get(agent_id)
            .and_then(|handle| handle.controller_signing_trust_ack.as_ref())
            .map(|ack| {
                ack.accepted && ack.current_fingerprint.as_deref() == Some(current_fingerprint)
            })
            .unwrap_or(false)
    }

    pub fn controller_signing_staged_rollout_targets(
        &self,
        target_ids: &[String],
        current_fingerprint: &str,
    ) -> Vec<fleet_domain::ControllerSigningStagedRolloutTarget> {
        target_ids
            .iter()
            .map(|agent_id| {
                let (connected, accepted_current, acknowledged_at) = self
                    .sessions
                    .get(agent_id)
                    .map(|handle| {
                        let ack = handle.controller_signing_trust_ack.as_ref();
                        (
                            true,
                            ack.map(|ack| {
                                ack.accepted
                                    && ack.current_fingerprint.as_deref()
                                        == Some(current_fingerprint)
                            })
                            .unwrap_or(false),
                            ack.map(|ack| ack.acknowledged_at),
                        )
                    })
                    .unwrap_or((false, false, None));
                fleet_domain::ControllerSigningStagedRolloutTarget::observed(
                    agent_id.clone(),
                    connected,
                    accepted_current,
                    acknowledged_at,
                )
            })
            .collect()
    }

    pub fn unregister(
        &mut self,
        agent_id: &str,
        connection_id: &str,
        close_reason: AgentSessionCloseReason,
    ) -> Option<AgentSessionEnded> {
        let should_remove = self
            .sessions
            .get(agent_id)
            .map(|handle| handle.connection_id == connection_id)
            .unwrap_or(false);
        if !should_remove {
            return None;
        }
        let removed = self.sessions.remove(agent_id)?;
        Some(AgentSessionEnded {
            agent_id: removed.agent_id,
            connection_id: removed.connection_id,
            close_reason,
        })
    }

    pub fn close(
        &mut self,
        agent_id: &str,
        close_reason: AgentSessionCloseReason,
    ) -> Option<AgentSessionEnded> {
        let removed = self.sessions.remove(agent_id)?;
        let _ = removed
            .outbound_sender
            .try_send(AgentSessionOutboundMessage::Close {
                reason: close_reason,
            });
        Some(AgentSessionEnded {
            agent_id: removed.agent_id,
            connection_id: removed.connection_id,
            close_reason,
        })
    }

    pub fn snapshot(&self) -> Vec<AgentSessionSummary> {
        self.sessions
            .values()
            .map(AgentSessionSummary::from_handle)
            .collect()
    }
}

impl AgentSessionSummary {
    fn from_handle(handle: &AgentSessionHandle) -> Self {
        let queue_capacity = handle
            .queue_capacity
            .unwrap_or_else(|| handle.outbound_sender.max_capacity());
        let queue_depth = queue_capacity.saturating_sub(handle.outbound_sender.capacity());
        let trust_ack = handle.controller_signing_trust_ack.as_ref();
        let certificate_ack = handle.agent_certificate_lifecycle_ack.as_ref();
        Self {
            agent_id: handle.agent_id.clone(),
            connected: true,
            connection_id: handle.connection_id.clone(),
            connected_at_ms: system_time_to_millis(handle.connected_at),
            last_session_seen_at_ms: system_time_to_millis(handle.last_seen_at),
            capabilities: handle.capabilities.clone(),
            queue_depth,
            queue_capacity: Some(queue_capacity),
            controller_signing_trust_accepted: trust_ack.map(|ack| ack.accepted),
            controller_signing_trust_current_fingerprint_prefix: trust_ack
                .and_then(|ack| ack.current_fingerprint.as_deref())
                .map(controller_signing_fingerprint_prefix)
                .map(str::to_owned),
            controller_signing_trust_entries_count: trust_ack.map(|ack| ack.entries_count),
            controller_signing_trust_reason_code: trust_ack.and_then(|ack| ack.reason_code.clone()),
            controller_signing_trust_acknowledged_at_ms: trust_ack
                .map(|ack| system_time_to_millis(ack.acknowledged_at)),
            agent_certificate_lifecycle_accepted: certificate_ack.map(|ack| ack.accepted),
            agent_certificate_lifecycle_state: certificate_ack
                .map(|ack| agent_certificate_lifecycle_state_wire_as_str(ack.state).to_owned()),
            agent_certificate_lifecycle_current_fingerprint_prefix: certificate_ack
                .and_then(|ack| ack.current_fingerprint.as_deref())
                .map(controller_signing_fingerprint_prefix)
                .map(str::to_owned),
            agent_certificate_lifecycle_reason_code: certificate_ack
                .and_then(|ack| ack.reason_code.clone()),
            agent_certificate_lifecycle_acknowledged_at_ms: certificate_ack
                .map(|ack| system_time_to_millis(ack.acknowledged_at)),
        }
    }
}

struct RegisteredAgentSessionGuard {
    sessions: Arc<Mutex<AgentSessionRegistry>>,
    agent_id: String,
    connection_id: String,
}

impl RegisteredAgentSessionGuard {
    fn new(
        sessions: Arc<Mutex<AgentSessionRegistry>>,
        agent_id: String,
        connection_id: String,
    ) -> Self {
        Self {
            sessions,
            agent_id,
            connection_id,
        }
    }
}

impl Drop for RegisteredAgentSessionGuard {
    fn drop(&mut self) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.unregister(
                &self.agent_id,
                &self.connection_id,
                AgentSessionCloseReason::HandlerEnded,
            );
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerIdentity {
    pub public_key: String,
    pub fingerprint: String,
    private_key: String,
}

impl ControllerIdentity {
    #[cfg(test)]
    fn dev_insecure() -> Self {
        Self {
            public_key: "dev-controller-public-key".to_owned(),
            fingerprint: "dev-controller-fingerprint".to_owned(),
            private_key: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollAgentRequest {
    pub token: String,
    pub agent_id: String,
    pub name: String,
    pub public_key: String,
    pub fingerprint: String,
    pub labels: Vec<EnrollAgentLabel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollAgentLabel {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollAgentResponse {
    pub agent_id: String,
    pub controller_public_key: String,
    pub controller_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerIdentityResponse {
    pub controller_public_key: String,
    pub controller_fingerprint: String,
    #[serde(default)]
    pub controller_signing_public_key: String,
    #[serde(default)]
    pub controller_signing_fingerprint: String,
    #[serde(default)]
    pub tls_endpoint: ControllerTlsEndpointResponse,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ControllerTlsEndpointResponse {
    pub external_url: Option<String>,
    pub tls_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerSigningRotationStatusResponse {
    pub controller_id: String,
    pub persisted_record_present: bool,
    pub persisted_state: String,
    pub readiness: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerSigningRotationRestartPlanResponse {
    pub controller_id: String,
    pub restart_required: bool,
    pub reload_supported: bool,
    pub recommended_action: String,
    pub readiness: String,
    pub bootstrap_guard: String,
    pub agent_trust_rollout: String,
    pub active_signing_fingerprint_prefix: String,
    pub selected_signing_fingerprint_prefix: String,
    pub blocked_reason: Option<String>,
    pub verification_commands: Vec<String>,
    pub safety_notes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSigningRotationRestartActionBody {
    #[serde(default)]
    pub confirm_external_restart: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerSigningRotationRestartActionResponse {
    pub controller_id: String,
    pub action: String,
    pub action_status: String,
    pub restart_required: bool,
    pub reload_supported: bool,
    pub readiness: String,
    pub bootstrap_guard: String,
    pub active_signing_fingerprint_prefix: String,
    pub selected_signing_fingerprint_prefix: String,
    pub service_command: String,
    pub verification_commands: Vec<String>,
    pub safety_notes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSigningRotationRequestBody {
    pub new_fingerprint: String,
    #[serde(default)]
    pub old_key_verifies_for_seconds: Option<u64>,
    #[serde(default)]
    pub old_key_verifies_until_ms: Option<u64>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSigningRotationValidateBody {
    pub candidate_public_key_path: PathBuf,
    pub candidate_private_key_path: PathBuf,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSigningRotationReasonBody {
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSigningTrustBundleRolloutBody {
    #[serde(default)]
    pub previous_public_key_path: Option<PathBuf>,
    #[serde(default)]
    pub agent_ids: Vec<String>,
    #[serde(default)]
    pub max_agent_count: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSigningTrustBundleStagedRolloutBody {
    #[serde(default)]
    pub previous_public_key_path: Option<PathBuf>,
    #[serde(default)]
    pub agent_ids: Vec<String>,
    pub batch_size: usize,
    pub max_failures: usize,
    pub ack_timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerSigningTrustBundleRolloutAgentResult {
    pub agent_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerSigningTrustBundleRolloutResponse {
    pub controller_id: String,
    pub persisted_state: String,
    pub attempted_count: usize,
    pub updated_count: usize,
    pub skipped_count: usize,
    pub failed_count: usize,
    pub entries_count: usize,
    pub current_fingerprint_prefix: String,
    pub previous_fingerprint_prefix: Option<String>,
    pub agent_results: Vec<ControllerSigningTrustBundleRolloutAgentResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerSigningTrustBundleStagedRolloutResponse {
    pub controller_id: String,
    pub persisted_state: String,
    pub rollout_state: String,
    pub target_count: usize,
    pub planned_count: usize,
    pub attempted_count: usize,
    pub updated_count: usize,
    pub skipped_count: usize,
    pub failed_count: usize,
    pub already_current_count: usize,
    pub unavailable_count: usize,
    pub pending_count: usize,
    pub entries_count: usize,
    pub current_fingerprint_prefix: String,
    pub previous_fingerprint_prefix: Option<String>,
    pub agent_results: Vec<ControllerSigningTrustBundleRolloutAgentResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct AgentCertificateLifecycleDispatchResult {
    pub(crate) agent_id: String,
    pub(crate) status: AgentCertificateLifecycleDispatchStatus,
    pub(crate) action: fleet_protocol::AgentCertificateLifecycleActionWire,
    pub(crate) state: fleet_protocol::AgentCertificateLifecycleStateWire,
    pub(crate) current_fingerprint_prefix: Option<String>,
    pub(crate) next_fingerprint_prefix: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum AgentCertificateLifecycleDispatchStatus {
    Sent,
    NotConnected,
    QueueFull,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentCertificateIssuanceRequestResponse {
    agent_id: String,
    action: String,
    lifecycle_state: String,
    dispatch_status: String,
    current_fingerprint_prefix: Option<String>,
    next_fingerprint_prefix: Option<String>,
    audit_event_action: String,
    updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentCertificateLifecycleStatusResponse {
    agent_id: String,
    record_present: bool,
    lifecycle_state: String,
    current_fingerprint_prefix: Option<String>,
    next_fingerprint_prefix: Option<String>,
    grace_until_ms: Option<u64>,
    revocation_reason: Option<String>,
    updated_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEnrollmentTokenResponse {
    pub id: String,
    pub token: String,
    pub expires_in_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEnrollmentTokenRequest {
    #[serde(default, alias = "labels")]
    pub default_labels: String,
    #[serde(default = "default_enrollment_token_max_uses")]
    pub max_uses: u32,
    #[serde(default = "default_enrollment_token_expires_in_seconds")]
    pub expires_in_seconds: u64,
}

impl Default for CreateEnrollmentTokenRequest {
    fn default() -> Self {
        Self {
            default_labels: String::new(),
            max_uses: default_enrollment_token_max_uses(),
            expires_in_seconds: default_enrollment_token_expires_in_seconds(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentTokenSummaryResponse {
    pub id: String,
    pub default_labels: String,
    pub max_uses: u32,
    pub used_count: u32,
    pub remaining_uses: u32,
    pub revoked: bool,
    pub expires_at_epoch: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCommandJobRequest {
    pub job_id: String,
    pub target_agent_ids: Vec<String>,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default, rename = "matchLabels", alias = "match_labels")]
    pub match_labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub strategy: Option<JobStrategyRequest>,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub timeout_seconds: u64,
    pub confirmed_high_risk: bool,
    #[serde(default = "default_confirmed_by")]
    pub confirmed_by: String,
    #[serde(default = "default_job_expiration_seconds")]
    pub expires_in_seconds: u64,
    #[serde(default)]
    pub nonce_prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCommandJobResponse {
    pub job_id: String,
    pub target_count: usize,
    pub assignment_count: usize,
    pub status: String,
    pub approval_request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDriftCheckJobRequest {
    pub job_id: String,
    pub target_agent_ids: Vec<String>,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default, rename = "matchLabels", alias = "match_labels")]
    pub match_labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub strategy: Option<JobStrategyRequest>,
    pub policy_document: String,
    #[serde(default = "default_drift_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default = "default_confirmed_by")]
    pub created_by: String,
    #[serde(default = "default_job_expiration_seconds")]
    pub expires_in_seconds: u64,
    #[serde(default)]
    pub nonce_prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDriftCheckJobResponse {
    pub job_id: String,
    pub target_count: usize,
    pub assignment_count: usize,
    pub status: String,
    pub approval_request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRunbookJobRequest {
    pub job_id: String,
    pub target_agent_ids: Vec<String>,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default, rename = "matchLabels", alias = "match_labels")]
    pub match_labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub strategy: Option<JobStrategyRequest>,
    pub runbook_document: String,
    #[serde(default = "default_drift_timeout_seconds")]
    pub timeout_seconds: u64,
    pub confirmed_high_risk: bool,
    #[serde(default = "default_confirmed_by")]
    pub confirmed_by: String,
    #[serde(default = "default_job_expiration_seconds")]
    pub expires_in_seconds: u64,
    #[serde(default)]
    pub nonce_prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRunbookJobResponse {
    pub job_id: String,
    pub target_count: usize,
    pub assignment_count: usize,
    pub status: String,
    pub approval_request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequestResponse {
    pub id: String,
    pub job_id: String,
    pub requester: String,
    pub approver: Option<String>,
    pub reason: String,
    pub status: String,
    pub expires_at_ms: u64,
    pub created_at_ms: u64,
    pub decided_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalDecisionRequest {
    #[serde(default)]
    pub actor: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpireApprovalsResponse {
    pub expired_count: usize,
    pub approvals: Vec<ApprovalRequestResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemediationRequestResponse {
    pub id: String,
    pub policy_id: String,
    pub policy_name: String,
    pub agent_id: String,
    pub runbook_ref: String,
    pub status: String,
    pub approval_required: bool,
    pub risk_summary: String,
    pub job_id: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRemediationApprovalRequest {
    #[serde(default)]
    pub approval_id: Option<String>,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub reason: String,
    #[serde(default = "default_job_expiration_seconds")]
    pub expires_in_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRemediationApprovalResponse {
    pub remediation: RemediationRequestResponse,
    pub approval: ApprovalRequestResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproveRemediationJobRequest {
    pub approval_id: String,
    pub job_id: String,
    pub runbook_document: String,
    #[serde(default = "default_drift_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default = "default_job_expiration_seconds")]
    pub expires_in_seconds: u64,
    #[serde(default)]
    pub nonce_prefix: Option<String>,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproveRemediationJobResponse {
    pub remediation: RemediationRequestResponse,
    pub approval: ApprovalRequestResponse,
    pub job_id: String,
    pub assignment_count: usize,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemediationJobRunningRequest {
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemediationJobResultRequest {
    pub job_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemediationVerifyRequest {
    pub agent_id: String,
    pub policy_id: String,
    pub policy_name: String,
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStrategyRequest {
    #[serde(default)]
    pub concurrency: Option<u32>,
    #[serde(default, rename = "maxFailures", alias = "max_failures")]
    pub max_failures: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct JobStrategyConfig {
    concurrency: u32,
    max_failures: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectorPreviewRequest {
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default, rename = "matchLabels", alias = "match_labels")]
    pub match_labels: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectorPreviewWarningResponse {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectorPreviewAgentResponse {
    pub agent_id: String,
    pub name: String,
    pub status: String,
    pub labels: Vec<AgentLabelResponse>,
    pub selected_for_dispatch: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectorPreviewResponse {
    pub matched_count: usize,
    pub selected_count: usize,
    pub disabled_count: usize,
    pub offline_count: usize,
    pub warnings: Vec<SelectorPreviewWarningResponse>,
    pub agents: Vec<SelectorPreviewAgentResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelJobRequest {
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelJobResponse {
    pub job_id: String,
    pub status: String,
    pub task_id: Option<String>,
    pub agent_id: Option<String>,
    pub assignment_status: Option<String>,
    pub canceled_count: usize,
    pub cancel_delivered_count: usize,
    pub cancel_delivered: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSummaryResponse {
    pub id: String,
    pub status: String,
    pub dispatch_state: String,
    pub risk: String,
    pub command_program: Option<String>,
    pub command_args: Vec<String>,
    pub selector_kind: String,
    pub selector_source: String,
    pub strategy: JobStrategyResponse,
    pub target_count: usize,
    pub target_agent_ids: Vec<String>,
    pub target_agents: Vec<JobTargetSummaryResponse>,
    pub assignment_summary: JobAssignmentSummaryResponse,
    pub rendered_artifacts: Vec<RenderedArtifactMetadataResponse>,
    pub target_connected: bool,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub expires_at_ms: Option<u64>,
    pub last_error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderedArtifactMetadataResponse {
    pub artifact_id: String,
    pub task_id: String,
    pub agent_id: String,
    pub retention_class: String,
    pub checksum_sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobTargetSummaryResponse {
    pub agent_id: String,
    pub name: String,
    pub status: String,
    pub snapshot_status: String,
    pub labels: Vec<AgentLabelResponse>,
    pub task_id: Option<String>,
    pub assignment_status: Option<String>,
    pub last_error: String,
    pub connected: bool,
    pub revoked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobAssignmentSummaryResponse {
    pub queued: usize,
    pub dispatched: usize,
    pub accepted: usize,
    pub started: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub rejected: usize,
    pub canceled: usize,
    pub expired: usize,
    pub skipped: usize,
    pub unknown: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStrategyResponse {
    pub concurrency: u32,
    #[serde(rename = "maxFailures")]
    pub max_failures: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobOutputChunkResponse {
    pub job_id: String,
    pub agent_id: String,
    pub stream: String,
    pub sequence: u64,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobArtifactBodyResponse {
    pub job_id: String,
    pub artifact_id: String,
    pub task_id: String,
    pub agent_id: String,
    pub retention_class: String,
    pub checksum_sha256: String,
    pub size_bytes: u64,
    pub content_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentLabelResponse {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    pub id: String,
    pub name: String,
    pub status: String,
    pub connected: bool,
    pub revoked: bool,
    pub fingerprint: String,
    pub labels: Vec<AgentLabelResponse>,
    pub assigned_policy_ids: Vec<String>,
    pub capabilities: Vec<String>,
    pub capability_reported_at_ms: Option<u64>,
    pub last_seen_at_ms: Option<u64>,
    pub last_seen_age_seconds: Option<u64>,
    pub hostname: Option<String>,
    pub os: Option<String>,
    pub arch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateAgentLabelsRequest {
    pub labels: Vec<AgentLabelResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavePolicyRequest {
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyResponse {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub source: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignPolicyRequest {
    pub agent_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyAssignmentResponse {
    pub policy_id: String,
    pub agent_id: String,
    pub assigned_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulePolicyDriftRequest {
    pub agent_id: String,
    pub interval_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledDriftResponse {
    pub policy_id: String,
    pub agent_id: String,
    pub interval_seconds: u64,
    pub next_due_at_ms: u64,
    pub last_checked_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestFactsResponse {
    pub agent_id: String,
    pub collected_at_ms: u64,
    pub agent_system_time_ms: u64,
    pub body: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactsSnapshotItemResponse {
    pub agent_id: String,
    pub collected_at_ms: u64,
    pub agent_system_time_ms: u64,
    pub body: serde_json::Value,
    pub cursor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactsSnapshotPageResponse {
    pub items: Vec<FactsSnapshotItemResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestMetricsResponse {
    pub agent_id: String,
    pub collected_at_ms: u64,
    pub agent_system_time_ms: u64,
    pub body: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshotItemResponse {
    pub agent_id: String,
    pub collected_at_ms: u64,
    pub agent_system_time_ms: u64,
    pub body: serde_json::Value,
    pub cursor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshotPageResponse {
    pub items: Vec<MetricsSnapshotItemResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentLogChunkItemResponse {
    pub agent_id: String,
    pub collected_at_ms: u64,
    pub line: String,
    pub cursor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentLogChunkPageResponse {
    pub items: Vec<AgentLogChunkItemResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestDriftReportResponse {
    pub agent_id: String,
    pub checked_at_ms: u64,
    pub agent_system_time_ms: u64,
    pub policy_name: String,
    pub status: String,
    pub severity: String,
    pub acknowledged: bool,
    pub acknowledged_by: Option<String>,
    pub acknowledged_at_ms: Option<u64>,
    pub resolved: bool,
    pub resolution_job_id: Option<String>,
    pub resolved_at_ms: Option<u64>,
    pub expected: String,
    pub actual: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftReportItemResponse {
    pub agent_id: String,
    pub checked_at_ms: u64,
    pub agent_system_time_ms: u64,
    pub policy_name: String,
    pub status: String,
    pub severity: String,
    pub acknowledged: bool,
    pub acknowledged_by: Option<String>,
    pub acknowledged_at_ms: Option<u64>,
    pub resolved: bool,
    pub resolution_job_id: Option<String>,
    pub resolved_at_ms: Option<u64>,
    pub expected: String,
    pub actual: String,
    pub cursor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftReportPageResponse {
    pub items: Vec<DriftReportItemResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEventResponse {
    pub category: String,
    pub action: String,
    pub actor: String,
    pub target: String,
    pub value_kind: String,
    pub value: String,
    pub occurred_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditExportEventResponse {
    pub category: String,
    pub action: String,
    pub actor: String,
    pub target: String,
    pub value_kind: String,
    pub value: String,
    pub occurred_at_ms: u64,
    pub cursor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditExportPageResponse {
    pub items: Vec<AuditExportEventResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug)]
pub enum ControllerError {
    Io(std::io::Error),
    Store(fleet_store::StoreError),
    Protocol(fleet_protocol::ProtocolError),
    Json(String),
    Tls(String),
    SigningKeyRotation(String),
    SecretProvider(String),
    UnsupportedDatabaseBackend(String),
}

impl Display for ControllerError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Store(error) => write!(formatter, "store error: {error:?}"),
            Self::Protocol(error) => write!(formatter, "{error}"),
            Self::Json(error) => write!(formatter, "json error: {error}"),
            Self::Tls(error) => write!(formatter, "tls error: {error}"),
            Self::SigningKeyRotation(error) => {
                write!(formatter, "controller signing key rotation error: {error}")
            }
            Self::SecretProvider(error) => write!(formatter, "secret provider error: {error}"),
            Self::UnsupportedDatabaseBackend(backend) => write!(
                formatter,
                "database backend is recognized but not implemented: {backend}"
            ),
        }
    }
}

impl std::error::Error for ControllerError {}

impl From<std::io::Error> for ControllerError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<fleet_store::StoreError> for ControllerError {
    fn from(value: fleet_store::StoreError) -> Self {
        Self::Store(value)
    }
}

impl From<fleet_protocol::ProtocolError> for ControllerError {
    fn from(value: fleet_protocol::ProtocolError) -> Self {
        Self::Protocol(value)
    }
}

pub fn start_controller_server(config: ControllerServerConfig) -> Result<(), ControllerError> {
    start_controller_server_until(config, || false)
}

fn controller_database_settings(
    config: &ControllerServerConfig,
) -> Result<DatabaseSettings, ControllerError> {
    match &config.database {
        Some(database) => Ok(database.clone()),
        None => DatabaseSettings::sqlite(config.data_dir.join("controller").join("fleet.db"))
            .map_err(|error| ControllerError::Json(error.to_string())),
    }
}

fn open_controller_store(database: &DatabaseSettings) -> Result<ControllerStore, ControllerError> {
    match database.backend() {
        DatabaseBackend::Sqlite { path } => {
            let store = ControllerStore::sqlite(SqliteStore::open(path)?);
            tracing::info!(
                database_backend = store.backend_name(),
                database_path = %path.display(),
                "database_connected"
            );
            Ok(store)
        }
        #[cfg(feature = "postgres")]
        DatabaseBackend::Postgres { settings } => {
            let store_settings = PostgresStoreConnectSettings::with_pool_settings(
                settings.url(),
                postgres_store_ssl_mode(settings.ssl_mode()),
                settings.connect_timeout(),
                settings.pool_max_connections(),
                settings.pool_checkout_timeout(),
            )?;
            let mut store = PostgresStore::connect_with_settings(&store_settings)?;
            store.migrate()?;
            let store = ControllerStore::postgres(store);
            tracing::info!(
                database_backend = store.backend_name(),
                postgres_ssl_mode = postgres_ssl_mode_label(settings.ssl_mode()),
                "database_connected"
            );
            Ok(store)
        }
        #[cfg(not(feature = "postgres"))]
        DatabaseBackend::Postgres { .. } => Err(ControllerError::UnsupportedDatabaseBackend(
            "postgres".to_owned(),
        )),
    }
}

fn controller_artifact_store_settings(
    config: &ControllerServerConfig,
) -> Result<ArtifactStoreSettings, ControllerError> {
    ArtifactStoreSettings::default_local(&config.data_dir)
        .map_err(|error| ControllerError::Json(error.to_string()))
}

fn controller_secret_provider_settings(config: &ControllerServerConfig) -> SecretProviderSettings {
    config.secret_provider.clone().unwrap_or_default()
}

fn controller_trust_settings(
    config: &ControllerServerConfig,
) -> Result<ControllerTrustSettings, ControllerError> {
    let agent_client_certificate = controller_agent_client_certificate_trust(config)?;
    ControllerTrustSettings::from_parts(
        config.tls_cert_path.clone(),
        config.tls_key_path.clone(),
        config
            .data_dir
            .join("controller")
            .join("controller_public.key"),
        config
            .data_dir
            .join("controller")
            .join("controller_private.key"),
        agent_client_certificate,
    )
    .map_err(|error| ControllerError::Tls(error.to_string()))
}

fn controller_agent_client_certificate_trust(
    config: &ControllerServerConfig,
) -> Result<AgentClientCertificateTrust, ControllerError> {
    match &config.agent_client_ca_cert_path {
        Some(path) => AgentClientCertificateTrust::required(path.clone())
            .map_err(|error| ControllerError::Tls(error.to_string())),
        None => Ok(AgentClientCertificateTrust::disabled()),
    }
}

fn ensure_agent_client_certificate_mtls_supported(
    trust: &AgentClientCertificateTrust,
) -> Result<(), ControllerError> {
    if matches!(trust, AgentClientCertificateTrust::Required { .. }) {
        return Err(ControllerError::Tls(
            AGENT_CLIENT_CERTIFICATE_MTLS_UNSUPPORTED.to_owned(),
        ));
    }
    Ok(())
}

fn build_controller_secret_provider(
    settings: &SecretProviderSettings,
    static_test_provider: Option<StaticSecretProvider>,
) -> Result<ControllerSecretProvider, ControllerSecretProviderConstructionError> {
    match settings.backend() {
        SecretProviderBackend::Disabled => {
            if static_test_provider.is_some() {
                return Err(
                    ControllerSecretProviderConstructionError::StaticTestFixtureSourceRequired,
                );
            }
            Ok(ControllerSecretProvider::Disabled(DisabledSecretProvider))
        }
        SecretProviderBackend::StaticTest { .. } => static_test_provider
            .map(ControllerSecretProvider::StaticTest)
            .ok_or(ControllerSecretProviderConstructionError::StaticTestFixtureSourceRequired),
    }
}

fn open_controller_artifact_store(
    settings: &ArtifactStoreSettings,
) -> Result<LocalArtifactStore, ControllerError> {
    match settings.backend() {
        ArtifactStoreBackend::Local { root } => {
            LocalArtifactStore::new(root).map_err(ControllerError::Store)
        }
    }
}

#[cfg(feature = "postgres")]
fn postgres_store_ssl_mode(mode: fleet_core::PostgresSslMode) -> PostgresStoreSslMode {
    match mode {
        fleet_core::PostgresSslMode::Disable => PostgresStoreSslMode::Disable,
        fleet_core::PostgresSslMode::Prefer => PostgresStoreSslMode::Prefer,
        fleet_core::PostgresSslMode::Require => PostgresStoreSslMode::Require,
    }
}

#[cfg(feature = "postgres")]
fn postgres_ssl_mode_label(mode: fleet_core::PostgresSslMode) -> &'static str {
    match mode {
        fleet_core::PostgresSslMode::Disable => "disable",
        fleet_core::PostgresSslMode::Prefer => "prefer",
        fleet_core::PostgresSslMode::Require => "require",
    }
}

pub fn start_controller_server_until<F>(
    config: ControllerServerConfig,
    should_shutdown: F,
) -> Result<(), ControllerError>
where
    F: Fn() -> bool + Send + Sync + 'static,
{
    validate_transport(&config)?;
    let trust_settings = controller_trust_settings(&config)?;
    let database = controller_database_settings(&config)?;
    let store = open_controller_store(&database)?;
    let artifact_settings = controller_artifact_store_settings(&config)?;
    let artifact_store = open_controller_artifact_store(&artifact_settings)?;
    let secret_provider_settings = controller_secret_provider_settings(&config);
    let secret_provider = build_controller_secret_provider(&secret_provider_settings, None)
        .map_err(|error| ControllerError::SecretProvider(error.to_string()))?;
    let identity =
        load_controller_signing_runtime_identity(&config.data_dir, &store, SystemTime::now())?;
    tracing::info!(
        secret_provider_mode = secret_provider.mode(),
        "secret_provider_configured"
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run_axum_controller_server(
        config,
        trust_settings,
        store,
        artifact_store,
        identity,
        should_shutdown,
    ))
}

async fn run_axum_controller_server<F>(
    config: ControllerServerConfig,
    trust_settings: ControllerTrustSettings,
    store: ControllerStore,
    artifact_store: LocalArtifactStore,
    identity: ControllerIdentity,
    should_shutdown: F,
) -> Result<(), ControllerError>
where
    F: Fn() -> bool + Send + Sync + 'static,
{
    ensure_agent_client_certificate_mtls_supported(trust_settings.agent_client_certificate())?;
    let bind_addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .map_err(|error| controller_bind_error(&bind_addr, error))?;
    let tls_acceptor = trust_settings
        .tls_server()
        .map(|settings| build_tls_acceptor(settings.cert_path(), settings.key_path()))
        .transpose()?;
    let insecure_http_target = insecure_http_transport_target(&config);
    if let Some(target) = &insecure_http_target {
        audit_insecure_http_transport_enabled(&store, target)?;
    }
    announce_controller_started(&config, &identity, insecure_http_target.as_deref());

    let state = ControllerAppState {
        store: Arc::new(Mutex::new(store)),
        artifact_store: Arc::new(Mutex::new(artifact_store)),
        identity: Arc::new(identity),
        metadata: Arc::new(ControllerRuntimeMetadata {
            external_url: config.external_url.clone(),
            tls_enabled: trust_settings.tls_server().is_some(),
            controller_signing_public_key_path: Some(
                trust_settings
                    .controller_signing()
                    .public_key_path()
                    .to_path_buf(),
            ),
            controller_signing_private_key_path: Some(
                trust_settings
                    .controller_signing()
                    .private_key_path()
                    .to_path_buf(),
            ),
            tls_cert_path: trust_settings
                .tls_server()
                .map(|settings| settings.cert_path().to_path_buf()),
            tls_key_path: trust_settings
                .tls_server()
                .map(|settings| settings.key_path().to_path_buf()),
        }),
        sessions: Arc::new(Mutex::new(AgentSessionRegistry::default())),
    };
    start_scheduled_drift_worker(state.store.clone(), state.identity.clone());
    start_controller_signing_staged_rollout_worker(
        state.store.clone(),
        state.sessions.clone(),
        state.identity.clone(),
        state.metadata.clone(),
    );
    start_retention_worker(state.store.clone());
    let app = Router::new()
        .route("/api/agents/ws", get(axum_agent_websocket))
        .fallback(axum_http_fallback)
        .with_state(state);

    if let Some(acceptor) = tls_acceptor {
        let listener = TlsControllerListener { listener, acceptor };
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                while !should_shutdown() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .map_err(ControllerError::Io)?;
    } else {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                while !should_shutdown() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .map_err(ControllerError::Io)?;
    }

    tracing::info!("controller_stopped");
    Ok(())
}

fn announce_controller_started(
    config: &ControllerServerConfig,
    identity: &ControllerIdentity,
    insecure_http_target: Option<&str>,
) {
    tracing::info!(
        bind_addr = %format!("{}:{}", config.host, config.port),
        external_url = %config.external_url.as_deref().unwrap_or(""),
        tls_enabled = config.tls_cert_path.is_some(),
        controller_fingerprint = %identity.fingerprint,
        "controller_started"
    );
    if let Some(target) = insecure_http_target {
        tracing::warn!(
            transport_target = %target,
            "insecure_http_transport_enabled"
        );
    }
    println!("controller listening on {}:{}", config.host, config.port);
    if let Some(external_url) = &config.external_url {
        println!("controller external url: {external_url}");
    }
    if config.tls_cert_path.is_some() {
        println!("controller transport: https");
    }
    if let Some(target) = insecure_http_target {
        eprintln!(
            "{}",
            fleet_core::format_warning_message(format!(
                "insecure HTTP controller URL enabled: {target}; HTTP is test-only and not encrypted; use HTTPS for product or production environments"
            ))
        );
    }
}

fn controller_bind_error(bind_addr: &str, error: std::io::Error) -> ControllerError {
    ControllerError::Io(std::io::Error::new(
        error.kind(),
        format!(
            "failed to bind controller listener on {bind_addr}: {error}. Make sure --host is an IP address assigned to this machine, or use --host 0.0.0.0 and set --external-url to a reachable IP/DNS name."
        ),
    ))
}

struct TlsControllerListener {
    listener: tokio::net::TcpListener,
    acceptor: TlsAcceptor,
}

impl axum::serve::Listener for TlsControllerListener {
    type Io = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (stream, remote_addr) = match self.listener.accept().await {
                Ok(accepted) => accepted,
                Err(error) => {
                    tracing::warn!(error = %error, "controller_tcp_accept_failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };

            match self.acceptor.accept(stream).await {
                Ok(tls_stream) => return (tls_stream, remote_addr),
                Err(error) => {
                    tracing::warn!(
                        remote_addr = %remote_addr,
                        error = %error,
                        "controller_tls_handshake_failed"
                    );
                    continue;
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

async fn axum_http_fallback(
    State(state): State<ControllerAppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> AxumResponse {
    let request = raw_http_request_from_axum(method, uri, headers, body);
    let result = state
        .store
        .lock()
        .map_err(|_| {
            ControllerError::Store(fleet_store::StoreError::Domain(
                "store lock poisoned".to_owned(),
            ))
        })
        .and_then(|store| {
            route_request_with_identity_and_sessions(
                &request,
                &store,
                Some(&state.artifact_store),
                &state.identity,
                &state.metadata,
                Some(&state.sessions),
            )
        });

    match result {
        Ok(response) => axum_response_from_raw(&response),
        Err(error) => {
            tracing::warn!(error = %error, "controller_request_failed");
            axum_response_from_raw(&response(
                500,
                "application/json",
                &format!("{{\"error\":\"{}\"}}\n", json_escape(&error.to_string())),
            ))
        }
    }
}

async fn axum_agent_websocket(
    State(state): State<ControllerAppState>,
    websocket: WebSocketUpgrade,
) -> impl IntoResponse {
    websocket.on_upgrade(move |socket| async move {
        if let Err(error) = handle_agent_websocket_axum(socket, state).await {
            tracing::warn!(error = %error, "controller_websocket_failed");
        }
    })
}

async fn handle_agent_websocket_axum(
    mut socket: WebSocket,
    state: ControllerAppState,
) -> Result<(), ControllerError> {
    let agent_hello = read_axum_wire_message(&mut socket).await?;
    let fleet_protocol::WirePayload::AgentHello {
        agent_id,
        fingerprint,
    } = agent_hello.payload
    else {
        let store = lock_store(&state)?;
        audit_security(&store, "websocket_expected_agent_hello", "unknown")?;
        return Ok(());
    };

    let Some(public_key) = ({
        let store = lock_store(&state)?;
        validate_agent_ws_hello(&store, &agent_id, &fingerprint)?
    }) else {
        return Ok(());
    };

    let nonce = generate_token("challenge")?;
    let challenge = fleet_protocol::WireMessage::new(
        fleet_core::generate_prefixed_ulid("msg")
            .map_err(|error| ControllerError::Json(error.to_string()))?,
        agent_hello.correlation_id.0,
        Some(agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::AuthChallenge {
            nonce: nonce.clone(),
        },
    );
    send_axum_wire_message(&mut socket, &challenge).await?;

    let auth_response = read_axum_wire_message(&mut socket).await?;
    let fleet_protocol::WirePayload::AuthResponse {
        nonce: seen_nonce,
        signature,
    } = &auth_response.payload
    else {
        let store = lock_store(&state)?;
        audit_security(&store, "websocket_expected_auth_response", &agent_id)?;
        return Ok(());
    };

    if !verify_agent_auth_response(&public_key, &nonce, seen_nonce, signature) {
        let store = lock_store(&state)?;
        audit_security(&store, "websocket_invalid_signature", &agent_id)?;
        return Ok(());
    }

    let accepted = fleet_protocol::WireMessage::new(
        fleet_core::generate_prefixed_ulid("msg")
            .map_err(|error| ControllerError::Json(error.to_string()))?,
        auth_response.correlation_id.0,
        Some(agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::AuthAccepted,
    );
    send_axum_wire_message(&mut socket, &accepted).await?;

    let (writer, mut reader) = socket.split();
    let connection_id = fleet_core::generate_prefixed_ulid("conn")
        .map_err(|error| ControllerError::Json(error.to_string()))?;
    let (outbound_sender, outbound_receiver) = mpsc::channel(AGENT_SESSION_OUTBOUND_QUEUE_CAPACITY);
    let mut write_task = tokio::spawn(agent_session_write_loop(writer, outbound_receiver));
    let register_outcome = {
        let mut sessions = lock_sessions(&state)?;
        sessions.register(AgentSessionHandle::new(
            agent_id.clone(),
            connection_id.clone(),
            SystemTime::now(),
            vec!["persistent_session".to_owned(), "split_writer".to_owned()],
            outbound_sender.clone(),
            Some(AGENT_SESSION_OUTBOUND_QUEUE_CAPACITY),
        ))
    };
    if let Some(replacement) = register_outcome.replaced {
        let store = lock_store(&state)?;
        audit_agent_session_replaced(&store, &replacement)?;
    }
    let _session_guard = RegisteredAgentSessionGuard::new(
        state.sessions.clone(),
        agent_id.clone(),
        connection_id.clone(),
    );
    {
        let store = lock_store(&state)?;
        store.mark_agent_online(&agent_id, SystemTime::now())?;
        audit_agent_session_started(&store, &agent_id, &connection_id)?;
    }
    dispatch_pending_assignments_for_agent(&state, &agent_id, 1)?;

    let read_loop =
        read_authenticated_agent_session_loop(&mut reader, &state, &agent_id, &connection_id);
    tokio::pin!(read_loop);

    let close_reason = tokio::select! {
        read_result = &mut read_loop => {
            let close_reason = close_reason_from_session_read_result(&read_result);
            let _ = outbound_sender.try_send(AgentSessionOutboundMessage::Close {
                reason: close_reason,
            });
            match (&mut write_task).await {
                Ok(Ok(writer_reason)) => {
                    if close_reason == AgentSessionCloseReason::NormalShutdown {
                        writer_reason
                    } else {
                        close_reason
                    }
                }
                Ok(Err(error)) => {
                    tracing::warn!(error = %error, "agent_session_writer_failed");
                    AgentSessionCloseReason::WriteFailure
                }
                Err(error) => {
                    tracing::warn!(error = %error, "agent_session_writer_join_failed");
                    AgentSessionCloseReason::WriteFailure
                }
            }
        }
        writer_result = &mut write_task => {
            match writer_result {
                Ok(Ok(reason)) => reason,
                Ok(Err(error)) => {
                    tracing::warn!(error = %error, "agent_session_writer_failed");
                    AgentSessionCloseReason::WriteFailure
                }
                Err(error) => {
                    tracing::warn!(error = %error, "agent_session_writer_join_failed");
                    AgentSessionCloseReason::WriteFailure
                }
            }
        }
    };

    if let Some(ended) = {
        let mut sessions = lock_sessions(&state)?;
        sessions.unregister(&agent_id, &connection_id, close_reason)
    } {
        tracing::debug!(
            agent_id = %ended.agent_id,
            connection_id = %ended.connection_id,
            close_reason = %ended.close_reason.as_str(),
            "agent_session_ended"
        );
        let store = lock_store(&state)?;
        audit_agent_session_ended(&store, &ended)?;
    }

    Ok(())
}

async fn read_task_data_until_close_axum(
    socket: &mut SplitStream<WebSocket>,
    state: &ControllerAppState,
    agent_id: &str,
    connection_id: &str,
    stop_after_idle: bool,
) -> Result<AgentSessionCloseReason, ControllerError> {
    loop {
        let read_result = if stop_after_idle {
            match tokio::time::timeout(
                HEARTBEAT_BOUND_IDLE_CLOSE_AFTER,
                read_axum_wire_message_from_stream(socket),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => return Ok(AgentSessionCloseReason::IdleTimeout),
            }
        } else {
            read_axum_wire_message_from_stream(socket).await
        };
        let message = match read_result {
            Ok(message) => message,
            Err(ControllerError::Json(error)) if error == "websocket closed" => {
                return Ok(AgentSessionCloseReason::NormalShutdown);
            }
            Err(error) => return Err(error),
        };
        {
            let mut sessions = lock_sessions(state)?;
            let seen_at = SystemTime::now();
            sessions.mark_seen(agent_id, connection_id, seen_at);
            if let Some(ack) =
                agent_controller_signing_trust_ack_from_wire(agent_id, &message, seen_at)
            {
                sessions.record_controller_signing_trust_ack(agent_id, connection_id, ack);
            }
            if let Some(ack) =
                agent_certificate_lifecycle_ack_from_wire(agent_id, &message, seen_at)
            {
                sessions.record_agent_certificate_lifecycle_ack(agent_id, connection_id, ack);
            }
        }
        let done = {
            let store = lock_store(state)?;
            handle_agent_task_data_message_with_artifact_store(
                &store,
                agent_id,
                message,
                Some(&state.artifact_store),
            )?
        };
        if done {
            return Ok(AgentSessionCloseReason::NormalShutdown);
        }
        {
            let store = lock_store(state)?;
            let _ = dispatch_pending_assignments(&*store, &state.sessions, None, None, 100)?;
        }
    }
}

fn agent_controller_signing_trust_ack_from_wire(
    agent_id: &str,
    message: &fleet_protocol::WireMessage,
    acknowledged_at: SystemTime,
) -> Option<AgentControllerSigningTrustAck> {
    let fleet_protocol::WirePayload::ControllerSigningTrustBundleAck {
        agent_id: event_agent_id,
        accepted,
        current_fingerprint,
        entries_count,
        reason_code,
    } = &message.payload
    else {
        return None;
    };
    if event_agent_id != agent_id {
        return None;
    }
    Some(AgentControllerSigningTrustAck {
        accepted: *accepted,
        current_fingerprint: current_fingerprint.clone(),
        entries_count: *entries_count,
        reason_code: reason_code.clone(),
        acknowledged_at,
    })
}

fn agent_certificate_lifecycle_ack_from_wire(
    agent_id: &str,
    message: &fleet_protocol::WireMessage,
    acknowledged_at: SystemTime,
) -> Option<AgentCertificateLifecycleRuntimeAck> {
    let fleet_protocol::WirePayload::AgentCertificateLifecycleAck {
        agent_id: event_agent_id,
        accepted,
        state,
        current_fingerprint,
        reason_code,
    } = &message.payload
    else {
        return None;
    };
    if event_agent_id != agent_id {
        return None;
    }
    Some(AgentCertificateLifecycleRuntimeAck {
        accepted: *accepted,
        state: *state,
        current_fingerprint: current_fingerprint.clone(),
        reason_code: reason_code.clone(),
        acknowledged_at,
    })
}

async fn read_authenticated_agent_session_loop(
    reader: &mut SplitStream<WebSocket>,
    state: &ControllerAppState,
    agent_id: &str,
    connection_id: &str,
) -> Result<AgentSessionCloseReason, ControllerError> {
    let heartbeat = read_axum_wire_message_from_stream(reader).await?;
    if let fleet_protocol::WirePayload::Heartbeat {
        agent_id: heartbeat_agent_id,
        ..
    } = heartbeat.payload
        && heartbeat_agent_id == agent_id
    {
        {
            let mut sessions = lock_sessions(state)?;
            sessions.mark_seen(agent_id, connection_id, SystemTime::now());
        }
        {
            let store = lock_store(state)?;
            store.mark_agent_online(agent_id, SystemTime::now())?;
        }
        read_task_data_until_close_axum(reader, state, agent_id, connection_id, false).await
    } else {
        let store = lock_store(state)?;
        audit_security(&store, "websocket_invalid_heartbeat", agent_id)?;
        Ok(AgentSessionCloseReason::ProtocolError)
    }
}

fn close_reason_from_session_read_result(
    result: &Result<AgentSessionCloseReason, ControllerError>,
) -> AgentSessionCloseReason {
    match result {
        Ok(reason) => *reason,
        Err(ControllerError::Store(_)) => AgentSessionCloseReason::StoreError,
        Err(ControllerError::Json(error)) if error.contains("outbound queue is full") => {
            AgentSessionCloseReason::WriteQueueOverflow
        }
        Err(ControllerError::Json(_)) | Err(ControllerError::Protocol(_)) => {
            AgentSessionCloseReason::ProtocolError
        }
        Err(ControllerError::Io(_))
        | Err(ControllerError::Tls(_))
        | Err(ControllerError::SigningKeyRotation(_))
        | Err(ControllerError::SecretProvider(_))
        | Err(ControllerError::UnsupportedDatabaseBackend(_)) => {
            AgentSessionCloseReason::WriteFailure
        }
    }
}

async fn agent_session_write_loop(
    mut writer: SplitSink<WebSocket, AxumWsMessage>,
    mut outbound_receiver: mpsc::Receiver<AgentSessionOutboundMessage>,
) -> Result<AgentSessionCloseReason, ControllerError> {
    while let Some(message) = outbound_receiver.recv().await {
        match message {
            AgentSessionOutboundMessage::Wire(message) => {
                writer
                    .send(AxumWsMessage::Text(
                        fleet_protocol::encode_message(&message)?.into(),
                    ))
                    .await
                    .map_err(|error| ControllerError::Json(error.to_string()))?;
            }
            AgentSessionOutboundMessage::Close { reason } => {
                let _ = writer.send(AxumWsMessage::Close(None)).await;
                return Ok(reason);
            }
        }
    }
    Ok(AgentSessionCloseReason::NormalShutdown)
}

async fn read_axum_wire_message(
    socket: &mut WebSocket,
) -> Result<fleet_protocol::WireMessage, ControllerError> {
    loop {
        let Some(message) = socket.recv().await else {
            return Err(ControllerError::Json("websocket closed".to_owned()));
        };
        match message.map_err(|error| ControllerError::Json(error.to_string()))? {
            AxumWsMessage::Text(body) => {
                return fleet_protocol::decode_message(&body).map_err(ControllerError::from);
            }
            AxumWsMessage::Close(_) => {
                return Err(ControllerError::Json("websocket closed".to_owned()));
            }
            AxumWsMessage::Binary(_) | AxumWsMessage::Ping(_) | AxumWsMessage::Pong(_) => {}
        }
    }
}

async fn read_axum_wire_message_from_stream(
    socket: &mut SplitStream<WebSocket>,
) -> Result<fleet_protocol::WireMessage, ControllerError> {
    loop {
        let Some(message) = socket.next().await else {
            return Err(ControllerError::Json("websocket closed".to_owned()));
        };
        match message.map_err(|error| ControllerError::Json(error.to_string()))? {
            AxumWsMessage::Text(body) => {
                return fleet_protocol::decode_message(&body).map_err(ControllerError::from);
            }
            AxumWsMessage::Close(_) => {
                return Err(ControllerError::Json("websocket closed".to_owned()));
            }
            AxumWsMessage::Binary(_) | AxumWsMessage::Ping(_) | AxumWsMessage::Pong(_) => {}
        }
    }
}

async fn send_axum_wire_message(
    socket: &mut WebSocket,
    message: &fleet_protocol::WireMessage,
) -> Result<(), ControllerError> {
    socket
        .send(AxumWsMessage::Text(
            fleet_protocol::encode_message(message)?.into(),
        ))
        .await
        .map_err(|error| ControllerError::Json(error.to_string()))
}

struct ControllerPendingAssignmentDispatcher<'a> {
    sessions: &'a Arc<Mutex<AgentSessionRegistry>>,
}

impl PendingAssignmentDispatcher for ControllerPendingAssignmentDispatcher<'_> {
    type Error = ControllerError;

    fn has_active_session(&self, agent_id: &AgentId) -> bool {
        self.sessions
            .lock()
            .map(|sessions| sessions.has_active_session(agent_id.as_str()))
            .unwrap_or(false)
    }

    fn dispatch(&mut self, assignment: &PendingTaskAssignment) -> Result<(), Self::Error> {
        let message = pending_task_assignment_to_wire_message(assignment)?;
        self.sessions
            .lock()
            .map_err(|_| {
                ControllerError::Store(fleet_store::StoreError::Domain(
                    "session registry lock poisoned".to_owned(),
                ))
            })?
            .try_send(assignment.envelope.target_agent_id.as_str(), message)
            .map_err(|error| ControllerError::Json(error.to_string()))
    }
}

fn dispatch_pending_assignments_for_created_job<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    sessions: &Arc<Mutex<AgentSessionRegistry>>,
    job_id: &str,
) -> Result<DispatchPendingAssignmentsOutput, ControllerError> {
    let job_id =
        JobId::new(job_id.to_owned()).map_err(|error| ControllerError::Json(error.to_string()))?;
    dispatch_pending_assignments(store, sessions, None, Some(job_id), 100)
}

fn dispatch_pending_assignments_for_agent(
    state: &ControllerAppState,
    agent_id: &str,
    limit: usize,
) -> Result<DispatchPendingAssignmentsOutput, ControllerError> {
    let agent_id = AgentId::new(agent_id.to_owned())
        .map_err(|error| ControllerError::Json(error.to_string()))?;
    let store = lock_store(state)?;
    dispatch_pending_assignments(&*store, &state.sessions, Some(agent_id), None, limit)
}

fn dispatch_pending_assignments<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    sessions: &Arc<Mutex<AgentSessionRegistry>>,
    agent_id: Option<AgentId>,
    job_id: Option<JobId>,
    limit: usize,
) -> Result<DispatchPendingAssignmentsOutput, ControllerError> {
    let store = store.into();
    let started_at = Instant::now();
    let agent_id_label = agent_id
        .as_ref()
        .map(|agent_id| agent_id.as_str().to_owned())
        .unwrap_or_default();
    let job_id_label = job_id
        .as_ref()
        .map(|job_id| job_id.as_str().to_owned())
        .unwrap_or_default();
    let mut repo = ControllerJobRepository { store };
    let mut dispatcher = ControllerPendingAssignmentDispatcher { sessions };
    let mut audit = ControllerAuditWriter { store };
    let output = DispatchPendingAssignments::execute(
        &mut repo,
        &mut dispatcher,
        &mut audit,
        DispatchPendingAssignmentsInput {
            agent_id,
            job_id,
            now: SystemTime::now(),
            limit,
        },
    )
    .map_err(|error| match error {
        fleet_application::DispatchPendingAssignmentsError::Repository(error)
        | fleet_application::DispatchPendingAssignmentsError::Audit(error) => {
            ControllerError::Store(error)
        }
    })?;

    tracing::info!(
        job_id = %job_id_label,
        agent_id = %agent_id_label,
        dispatched_count = output.dispatched_count,
        queued_count = output.queued_count,
        failed_count = output.failed_count,
        skipped_concurrency_count = output.skipped_concurrency_count,
        skipped_max_failures_count = output.skipped_max_failures_count,
        skipped_expired_count = output.skipped_expired_count,
        skipped_disabled_count = output.skipped_disabled_count,
        dispatch_latency_ms = started_at.elapsed().as_millis(),
        "task_dispatch_checked"
    );

    Ok(output)
}

fn pending_task_assignment_to_wire_message(
    assignment: &PendingTaskAssignment,
) -> Result<fleet_protocol::WireMessage, ControllerError> {
    Ok(fleet_protocol::WireMessage::new(
        fleet_core::generate_prefixed_ulid("msg")
            .map_err(|error| ControllerError::Json(error.to_string()))?,
        assignment.envelope.task_id.as_str().to_owned(),
        Some(assignment.envelope.target_agent_id.as_str().to_owned()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::TaskAssignment {
            envelope: task_envelope_to_wire(&assignment.envelope),
            task: task_kind_to_wire(&assignment.task),
        },
    ))
}

fn lock_store(
    state: &ControllerAppState,
) -> Result<std::sync::MutexGuard<'_, ControllerStore>, ControllerError> {
    state.store.lock().map_err(|_| {
        ControllerError::Store(fleet_store::StoreError::Domain(
            "store lock poisoned".to_owned(),
        ))
    })
}

fn lock_sessions(
    state: &ControllerAppState,
) -> Result<std::sync::MutexGuard<'_, AgentSessionRegistry>, ControllerError> {
    state.sessions.lock().map_err(|_| {
        ControllerError::Store(fleet_store::StoreError::Domain(
            "session registry lock poisoned".to_owned(),
        ))
    })
}

fn audit_agent_session_replaced<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    replacement: &AgentSessionReplacement,
) -> Result<(), ControllerError> {
    let store = store.into();
    store.write_audit_event(AuditEvent {
        category: AuditCategory::Agent,
        action: "agent_session_replaced".to_owned(),
        actor: AuditActor::new("controller"),
        target: AuditTarget::new(replacement.agent_id.clone()),
        value: AuditValue::Plain(format!(
            "old_connection_id={},new_connection_id={},close_reason={}",
            replacement.old_connection_id,
            replacement.new_connection_id,
            replacement.close_reason.as_str()
        )),
        occurred_at: SystemTime::now(),
    })?;
    Ok(())
}

fn audit_agent_session_started<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    agent_id: &str,
    connection_id: &str,
) -> Result<(), ControllerError> {
    let store = store.into();
    store.write_audit_event(AuditEvent {
        category: AuditCategory::Agent,
        action: "agent_session_started".to_owned(),
        actor: AuditActor::new("controller"),
        target: AuditTarget::new(agent_id),
        value: AuditValue::Plain(format!("connection_id={connection_id}")),
        occurred_at: SystemTime::now(),
    })?;
    Ok(())
}

fn audit_agent_session_ended<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    ended: &AgentSessionEnded,
) -> Result<(), ControllerError> {
    let store = store.into();
    store.write_audit_event(AuditEvent {
        category: AuditCategory::Agent,
        action: "agent_session_ended".to_owned(),
        actor: AuditActor::new("controller"),
        target: AuditTarget::new(ended.agent_id.clone()),
        value: AuditValue::Plain(format!(
            "connection_id={},close_reason={}",
            ended.connection_id,
            ended.close_reason.as_str()
        )),
        occurred_at: SystemTime::now(),
    })?;
    Ok(())
}

fn audit_agent_session_revoked_closed<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    ended: &AgentSessionEnded,
) -> Result<(), ControllerError> {
    let store = store.into();
    store.write_audit_event(AuditEvent {
        category: AuditCategory::Agent,
        action: "agent_session_revoked_closed".to_owned(),
        actor: AuditActor::new("controller"),
        target: AuditTarget::new(ended.agent_id.clone()),
        value: AuditValue::Plain(format!(
            "connection_id={},close_reason={}",
            ended.connection_id,
            ended.close_reason.as_str()
        )),
        occurred_at: SystemTime::now(),
    })?;
    Ok(())
}

fn raw_http_request_from_axum(method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> String {
    let target = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or(uri.path());
    let mut request = format!("{method} {target} HTTP/1.1\r\n");
    let mut has_content_length = false;
    for (name, value) in &headers {
        if name.as_str().eq_ignore_ascii_case("content-length") {
            has_content_length = true;
        }
        if let Ok(value) = value.to_str() {
            request.push_str(name.as_str());
            request.push_str(": ");
            request.push_str(value);
            request.push_str("\r\n");
        }
    }
    if !has_content_length {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    request.push_str(&String::from_utf8_lossy(&body));
    request
}

fn axum_response_from_raw(raw: &str) -> AxumResponse {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .and_then(|code| StatusCode::from_u16(code).ok())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = body.to_owned().into_response();
    *response.status_mut() = status;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") || name.eq_ignore_ascii_case("connection") {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.trim().as_bytes()),
            HeaderValue::from_str(value.trim()),
        ) {
            response.headers_mut().insert(name, value);
        }
    }
    response
}

pub fn create_admin_token(store: &SqliteStore) -> Result<Option<String>, ControllerError> {
    let mut repo = ControllerAdminTokenRepository {
        store: store.into(),
    };
    let token = generate_token("admin")?;
    let created = EnsureAdminToken::execute(&mut repo, &hash_token(&token))?;
    if !created {
        return Ok(None);
    }
    Ok(Some(token))
}

fn load_controller_identity(data_dir: &Path) -> Result<ControllerIdentity, ControllerError> {
    let pair = ControllerSigningKeyFilePair {
        public_key_path: data_dir.join("controller").join("controller_public.key"),
        private_key_path: data_dir.join("controller").join("controller_private.key"),
    };
    load_controller_identity_from_signing_pair(&pair)
}

fn load_controller_identity_from_signing_pair(
    pair: &ControllerSigningKeyFilePair,
) -> Result<ControllerIdentity, ControllerError> {
    validate_controller_signing_private_key_permissions(&pair.private_key_path)?;
    let public_key = read_controller_signing_key_file()(&pair.public_key_path)?
        .trim()
        .to_owned();
    let private_key = read_controller_signing_key_file()(&pair.private_key_path)?
        .trim()
        .to_owned();
    let validation = fleet_core::validate_signing_material_pair(
        &public_key,
        &private_key,
        CONTROLLER_SIGNING_VALIDATION_CHALLENGE,
    )
    .map_err(|_| {
        ControllerError::SigningKeyRotation(
            "active controller signing material failed validation".to_owned(),
        )
    })?;
    Ok(ControllerIdentity {
        public_key,
        fingerprint: validation.public_key_fingerprint,
        private_key,
    })
}

fn load_controller_signing_runtime_identity(
    data_dir: &Path,
    store: &ControllerStore,
    now: SystemTime,
) -> Result<ControllerIdentity, ControllerError> {
    let identity = load_controller_identity(data_dir)?;
    let rotation = store
        .load_signing_key_rotation(DEFAULT_CONTROLLER_ID)
        .map_err(controller_signing_rotation_load_error)?;
    guard_controller_signing_runtime_identity(identity, rotation, now)
}

fn guard_controller_signing_runtime_identity(
    identity: ControllerIdentity,
    rotation: Option<SigningKeyRotationRecord>,
    now: SystemTime,
) -> Result<ControllerIdentity, ControllerError> {
    let Some(rotation) = rotation else {
        return Ok(identity);
    };
    let selected = select_controller_signing_fingerprint(&rotation.rotation, now);
    if selected.fingerprint.as_str() != identity.fingerprint {
        return Err(ControllerError::SigningKeyRotation(format!(
            "active controller signing fingerprint prefix {} does not match persisted signing rotation state selected fingerprint prefix {}",
            controller_signing_fingerprint_prefix(&identity.fingerprint),
            controller_signing_fingerprint_prefix(selected.fingerprint.as_str())
        )));
    }
    Ok(identity)
}

fn controller_signing_rotation_load_error(_: fleet_store::StoreError) -> ControllerError {
    ControllerError::SigningKeyRotation(
        "persisted controller signing rotation state could not be loaded or is invalid".to_owned(),
    )
}

fn controller_signing_fingerprint_prefix(fingerprint: &str) -> &str {
    fingerprint
        .char_indices()
        .nth(12)
        .map(|(index, _)| &fingerprint[..index])
        .unwrap_or(fingerprint)
}

fn controller_signing_previous_public_key_backup_path(
    metadata: &ControllerRuntimeMetadata,
) -> Result<PathBuf, ControllerError> {
    metadata
        .controller_signing_public_key_path
        .as_ref()
        .map(|path| path.with_file_name("controller_public.key.bak"))
        .ok_or_else(|| {
            ControllerError::SigningKeyRotation(
                "controller signing public key bootstrap path is required for staged rollout worker"
                    .to_owned(),
            )
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSigningKeyFilePair {
    pub public_key_path: PathBuf,
    pub private_key_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSigningKeyCandidateInput {
    pub candidate: ControllerSigningKeyFilePair,
    pub active: ControllerSigningKeyFilePair,
    pub disallowed_paths: Vec<PathBuf>,
    pub challenge: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSigningKeyCandidate {
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSigningKeySwapInput {
    pub candidate: ControllerSigningKeyFilePair,
    pub active: ControllerSigningKeyFilePair,
    pub backup_dir: PathBuf,
    pub disallowed_paths: Vec<PathBuf>,
    pub challenge: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSigningKeySwapOutcome {
    pub fingerprint: String,
    pub final_state: ControllerSigningKeySwapState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerSigningKeySwapState {
    CandidateRead,
    CandidateValidated,
    BackupCreated,
    PublicKeySwapped,
    PrivateKeySwapped,
    SwapVerified,
    Completed,
    RollbackRequired,
    RolledBack,
    Failed,
}

pub fn validate_controller_signing_key_candidate(
    input: &ControllerSigningKeyCandidateInput,
) -> Result<ControllerSigningKeyCandidate, ControllerError> {
    validate_controller_signing_key_path_separation(
        &input.candidate,
        &input.active,
        &input.disallowed_paths,
    )?;
    validate_controller_signing_private_key_permissions(&input.candidate.private_key_path)?;
    let public_key = read_controller_signing_key_file()(&input.candidate.public_key_path)?;
    let private_key = read_controller_signing_key_file()(&input.candidate.private_key_path)?;
    let validation = fleet_core::validate_signing_material_pair(
        public_key.trim(),
        private_key.trim(),
        &input.challenge,
    )
    .map_err(|_| {
        ControllerError::SigningKeyRotation(
            "candidate signing material failed validation".to_owned(),
        )
    })?;
    Ok(ControllerSigningKeyCandidate {
        fingerprint: validation.public_key_fingerprint,
    })
}

pub fn swap_controller_signing_key_files(
    input: &ControllerSigningKeySwapInput,
) -> Result<ControllerSigningKeySwapOutcome, ControllerError> {
    swap_controller_signing_key_files_inner(input, None)
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControllerSigningKeySwapFault {
    AfterPublicKeySwap,
}

#[cfg(not(test))]
type ControllerSigningKeySwapFault = ();

fn swap_controller_signing_key_files_inner(
    input: &ControllerSigningKeySwapInput,
    fault: Option<ControllerSigningKeySwapFault>,
) -> Result<ControllerSigningKeySwapOutcome, ControllerError> {
    let candidate =
        validate_controller_signing_key_candidate(&ControllerSigningKeyCandidateInput {
            candidate: input.candidate.clone(),
            active: input.active.clone(),
            disallowed_paths: input.disallowed_paths.clone(),
            challenge: input.challenge.clone(),
        })?;
    let _state = ControllerSigningKeySwapState::CandidateValidated;

    let old_public = read_controller_signing_key_file()(&input.active.public_key_path)?;
    let old_private = read_controller_signing_key_file()(&input.active.private_key_path)?;
    let new_public = read_controller_signing_key_file()(&input.candidate.public_key_path)?;
    let new_private = read_controller_signing_key_file()(&input.candidate.private_key_path)?;
    std::fs::create_dir_all(&input.backup_dir).map_err(|_| signing_key_rotation_io_error())?;
    write_public_key_file(
        &input.backup_dir.join("controller_public.key.bak"),
        &old_public,
    )?;
    write_private_key_file(
        &input.backup_dir.join("controller_private.key.bak"),
        &old_private,
    )?;
    let _state = ControllerSigningKeySwapState::BackupCreated;

    if let Err(error) = replace_controller_key_files(input, &new_public, &new_private, fault) {
        rollback_controller_signing_key_files(&input.active, &old_public, &old_private)?;
        return Err(error);
    }

    let swapped_public = read_controller_signing_key_file()(&input.active.public_key_path)?;
    let swapped_private = read_controller_signing_key_file()(&input.active.private_key_path)?;
    if fleet_core::validate_signing_material_pair(
        swapped_public.trim(),
        swapped_private.trim(),
        &input.challenge,
    )
    .is_err()
    {
        rollback_controller_signing_key_files(&input.active, &old_public, &old_private)?;
        return Err(ControllerError::SigningKeyRotation(
            "swapped signing material failed verification and rollback completed".to_owned(),
        ));
    }
    let _state = ControllerSigningKeySwapState::SwapVerified;

    Ok(ControllerSigningKeySwapOutcome {
        fingerprint: candidate.fingerprint,
        final_state: ControllerSigningKeySwapState::Completed,
    })
}

fn replace_controller_key_files(
    input: &ControllerSigningKeySwapInput,
    new_public: &str,
    new_private: &str,
    fault: Option<ControllerSigningKeySwapFault>,
) -> Result<(), ControllerError> {
    replace_public_key_file(&input.active.public_key_path, new_public)?;
    let _state = ControllerSigningKeySwapState::PublicKeySwapped;
    #[cfg(test)]
    if fault == Some(ControllerSigningKeySwapFault::AfterPublicKeySwap) {
        return Err(ControllerError::SigningKeyRotation(
            "simulated key swap failure after public key replacement; rollback completed"
                .to_owned(),
        ));
    }
    #[cfg(not(test))]
    let _ = fault;
    replace_private_key_file(&input.active.private_key_path, new_private)?;
    let _state = ControllerSigningKeySwapState::PrivateKeySwapped;
    Ok(())
}

fn rollback_controller_signing_key_files(
    active: &ControllerSigningKeyFilePair,
    old_public: &str,
    old_private: &str,
) -> Result<(), ControllerError> {
    let _state = ControllerSigningKeySwapState::RollbackRequired;
    write_public_key_file(&active.public_key_path, old_public)?;
    write_private_key_file(&active.private_key_path, old_private)?;
    let _state = ControllerSigningKeySwapState::RolledBack;
    Ok(())
}

fn validate_controller_signing_key_path_separation(
    candidate: &ControllerSigningKeyFilePair,
    active: &ControllerSigningKeyFilePair,
    disallowed_paths: &[PathBuf],
) -> Result<(), ControllerError> {
    if candidate.public_key_path == candidate.private_key_path
        || candidate.public_key_path == active.private_key_path
        || candidate.private_key_path == active.public_key_path
        || candidate.public_key_path == active.public_key_path
        || candidate.private_key_path == active.private_key_path
        || disallowed_paths
            .iter()
            .any(|path| path == &candidate.public_key_path || path == &candidate.private_key_path)
    {
        return Err(ControllerError::SigningKeyRotation(
            "candidate signing key files must be separate from active and transport key files"
                .to_owned(),
        ));
    }
    Ok(())
}

fn validate_controller_signing_private_key_permissions(path: &Path) -> Result<(), ControllerError> {
    #[cfg(unix)]
    {
        let mode = std::fs::metadata(path)
            .map_err(|_| signing_key_rotation_io_error())?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(ControllerError::SigningKeyRotation(
                "controller signing private key file permissions are insecure".to_owned(),
            ));
        }
    }

    #[cfg(not(unix))]
    {
        let _ = path;
    }

    Ok(())
}

fn read_controller_signing_key_file() -> impl Fn(&Path) -> Result<String, ControllerError> {
    |path| std::fs::read_to_string(path).map_err(|_| signing_key_rotation_io_error())
}

fn replace_public_key_file(path: &Path, body: &str) -> Result<(), ControllerError> {
    let temp_path = controller_signing_swap_temp_path(path);
    write_public_key_file(&temp_path, body)?;
    std::fs::rename(&temp_path, path).map_err(|_| signing_key_rotation_io_error())
}

fn replace_private_key_file(path: &Path, body: &str) -> Result<(), ControllerError> {
    let temp_path = controller_signing_swap_temp_path(path);
    write_private_key_file(&temp_path, body)?;
    std::fs::rename(&temp_path, path).map_err(|_| signing_key_rotation_io_error())
}

fn write_public_key_file(path: &Path, body: &str) -> Result<(), ControllerError> {
    std::fs::write(path, body).map_err(|_| signing_key_rotation_io_error())
}

fn write_private_key_file(path: &Path, body: &str) -> Result<(), ControllerError> {
    std::fs::write(path, body).map_err(|_| signing_key_rotation_io_error())?;
    #[cfg(unix)]
    {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| signing_key_rotation_io_error())?;
    }
    Ok(())
}

fn controller_signing_swap_temp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("controller-signing-key");
    path.with_file_name(format!("{file_name}.swap"))
}

fn signing_key_rotation_io_error() -> ControllerError {
    ControllerError::SigningKeyRotation("filesystem operation failed".to_owned())
}

fn validate_agent_ws_hello<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    agent_id: &str,
    fingerprint: &str,
) -> Result<Option<String>, ControllerError> {
    let store = store.into();
    let Some((public_key, stored_fingerprint)) = store.find_agent_identity(agent_id)? else {
        if store
            .find_agent_by_id(agent_id)?
            .is_some_and(|agent| agent.status() == AgentStatus::Disabled)
        {
            audit_security_with_value(
                store,
                "agent_session_auth_failed",
                agent_id,
                AuditValue::Plain("reason=revoked".to_owned()),
            )?;
            return Ok(None);
        }
        audit_security(store, "websocket_unknown_agent", agent_id)?;
        return Ok(None);
    };
    if stored_fingerprint != fingerprint {
        audit_security(store, "websocket_fingerprint_mismatch", agent_id)?;
        return Ok(None);
    }
    Ok(Some(public_key))
}

fn verify_agent_auth_response(
    public_key: &str,
    expected_nonce: &str,
    seen_nonce: &str,
    signature: &str,
) -> bool {
    seen_nonce == expected_nonce
        && fleet_core::verify_challenge_signature(public_key, expected_nonce, signature)
            .unwrap_or(false)
}

#[cfg(test)]
fn handle_agent_task_data_message<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    agent_id: &str,
    message: fleet_protocol::WireMessage,
) -> Result<bool, ControllerError> {
    handle_agent_task_data_message_with_artifact_store(store, agent_id, message, None)
}

fn handle_agent_task_data_message_with_artifact_store<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    agent_id: &str,
    message: fleet_protocol::WireMessage,
    artifact_store: Option<&Mutex<LocalArtifactStore>>,
) -> Result<bool, ControllerError> {
    let store = store.into();
    let agent_message_time = millis_to_system_time(message.timestamp_ms);
    match message.payload {
        fleet_protocol::WirePayload::OutputChunk {
            job_id,
            task_id: _,
            stream,
            sequence,
            data,
        } => {
            let stream = output_stream_from_wire(stream);
            append_agent_output_chunk(
                store,
                agent_id,
                JobOutputChunk {
                    job_id,
                    agent_id: agent_id.to_owned(),
                    stream,
                    sequence,
                    body: data,
                },
            )?;
        }
        fleet_protocol::WirePayload::TaskAck { job_id, task_id } => {
            let changed = store.update_active_task_assignment_status(
                &task_id,
                AssignmentStatus::Accepted,
                agent_message_time,
                None,
            )?;
            if changed {
                apply_job_aggregate_after_assignment_update(store, &job_id, agent_message_time)?;
                audit_job(
                    store,
                    "task_accepted",
                    &job_id,
                    AuditValue::Plain(format!(
                        "agent_id={agent_id},task_id={task_id},assignment_status=accepted"
                    )),
                )?;
            }
        }
        fleet_protocol::WirePayload::TaskStarted { job_id, task_id } => {
            let changed = store.update_active_task_assignment_status(
                &task_id,
                AssignmentStatus::Started,
                agent_message_time,
                None,
            )?;
            if changed {
                apply_job_aggregate_after_assignment_update(store, &job_id, agent_message_time)?;
                audit_job(
                    store,
                    "task_started",
                    &job_id,
                    AuditValue::Plain(format!(
                        "agent_id={agent_id},task_id={task_id},assignment_status=started"
                    )),
                )?;
            }
        }
        fleet_protocol::WirePayload::TaskRejected {
            job_id,
            task_id,
            reason_code,
            reason,
        } => {
            let reason_code = task_rejection_reason_code_to_str(reason_code);
            let changed = store.update_active_task_assignment_status(
                &task_id,
                AssignmentStatus::Rejected,
                agent_message_time,
                Some(&reason),
            )?;
            if changed {
                apply_job_aggregate_after_assignment_update(store, &job_id, agent_message_time)?;
                audit_job(
                    store,
                    "task_rejected",
                    &job_id,
                    AuditValue::Plain(format!(
                        "agent_id={agent_id},task_id={task_id},assignment_status=rejected,reason_code={reason_code},reason={}",
                        fleet_core::redact_secret(&reason)
                    )),
                )?;
            }
        }
        fleet_protocol::WirePayload::TaskCancel {
            job_id,
            task_id,
            reason: _,
        } => {
            audit_security_with_value(
                store,
                "unexpected_agent_task_cancel",
                agent_id,
                AuditValue::Plain(format!("job_id={job_id},task_id={task_id}")),
            )?;
        }
        fleet_protocol::WirePayload::TaskResult {
            job_id,
            task_id,
            exit_code,
            status,
            reason,
            artifacts,
        } => {
            let result_status = status.unwrap_or({
                if exit_code == 0 {
                    fleet_protocol::TaskResultStatus::Succeeded
                } else {
                    fleet_protocol::TaskResultStatus::Failed
                }
            });
            let (assignment_status, audit_action) = task_result_status_to_domain(result_status);
            let last_error = task_result_last_error(exit_code, &reason);
            let changed = store.update_active_task_assignment_status(
                &task_id,
                assignment_status,
                agent_message_time,
                Some(&last_error),
            )?;
            if changed {
                store_task_result_artifacts(
                    store,
                    &job_id,
                    &task_id,
                    agent_id,
                    artifacts,
                    agent_message_time,
                    artifact_store,
                )?;
                apply_job_aggregate_after_assignment_update(store, &job_id, agent_message_time)?;
                audit_job(
                    store,
                    audit_action,
                    &job_id,
                    AuditValue::Plain(format!(
                        "agent_id={agent_id},task_id={task_id},assignment_status={},exit_code={exit_code},reason={}",
                        assignment_status.as_str(),
                        fleet_core::redact_secret(&reason)
                    )),
                )?;
            } else {
                audit_job(
                    store,
                    "task_result_ignored",
                    &job_id,
                    AuditValue::Plain(format!(
                        "agent_id={agent_id},task_id={task_id},exit_code={exit_code},reason=terminal_assignment"
                    )),
                )?;
            }
        }
        fleet_protocol::WirePayload::SecurityEvent {
            agent_id: event_agent_id,
            action,
            detail,
        } => {
            if event_agent_id != agent_id {
                audit_security(store, "websocket_security_event_agent_mismatch", agent_id)?;
            } else {
                audit_security_with_value(
                    store,
                    &action,
                    agent_id,
                    AuditValue::Plain(format!("detail={detail}")),
                )?;
            }
        }
        fleet_protocol::WirePayload::ControllerSigningTrustBundleAck {
            agent_id: event_agent_id,
            accepted,
            current_fingerprint,
            entries_count,
            reason_code,
        } => {
            if event_agent_id != agent_id {
                audit_security(
                    store,
                    "websocket_controller_signing_trust_ack_agent_mismatch",
                    agent_id,
                )?;
            } else {
                audit_security_with_value(
                    store,
                    "controller_signing_trust_bundle_acknowledged",
                    agent_id,
                    controller_signing_trust_ack_audit_value(
                        accepted,
                        current_fingerprint.as_deref(),
                        entries_count,
                        reason_code.as_deref(),
                    ),
                )?;
            }
        }
        fleet_protocol::WirePayload::AgentCertificateLifecycleAck {
            agent_id: event_agent_id,
            accepted,
            state,
            current_fingerprint,
            reason_code,
        } => {
            if event_agent_id != agent_id {
                audit_security(
                    store,
                    "websocket_agent_certificate_lifecycle_ack_agent_mismatch",
                    agent_id,
                )?;
            } else {
                audit_security_with_value(
                    store,
                    "agent_certificate_lifecycle_acknowledged",
                    agent_id,
                    agent_certificate_lifecycle_ack_audit_value(
                        accepted,
                        state,
                        current_fingerprint.as_deref(),
                        reason_code.as_deref(),
                    ),
                )?;
            }
        }
        fleet_protocol::WirePayload::FactsSnapshot {
            agent_id: event_agent_id,
            body,
        } => {
            if event_agent_id != agent_id {
                audit_security(store, "websocket_facts_agent_mismatch", agent_id)?;
            } else {
                if facts_payload_is_degraded(&body) {
                    store.mark_agent_degraded(agent_id, agent_message_time)?;
                }
                store.insert_facts_snapshot(agent_id, &body, agent_message_time)?;
            }
        }
        fleet_protocol::WirePayload::MetricsSnapshot {
            agent_id: event_agent_id,
            body,
        } => {
            if event_agent_id != agent_id {
                audit_security(store, "websocket_metrics_agent_mismatch", agent_id)?;
            } else {
                store.insert_metrics_snapshot(agent_id, &body, agent_message_time)?;
            }
        }
        fleet_protocol::WirePayload::CapabilitySnapshot {
            agent_id: event_agent_id,
            privilege_level,
            package_manager,
            service_manager,
            capabilities,
            reported_at_ms,
        } => {
            if event_agent_id != agent_id {
                audit_security(store, "websocket_capability_agent_mismatch", agent_id)?;
            } else {
                let capability_count = capabilities.len();
                let snapshot = capability_snapshot_from_wire(
                    privilege_level,
                    package_manager,
                    service_manager,
                    capabilities,
                    millis_to_system_time(reported_at_ms),
                )?;
                let capability_agent_id = AgentId::new(agent_id.to_owned())
                    .map_err(|error| ControllerError::Json(error.to_string()))?;
                store.save_agent_capability_snapshot(&capability_agent_id, snapshot)?;
                audit_agent(
                    store,
                    "agent_capability_reported",
                    agent_id,
                    AuditValue::Plain(format!(
                        "agent_id={agent_id},capability_count={capability_count}"
                    )),
                )?;
            }
        }
        fleet_protocol::WirePayload::LogChunk {
            agent_id: event_agent_id,
            line,
        } => {
            if event_agent_id != agent_id {
                audit_security(store, "websocket_log_agent_mismatch", agent_id)?;
            } else {
                store.insert_agent_log_chunk(
                    agent_id,
                    &sanitize_agent_log_line(&line),
                    agent_message_time,
                )?;
            }
        }
        fleet_protocol::WirePayload::DriftReport {
            agent_id: event_agent_id,
            status,
            expected,
            actual,
        } => {
            if event_agent_id != agent_id {
                audit_security(store, "websocket_drift_agent_mismatch", agent_id)?;
            } else {
                let report = DriftReport {
                    policy_name: "agent-reported".to_owned(),
                    status: parse_drift_status(&status),
                    severity: DriftSeverity::for_status(parse_drift_status(&status)),
                    acknowledgement: DriftAcknowledgement::Open,
                    expected,
                    actual,
                };
                store.insert_drift_report(agent_id, &report, agent_message_time)?;
                audit_drift(
                    store,
                    "drift_report_received",
                    agent_id,
                    AuditValue::Plain(format!(
                        "policy_name={},status={}",
                        report.policy_name,
                        drift_status_to_str(&report.status)
                    )),
                )?;
            }
        }
        _ => audit_security(store, "websocket_unexpected_task_data", agent_id)?,
    }
    Ok(false)
}

fn controller_signing_trust_ack_audit_value(
    accepted: bool,
    current_fingerprint: Option<&str>,
    entries_count: usize,
    reason_code: Option<&str>,
) -> AuditValue {
    let current_fingerprint_prefix = current_fingerprint
        .map(controller_signing_fingerprint_prefix)
        .unwrap_or("none");
    AuditValue::Plain(format!(
        "accepted={accepted},current_fingerprint_prefix={current_fingerprint_prefix},entries_count={entries_count},reason_code={}",
        bounded_controller_signing_trust_ack_reason_code(reason_code)
    ))
}

fn bounded_controller_signing_trust_ack_reason_code(reason_code: Option<&str>) -> String {
    let Some(reason_code) = reason_code else {
        return "none".to_owned();
    };
    let redacted = fleet_core::redact_secret(reason_code);
    if redacted.is_empty() || redacted.len() > 64 {
        return "redacted".to_owned();
    }
    if redacted
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.'))
    {
        redacted
    } else {
        "redacted".to_owned()
    }
}

fn agent_certificate_lifecycle_ack_audit_value(
    accepted: bool,
    state: fleet_protocol::AgentCertificateLifecycleStateWire,
    current_fingerprint: Option<&str>,
    reason_code: Option<&str>,
) -> AuditValue {
    let current_fingerprint_prefix = current_fingerprint
        .map(controller_signing_fingerprint_prefix)
        .unwrap_or("none");
    AuditValue::Plain(format!(
        "accepted={accepted},state={},current_fingerprint_prefix={current_fingerprint_prefix},reason_code={}",
        agent_certificate_lifecycle_state_wire_as_str(state),
        bounded_agent_certificate_lifecycle_ack_reason_code(reason_code)
    ))
}

fn bounded_agent_certificate_lifecycle_ack_reason_code(reason_code: Option<&str>) -> String {
    let Some(reason_code) = reason_code else {
        return "none".to_owned();
    };
    let redacted = fleet_core::redact_secret(reason_code);
    if redacted.is_empty() || redacted.len() > 64 {
        return "redacted".to_owned();
    }
    if redacted
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.'))
    {
        redacted
    } else {
        "redacted".to_owned()
    }
}

fn agent_certificate_lifecycle_state_wire_as_str(
    state: fleet_protocol::AgentCertificateLifecycleStateWire,
) -> &'static str {
    match state {
        fleet_protocol::AgentCertificateLifecycleStateWire::NotIssued => "not_issued",
        fleet_protocol::AgentCertificateLifecycleStateWire::IssuanceRequested => {
            "issuance_requested"
        }
        fleet_protocol::AgentCertificateLifecycleStateWire::Issued => "issued",
        fleet_protocol::AgentCertificateLifecycleStateWire::RenewalRequested => "renewal_requested",
        fleet_protocol::AgentCertificateLifecycleStateWire::DualCertificateActive => {
            "dual_certificate_active"
        }
        fleet_protocol::AgentCertificateLifecycleStateWire::Revoked => "revoked",
        fleet_protocol::AgentCertificateLifecycleStateWire::Expired => "expired",
        fleet_protocol::AgentCertificateLifecycleStateWire::Failed => "failed",
    }
}

fn append_agent_output_chunk<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    agent_id: &str,
    chunk: JobOutputChunk,
) -> Result<(), ControllerError> {
    let store = store.into();
    match store.append_job_output_chunk_record(&chunk) {
        Ok(()) => Ok(()),
        Err(fleet_store::StoreError::ConstraintViolation(_)) => {
            audit_security_with_value(
                store,
                "websocket_output_chunk_conflict",
                agent_id,
                AuditValue::Plain(format!(
                    "job_id={},stream={},sequence={},reason=duplicate_body_mismatch",
                    chunk.job_id,
                    job_output_stream_to_str(chunk.stream),
                    chunk.sequence
                )),
            )?;
            Err(ControllerError::Protocol(
                fleet_protocol::ProtocolError::Json(
                    "duplicate output chunk body mismatch".to_owned(),
                ),
            ))
        }
        Err(error) => Err(ControllerError::Store(error)),
    }
}

fn task_envelope_to_wire(envelope: &TaskEnvelope) -> fleet_protocol::SignedTaskEnvelopeWire {
    fleet_protocol::SignedTaskEnvelopeWire {
        job_id: envelope.job_id.as_str().to_owned(),
        task_id: envelope.task_id.as_str().to_owned(),
        target_agent_id: envelope.target_agent_id.as_str().to_owned(),
        issued_at_ms: system_time_to_millis(envelope.issued_at),
        expires_at_ms: system_time_to_millis(envelope.expires_at.as_system_time()),
        nonce: envelope.nonce.as_str().to_owned(),
        payload_hash: envelope.payload_hash.clone(),
        signature: envelope
            .signature
            .as_ref()
            .map(|signature| signature.as_str().to_owned())
            .unwrap_or_default(),
    }
}

fn task_kind_to_wire(task: &fleet_domain::TaskKind) -> fleet_protocol::TaskWire {
    match task {
        fleet_domain::TaskKind::Command(command) => command_task_to_wire(command),
        fleet_domain::TaskKind::DriftCheck(drift_check) => {
            fleet_protocol::TaskWire::DriftCheck(fleet_protocol::DriftCheckTaskWire {
                policy_document: drift_check.policy_document().to_owned(),
            })
        }
        fleet_domain::TaskKind::RunbookExecution(runbook) => {
            fleet_protocol::TaskWire::RunbookExecution(fleet_protocol::RunbookExecutionTaskWire {
                runbook_document: runbook.runbook_document().to_owned(),
                timeout_ms: runbook.timeout().as_millis() as u64,
                confirmed_high_risk: true,
            })
        }
    }
}

fn command_task_to_wire(command: &fleet_domain::CommandTask) -> fleet_protocol::TaskWire {
    fleet_protocol::TaskWire::Command(fleet_protocol::CommandTaskWire {
        program: command.program().to_owned(),
        args: command.args().to_vec(),
        timeout_ms: command.timeout().as_millis() as u64,
        max_output_bytes: command.max_output_bytes(),
    })
}

fn output_stream_from_wire(stream: fleet_protocol::OutputStream) -> JobOutputStream {
    match stream {
        fleet_protocol::OutputStream::Stdout => JobOutputStream::Stdout,
        fleet_protocol::OutputStream::Stderr => JobOutputStream::Stderr,
    }
}

fn task_rejection_reason_code_to_str(
    reason_code: fleet_protocol::TaskRejectionReasonCode,
) -> &'static str {
    match reason_code {
        fleet_protocol::TaskRejectionReasonCode::AgentBusy => "agent_busy",
        fleet_protocol::TaskRejectionReasonCode::InvalidSignature => "invalid_signature",
        fleet_protocol::TaskRejectionReasonCode::Expired => "expired",
        fleet_protocol::TaskRejectionReasonCode::Replay => "replay",
        fleet_protocol::TaskRejectionReasonCode::TargetMismatch => "target_mismatch",
        fleet_protocol::TaskRejectionReasonCode::InvalidTask => "invalid_task",
        fleet_protocol::TaskRejectionReasonCode::CapabilityUnsupported => "capability_unsupported",
        fleet_protocol::TaskRejectionReasonCode::LocalPolicy => "local_policy",
        fleet_protocol::TaskRejectionReasonCode::InternalError => "internal_error",
    }
}

fn capability_snapshot_from_wire(
    privilege_level: fleet_protocol::CapabilityPrivilegeLevelWire,
    package_manager: Option<fleet_protocol::PackageManagerWire>,
    service_manager: Option<fleet_protocol::ServiceManagerWire>,
    capabilities: Vec<String>,
    reported_at: SystemTime,
) -> Result<AgentCapabilitySnapshot, ControllerError> {
    let privilege_level = match privilege_level {
        fleet_protocol::CapabilityPrivilegeLevelWire::Unprivileged => PrivilegeLevel::Unprivileged,
        fleet_protocol::CapabilityPrivilegeLevelWire::SudoAvailable => {
            PrivilegeLevel::SudoAvailable
        }
        fleet_protocol::CapabilityPrivilegeLevelWire::Root => PrivilegeLevel::Root,
    };
    let package_manager = package_manager.map(|manager| match manager {
        fleet_protocol::PackageManagerWire::Apt => PackageManager::Apt,
        fleet_protocol::PackageManagerWire::Dnf => PackageManager::Dnf,
        fleet_protocol::PackageManagerWire::Yum => PackageManager::Yum,
        fleet_protocol::PackageManagerWire::Apk => PackageManager::Apk,
        fleet_protocol::PackageManagerWire::Brew => PackageManager::Brew,
    });
    let service_manager = service_manager.map(|manager| match manager {
        fleet_protocol::ServiceManagerWire::Systemd => ServiceManager::Systemd,
        fleet_protocol::ServiceManagerWire::Launchd => ServiceManager::Launchd,
        fleet_protocol::ServiceManagerWire::OpenRc => ServiceManager::OpenRc,
    });
    let capabilities = capabilities
        .into_iter()
        .map(|capability| {
            AgentCapability::parse(&capability).ok_or_else(|| {
                ControllerError::Json(format!("unknown capability name: {capability}"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AgentCapabilitySnapshot::reported(
        AgentRuntimeProfile::new(
            privilege_level,
            package_manager,
            service_manager,
            capabilities,
        ),
        reported_at,
    ))
}

fn apply_job_aggregate_after_assignment_update<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    job_id: &str,
    now: SystemTime,
) -> Result<(), ControllerError> {
    let store = store.into();
    let canceled_count =
        store.cancel_queued_assignments_after_max_failures(job_id, now, "maxFailures reached")?;
    if canceled_count > 0 {
        audit_job(
            store,
            "job_max_failures_reached",
            job_id,
            AuditValue::Plain(format!(
                "canceled_queued_count={canceled_count},reason=maxFailures reached"
            )),
        )?;
    }
    store.recompute_job_status_from_assignments(job_id)?;
    Ok(())
}

fn task_result_status_to_domain(
    status: fleet_protocol::TaskResultStatus,
) -> (AssignmentStatus, &'static str) {
    match status {
        fleet_protocol::TaskResultStatus::Succeeded => {
            (AssignmentStatus::Succeeded, "job_completed")
        }
        fleet_protocol::TaskResultStatus::Failed => (AssignmentStatus::Failed, "job_failed"),
        fleet_protocol::TaskResultStatus::Canceled => (AssignmentStatus::Canceled, "job_canceled"),
        fleet_protocol::TaskResultStatus::TimedOut => (AssignmentStatus::Expired, "job_timed_out"),
    }
}

fn store_task_result_artifacts<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    job_id: &str,
    task_id: &str,
    agent_id: &str,
    artifacts: Vec<fleet_protocol::TaskResultArtifactWire>,
    created_at: SystemTime,
    artifact_store: Option<&Mutex<LocalArtifactStore>>,
) -> Result<(), ControllerError> {
    let store = store.into();
    if artifacts.is_empty() {
        return Ok(());
    }
    for artifact in artifacts {
        let artifact_id = ArtifactId::new(artifact.artifact_id).map_err(|error| {
            ControllerError::Store(fleet_store::StoreError::Domain(error.to_string()))
        })?;
        let checksum = ArtifactChecksum::sha256(artifact.checksum_sha256).map_err(|error| {
            ControllerError::Store(fleet_store::StoreError::Domain(error.to_string()))
        })?;
        let retention_class =
            ArtifactRetentionClass::parse(&artifact.retention_class).map_err(|error| {
                ControllerError::Store(fleet_store::StoreError::Domain(error.to_string()))
            })?;
        let metadata = RenderedArtifactMetadata::new(
            artifact_id,
            JobId::new(job_id.to_owned()).map_err(|error| {
                ControllerError::Store(fleet_store::StoreError::Domain(error.to_string()))
            })?,
            AgentId::new(agent_id.to_owned()).map_err(|error| {
                ControllerError::Store(fleet_store::StoreError::Domain(error.to_string()))
            })?,
            TaskId::new(task_id.to_owned()).map_err(|error| {
                ControllerError::Store(fleet_store::StoreError::Domain(error.to_string()))
            })?,
            artifact.destination,
            checksum,
            artifact.size_bytes,
            retention_class,
            created_at,
        )
        .map_err(|error| {
            ControllerError::Store(fleet_store::StoreError::Domain(error.to_string()))
        })?;
        if let Some(content_bytes) = artifact.content_bytes {
            store_task_result_artifact_body(&metadata, content_bytes, artifact_store)?;
        }
        store.save_rendered_artifact_metadata_record(&metadata)?;
    }
    Ok(())
}

fn store_task_result_artifact_body(
    metadata: &RenderedArtifactMetadata,
    content_bytes: Vec<u8>,
    artifact_store: Option<&Mutex<LocalArtifactStore>>,
) -> Result<(), ControllerError> {
    if content_bytes.len() > DEFAULT_MAX_ARTIFACT_BODY_BYTES {
        tracing::warn!(
            artifact_id = metadata.id.as_str(),
            checksum_prefix = artifact_checksum_prefix(&metadata.checksum),
            size_bytes = content_bytes.len(),
            max_size_bytes = DEFAULT_MAX_ARTIFACT_BODY_BYTES,
            status = "body_too_large",
            "artifact_body_ingest_failed"
        );
        return Err(ControllerError::Store(fleet_store::StoreError::Domain(
            format!(
                "artifact body exceeds max size: artifact_id={},size_bytes={},max_size_bytes={}",
                metadata.id.as_str(),
                content_bytes.len(),
                DEFAULT_MAX_ARTIFACT_BODY_BYTES
            ),
        )));
    }
    if content_bytes.len() as u64 != metadata.size_bytes {
        tracing::warn!(
            artifact_id = metadata.id.as_str(),
            checksum_prefix = artifact_checksum_prefix(&metadata.checksum),
            expected_size_bytes = metadata.size_bytes,
            actual_size_bytes = content_bytes.len(),
            status = "size_mismatch",
            "artifact_body_ingest_failed"
        );
        return Err(ControllerError::Store(fleet_store::StoreError::Domain(
            format!(
                "artifact body size mismatch: artifact_id={},expected_size_bytes={},actual_size_bytes={}",
                metadata.id.as_str(),
                metadata.size_bytes,
                content_bytes.len()
            ),
        )));
    }
    let Some(artifact_store) = artifact_store else {
        tracing::warn!(
            artifact_id = metadata.id.as_str(),
            checksum_prefix = artifact_checksum_prefix(&metadata.checksum),
            status = "store_unavailable",
            "artifact_body_ingest_failed"
        );
        return Err(ControllerError::Store(fleet_store::StoreError::Domain(
            "artifact body present but artifact store is not configured".to_owned(),
        )));
    };
    let mut artifact_store = artifact_store.lock().map_err(|_| {
        ControllerError::Store(fleet_store::StoreError::Domain(
            "artifact store lock poisoned".to_owned(),
        ))
    })?;
    artifact_store
        .put(fleet_application::ArtifactStorePut {
            id: metadata.id.clone(),
            retention_class: metadata.retention_class,
            expected_checksum: metadata.checksum.clone(),
            bytes: content_bytes,
        })
        .map_err(|error| {
            tracing::warn!(
                artifact_id = metadata.id.as_str(),
                checksum_prefix = artifact_checksum_prefix(&metadata.checksum),
                status = "checksum_or_store_failure",
                "artifact_body_ingest_failed"
            );
            ControllerError::Store(error)
        })?;
    tracing::info!(
        job_id = metadata.job_id.as_str(),
        task_id = metadata.task_id.as_str(),
        agent_id = metadata.agent_id.as_str(),
        artifact_id = metadata.id.as_str(),
        retention_class = metadata.retention_class.as_str(),
        size_bytes = metadata.size_bytes,
        checksum_prefix = artifact_checksum_prefix(&metadata.checksum),
        "artifact_body_stored"
    );
    Ok(())
}

fn task_result_last_error(exit_code: i32, reason: &str) -> String {
    if reason.is_empty() {
        format!("exit_code={exit_code}")
    } else {
        format!(
            "exit_code={exit_code},reason={}",
            fleet_core::redact_secret(reason)
        )
    }
}

fn sanitize_agent_log_line(line: &str) -> String {
    let redacted = fleet_core::redact_secret(line);
    if redacted.len() <= AGENT_LOG_CHUNK_MAX_BYTES {
        return redacted;
    }
    let mut end = AGENT_LOG_CHUNK_MAX_BYTES;
    while !redacted.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...[truncated]", &redacted[..end])
}

fn audit_security<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    action: &str,
    target: &str,
) -> Result<(), ControllerError> {
    let store = store.into();
    store.write_audit_event(AuditEvent::security(action, target))?;
    Ok(())
}

fn audit_security_with_value<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    action: &str,
    target: &str,
    value: AuditValue,
) -> Result<(), ControllerError> {
    let store = store.into();
    store.write_audit_event(AuditEvent {
        category: AuditCategory::Security,
        action: action.to_owned(),
        actor: AuditActor::new("agent"),
        target: AuditTarget::new(target),
        value,
        occurred_at: SystemTime::now(),
    })?;
    Ok(())
}

fn audit_insecure_http_transport_enabled<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    target: &str,
) -> Result<(), ControllerError> {
    let store = store.into();
    store.write_audit_event(AuditEvent {
        category: AuditCategory::Security,
        action: "insecure_http_transport_enabled".to_owned(),
        actor: AuditActor::new("controller"),
        target: AuditTarget::new(target),
        value: AuditValue::Plain("http_without_tls".to_owned()),
        occurred_at: SystemTime::now(),
    })?;
    Ok(())
}

fn audit_agent<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    action: &str,
    target: &str,
    value: AuditValue,
) -> Result<(), ControllerError> {
    let store = store.into();
    store.write_audit_event(AuditEvent {
        category: AuditCategory::Agent,
        action: action.to_owned(),
        actor: AuditActor::new("agent"),
        target: AuditTarget::new(target),
        value,
        occurred_at: SystemTime::now(),
    })?;
    Ok(())
}

fn audit_job<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    action: &str,
    target: &str,
    value: AuditValue,
) -> Result<(), ControllerError> {
    let store = store.into();
    store.write_audit_event(AuditEvent {
        category: AuditCategory::Job,
        action: action.to_owned(),
        actor: AuditActor::new("controller"),
        target: AuditTarget::new(target),
        value,
        occurred_at: SystemTime::now(),
    })?;
    Ok(())
}

fn audit_drift<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    action: &str,
    target: &str,
    value: AuditValue,
) -> Result<(), ControllerError> {
    let store = store.into();
    store.write_audit_event(AuditEvent {
        category: AuditCategory::Drift,
        action: action.to_owned(),
        actor: AuditActor::new("agent"),
        target: AuditTarget::new(target),
        value,
        occurred_at: SystemTime::now(),
    })?;
    Ok(())
}

fn start_scheduled_drift_worker(
    store: Arc<Mutex<ControllerStore>>,
    identity: Arc<ControllerIdentity>,
) {
    tokio::spawn(async move {
        tracing::info!("scheduled_drift_worker_started");
        loop {
            tokio::time::sleep(SCHEDULED_DRIFT_WORKER_INTERVAL).await;
            let result = {
                let Ok(store) = store.lock() else {
                    tracing::error!("scheduled_drift_worker_failed reason=store_lock_poisoned");
                    continue;
                };
                run_due_scheduled_drift_once(&store, identity.as_ref(), SystemTime::now())
            };
            match result {
                Ok(output)
                    if output.created_count > 0
                        || output.missed_count > 0
                        || output.skipped_disabled_count > 0 =>
                {
                    tracing::info!(
                        created_count = output.created_count,
                        missed_count = output.missed_count,
                        skipped_disabled_count = output.skipped_disabled_count,
                        "scheduled_drift_worker_completed"
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::error!(error = %error, "scheduled_drift_worker_failed");
                }
            }
        }
    });
}

fn start_controller_signing_staged_rollout_worker(
    store: Arc<Mutex<ControllerStore>>,
    sessions: Arc<Mutex<AgentSessionRegistry>>,
    identity: Arc<ControllerIdentity>,
    metadata: Arc<ControllerRuntimeMetadata>,
) {
    tokio::spawn(async move {
        tracing::info!("controller_signing_staged_rollout_worker_started");
        loop {
            tokio::time::sleep(CONTROLLER_SIGNING_STAGED_ROLLOUT_WORKER_INTERVAL).await;
            let result = {
                let Ok(store) = store.lock() else {
                    tracing::error!(
                        "controller_signing_staged_rollout_worker_failed reason=store_lock_poisoned"
                    );
                    continue;
                };
                run_controller_signing_staged_rollout_once(
                    &store,
                    &sessions,
                    identity.as_ref(),
                    metadata.as_ref(),
                    SystemTime::now(),
                )
            };
            match result {
                Ok(output)
                    if output.loaded
                        && (output.planned_count > 0
                            || output.updated_count > 0
                            || output.failed_count > 0
                            || output.pending_count > 0) =>
                {
                    tracing::info!(
                        rollout_state = output.rollout_state.as_deref().unwrap_or("none"),
                        planned_count = output.planned_count,
                        updated_count = output.updated_count,
                        skipped_count = output.skipped_count,
                        failed_count = output.failed_count,
                        pending_count = output.pending_count,
                        "controller_signing_staged_rollout_worker_completed"
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::error!(
                        error = %error,
                        "controller_signing_staged_rollout_worker_failed"
                    );
                }
            }
        }
    });
}

fn start_retention_worker(store: Arc<Mutex<ControllerStore>>) {
    tokio::spawn(async move {
        tracing::info!("retention_worker_started");
        loop {
            tokio::time::sleep(RETENTION_WORKER_INTERVAL).await;
            let result = {
                let Ok(store) = store.lock() else {
                    tracing::error!("retention_worker_failed reason=store_lock_poisoned");
                    continue;
                };
                run_retention_cleanup_once(&store, SystemTime::now())
            };
            match result {
                Ok(output) if output.summary.total() > 0 => {
                    tracing::info!(
                        job_output_chunks = output.summary.job_output_chunks,
                        facts_snapshots = output.summary.facts_snapshots,
                        metrics_snapshots = output.summary.metrics_snapshots,
                        agent_log_chunks = output.summary.agent_log_chunks,
                        total = output.summary.total(),
                        "retention_worker_completed"
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::error!(error = %error, "retention_worker_failed");
                }
            }
        }
    });
}

#[cfg(test)]
fn route_request(request: &str, store: &SqliteStore) -> Result<String, ControllerError> {
    route_request_with_identity(
        request,
        store,
        &ControllerIdentity::dev_insecure(),
        &ControllerRuntimeMetadata::default(),
    )
}

#[cfg(test)]
fn route_request_with_identity(
    request: &str,
    store: &SqliteStore,
    identity: &ControllerIdentity,
    metadata: &ControllerRuntimeMetadata,
) -> Result<String, ControllerError> {
    route_request_with_identity_and_sessions(request, store, None, identity, metadata, None)
}

#[cfg(test)]
fn route_request_with_artifact_store(
    request: &str,
    store: &SqliteStore,
    artifact_store: &Mutex<LocalArtifactStore>,
) -> Result<String, ControllerError> {
    route_request_with_identity_and_sessions(
        request,
        store,
        Some(artifact_store),
        &ControllerIdentity::dev_insecure(),
        &ControllerRuntimeMetadata::default(),
        None,
    )
}

fn route_request_with_identity_and_sessions<'a>(
    request: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    artifact_store: Option<&Mutex<LocalArtifactStore>>,
    identity: &ControllerIdentity,
    metadata: &ControllerRuntimeMetadata,
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
) -> Result<String, ControllerError> {
    let store = store.into();
    let Some(request_line) = request.lines().next() else {
        return Ok(response(400, "text/plain", "bad request\n"));
    };
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let raw_path = parts.next().unwrap_or_default();
    let route_path = path_without_query(raw_path);

    if method == "GET" && route_path == "/healthz" {
        return Ok(response(200, "application/json", "{\"status\":\"ok\"}\n"));
    }

    if method == "GET" && route_path == "/favicon.ico" {
        return Ok(response(204, "image/x-icon", ""));
    }

    if method == "GET" && route_path == "/openapi.json" {
        return Ok(response(
            200,
            "application/json; charset=utf-8",
            OPENAPI_JSON,
        ));
    }

    if method == "GET"
        && matches!(
            route_path,
            "/swagger-ui" | "/swagger-ui/" | "/swagger-ui/index.html"
        )
    {
        return Ok(response(200, "text/html; charset=utf-8", SWAGGER_UI_HTML));
    }

    if method == "GET" && route_path == "/api/controller/identity" {
        let body = serde_json::to_string(&controller_identity_response(identity, metadata))
            .map_err(|error| ControllerError::Json(error.to_string()))?;
        return Ok(response(200, "application/json", &format!("{body}\n")));
    }

    if method == "GET" && route_path.starts_with("/admin") {
        return Ok(admin_static_response(raw_path));
    }

    let protected_api = route_path.starts_with("/api/")
        && !matches!(route_path, "/api/agents/enroll" | "/api/agents/ws");
    let admin_context = if protected_api {
        let Some(context) = authenticate_admin_request(request, store)? else {
            return Ok(response(
                401,
                "application/json",
                "{\"error\":\"unauthorized\"}\n",
            ));
        };
        if let Some(permission) = required_permission_for_route(method, route_path)
            && !context.allows(permission)
        {
            return Ok(forbidden_response(permission));
        }
        Some(context)
    } else {
        None
    };
    let admin_actor = admin_context
        .as_ref()
        .map(|context| context.actor_id.as_str())
        .unwrap_or("anonymous");

    match (method, route_path) {
        ("POST", "/api/agents/enroll") => {
            match enroll_agent(request_body(request), store, identity) {
                Ok(body) => Ok(response(201, "application/json", &format!("{body}\n"))),
                Err(ControllerError::Store(fleet_store::StoreError::NotFound)) => Ok(response(
                    401,
                    "application/json",
                    "{\"error\":\"invalid_enrollment_token\"}\n",
                )),
                Err(ControllerError::Store(fleet_store::StoreError::Domain(message))) => {
                    Ok(response(
                        400,
                        "application/json",
                        &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                    ))
                }
                Err(ControllerError::Store(fleet_store::StoreError::DuplicateAgent)) => Ok(
                    response(409, "application/json", "{\"error\":\"duplicate_agent\"}\n"),
                ),
                Err(ControllerError::Store(fleet_store::StoreError::ConstraintViolation(_))) => {
                    Ok(response(
                        409,
                        "application/json",
                        "{\"error\":\"duplicate_or_constraint_violation\"}\n",
                    ))
                }
                Err(error) => Err(error),
            }
        }
        ("GET", "/api/controller/signing-rotation/status") => {
            let body = serde_json::to_string(&controller_signing_rotation_status_response(
                store,
                identity,
                SystemTime::now(),
            )?)
            .map_err(|error| ControllerError::Json(error.to_string()))?;
            Ok(response(200, "application/json", &format!("{body}\n")))
        }
        ("GET", "/api/controller/signing-rotation/restart-plan") => {
            let body = serde_json::to_string(&controller_signing_rotation_restart_plan_response(
                store,
                identity,
                SystemTime::now(),
            )?)
            .map_err(|error| ControllerError::Json(error.to_string()))?;
            Ok(response(200, "application/json", &format!("{body}\n")))
        }
        ("POST", "/api/controller/signing-rotation/restart-action") => {
            controller_signing_rotation_route_response(controller_signing_rotation_restart_action(
                request_body(request),
                store,
                identity,
                admin_actor,
                SystemTime::now(),
            ))
        }
        ("POST", "/api/controller/signing-rotation/request") => {
            controller_signing_rotation_route_response(controller_signing_rotation_request(
                request_body(request),
                store,
                identity,
                admin_actor,
                SystemTime::now(),
            ))
        }
        ("POST", "/api/controller/signing-rotation/validate") => {
            controller_signing_rotation_route_response(controller_signing_rotation_validate(
                request_body(request),
                store,
                identity,
                metadata,
                admin_actor,
                SystemTime::now(),
            ))
        }
        ("POST", "/api/controller/signing-rotation/activate") => {
            controller_signing_rotation_route_response(controller_signing_rotation_activate(
                store,
                identity,
                admin_actor,
                SystemTime::now(),
            ))
        }
        ("POST", "/api/controller/signing-rotation/retire") => {
            controller_signing_rotation_route_response(controller_signing_rotation_retire(
                store,
                identity,
                admin_actor,
                SystemTime::now(),
            ))
        }
        ("POST", "/api/controller/signing-rotation/fail") => {
            controller_signing_rotation_route_response(controller_signing_rotation_fail(
                request_body(request),
                store,
                identity,
                admin_actor,
                SystemTime::now(),
            ))
        }
        ("POST", "/api/controller/signing-rotation/rollout-trust-bundle") => {
            controller_signing_rotation_route_response(controller_signing_trust_bundle_rollout(
                request_body(request),
                store,
                sessions,
                identity,
                admin_actor,
                SystemTime::now(),
                ControllerSigningTrustBundleRolloutMode::Manual,
            ))
        }
        ("POST", "/api/controller/signing-rotation/rollout-trust-bundle/staged") => {
            controller_signing_rotation_route_response(
                controller_signing_trust_bundle_staged_rollout(
                    request_body(request),
                    store,
                    sessions,
                    identity,
                    admin_actor,
                    SystemTime::now(),
                ),
            )
        }
        ("POST", "/api/controller/signing-rotation/rollout-trust-bundle/retry") => {
            controller_signing_rotation_route_response(controller_signing_trust_bundle_rollout(
                request_body(request),
                store,
                sessions,
                identity,
                admin_actor,
                SystemTime::now(),
                ControllerSigningTrustBundleRolloutMode::Retry,
            ))
        }
        ("POST", "/api/enrollment-tokens") => {
            match create_enrollment_token(request_body(request), store, admin_actor) {
                Ok(body) => Ok(response(201, "application/json", &format!("{body}\n"))),
                Err(ControllerError::Json(message)) => Ok(response(
                    400,
                    "application/json",
                    &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                )),
                Err(ControllerError::Store(fleet_store::StoreError::NotFound)) => Ok(response(
                    404,
                    "application/json",
                    "{\"error\":\"not_found\"}\n",
                )),
                Err(error) => Err(error),
            }
        }
        ("POST", "/api/selectors/preview") => {
            match preview_selector(request_body(request), store) {
                Ok(body) => Ok(response(200, "application/json", &format!("{body}\n"))),
                Err(ControllerError::Json(message)) => Ok(response(
                    400,
                    "application/json",
                    &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                )),
                Err(ControllerError::Store(fleet_store::StoreError::NotFound)) => Ok(response(
                    404,
                    "application/json",
                    "{\"error\":\"not_found\"}\n",
                )),
                Err(error) => Err(error),
            }
        }
        ("POST", "/api/jobs/command") => {
            match create_command_job(request_body(request), store, identity, admin_actor) {
                Ok(output) => {
                    if let Some(sessions) = sessions {
                        dispatch_pending_assignments_for_created_job(
                            store,
                            sessions,
                            &output.job_id,
                        )?;
                    }
                    Ok(response(
                        201,
                        "application/json",
                        &format!("{}\n", output.body),
                    ))
                }
                Err(CreateCommandJobHttpError::BadRequest(message)) => Ok(response(
                    400,
                    "application/json",
                    &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                )),
                Err(CreateCommandJobHttpError::Conflict(message)) => Ok(response(
                    409,
                    "application/json",
                    &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                )),
                Err(CreateCommandJobHttpError::Internal(error)) => Err(error),
            }
        }
        ("POST", "/api/jobs/drift-check") => {
            match create_drift_check_job(request_body(request), store, identity, admin_actor) {
                Ok(output) => {
                    if let Some(sessions) = sessions {
                        dispatch_pending_assignments_for_created_job(
                            store,
                            sessions,
                            &output.job_id,
                        )?;
                    }
                    Ok(response(
                        201,
                        "application/json",
                        &format!("{}\n", output.body),
                    ))
                }
                Err(CreateCommandJobHttpError::BadRequest(message)) => Ok(response(
                    400,
                    "application/json",
                    &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                )),
                Err(CreateCommandJobHttpError::Conflict(message)) => Ok(response(
                    409,
                    "application/json",
                    &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                )),
                Err(CreateCommandJobHttpError::Internal(error)) => Err(error),
            }
        }
        ("POST", "/api/jobs/runbook") => {
            match create_runbook_job(request_body(request), store, identity, admin_actor) {
                Ok(output) => {
                    if let Some(sessions) = sessions {
                        dispatch_pending_assignments_for_created_job(
                            store,
                            sessions,
                            &output.job_id,
                        )?;
                    }
                    Ok(response(
                        201,
                        "application/json",
                        &format!("{}\n", output.body),
                    ))
                }
                Err(CreateCommandJobHttpError::BadRequest(message)) => Ok(response(
                    400,
                    "application/json",
                    &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                )),
                Err(CreateCommandJobHttpError::Conflict(message)) => Ok(response(
                    409,
                    "application/json",
                    &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                )),
                Err(CreateCommandJobHttpError::Internal(error)) => Err(error),
            }
        }
        ("GET", "/api/jobs") => {
            let body = list_jobs(store, sessions)?;
            Ok(response(200, "application/json", &format!("{body}\n")))
        }
        ("GET", "/api/approvals") => {
            let body = list_approvals(raw_path, store)?;
            Ok(response(200, "application/json", &format!("{body}\n")))
        }
        ("POST", "/api/approvals/expire") => match expire_approvals(store) {
            Ok(body) => Ok(response(200, "application/json", &format!("{body}\n"))),
            Err(error) => Err(error),
        },
        ("GET", "/api/remediations") => match list_remediations(raw_path, store) {
            Ok(body) => Ok(response(200, "application/json", &format!("{body}\n"))),
            Err(ControllerError::Json(message)) => Ok(response(
                400,
                "application/json",
                &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
            )),
            Err(error) => Err(error),
        },
        ("GET", path) if path.starts_with("/api/remediations/") => {
            let remediation_id = path
                .trim_start_matches("/api/remediations/")
                .trim_end_matches('/');
            match get_remediation(remediation_id, store)? {
                Some(body) => Ok(response(200, "application/json", &format!("{body}\n"))),
                None => Ok(response(
                    404,
                    "application/json",
                    "{\"error\":\"not_found\"}\n",
                )),
            }
        }
        ("POST", path)
            if path.starts_with("/api/remediations/") && path.ends_with("/approval-request") =>
        {
            let remediation_id = trim_remediation_action_path(path, "/approval-request");
            match create_remediation_approval_request(
                remediation_id,
                request_body(request),
                store,
                admin_actor,
            ) {
                Ok(body) => Ok(response(201, "application/json", &format!("{body}\n"))),
                Err(error) => remediation_http_error_response(error),
            }
        }
        ("POST", path) if path.starts_with("/api/remediations/") && path.ends_with("/approve") => {
            let remediation_id = trim_remediation_action_path(path, "/approve");
            match approve_remediation_job(
                remediation_id,
                request_body(request),
                store,
                identity,
                admin_actor,
            ) {
                Ok((body, job_id)) => {
                    if let Some(sessions) = sessions {
                        dispatch_pending_assignments_for_created_job(store, sessions, &job_id)?;
                    }
                    Ok(response(200, "application/json", &format!("{body}\n")))
                }
                Err(error) => remediation_http_error_response(error),
            }
        }
        ("POST", path) if path.starts_with("/api/remediations/") && path.ends_with("/running") => {
            let remediation_id = trim_remediation_action_path(path, "/running");
            match mark_remediation_running(
                remediation_id,
                request_body(request),
                store,
                admin_actor,
            ) {
                Ok(body) => Ok(response(200, "application/json", &format!("{body}\n"))),
                Err(error) => remediation_http_error_response(error),
            }
        }
        ("POST", path) if path.starts_with("/api/remediations/") && path.ends_with("/result") => {
            let remediation_id = trim_remediation_action_path(path, "/result");
            match record_remediation_result(
                remediation_id,
                request_body(request),
                store,
                admin_actor,
            ) {
                Ok(body) => Ok(response(200, "application/json", &format!("{body}\n"))),
                Err(error) => remediation_http_error_response(error),
            }
        }
        ("POST", path) if path.starts_with("/api/remediations/") && path.ends_with("/verify") => {
            let remediation_id = trim_remediation_action_path(path, "/verify");
            match verify_remediation(remediation_id, request_body(request), store, admin_actor) {
                Ok(body) => Ok(response(200, "application/json", &format!("{body}\n"))),
                Err(error) => remediation_http_error_response(error),
            }
        }
        ("POST", path) if path.starts_with("/api/approvals/") && path.ends_with("/approve") => {
            let approval_id = path
                .trim_start_matches("/api/approvals/")
                .trim_end_matches("/approve")
                .trim_end_matches('/');
            match approve_approval(approval_id, request_body(request), store, admin_actor) {
                Ok(Some((body, job_id))) => {
                    if let Some(sessions) = sessions {
                        dispatch_pending_assignments_for_created_job(store, sessions, &job_id)?;
                    }
                    Ok(response(200, "application/json", &format!("{body}\n")))
                }
                Ok(None) => Ok(response(
                    404,
                    "application/json",
                    "{\"error\":\"not_found\"}\n",
                )),
                Err(ControllerError::Json(message)) => Ok(response(
                    400,
                    "application/json",
                    &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                )),
                Err(error) => Err(error),
            }
        }
        ("POST", path) if path.starts_with("/api/approvals/") && path.ends_with("/reject") => {
            let approval_id = path
                .trim_start_matches("/api/approvals/")
                .trim_end_matches("/reject")
                .trim_end_matches('/');
            match reject_approval(approval_id, request_body(request), store, admin_actor) {
                Ok(Some(body)) => Ok(response(200, "application/json", &format!("{body}\n"))),
                Ok(None) => Ok(response(
                    404,
                    "application/json",
                    "{\"error\":\"not_found\"}\n",
                )),
                Err(ControllerError::Json(message)) => Ok(response(
                    400,
                    "application/json",
                    &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                )),
                Err(error) => Err(error),
            }
        }
        ("GET", path) if path.starts_with("/api/jobs/") && path.ends_with("/output") => {
            let job_id = path
                .trim_start_matches("/api/jobs/")
                .trim_end_matches("/output")
                .trim_end_matches('/');
            let body = list_job_output(job_id, store)?;
            Ok(response(200, "application/json", &format!("{body}\n")))
        }
        ("GET", path) if path.starts_with("/api/jobs/") && path.contains("/artifacts/") => {
            let Some((job_id, artifact_id)) = parse_job_artifact_path(path) else {
                return Ok(response(
                    404,
                    "application/json",
                    "{\"error\":\"not_found\"}\n",
                ));
            };
            let Some(artifact_store) = artifact_store else {
                return Err(ControllerError::Store(fleet_store::StoreError::Domain(
                    "artifact store is not configured".to_owned(),
                )));
            };
            match get_job_artifact(job_id, artifact_id, store, artifact_store)? {
                ArtifactHttpResult::Found(body) => {
                    Ok(response(200, "application/json", &format!("{body}\n")))
                }
                ArtifactHttpResult::NotFound => Ok(response(
                    404,
                    "application/json",
                    "{\"error\":\"not_found\"}\n",
                )),
                ArtifactHttpResult::Corrupt => Ok(response(
                    409,
                    "application/json",
                    "{\"error\":\"artifact_corrupt\"}\n",
                )),
            }
        }
        ("POST", path) if path.starts_with("/api/jobs/") && path.ends_with("/cancel") => {
            let job_id = path
                .trim_start_matches("/api/jobs/")
                .trim_end_matches("/cancel")
                .trim_end_matches('/');
            match cancel_job(job_id, request_body(request), store, sessions) {
                Ok(Some(body)) => Ok(response(200, "application/json", &format!("{body}\n"))),
                Ok(None) => Ok(response(
                    404,
                    "application/json",
                    "{\"error\":\"not_found\"}\n",
                )),
                Err(ControllerError::Json(message)) => Ok(response(
                    400,
                    "application/json",
                    &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                )),
                Err(error) => Err(error),
            }
        }
        ("GET", path) if path.starts_with("/api/jobs/") => {
            let job_id = path.trim_start_matches("/api/jobs/").trim_end_matches('/');
            match get_job(job_id, store, sessions)? {
                Some(body) => Ok(response(200, "application/json", &format!("{body}\n"))),
                None => Ok(response(
                    404,
                    "application/json",
                    "{\"error\":\"not_found\"}\n",
                )),
            }
        }
        ("GET", "/api/agents") => {
            let body = list_agents(store, sessions)?;
            Ok(response(200, "application/json", &format!("{body}\n")))
        }
        ("POST", path)
            if path.starts_with("/api/agents/")
                && path.ends_with("/certificate-lifecycle/request-issuance") =>
        {
            let agent_id = trim_agent_action_path(path, "/certificate-lifecycle/request-issuance");
            match request_agent_certificate_issuance(
                agent_id,
                store,
                sessions,
                admin_actor,
                SystemTime::now(),
            ) {
                Ok(Some(body)) => Ok(response(202, "application/json", &format!("{body}\n"))),
                Ok(None) => Ok(response(
                    404,
                    "application/json",
                    "{\"error\":\"not_found\"}\n",
                )),
                Err(error) => agent_certificate_lifecycle_http_error_response(error),
            }
        }
        ("GET", path)
            if path.starts_with("/api/agents/")
                && path.ends_with("/certificate-lifecycle/status") =>
        {
            let agent_id = trim_agent_action_path(path, "/certificate-lifecycle/status");
            match get_agent_certificate_lifecycle_status(agent_id, store) {
                Ok(Some(body)) => Ok(response(200, "application/json", &format!("{body}\n"))),
                Ok(None) => Ok(response(
                    404,
                    "application/json",
                    "{\"error\":\"not_found\"}\n",
                )),
                Err(error) => agent_certificate_lifecycle_http_error_response(error),
            }
        }
        ("GET", "/api/policies") => {
            let body = list_policies(store)?;
            Ok(response(200, "application/json", &format!("{body}\n")))
        }
        ("POST", "/api/policies") => match save_policy(request_body(request), store, admin_actor) {
            Ok(body) => Ok(response(201, "application/json", &format!("{body}\n"))),
            Err(ControllerError::Json(message)) => Ok(response(
                400,
                "application/json",
                &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
            )),
            Err(ControllerError::Store(fleet_store::StoreError::Domain(message))) => Ok(response(
                400,
                "application/json",
                &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
            )),
            Err(error) => Err(error),
        },
        ("POST", path) if path.starts_with("/api/policies/") && path.ends_with("/assignments") => {
            let policy_id = path
                .trim_start_matches("/api/policies/")
                .trim_end_matches("/assignments")
                .trim_end_matches('/');
            match assign_policy(policy_id, request_body(request), store, admin_actor) {
                Ok(body) => Ok(response(201, "application/json", &format!("{body}\n"))),
                Err(ControllerError::Json(message)) => Ok(response(
                    400,
                    "application/json",
                    &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                )),
                Err(ControllerError::Store(fleet_store::StoreError::NotFound)) => Ok(response(
                    404,
                    "application/json",
                    "{\"error\":\"not_found\"}\n",
                )),
                Err(error) => Err(error),
            }
        }
        ("POST", path) if path.starts_with("/api/policies/") && path.ends_with("/schedules") => {
            let policy_id = path
                .trim_start_matches("/api/policies/")
                .trim_end_matches("/schedules")
                .trim_end_matches('/');
            match schedule_policy_drift(policy_id, request_body(request), store, admin_actor) {
                Ok(body) => Ok(response(201, "application/json", &format!("{body}\n"))),
                Err(ControllerError::Json(message)) => Ok(response(
                    400,
                    "application/json",
                    &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                )),
                Err(ControllerError::Store(fleet_store::StoreError::NotFound)) => Ok(response(
                    404,
                    "application/json",
                    "{\"error\":\"not_found\"}\n",
                )),
                Err(error) => Err(error),
            }
        }
        ("GET", "/api/drift/scheduled") => {
            let body = list_due_scheduled_drift(store)?;
            Ok(response(200, "application/json", &format!("{body}\n")))
        }
        ("GET", "/api/audit") => {
            let body = list_audit_events(store)?;
            Ok(response(200, "application/json", &format!("{body}\n")))
        }
        ("GET", "/api/audit/export") => match export_audit_events(raw_path, store) {
            Ok(body) => Ok(response(200, "application/json", &format!("{body}\n"))),
            Err(ControllerError::Json(message)) => Ok(response(
                400,
                "application/json",
                &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
            )),
            Err(error) => Err(error),
        },
        ("GET", path) if path.starts_with("/api/agents/") && path.ends_with("/policies") => {
            let agent_id = path
                .trim_start_matches("/api/agents/")
                .trim_end_matches("/policies")
                .trim_end_matches('/');
            let body = list_agent_policies(agent_id, store)?;
            Ok(response(200, "application/json", &format!("{body}\n")))
        }
        ("GET", path) if path.starts_with("/api/agents/") && path.ends_with("/facts/latest") => {
            let agent_id = path
                .trim_start_matches("/api/agents/")
                .trim_end_matches("/facts/latest")
                .trim_end_matches('/');
            match latest_facts(agent_id, store)? {
                Some(body) => Ok(response(200, "application/json", &format!("{body}\n"))),
                None => Ok(response(200, "application/json", "null\n")),
            }
        }
        ("GET", path) if path.starts_with("/api/agents/") && path.ends_with("/facts") => {
            let agent_id = path
                .trim_start_matches("/api/agents/")
                .trim_end_matches("/facts")
                .trim_end_matches('/');
            match list_facts_snapshots(agent_id, raw_path, store) {
                Ok(body) => Ok(response(200, "application/json", &format!("{body}\n"))),
                Err(ControllerError::Json(message)) => Ok(response(
                    400,
                    "application/json",
                    &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                )),
                Err(error) => Err(error),
            }
        }
        ("GET", path) if path.starts_with("/api/agents/") && path.ends_with("/metrics/latest") => {
            let agent_id = path
                .trim_start_matches("/api/agents/")
                .trim_end_matches("/metrics/latest")
                .trim_end_matches('/');
            match latest_metrics(agent_id, store)? {
                Some(body) => Ok(response(200, "application/json", &format!("{body}\n"))),
                None => Ok(response(200, "application/json", "null\n")),
            }
        }
        ("GET", path) if path.starts_with("/api/agents/") && path.ends_with("/metrics") => {
            let agent_id = path
                .trim_start_matches("/api/agents/")
                .trim_end_matches("/metrics")
                .trim_end_matches('/');
            match list_metrics_snapshots(agent_id, raw_path, store) {
                Ok(body) => Ok(response(200, "application/json", &format!("{body}\n"))),
                Err(ControllerError::Json(message)) => Ok(response(
                    400,
                    "application/json",
                    &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                )),
                Err(error) => Err(error),
            }
        }
        ("GET", path) if path.starts_with("/api/agents/") && path.ends_with("/logs") => {
            let agent_id = path
                .trim_start_matches("/api/agents/")
                .trim_end_matches("/logs")
                .trim_end_matches('/');
            match list_agent_logs(agent_id, raw_path, store) {
                Ok(body) => Ok(response(200, "application/json", &format!("{body}\n"))),
                Err(ControllerError::Json(message)) => Ok(response(
                    400,
                    "application/json",
                    &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                )),
                Err(error) => Err(error),
            }
        }
        ("GET", path) if path.starts_with("/api/agents/") && path.ends_with("/drift/latest") => {
            let agent_id = path
                .trim_start_matches("/api/agents/")
                .trim_end_matches("/drift/latest")
                .trim_end_matches('/');
            match latest_drift_report(agent_id, store)? {
                Some(body) => Ok(response(200, "application/json", &format!("{body}\n"))),
                None => Ok(response(200, "application/json", "null\n")),
            }
        }
        ("GET", path) if path.starts_with("/api/agents/") && path.ends_with("/drift") => {
            let agent_id = path
                .trim_start_matches("/api/agents/")
                .trim_end_matches("/drift")
                .trim_end_matches('/');
            match list_drift_reports(agent_id, raw_path, store) {
                Ok(body) => Ok(response(200, "application/json", &format!("{body}\n"))),
                Err(ControllerError::Json(message)) => Ok(response(
                    400,
                    "application/json",
                    &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                )),
                Err(error) => Err(error),
            }
        }
        ("POST", path) if path.starts_with("/api/agents/") && path.ends_with("/revoke-key") => {
            let agent_id = path
                .trim_start_matches("/api/agents/")
                .trim_end_matches("/revoke-key")
                .trim_end_matches('/');
            match revoke_agent_key(agent_id, store, sessions, admin_actor) {
                Ok(Some(body)) => Ok(response(200, "application/json", &format!("{body}\n"))),
                Ok(None) => Ok(response(
                    404,
                    "application/json",
                    "{\"error\":\"not_found\"}\n",
                )),
                Err(ControllerError::Store(fleet_store::StoreError::Domain(message))) => {
                    Ok(response(
                        400,
                        "application/json",
                        &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                    ))
                }
                Err(error) => Err(error),
            }
        }
        ("GET", path) if path.starts_with("/api/agents/") && path != "/api/agents/ws" => {
            let agent_id = path.trim_start_matches("/api/agents/");
            match get_agent(agent_id, store, sessions)? {
                Some(body) => Ok(response(200, "application/json", &format!("{body}\n"))),
                None => Ok(response(
                    404,
                    "application/json",
                    "{\"error\":\"not_found\"}\n",
                )),
            }
        }
        ("PATCH", path) if path.starts_with("/api/agents/") && path.ends_with("/labels") => {
            let agent_id = path
                .trim_start_matches("/api/agents/")
                .trim_end_matches("/labels")
                .trim_end_matches('/');
            match update_agent_labels(
                agent_id,
                request_body(request),
                store,
                sessions,
                admin_actor,
            ) {
                Ok(Some(body)) => Ok(response(200, "application/json", &format!("{body}\n"))),
                Ok(None) => Ok(response(
                    404,
                    "application/json",
                    "{\"error\":\"not_found\"}\n",
                )),
                Err(ControllerError::Store(fleet_store::StoreError::Domain(message))) => {
                    Ok(response(
                        400,
                        "application/json",
                        &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                    ))
                }
                Err(ControllerError::Json(message)) => Ok(response(
                    400,
                    "application/json",
                    &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                )),
                Err(error) => Err(error),
            }
        }
        ("GET", "/api/enrollment-tokens") => {
            let body = list_enrollment_tokens(store)?;
            Ok(response(200, "application/json", &format!("[{body}]\n")))
        }
        ("DELETE", path) if path.starts_with("/api/enrollment-tokens/") => {
            let id = path.trim_start_matches("/api/enrollment-tokens/");
            if revoke_enrollment_token(id, store, admin_actor)? {
                Ok(response(204, "application/json", ""))
            } else {
                Ok(response(
                    404,
                    "application/json",
                    "{\"error\":\"not_found\"}\n",
                ))
            }
        }
        _ => Ok(response(
            404,
            "application/json",
            "{\"error\":\"not_found\"}\n",
        )),
    }
}

fn controller_identity_response(
    identity: &ControllerIdentity,
    metadata: &ControllerRuntimeMetadata,
) -> ControllerIdentityResponse {
    ControllerIdentityResponse {
        controller_public_key: identity.public_key.clone(),
        controller_fingerprint: identity.fingerprint.clone(),
        controller_signing_public_key: identity.public_key.clone(),
        controller_signing_fingerprint: identity.fingerprint.clone(),
        tls_endpoint: ControllerTlsEndpointResponse {
            external_url: metadata.external_url.clone(),
            tls_enabled: metadata.tls_enabled,
        },
    }
}

fn controller_signing_rotation_status_response<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    identity: &ControllerIdentity,
    now: SystemTime,
) -> Result<ControllerSigningRotationStatusResponse, ControllerError> {
    let active_fingerprint = fleet_domain::SigningKeyFingerprint::new(identity.fingerprint.clone())
        .map_err(|error| ControllerError::SigningKeyRotation(error.to_string()))?;
    let status = QueryControllerSigningRotationStatus::execute(
        &store.into(),
        ControllerSigningRotationStatusInput {
            controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
            active_fingerprint,
            now,
        },
    )
    .map_err(controller_signing_rotation_load_error)?;
    Ok(controller_signing_rotation_status_to_response(status))
}

fn controller_signing_rotation_status_to_response(
    status: ControllerSigningRotationStatus,
) -> ControllerSigningRotationStatusResponse {
    ControllerSigningRotationStatusResponse {
        controller_id: status.controller_id,
        persisted_record_present: status.persisted_record_present,
        persisted_state: status.persisted_state,
        readiness: status.readiness.as_str().to_owned(),
        active_signing_fingerprint_prefix: status.active_signing_fingerprint_prefix,
        selected_signing_fingerprint_prefix: status.selected_signing_fingerprint_prefix,
        old_fingerprint_prefix: status.old_fingerprint_prefix,
        new_fingerprint_prefix: status.new_fingerprint_prefix,
        requested_at_ms: status.requested_at_ms,
        validated_at_ms: status.validated_at_ms,
        activated_at_ms: status.activated_at_ms,
        old_key_verifies_until_ms: status.old_key_verifies_until_ms,
        retired_at_ms: status.retired_at_ms,
        failed_at_ms: status.failed_at_ms,
        bootstrap_guard: status.bootstrap_guard,
        agent_trust_rollout: status.agent_trust_rollout,
    }
}

fn controller_signing_rotation_restart_plan_response<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    identity: &ControllerIdentity,
    now: SystemTime,
) -> Result<ControllerSigningRotationRestartPlanResponse, ControllerError> {
    let status = controller_signing_rotation_status_response(store, identity, now)?;
    Ok(controller_signing_rotation_restart_plan_from_status(status))
}

fn controller_signing_rotation_restart_plan_from_status(
    status: ControllerSigningRotationStatusResponse,
) -> ControllerSigningRotationRestartPlanResponse {
    let restart_required = status.bootstrap_guard != "active_matches_selected";
    let recommended_action = if restart_required {
        "restart_controller_process"
    } else {
        match status.readiness.as_str() {
            "new_material_validated_waiting_activation" => "activate_rotation_when_ready",
            "old_key_retirement_available" => "retire_old_key_when_agents_verified",
            "terminal_failed" | "terminal_canceled" => "review_terminal_rotation_state",
            _ => "none",
        }
    }
    .to_owned();
    let blocked_reason = restart_required.then(|| {
        "active signer does not match persisted selected signer; restart controller with validated signing material and verify status before retiring old key".to_owned()
    });
    ControllerSigningRotationRestartPlanResponse {
        controller_id: status.controller_id,
        restart_required,
        reload_supported: false,
        recommended_action,
        readiness: status.readiness,
        bootstrap_guard: status.bootstrap_guard,
        agent_trust_rollout: status.agent_trust_rollout,
        active_signing_fingerprint_prefix: status.active_signing_fingerprint_prefix,
        selected_signing_fingerprint_prefix: status.selected_signing_fingerprint_prefix,
        blocked_reason,
        verification_commands: vec![
            "sponzey controller signing-rotation-status --controller-url <controller-url>"
                .to_owned(),
            "sponzey controller signing-rotation restart-plan --controller-url <controller-url>"
                .to_owned(),
        ],
        safety_notes: vec![
            "this version does not support in-process controller signing key reload".to_owned(),
            "restart the controller process through the service manager after validated material is in place".to_owned(),
            "verify bootstrap_guard is active_matches_selected before retiring the old key"
                .to_owned(),
        ],
    }
}

fn controller_signing_rotation_restart_action<'a>(
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    identity: &ControllerIdentity,
    actor: &str,
    now: SystemTime,
) -> Result<String, ControllerError> {
    let request: ControllerSigningRotationRestartActionBody =
        parse_controller_signing_rotation_body(body)?;
    if !request.confirm_external_restart {
        return Err(ControllerError::SigningKeyRotation(
            "external controller restart confirmation is required".to_owned(),
        ));
    }
    let plan = controller_signing_rotation_restart_plan_response(store, identity, now)?;
    if !plan.restart_required {
        return Err(ControllerError::SigningKeyRotation(
            "controller restart is not required for current signing rotation state".to_owned(),
        ));
    }
    let response_body = ControllerSigningRotationRestartActionResponse {
        controller_id: plan.controller_id,
        action: "external_service_manager_restart".to_owned(),
        action_status: "audit_recorded_external_restart_required".to_owned(),
        restart_required: plan.restart_required,
        reload_supported: plan.reload_supported,
        readiness: plan.readiness,
        bootstrap_guard: plan.bootstrap_guard,
        active_signing_fingerprint_prefix: plan.active_signing_fingerprint_prefix,
        selected_signing_fingerprint_prefix: plan.selected_signing_fingerprint_prefix,
        service_command: "sponzey controller restart-service --dry-run".to_owned(),
        verification_commands: plan.verification_commands,
        safety_notes: vec![
            "controller restart is executed outside the HTTP handler through an explicit service-manager command".to_owned(),
            "this action records operator intent only and does not reload keys or mutate runtime config".to_owned(),
            "after service restart, verify bootstrap_guard is active_matches_selected".to_owned(),
        ],
    };
    store.into().write_audit_event(AuditEvent {
        category: AuditCategory::Security,
        action: "controller_signing_rotation_restart_action_requested".to_owned(),
        actor: AuditActor::new(actor.to_owned()),
        target: AuditTarget::new(DEFAULT_CONTROLLER_ID.to_owned()),
        value: AuditValue::Plain(format!(
            "action={},action_status={},restart_required={},reload_supported={},bootstrap_guard={},active_signing_fingerprint_prefix={},selected_signing_fingerprint_prefix={},reason={}",
            response_body.action,
            response_body.action_status,
            response_body.restart_required,
            response_body.reload_supported,
            response_body.bootstrap_guard,
            response_body.active_signing_fingerprint_prefix,
            response_body.selected_signing_fingerprint_prefix,
            fleet_core::redact_secret(request.reason.as_deref().unwrap_or("operator_requested_external_restart"))
        )),
        occurred_at: now,
    })?;
    let body = serde_json::to_string(&response_body)
        .map_err(|error| ControllerError::Json(error.to_string()))?;
    Ok(response(200, "application/json", &format!("{body}\n")))
}

fn controller_signing_rotation_request<'a>(
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    identity: &ControllerIdentity,
    actor: &str,
    now: SystemTime,
) -> Result<String, ControllerError> {
    let request: ControllerSigningRotationRequestBody =
        parse_controller_signing_rotation_body(body)?;
    let old_fingerprint = signing_fingerprint_from_str(&identity.fingerprint)?;
    let new_fingerprint = signing_fingerprint_from_str(&request.new_fingerprint)?;
    let old_key_verifies_until = signing_rotation_requested_until(&request, now)?;
    let mut repo = store.into();
    let mut audit = ControllerAuditRepository {
        store: store.into(),
    };
    RequestSigningKeyRotation::execute(
        &mut repo,
        &mut audit,
        RequestSigningKeyRotationInput {
            controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
            actor: actor.to_owned(),
            old_fingerprint,
            new_fingerprint,
            requested_at: now,
            old_key_verifies_until,
        },
    )
    .map_err(controller_signing_rotation_operation_error)?;
    controller_signing_rotation_ok_response(store, identity, now)
}

fn controller_signing_rotation_validate<'a>(
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    identity: &ControllerIdentity,
    metadata: &ControllerRuntimeMetadata,
    actor: &str,
    now: SystemTime,
) -> Result<String, ControllerError> {
    let request: ControllerSigningRotationValidateBody =
        parse_controller_signing_rotation_body(body)?;
    let active = metadata
        .controller_signing_key_file_pair()
        .ok_or_else(|| {
            ControllerError::SigningKeyRotation(
                "controller signing key file context is unavailable".to_owned(),
            )
        })
        .map_err(controller_signing_rotation_safe_error_response)?;
    let candidate = ControllerSigningKeyFilePair {
        public_key_path: request.candidate_public_key_path,
        private_key_path: request.candidate_private_key_path,
    };
    let validated =
        validate_controller_signing_key_candidate(&ControllerSigningKeyCandidateInput {
            candidate,
            active,
            disallowed_paths: metadata.disallowed_signing_candidate_paths(),
            challenge: CONTROLLER_SIGNING_VALIDATION_CHALLENGE.to_owned(),
        })
        .map_err(controller_signing_rotation_safe_error_response)?;
    let mut repo = store.into();
    let mut audit = ControllerAuditRepository {
        store: store.into(),
    };
    ValidateSigningKeyRotation::execute(
        &mut repo,
        &mut audit,
        ValidateSigningKeyRotationInput {
            controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
            actor: actor.to_owned(),
            validated_new_fingerprint: signing_fingerprint_from_str(&validated.fingerprint)?,
            validated_at: now,
        },
    )
    .map_err(controller_signing_rotation_operation_error)?;
    controller_signing_rotation_ok_response(store, identity, now)
}

fn controller_signing_rotation_activate<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    identity: &ControllerIdentity,
    actor: &str,
    now: SystemTime,
) -> Result<String, ControllerError> {
    let mut repo = store.into();
    let mut audit = ControllerAuditRepository {
        store: store.into(),
    };
    ActivateSigningKeyRotation::execute(
        &mut repo,
        &mut audit,
        ActivateSigningKeyRotationInput {
            controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
            actor: actor.to_owned(),
            activated_at: now,
        },
    )
    .map_err(controller_signing_rotation_operation_error)?;
    controller_signing_rotation_ok_response(store, identity, now)
}

fn controller_signing_rotation_retire<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    identity: &ControllerIdentity,
    actor: &str,
    now: SystemTime,
) -> Result<String, ControllerError> {
    let mut repo = store.into();
    let mut audit = ControllerAuditRepository {
        store: store.into(),
    };
    RetireSigningKeyRotation::execute(
        &mut repo,
        &mut audit,
        RetireSigningKeyRotationInput {
            controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
            actor: actor.to_owned(),
            retired_at: now,
        },
    )
    .map_err(controller_signing_rotation_operation_error)?;
    controller_signing_rotation_ok_response(store, identity, now)
}

fn controller_signing_rotation_fail<'a>(
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    identity: &ControllerIdentity,
    actor: &str,
    now: SystemTime,
) -> Result<String, ControllerError> {
    let request: ControllerSigningRotationReasonBody =
        parse_controller_signing_rotation_body(body)?;
    let mut repo = store.into();
    let mut audit = ControllerAuditRepository {
        store: store.into(),
    };
    FailSigningKeyRotation::execute(
        &mut repo,
        &mut audit,
        FailSigningKeyRotationInput {
            controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
            actor: actor.to_owned(),
            failed_at: now,
            failure_summary: fleet_core::redact_secret(
                request
                    .reason
                    .as_deref()
                    .unwrap_or("operator requested failure"),
            ),
        },
    )
    .map_err(controller_signing_rotation_operation_error)?;
    controller_signing_rotation_ok_response(store, identity, now)
}

fn controller_signing_trust_bundle_rollout<'a>(
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
    identity: &ControllerIdentity,
    actor: &str,
    now: SystemTime,
    mode: ControllerSigningTrustBundleRolloutMode,
) -> Result<String, ControllerError> {
    let request: ControllerSigningTrustBundleRolloutBody =
        parse_controller_signing_rotation_body(body)?;
    if matches!(request.max_agent_count, Some(0)) {
        return Err(ControllerError::SigningKeyRotation(
            "trust bundle rollout max_agent_count must be greater than zero".to_owned(),
        ));
    }
    let sessions = sessions.ok_or_else(|| {
        ControllerError::SigningKeyRotation(
            "agent session registry is unavailable for trust bundle rollout".to_owned(),
        )
    })?;
    let store_ref = store.into();
    let record = store_ref
        .load_signing_key_rotation(DEFAULT_CONTROLLER_ID)
        .map_err(controller_signing_rotation_load_error)?
        .ok_or_else(|| {
            ControllerError::SigningKeyRotation(
                "controller signing rotation record is required for trust bundle rollout"
                    .to_owned(),
            )
        })?;
    let status = controller_signing_rotation_status_response(store, identity, now)?;
    if status.bootstrap_guard != "active_matches_selected" {
        return Err(ControllerError::SigningKeyRotation(
            "controller restart is required before trust bundle rollout".to_owned(),
        ));
    }
    let entries = controller_signing_trust_bundle_entries_from_rotation(
        &record.rotation,
        identity,
        request.previous_public_key_path.as_deref(),
    )?;
    let agent_results = dispatch_controller_signing_trust_bundle(
        sessions,
        &entries,
        request.agent_ids,
        request.max_agent_count,
        now,
    )?;
    let attempted_count = agent_results.len();
    let updated_count = agent_results
        .iter()
        .filter(|result| result.status == "sent")
        .count();
    let skipped_count = agent_results
        .iter()
        .filter(|result| result.status.starts_with("skipped"))
        .count();
    let failed_count = attempted_count
        .saturating_sub(updated_count)
        .saturating_sub(skipped_count);
    let current_fingerprint_prefix = entries
        .iter()
        .find(|entry| entry.role == fleet_protocol::ControllerSigningTrustRoleWire::Current)
        .map(|entry| controller_signing_fingerprint_prefix(&entry.fingerprint).to_owned())
        .unwrap_or_default();
    let previous_fingerprint_prefix = entries
        .iter()
        .find(|entry| entry.role == fleet_protocol::ControllerSigningTrustRoleWire::Previous)
        .map(|entry| controller_signing_fingerprint_prefix(&entry.fingerprint).to_owned());
    let response_body = ControllerSigningTrustBundleRolloutResponse {
        controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
        persisted_state: record.rotation.state().as_str().to_owned(),
        attempted_count,
        updated_count,
        skipped_count,
        failed_count,
        entries_count: entries.len(),
        current_fingerprint_prefix: current_fingerprint_prefix.clone(),
        previous_fingerprint_prefix: previous_fingerprint_prefix.clone(),
        agent_results,
    };
    store.into().write_audit_event(AuditEvent {
        category: AuditCategory::Security,
        action: mode.audit_action().to_owned(),
        actor: AuditActor::new(actor.to_owned()),
        target: AuditTarget::new(DEFAULT_CONTROLLER_ID.to_owned()),
        value: AuditValue::Plain(format!(
            "mode={},state={},attempted_count={},updated_count={},skipped_count={},failed_count={},entries_count={},current_fingerprint_prefix={},previous_fingerprint_prefix={}",
            mode.as_str(),
            response_body.persisted_state,
            attempted_count,
            updated_count,
            skipped_count,
            failed_count,
            response_body.entries_count,
            current_fingerprint_prefix,
            previous_fingerprint_prefix.unwrap_or_else(|| "none".to_owned())
        )),
        occurred_at: now,
    })?;
    let body = serde_json::to_string(&response_body)
        .map_err(|error| ControllerError::Json(error.to_string()))?;
    Ok(response(200, "application/json", &format!("{body}\n")))
}

fn controller_signing_trust_bundle_staged_rollout<'a>(
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
    identity: &ControllerIdentity,
    actor: &str,
    now: SystemTime,
) -> Result<String, ControllerError> {
    let request: ControllerSigningTrustBundleStagedRolloutBody =
        parse_controller_signing_rotation_body(body)?;
    let sessions = sessions.ok_or_else(|| {
        ControllerError::SigningKeyRotation(
            "agent session registry is unavailable for staged trust bundle rollout".to_owned(),
        )
    })?;
    let store_ref = store.into();
    let record = store_ref
        .load_signing_key_rotation(DEFAULT_CONTROLLER_ID)
        .map_err(controller_signing_rotation_load_error)?
        .ok_or_else(|| {
            ControllerError::SigningKeyRotation(
                "controller signing rotation record is required for staged trust bundle rollout"
                    .to_owned(),
            )
        })?;
    let status = controller_signing_rotation_status_response(store, identity, now)?;
    if status.bootstrap_guard != "active_matches_selected" {
        return Err(ControllerError::SigningKeyRotation(
            "controller restart is required before staged trust bundle rollout".to_owned(),
        ));
    }
    let entries = controller_signing_trust_bundle_entries_from_rotation(
        &record.rotation,
        identity,
        request.previous_public_key_path.as_deref(),
    )?;
    let current_fingerprint = entries
        .iter()
        .find(|entry| entry.role == fleet_protocol::ControllerSigningTrustRoleWire::Current)
        .map(|entry| entry.fingerprint.clone())
        .ok_or_else(|| {
            ControllerError::SigningKeyRotation(
                "staged trust bundle rollout requires current fingerprint".to_owned(),
            )
        })?;
    let previous_fingerprint = entries
        .iter()
        .find(|entry| entry.role == fleet_protocol::ControllerSigningTrustRoleWire::Previous)
        .map(|entry| entry.fingerprint.clone());
    let target_ids = controller_signing_staged_rollout_target_ids(sessions, request.agent_ids)?;
    let observations = {
        let sessions = sessions.lock().map_err(|_| {
            ControllerError::Store(fleet_store::StoreError::Domain(
                "session registry lock poisoned".to_owned(),
            ))
        })?;
        sessions.controller_signing_staged_rollout_targets(&target_ids, &current_fingerprint)
    };
    let config = fleet_domain::ControllerSigningStagedRolloutConfig {
        batch_size: request.batch_size,
        max_failures: request.max_failures,
        ack_timeout: Duration::from_secs(request.ack_timeout_seconds),
    };
    let mut staged_rollout = controller_signing_staged_rollout_for_tick(
        store_ref,
        target_ids.clone(),
        config,
        &current_fingerprint,
        previous_fingerprint.as_deref(),
    )?;
    controller_signing_staged_rollout_observe_waiting(&mut staged_rollout, &observations, now)?;
    let (planned_agent_ids, already_current_count, unavailable_count, pending_count) =
        if staged_rollout.state().is_terminal()
            || staged_rollout.state()
                == fleet_domain::ControllerSigningStagedRolloutState::WaitingForAck
        {
            let (already_current_count, unavailable_count, pending_count) =
                controller_signing_staged_rollout_counts(&staged_rollout);
            (
                Vec::new(),
                already_current_count,
                unavailable_count,
                pending_count,
            )
        } else {
            let plan = staged_rollout
                .plan_next_batch(&observations, now)
                .map_err(controller_signing_staged_rollout_error)?;
            (
                plan.agent_ids,
                plan.already_current_count,
                plan.unavailable_count,
                plan.pending_count,
            )
        };
    let agent_results = if planned_agent_ids.is_empty() {
        Vec::new()
    } else {
        let results = dispatch_controller_signing_trust_bundle(
            sessions,
            &entries,
            planned_agent_ids.clone(),
            Some(planned_agent_ids.len()),
            now,
        )?;
        let sent_agent_ids = results
            .iter()
            .filter(|result| result.status == "sent")
            .map(|result| result.agent_id.clone())
            .collect::<Vec<_>>();
        if !sent_agent_ids.is_empty() {
            staged_rollout
                .batch_dispatched(&sent_agent_ids, now)
                .map_err(controller_signing_staged_rollout_error)?;
        }
        results
    };
    let attempted_count = agent_results.len();
    let updated_count = agent_results
        .iter()
        .filter(|result| result.status == "sent")
        .count();
    let dispatch_skipped_count = agent_results
        .iter()
        .filter(|result| result.status.starts_with("skipped"))
        .count();
    let skipped_count = already_current_count + unavailable_count + dispatch_skipped_count;
    let dispatch_failed_count = attempted_count
        .saturating_sub(updated_count)
        .saturating_sub(dispatch_skipped_count);
    let failed_count = staged_rollout.snapshot().failed_agent_ids.len() + dispatch_failed_count;
    let current_fingerprint_prefix =
        controller_signing_fingerprint_prefix(&current_fingerprint).to_owned();
    let previous_fingerprint_prefix = previous_fingerprint
        .as_deref()
        .map(|fingerprint| controller_signing_fingerprint_prefix(fingerprint).to_owned());
    let mut staged_store = store.into();
    staged_store.save_controller_signing_staged_rollout(ControllerSigningStagedRolloutRecord {
        controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
        current_fingerprint: current_fingerprint.clone(),
        previous_fingerprint: previous_fingerprint.clone(),
        rollout: staged_rollout.clone(),
        updated_at: now,
    })?;
    let response_body = ControllerSigningTrustBundleStagedRolloutResponse {
        controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
        persisted_state: record.rotation.state().as_str().to_owned(),
        rollout_state: staged_rollout.state().as_str().to_owned(),
        target_count: target_ids.len(),
        planned_count: planned_agent_ids.len(),
        attempted_count,
        updated_count,
        skipped_count,
        failed_count,
        already_current_count,
        unavailable_count,
        pending_count,
        entries_count: entries.len(),
        current_fingerprint_prefix: current_fingerprint_prefix.clone(),
        previous_fingerprint_prefix: previous_fingerprint_prefix.clone(),
        agent_results,
    };
    store.into().write_audit_event(AuditEvent {
        category: AuditCategory::Security,
        action: "controller_signing_trust_bundle_staged_rollout".to_owned(),
        actor: AuditActor::new(actor.to_owned()),
        target: AuditTarget::new(DEFAULT_CONTROLLER_ID.to_owned()),
        value: AuditValue::Plain(format!(
            "mode=staged,state={},rollout_state={},target_count={},planned_count={},updated_count={},skipped_count={},failed_count={},already_current_count={},unavailable_count={},pending_count={},entries_count={},current_fingerprint_prefix={},previous_fingerprint_prefix={}",
            response_body.persisted_state,
            response_body.rollout_state,
            response_body.target_count,
            response_body.planned_count,
            response_body.updated_count,
            response_body.skipped_count,
            response_body.failed_count,
            response_body.already_current_count,
            response_body.unavailable_count,
            response_body.pending_count,
            response_body.entries_count,
            current_fingerprint_prefix,
            previous_fingerprint_prefix.unwrap_or_else(|| "none".to_owned())
        )),
        occurred_at: now,
    })?;
    let body = serde_json::to_string(&response_body)
        .map_err(|error| ControllerError::Json(error.to_string()))?;
    Ok(response(200, "application/json", &format!("{body}\n")))
}

fn controller_signing_staged_rollout_for_tick(
    store: ControllerStoreRef<'_>,
    target_ids: Vec<String>,
    config: fleet_domain::ControllerSigningStagedRolloutConfig,
    current_fingerprint: &str,
    previous_fingerprint: Option<&str>,
) -> Result<fleet_domain::ControllerSigningStagedRollout, ControllerError> {
    let persisted = store
        .load_controller_signing_staged_rollout(DEFAULT_CONTROLLER_ID)
        .map_err(ControllerError::Store)?;
    if let Some(record) = persisted {
        let fingerprint_matches = record.current_fingerprint == current_fingerprint
            && record.previous_fingerprint.as_deref() == previous_fingerprint;
        if fingerprint_matches {
            let snapshot = record.rollout.snapshot();
            if snapshot.target_ids == target_ids && snapshot.config == config {
                return Ok(record.rollout);
            }
            if !snapshot.state.is_terminal() {
                return Err(ControllerError::SigningKeyRotation(
                    "persisted staged rollout target or configuration differs from request"
                        .to_owned(),
                ));
            }
        }
    }
    fleet_domain::ControllerSigningStagedRollout::new(target_ids, config)
        .map_err(controller_signing_staged_rollout_error)
}

fn controller_signing_staged_rollout_observe_waiting(
    rollout: &mut fleet_domain::ControllerSigningStagedRollout,
    observations: &[fleet_domain::ControllerSigningStagedRolloutTarget],
    now: SystemTime,
) -> Result<(), ControllerError> {
    if rollout.state().is_terminal() {
        return Ok(());
    }
    for observation in observations
        .iter()
        .filter(|observation| observation.accepted_current)
    {
        rollout
            .ack_observed(
                &observation.agent_id,
                observation.acknowledged_at.unwrap_or(now),
            )
            .map_err(controller_signing_staged_rollout_error)?;
    }
    if rollout.state() == fleet_domain::ControllerSigningStagedRolloutState::WaitingForAck {
        let _ = rollout
            .ack_timeout(now)
            .map_err(controller_signing_staged_rollout_error)?;
    }
    Ok(())
}

fn controller_signing_staged_rollout_counts(
    rollout: &fleet_domain::ControllerSigningStagedRollout,
) -> (usize, usize, usize) {
    let snapshot = rollout.snapshot();
    let blocked_count = snapshot.acknowledged_agent_ids.len()
        + snapshot.unavailable_agent_ids.len()
        + snapshot.failed_agent_ids.len()
        + snapshot.in_flight.len();
    (
        snapshot.acknowledged_agent_ids.len(),
        snapshot.unavailable_agent_ids.len(),
        snapshot.target_ids.len().saturating_sub(blocked_count),
    )
}

fn controller_signing_staged_rollout_target_ids(
    sessions: &Arc<Mutex<AgentSessionRegistry>>,
    requested_agent_ids: Vec<String>,
) -> Result<Vec<String>, ControllerError> {
    if requested_agent_ids.is_empty() {
        let sessions = sessions.lock().map_err(|_| {
            ControllerError::Store(fleet_store::StoreError::Domain(
                "session registry lock poisoned".to_owned(),
            ))
        })?;
        Ok(sessions
            .snapshot()
            .into_iter()
            .map(|session| session.agent_id)
            .collect::<Vec<_>>())
    } else {
        Ok(requested_agent_ids
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }
}

fn controller_signing_staged_rollout_error(
    error: fleet_domain::SigningStagedRolloutError,
) -> ControllerError {
    ControllerError::SigningKeyRotation(error.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControllerSigningTrustBundleRolloutMode {
    Manual,
    Retry,
}

impl ControllerSigningTrustBundleRolloutMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Retry => "retry",
        }
    }

    fn audit_action(self) -> &'static str {
        match self {
            Self::Manual => "controller_signing_trust_bundle_rollout",
            Self::Retry => "controller_signing_trust_bundle_rollout_retry",
        }
    }
}

fn controller_signing_trust_bundle_entries_from_rotation(
    rotation: &fleet_domain::ControllerSigningKeyRotation,
    identity: &ControllerIdentity,
    previous_public_key_path: Option<&Path>,
) -> Result<Vec<fleet_protocol::ControllerSigningTrustEntryWire>, ControllerError> {
    let snapshot = rotation.snapshot();
    let activated_at = snapshot.activated_at.ok_or_else(|| {
        ControllerError::SigningKeyRotation(
            "controller signing rotation must be activated before trust bundle rollout".to_owned(),
        )
    })?;
    match snapshot.state {
        fleet_domain::SigningKeyRotationState::DualTrustActive
        | fleet_domain::SigningKeyRotationState::OldKeyRetired => {}
        _ => {
            return Err(ControllerError::SigningKeyRotation(
                "controller signing rotation state is not ready for trust bundle rollout"
                    .to_owned(),
            ));
        }
    }
    let current_fingerprint =
        fleet_domain::SigningKeyFingerprint::new(identity.fingerprint.clone())
            .map_err(|error| ControllerError::SigningKeyRotation(error.to_string()))?;
    let current_public_fingerprint = fleet_core::fingerprint_public_key(&identity.public_key)
        .map_err(|_| {
            ControllerError::SigningKeyRotation(
                "active controller signing public key is invalid".to_owned(),
            )
        })?;
    if current_public_fingerprint != identity.fingerprint {
        return Err(ControllerError::SigningKeyRotation(
            "active controller signing public key does not match active fingerprint".to_owned(),
        ));
    }
    let current_public_key =
        fleet_domain::ControllerSigningPublicKey::new(identity.public_key.clone())
            .map_err(|error| ControllerError::SigningKeyRotation(error.to_string()))?;
    let mut domain_entries = vec![
        fleet_domain::ControllerSigningTrustEntry::new(
            fleet_domain::ControllerSigningTrustRole::Current,
            current_fingerprint,
            current_public_key,
            activated_at,
            None,
        )
        .map_err(|error| ControllerError::SigningKeyRotation(error.to_string()))?,
    ];

    if snapshot.state == fleet_domain::SigningKeyRotationState::DualTrustActive {
        let previous_public_key_path = previous_public_key_path.ok_or_else(|| {
            ControllerError::SigningKeyRotation(
                "previous public key path is required for dual-trust rollout".to_owned(),
            )
        })?;
        let previous_public_key = read_controller_signing_key_file()(previous_public_key_path)?
            .trim()
            .to_owned();
        let previous_public_fingerprint = fleet_core::fingerprint_public_key(&previous_public_key)
            .map_err(|_| {
                ControllerError::SigningKeyRotation(
                    "previous controller signing public key is invalid".to_owned(),
                )
            })?;
        if previous_public_fingerprint != snapshot.old_fingerprint.as_str() {
            return Err(ControllerError::SigningKeyRotation(
                "previous controller signing public key does not match old fingerprint".to_owned(),
            ));
        }
        let old_key_verifies_until = snapshot.old_key_verifies_until.ok_or_else(|| {
            ControllerError::SigningKeyRotation(
                "old-key verification window is required for dual-trust rollout".to_owned(),
            )
        })?;
        domain_entries.push(
            fleet_domain::ControllerSigningTrustEntry::new(
                fleet_domain::ControllerSigningTrustRole::Previous,
                snapshot.old_fingerprint,
                fleet_domain::ControllerSigningPublicKey::new(previous_public_key)
                    .map_err(|error| ControllerError::SigningKeyRotation(error.to_string()))?,
                activated_at,
                Some(old_key_verifies_until),
            )
            .map_err(|error| ControllerError::SigningKeyRotation(error.to_string()))?,
        );
    }

    let bundle = fleet_domain::ControllerSigningTrustBundle::new(domain_entries)
        .map_err(|error| ControllerError::SigningKeyRotation(error.to_string()))?;
    Ok(bundle
        .entries()
        .iter()
        .map(controller_signing_trust_entry_to_wire)
        .collect())
}

fn controller_signing_trust_entry_to_wire(
    entry: &fleet_domain::ControllerSigningTrustEntry,
) -> fleet_protocol::ControllerSigningTrustEntryWire {
    fleet_protocol::ControllerSigningTrustEntryWire {
        fingerprint: entry.fingerprint().as_str().to_owned(),
        public_key: entry.public_key().as_str().to_owned(),
        role: match entry.role() {
            fleet_domain::ControllerSigningTrustRole::Current => {
                fleet_protocol::ControllerSigningTrustRoleWire::Current
            }
            fleet_domain::ControllerSigningTrustRole::Previous => {
                fleet_protocol::ControllerSigningTrustRoleWire::Previous
            }
        },
        valid_from_ms: system_time_to_millis(entry.valid_from()),
        valid_until_ms: entry.valid_until().map(system_time_to_millis),
    }
}

fn dispatch_controller_signing_trust_bundle(
    sessions: &Arc<Mutex<AgentSessionRegistry>>,
    entries: &[fleet_protocol::ControllerSigningTrustEntryWire],
    requested_agent_ids: Vec<String>,
    max_agent_count: Option<usize>,
    now: SystemTime,
) -> Result<Vec<ControllerSigningTrustBundleRolloutAgentResult>, ControllerError> {
    let sessions = sessions.lock().map_err(|_| {
        ControllerError::Store(fleet_store::StoreError::Domain(
            "session registry lock poisoned".to_owned(),
        ))
    })?;
    let target_ids = if requested_agent_ids.is_empty() {
        sessions
            .snapshot()
            .into_iter()
            .map(|session| session.agent_id)
            .collect::<BTreeSet<_>>()
    } else {
        requested_agent_ids.into_iter().collect::<BTreeSet<_>>()
    };
    let selected_target_ids: Vec<String> = match max_agent_count {
        Some(max) => target_ids.into_iter().take(max).collect(),
        None => target_ids.into_iter().collect(),
    };
    let current_fingerprint = entries
        .iter()
        .find(|entry| entry.role == fleet_protocol::ControllerSigningTrustRoleWire::Current)
        .map(|entry| entry.fingerprint.as_str());
    let mut results = Vec::new();
    for agent_id in selected_target_ids {
        if !sessions.has_active_session(&agent_id) {
            results.push(ControllerSigningTrustBundleRolloutAgentResult {
                agent_id,
                status: "skipped_not_connected".to_owned(),
            });
            continue;
        }
        if current_fingerprint
            .map(|fingerprint| sessions.controller_signing_trust_is_current(&agent_id, fingerprint))
            .unwrap_or(false)
        {
            results.push(ControllerSigningTrustBundleRolloutAgentResult {
                agent_id,
                status: "skipped_already_current".to_owned(),
            });
            continue;
        }
        let message = fleet_protocol::WireMessage::new(
            fleet_core::generate_prefixed_ulid("msg")
                .unwrap_or_else(|_| "msg-trust-bundle-rollout".to_owned()),
            fleet_core::generate_prefixed_ulid("corr")
                .unwrap_or_else(|_| "corr-trust-bundle-rollout".to_owned()),
            Some(agent_id.clone()),
            system_time_to_millis(now),
            fleet_protocol::WirePayload::ControllerSigningTrustBundleUpdate {
                entries: entries.to_vec(),
            },
        );
        let status = match sessions.try_send(&agent_id, message) {
            Ok(()) => "sent".to_owned(),
            Err(AgentSessionSendError::NotConnected) => "skipped_not_connected".to_owned(),
            Err(AgentSessionSendError::QueueFull) => "failed_queue_full".to_owned(),
            Err(AgentSessionSendError::Closed) => "failed_queue_closed".to_owned(),
        };
        results.push(ControllerSigningTrustBundleRolloutAgentResult { agent_id, status });
    }
    Ok(results)
}

#[allow(dead_code)]
pub(crate) fn dispatch_agent_certificate_lifecycle_update(
    sessions: &Arc<Mutex<AgentSessionRegistry>>,
    action: fleet_protocol::AgentCertificateLifecycleActionWire,
    record: &fleet_application::AgentCertificateLifecycleRecord,
    correlation_id: &str,
    timestamp_ms: u64,
) -> Result<AgentCertificateLifecycleDispatchResult, ControllerError> {
    let message =
        agent_certificate_lifecycle_update_message(action, record, correlation_id, timestamp_ms)?;
    let status = {
        let sessions = sessions.lock().map_err(|_| {
            ControllerError::Store(fleet_store::StoreError::Domain(
                "session registry lock poisoned".to_owned(),
            ))
        })?;
        match sessions.try_send(record.agent_id.as_str(), message) {
            Ok(()) => AgentCertificateLifecycleDispatchStatus::Sent,
            Err(AgentSessionSendError::NotConnected) => {
                AgentCertificateLifecycleDispatchStatus::NotConnected
            }
            Err(AgentSessionSendError::QueueFull) => {
                AgentCertificateLifecycleDispatchStatus::QueueFull
            }
            Err(AgentSessionSendError::Closed) => AgentCertificateLifecycleDispatchStatus::Closed,
        }
    };
    Ok(agent_certificate_lifecycle_dispatch_result(
        action, record, status,
    ))
}

#[allow(dead_code)]
pub(crate) fn agent_certificate_lifecycle_update_message(
    action: fleet_protocol::AgentCertificateLifecycleActionWire,
    record: &fleet_application::AgentCertificateLifecycleRecord,
    correlation_id: &str,
    timestamp_ms: u64,
) -> Result<fleet_protocol::WireMessage, ControllerError> {
    Ok(fleet_protocol::WireMessage::new(
        fleet_core::generate_prefixed_ulid("msg")
            .map_err(|error| ControllerError::Json(error.to_string()))?,
        correlation_id.to_owned(),
        Some(record.agent_id.as_str().to_owned()),
        timestamp_ms,
        fleet_protocol::WirePayload::AgentCertificateLifecycleUpdate {
            agent_id: record.agent_id.as_str().to_owned(),
            action,
            state: agent_certificate_lifecycle_state_to_wire(record.lifecycle.state),
            current_certificate: record
                .lifecycle
                .current_certificate
                .as_ref()
                .map(agent_certificate_metadata_to_wire),
            next_certificate: record
                .lifecycle
                .next_certificate
                .as_ref()
                .map(agent_certificate_metadata_to_wire),
            grace_until_ms: record.lifecycle.grace_until.map(system_time_to_millis),
            reason_code: record
                .lifecycle
                .revocation_reason
                .map(|reason| reason.as_str().to_owned()),
        },
    ))
}

#[allow(dead_code)]
pub(crate) fn agent_certificate_metadata_to_wire(
    certificate: &fleet_domain::AgentCertificate,
) -> fleet_protocol::AgentCertificateMetadataWire {
    fleet_protocol::AgentCertificateMetadataWire {
        serial: certificate.serial().as_str().to_owned(),
        fingerprint: certificate.fingerprint().as_str().to_owned(),
        not_before_ms: system_time_to_millis(certificate.validity().not_before()),
        not_after_ms: system_time_to_millis(certificate.validity().not_after()),
    }
}

#[allow(dead_code)]
pub(crate) fn agent_certificate_lifecycle_dispatch_result(
    action: fleet_protocol::AgentCertificateLifecycleActionWire,
    record: &fleet_application::AgentCertificateLifecycleRecord,
    status: AgentCertificateLifecycleDispatchStatus,
) -> AgentCertificateLifecycleDispatchResult {
    AgentCertificateLifecycleDispatchResult {
        agent_id: record.agent_id.as_str().to_owned(),
        status,
        action,
        state: agent_certificate_lifecycle_state_to_wire(record.lifecycle.state),
        current_fingerprint_prefix: record.lifecycle.current_certificate.as_ref().map(
            |certificate| {
                controller_signing_fingerprint_prefix(certificate.fingerprint().as_str()).to_owned()
            },
        ),
        next_fingerprint_prefix: record
            .lifecycle
            .next_certificate
            .as_ref()
            .map(|certificate| {
                controller_signing_fingerprint_prefix(certificate.fingerprint().as_str()).to_owned()
            }),
    }
}

#[allow(dead_code)]
pub(crate) fn agent_certificate_lifecycle_state_to_wire(
    state: fleet_domain::AgentCertificateLifecycleState,
) -> fleet_protocol::AgentCertificateLifecycleStateWire {
    match state {
        fleet_domain::AgentCertificateLifecycleState::NotIssued => {
            fleet_protocol::AgentCertificateLifecycleStateWire::NotIssued
        }
        fleet_domain::AgentCertificateLifecycleState::IssuanceRequested => {
            fleet_protocol::AgentCertificateLifecycleStateWire::IssuanceRequested
        }
        fleet_domain::AgentCertificateLifecycleState::Issued => {
            fleet_protocol::AgentCertificateLifecycleStateWire::Issued
        }
        fleet_domain::AgentCertificateLifecycleState::RenewalRequested => {
            fleet_protocol::AgentCertificateLifecycleStateWire::RenewalRequested
        }
        fleet_domain::AgentCertificateLifecycleState::DualCertificateActive => {
            fleet_protocol::AgentCertificateLifecycleStateWire::DualCertificateActive
        }
        fleet_domain::AgentCertificateLifecycleState::Revoked => {
            fleet_protocol::AgentCertificateLifecycleStateWire::Revoked
        }
        fleet_domain::AgentCertificateLifecycleState::Expired => {
            fleet_protocol::AgentCertificateLifecycleStateWire::Expired
        }
        fleet_domain::AgentCertificateLifecycleState::Failed => {
            fleet_protocol::AgentCertificateLifecycleStateWire::Failed
        }
    }
}

fn agent_certificate_issuance_request_response(
    output: &fleet_application::AgentCertificateLifecycleOperationOutput,
    dispatch: &AgentCertificateLifecycleDispatchResult,
) -> AgentCertificateIssuanceRequestResponse {
    AgentCertificateIssuanceRequestResponse {
        agent_id: output.record.agent_id.as_str().to_owned(),
        action: agent_certificate_lifecycle_action_wire_as_str(dispatch.action).to_owned(),
        lifecycle_state: agent_certificate_lifecycle_state_wire_as_str(dispatch.state).to_owned(),
        dispatch_status: agent_certificate_lifecycle_dispatch_status_as_str(dispatch.status)
            .to_owned(),
        current_fingerprint_prefix: dispatch.current_fingerprint_prefix.clone(),
        next_fingerprint_prefix: dispatch.next_fingerprint_prefix.clone(),
        audit_event_action: output.audit_event.action.clone(),
        updated_at_ms: system_time_to_millis(output.record.updated_at),
    }
}

fn agent_certificate_lifecycle_status_response(
    agent_id: &AgentId,
    record: Option<&fleet_application::AgentCertificateLifecycleRecord>,
) -> AgentCertificateLifecycleStatusResponse {
    match record {
        Some(record) => AgentCertificateLifecycleStatusResponse {
            agent_id: agent_id.as_str().to_owned(),
            record_present: true,
            lifecycle_state: record.lifecycle.state.as_str().to_owned(),
            current_fingerprint_prefix: record.lifecycle.current_certificate.as_ref().map(
                |certificate| {
                    controller_signing_fingerprint_prefix(certificate.fingerprint().as_str())
                        .to_owned()
                },
            ),
            next_fingerprint_prefix: record.lifecycle.next_certificate.as_ref().map(
                |certificate| {
                    controller_signing_fingerprint_prefix(certificate.fingerprint().as_str())
                        .to_owned()
                },
            ),
            grace_until_ms: record.lifecycle.grace_until.map(system_time_to_millis),
            revocation_reason: record
                .lifecycle
                .revocation_reason
                .map(|reason| reason.as_str().to_owned()),
            updated_at_ms: Some(system_time_to_millis(record.updated_at)),
        },
        None => AgentCertificateLifecycleStatusResponse {
            agent_id: agent_id.as_str().to_owned(),
            record_present: false,
            lifecycle_state: fleet_domain::AgentCertificateLifecycleState::NotIssued
                .as_str()
                .to_owned(),
            current_fingerprint_prefix: None,
            next_fingerprint_prefix: None,
            grace_until_ms: None,
            revocation_reason: None,
            updated_at_ms: None,
        },
    }
}

fn map_agent_certificate_lifecycle_error(
    error: AgentCertificateLifecycleUseCaseError<fleet_store::StoreError, fleet_store::StoreError>,
) -> ControllerError {
    match error {
        AgentCertificateLifecycleUseCaseError::Repository(error)
        | AgentCertificateLifecycleUseCaseError::Audit(error) => ControllerError::Store(error),
        AgentCertificateLifecycleUseCaseError::Domain(error) => {
            ControllerError::Store(fleet_store::StoreError::Domain(error.to_string()))
        }
        AgentCertificateLifecycleUseCaseError::NotFound => {
            ControllerError::Store(fleet_store::StoreError::NotFound)
        }
    }
}

fn agent_certificate_lifecycle_http_error_response(
    error: ControllerError,
) -> Result<String, ControllerError> {
    match error {
        ControllerError::Json(message) => Ok(response(
            400,
            "application/json",
            &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
        )),
        ControllerError::Store(fleet_store::StoreError::NotFound) => Ok(response(
            404,
            "application/json",
            "{\"error\":\"not_found\"}\n",
        )),
        ControllerError::Store(fleet_store::StoreError::Domain(message)) => Ok(response(
            409,
            "application/json",
            &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
        )),
        ControllerError::Store(fleet_store::StoreError::ConstraintViolation(message)) => {
            Ok(response(
                409,
                "application/json",
                &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
            ))
        }
        error => Err(error),
    }
}

fn agent_certificate_lifecycle_action_wire_as_str(
    action: fleet_protocol::AgentCertificateLifecycleActionWire,
) -> &'static str {
    match action {
        fleet_protocol::AgentCertificateLifecycleActionWire::RequestIssuance => "request_issuance",
        fleet_protocol::AgentCertificateLifecycleActionWire::Issue => "issue",
        fleet_protocol::AgentCertificateLifecycleActionWire::RequestRenewal => "request_renewal",
        fleet_protocol::AgentCertificateLifecycleActionWire::ActivateRenewal => "activate_renewal",
        fleet_protocol::AgentCertificateLifecycleActionWire::CompleteRotation => {
            "complete_rotation"
        }
        fleet_protocol::AgentCertificateLifecycleActionWire::Revoke => "revoke",
        fleet_protocol::AgentCertificateLifecycleActionWire::Expire => "expire",
        fleet_protocol::AgentCertificateLifecycleActionWire::Fail => "fail",
    }
}

fn agent_certificate_lifecycle_dispatch_status_as_str(
    status: AgentCertificateLifecycleDispatchStatus,
) -> &'static str {
    match status {
        AgentCertificateLifecycleDispatchStatus::Sent => "sent",
        AgentCertificateLifecycleDispatchStatus::NotConnected => "not_connected",
        AgentCertificateLifecycleDispatchStatus::QueueFull => "queue_full",
        AgentCertificateLifecycleDispatchStatus::Closed => "closed",
    }
}

fn parse_controller_signing_rotation_body<T>(body: &str) -> Result<T, ControllerError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_str(body)
        .map_err(|_| ControllerError::Json("invalid_signing_rotation_request".to_owned()))
        .map_err(controller_signing_rotation_safe_error_response)
}

fn signing_rotation_requested_until(
    request: &ControllerSigningRotationRequestBody,
    now: SystemTime,
) -> Result<SystemTime, ControllerError> {
    match (
        request.old_key_verifies_for_seconds,
        request.old_key_verifies_until_ms,
    ) {
        (Some(_), Some(_)) | (None, None) => Err(ControllerError::Json(
            "exactly one old-key verification window must be provided".to_owned(),
        )),
        (Some(seconds), None) => now
            .checked_add(Duration::from_secs(seconds))
            .ok_or_else(|| {
                ControllerError::Json("old-key verification window overflowed".to_owned())
            }),
        (None, Some(millis)) => Ok(millis_to_system_time(millis)),
    }
    .map_err(controller_signing_rotation_safe_error_response)
}

fn signing_fingerprint_from_str(
    value: &str,
) -> Result<fleet_domain::SigningKeyFingerprint, ControllerError> {
    fleet_domain::SigningKeyFingerprint::new(value.to_owned())
        .map_err(|error| ControllerError::SigningKeyRotation(error.to_string()))
        .map_err(controller_signing_rotation_safe_error_response)
}

fn controller_signing_rotation_ok_response<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    identity: &ControllerIdentity,
    now: SystemTime,
) -> Result<String, ControllerError> {
    let body = serde_json::to_string(&controller_signing_rotation_status_response(
        store, identity, now,
    )?)
    .map_err(|error| ControllerError::Json(error.to_string()))?;
    Ok(response(200, "application/json", &format!("{body}\n")))
}

fn controller_signing_rotation_route_response(
    result: Result<String, ControllerError>,
) -> Result<String, ControllerError> {
    match result {
        Ok(response) => Ok(response),
        Err(ControllerError::Json(_)) => Ok(response(
            400,
            "application/json",
            "{\"error\":\"invalid_signing_rotation_request\"}\n",
        )),
        Err(ControllerError::SigningKeyRotation(_)) => Ok(response(
            409,
            "application/json",
            "{\"error\":\"signing_rotation_conflict\"}\n",
        )),
        Err(error) => Err(error),
    }
}

fn controller_signing_rotation_operation_error(
    error: SigningKeyRotationUseCaseError<fleet_store::StoreError, fleet_store::StoreError>,
) -> ControllerError {
    match error {
        SigningKeyRotationUseCaseError::Repository(error)
        | SigningKeyRotationUseCaseError::Audit(error) => ControllerError::Store(error),
        SigningKeyRotationUseCaseError::Domain(error) => ControllerError::SigningKeyRotation(
            format!("invalid signing key rotation transition: {error}"),
        ),
        SigningKeyRotationUseCaseError::FingerprintMismatch => ControllerError::SigningKeyRotation(
            "validated signing fingerprint does not match requested rotation fingerprint"
                .to_owned(),
        ),
        SigningKeyRotationUseCaseError::NotFound => {
            ControllerError::SigningKeyRotation("signing key rotation record not found".to_owned())
        }
    }
}

fn controller_signing_rotation_safe_error_response(error: ControllerError) -> ControllerError {
    match error {
        ControllerError::Json(_) => {
            ControllerError::Json("invalid_signing_rotation_request".to_owned())
        }
        ControllerError::SigningKeyRotation(_) => ControllerError::SigningKeyRotation(
            "controller signing rotation request could not be applied".to_owned(),
        ),
        other => other,
    }
}

impl ControllerRuntimeMetadata {
    fn controller_signing_key_file_pair(&self) -> Option<ControllerSigningKeyFilePair> {
        Some(ControllerSigningKeyFilePair {
            public_key_path: self.controller_signing_public_key_path.clone()?,
            private_key_path: self.controller_signing_private_key_path.clone()?,
        })
    }

    fn disallowed_signing_candidate_paths(&self) -> Vec<PathBuf> {
        [self.tls_cert_path.clone(), self.tls_key_path.clone()]
            .into_iter()
            .flatten()
            .collect()
    }
}

fn admin_static_response(path: &str) -> String {
    let path = path_without_query(path);
    match path {
        "/admin" | "/admin/" | "/admin/index.html" => {
            response(200, "text/html; charset=utf-8", ADMIN_INDEX_HTML)
        }
        "/admin/styles.css" => response(200, "text/css; charset=utf-8", ADMIN_STYLES_CSS),
        "/admin/app.js" => response(200, "application/javascript; charset=utf-8", ADMIN_APP_JS),
        "/admin/api-client.js" => response(
            200,
            "application/javascript; charset=utf-8",
            ADMIN_API_CLIENT_JS,
        ),
        "/admin/api.schema.json" => response(
            200,
            "application/json; charset=utf-8",
            ADMIN_API_SCHEMA_JSON,
        ),
        _ => response(404, "application/json", "{\"error\":\"not_found\"}\n"),
    }
}

fn path_without_query(path: &str) -> &str {
    path.split_once('?').map(|(path, _)| path).unwrap_or(path)
}

fn trim_remediation_action_path<'a>(path: &'a str, suffix: &str) -> &'a str {
    path.trim_start_matches("/api/remediations/")
        .trim_end_matches(suffix)
        .trim_end_matches('/')
}

fn trim_agent_action_path<'a>(path: &'a str, suffix: &str) -> &'a str {
    path.trim_start_matches("/api/agents/")
        .trim_end_matches(suffix)
        .trim_end_matches('/')
}

#[derive(Debug, Clone, Copy)]
struct SnapshotPageRequest {
    limit: usize,
    before: Option<SnapshotPageCursor>,
}

impl SnapshotPageRequest {
    fn fetch_limit(self) -> usize {
        self.limit.saturating_add(1).min(501)
    }
}

fn parse_snapshot_page_request(raw_path: &str) -> Result<SnapshotPageRequest, ControllerError> {
    let limit = match query_param(raw_path, "limit") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| ControllerError::Json("limit must be a positive integer".to_owned()))?,
        None => 50,
    };
    if limit == 0 {
        return Err(ControllerError::Json(
            "limit must be a positive integer".to_owned(),
        ));
    }
    let before = query_param(raw_path, "before")
        .map(parse_snapshot_page_cursor)
        .transpose()?;
    Ok(SnapshotPageRequest {
        limit: limit.min(500),
        before,
    })
}

fn query_param<'a>(raw_path: &'a str, name: &str) -> Option<&'a str> {
    raw_path.split_once('?')?.1.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == name { Some(value) } else { None }
    })
}

fn parse_snapshot_page_cursor(value: &str) -> Result<SnapshotPageCursor, ControllerError> {
    let value = value.replace("%3A", ":").replace("%3a", ":");
    let (occurred_at_seconds, row_id) = value.split_once(':').ok_or_else(|| {
        ControllerError::Json("before cursor must be <seconds>:<row_id>".to_owned())
    })?;
    let occurred_at_seconds = occurred_at_seconds
        .parse::<u64>()
        .map_err(|_| ControllerError::Json("before cursor seconds must be numeric".to_owned()))?;
    let row_id = row_id
        .parse::<i64>()
        .map_err(|_| ControllerError::Json("before cursor row id must be numeric".to_owned()))?;
    if row_id <= 0 {
        return Err(ControllerError::Json(
            "before cursor row id must be positive".to_owned(),
        ));
    }
    Ok(SnapshotPageCursor {
        occurred_at: UNIX_EPOCH + Duration::from_secs(occurred_at_seconds),
        row_id,
    })
}

fn encode_snapshot_page_cursor(cursor: SnapshotPageCursor) -> String {
    format!(
        "{}:{}",
        system_time_to_millis(cursor.occurred_at) / 1000,
        cursor.row_id
    )
}

fn next_snapshot_cursor(last_cursor: Option<SnapshotPageCursor>, has_more: bool) -> Option<String> {
    if has_more {
        last_cursor.map(encode_snapshot_page_cursor)
    } else {
        None
    }
}

enum CreateCommandJobHttpError {
    BadRequest(String),
    Conflict(String),
    Internal(ControllerError),
}

enum RemediationHttpError {
    BadRequest(String),
    NotFound,
    Conflict(String),
    Internal(ControllerError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CreateJobHttpOutput {
    job_id: String,
    body: String,
}

fn create_enrollment_token<'a>(
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    actor: &str,
) -> Result<String, ControllerError> {
    let request = parse_create_enrollment_token_request(body)?;
    if request.max_uses == 0 {
        return Err(ControllerError::Json(
            "max_uses must be greater than zero".to_owned(),
        ));
    }
    if request.expires_in_seconds == 0 {
        return Err(ControllerError::Json(
            "expires_in_seconds must be greater than zero".to_owned(),
        ));
    }
    let id = fleet_core::generate_prefixed_ulid("et")
        .map_err(|error| ControllerError::Json(error.to_string()))?;
    let token = generate_token("enroll")?;
    let now = SystemTime::now();
    let mut repo = ControllerEnrollmentTokenRepository {
        store: store.into(),
    };
    let mut audit = ControllerAuditWriter {
        store: store.into(),
    };
    let output = CreateEnrollmentToken::execute(
        &mut repo,
        &mut audit,
        CreateEnrollmentTokenInput {
            id,
            token_hash: hash_token(&token),
            default_labels: request.default_labels,
            expires_at: now + Duration::from_secs(request.expires_in_seconds),
            max_uses: request.max_uses,
            actor: actor.to_owned(),
            occurred_at: now,
        },
    )
    .map_err(map_enrollment_token_use_case_error)?;

    serde_json::to_string(&CreateEnrollmentTokenResponse {
        id: output.id,
        token,
        expires_in_seconds: request.expires_in_seconds,
    })
    .map_err(|error| ControllerError::Json(error.to_string()))
}

fn parse_create_enrollment_token_request(
    body: &str,
) -> Result<CreateEnrollmentTokenRequest, ControllerError> {
    if body.trim().is_empty() {
        return Ok(CreateEnrollmentTokenRequest::default());
    }
    serde_json::from_str(body).map_err(|error| ControllerError::Json(error.to_string()))
}

fn list_enrollment_tokens<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<String, ControllerError> {
    let repo = ControllerEnrollmentTokenRepository {
        store: store.into(),
    };
    let records = ListEnrollmentTokens::execute(&repo).map_err(ControllerError::Store)?;
    Ok(records
        .into_iter()
        .map(|record| {
            serde_json::to_string(&EnrollmentTokenSummaryResponse {
                id: record.id,
                default_labels: record.default_labels,
                max_uses: record.max_uses,
                used_count: record.used_count,
                remaining_uses: record.max_uses.saturating_sub(record.used_count),
                revoked: record.revoked,
                expires_at_epoch: system_time_to_millis(record.expires_at) / 1000,
            })
            .map_err(|error| ControllerError::Json(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?
        .join(","))
}

fn revoke_enrollment_token<'a>(
    id: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    actor: &str,
) -> Result<bool, ControllerError> {
    let mut repo = ControllerEnrollmentTokenRepository {
        store: store.into(),
    };
    let mut audit = ControllerAuditWriter {
        store: store.into(),
    };
    let output = RevokeEnrollmentToken::execute(
        &mut repo,
        &mut audit,
        RevokeEnrollmentTokenInput {
            id: id.to_owned(),
            actor: actor.to_owned(),
            occurred_at: SystemTime::now(),
        },
    )
    .map_err(map_enrollment_token_use_case_error)?;
    Ok(output.revoked)
}

fn map_enrollment_token_use_case_error(
    error: EnrollmentTokenUseCaseError<fleet_store::StoreError, fleet_store::StoreError>,
) -> ControllerError {
    match error {
        EnrollmentTokenUseCaseError::Repository(error)
        | EnrollmentTokenUseCaseError::Audit(error) => ControllerError::Store(error),
    }
}

fn preview_selector<'a>(
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<String, ControllerError> {
    let request: SelectorPreviewRequest =
        serde_json::from_str(body).map_err(|error| ControllerError::Json(error.to_string()))?;
    let selector = selector_from_parts(request.selector.as_deref(), request.match_labels.as_ref())
        .map_err(ControllerError::Json)?
        .ok_or_else(|| {
            ControllerError::Json("selector preview requires selector or matchLabels".to_owned())
        })?;
    mark_stale_agents_offline_for_inventory(store)?;
    let repo = ControllerAgentInventoryRepository {
        store: store.into(),
    };
    let output = PreviewSelector::execute(&repo, SelectorPreviewInput { selector })?;
    let response = SelectorPreviewResponse {
        matched_count: output.matched_count,
        selected_count: output.selected_count,
        disabled_count: output.disabled_count,
        offline_count: output.offline_count,
        warnings: output
            .warnings
            .into_iter()
            .map(|warning| SelectorPreviewWarningResponse {
                code: warning.code,
                message: warning.message,
            })
            .collect(),
        agents: output
            .agents
            .into_iter()
            .map(|agent| SelectorPreviewAgentResponse {
                agent_id: agent.agent_id,
                name: agent.name,
                status: agent.status,
                labels: agent
                    .labels
                    .into_iter()
                    .map(|(key, value)| AgentLabelResponse { key, value })
                    .collect(),
                selected_for_dispatch: agent.selected_for_dispatch,
            })
            .collect(),
    };
    serde_json::to_string(&response).map_err(|error| ControllerError::Json(error.to_string()))
}

fn create_command_job<'a>(
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    identity: &ControllerIdentity,
    actor: &str,
) -> Result<CreateJobHttpOutput, CreateCommandJobHttpError> {
    let request: CreateCommandJobRequest = serde_json::from_str(body)
        .map_err(|error| CreateCommandJobHttpError::BadRequest(error.to_string()))?;
    let issued_at = SystemTime::now();
    let expires_at = issued_at + Duration::from_secs(request.expires_in_seconds);
    let job_id = request.job_id.clone();
    let target_agent_ids = resolve_command_targets(store, &request)?;
    let selector_snapshot = job_selector_snapshot(
        &request.target_agent_ids,
        request.selector.as_deref(),
        request.match_labels.as_ref(),
        &target_agent_ids,
    )?;
    let strategy = normalize_job_strategy(request.strategy.as_ref())?;
    let nonce_prefix = match request.nonce_prefix {
        Some(prefix) => prefix,
        None => fleet_core::generate_prefixed_ulid("nonce").map_err(|error| {
            CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
        })?,
    };
    let approval_request_id = fleet_core::generate_prefixed_ulid("approval").map_err(|error| {
        CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
    })?;
    let input = CreateCommandJobInput {
        job_id: request.job_id,
        target_agent_ids,
        program: request.program,
        args: request.args,
        timeout: Duration::from_secs(request.timeout_seconds),
        confirmed_high_risk: request.confirmed_high_risk,
        confirmed_by: actor.to_owned(),
        issued_at,
        expires_at,
        nonce_prefix,
        approval_request_id,
        approval_expires_at: expires_at,
    };
    let mut job_repo = ControllerJobRepository {
        store: store.into(),
    };
    let mut audit_writer = ControllerAuditWriter {
        store: store.into(),
    };
    let mut signer = ControllerTaskSigner {
        private_key: &identity.private_key,
        signing_fingerprint: &identity.fingerprint,
    };

    let output = CreateCommandJob::execute(&mut job_repo, &mut audit_writer, &mut signer, input)
        .map_err(map_create_command_job_error)?;
    persist_job_strategy(store, &job_id, strategy)?;
    persist_job_selector_snapshot(store, &job_id, selector_snapshot)?;
    let body = serde_json::to_string(&CreateCommandJobResponse {
        job_id: job_id.clone(),
        target_count: output.targets.len(),
        assignment_count: output.envelopes.len(),
        status: if output.approval_request.is_some() {
            "pending_approval".to_owned()
        } else {
            "queued".to_owned()
        },
        approval_request_id: output.approval_request.map(|approval| approval.id),
    })
    .map_err(|error| {
        CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
    })?;
    Ok(CreateJobHttpOutput { job_id, body })
}

fn create_drift_check_job<'a>(
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    identity: &ControllerIdentity,
    actor: &str,
) -> Result<CreateJobHttpOutput, CreateCommandJobHttpError> {
    let request: CreateDriftCheckJobRequest = serde_json::from_str(body)
        .map_err(|error| CreateCommandJobHttpError::BadRequest(error.to_string()))?;
    fleet_domain::parse_policy_document(&request.policy_document)
        .map_err(|error| CreateCommandJobHttpError::BadRequest(error.to_string()))?;
    let issued_at = SystemTime::now();
    let expires_at = issued_at + Duration::from_secs(request.expires_in_seconds);
    let job_id = request.job_id.clone();
    let target_agent_ids = resolve_drift_check_targets(store, &request)?;
    let selector_snapshot = job_selector_snapshot(
        &request.target_agent_ids,
        request.selector.as_deref(),
        request.match_labels.as_ref(),
        &target_agent_ids,
    )?;
    let strategy = normalize_job_strategy(request.strategy.as_ref())?;
    let nonce_prefix = match request.nonce_prefix {
        Some(prefix) => prefix,
        None => fleet_core::generate_prefixed_ulid("nonce").map_err(|error| {
            CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
        })?,
    };
    let approval_request_id = fleet_core::generate_prefixed_ulid("approval").map_err(|error| {
        CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
    })?;
    let input = CreateDriftCheckJobInput {
        job_id: request.job_id,
        target_agent_ids,
        policy_document: request.policy_document,
        timeout: Duration::from_secs(request.timeout_seconds),
        created_by: actor.to_owned(),
        issued_at,
        expires_at,
        nonce_prefix,
        approval_request_id,
        approval_expires_at: expires_at,
    };
    let mut job_repo = ControllerJobRepository {
        store: store.into(),
    };
    let mut audit_writer = ControllerAuditWriter {
        store: store.into(),
    };
    let mut signer = ControllerTaskSigner {
        private_key: &identity.private_key,
        signing_fingerprint: &identity.fingerprint,
    };

    let output = CreateDriftCheckJob::execute(&mut job_repo, &mut audit_writer, &mut signer, input)
        .map_err(map_create_drift_check_job_error)?;
    persist_job_strategy(store, &job_id, strategy)?;
    persist_job_selector_snapshot(store, &job_id, selector_snapshot)?;
    let body = serde_json::to_string(&CreateDriftCheckJobResponse {
        job_id: job_id.clone(),
        target_count: output.targets.len(),
        assignment_count: output.envelopes.len(),
        status: if output.approval_request.is_some() {
            "pending_approval".to_owned()
        } else {
            "queued".to_owned()
        },
        approval_request_id: output.approval_request.map(|approval| approval.id),
    })
    .map_err(|error| {
        CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
    })?;
    Ok(CreateJobHttpOutput { job_id, body })
}

fn create_runbook_job<'a>(
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    identity: &ControllerIdentity,
    actor: &str,
) -> Result<CreateJobHttpOutput, CreateCommandJobHttpError> {
    let request: CreateRunbookJobRequest = serde_json::from_str(body)
        .map_err(|error| CreateCommandJobHttpError::BadRequest(error.to_string()))?;
    let runbook = fleet_domain::parse_runbook_document(&request.runbook_document)
        .map_err(|error| CreateCommandJobHttpError::BadRequest(error.to_string()))?;
    let issued_at = SystemTime::now();
    let expires_at = issued_at + Duration::from_secs(request.expires_in_seconds);
    let job_id = request.job_id.clone();
    let (target_agent_ids, selector_snapshot) =
        resolve_runbook_targets_and_snapshot(store, &request, &runbook.target_selector)?;
    let strategy = normalize_job_strategy(request.strategy.as_ref())?;
    let nonce_prefix = match request.nonce_prefix {
        Some(prefix) => prefix,
        None => fleet_core::generate_prefixed_ulid("nonce").map_err(|error| {
            CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
        })?,
    };
    let approval_request_id = fleet_core::generate_prefixed_ulid("approval").map_err(|error| {
        CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
    })?;
    let input = CreateRunbookJobInput {
        job_id: request.job_id,
        target_agent_ids,
        runbook_document: request.runbook_document,
        timeout: Duration::from_secs(request.timeout_seconds),
        confirmed_high_risk: request.confirmed_high_risk,
        confirmed_by: actor.to_owned(),
        issued_at,
        expires_at,
        nonce_prefix,
        approval_request_id,
        approval_expires_at: expires_at,
    };
    let mut job_repo = ControllerJobRepository {
        store: store.into(),
    };
    let mut audit_writer = ControllerAuditWriter {
        store: store.into(),
    };
    let mut signer = ControllerTaskSigner {
        private_key: &identity.private_key,
        signing_fingerprint: &identity.fingerprint,
    };

    let output = CreateRunbookJob::execute(&mut job_repo, &mut audit_writer, &mut signer, input)
        .map_err(map_create_runbook_job_error)?;
    persist_job_strategy(store, &job_id, strategy)?;
    persist_job_selector_snapshot(store, &job_id, selector_snapshot)?;
    let body = serde_json::to_string(&CreateRunbookJobResponse {
        job_id: job_id.clone(),
        target_count: output.targets.len(),
        assignment_count: output.envelopes.len(),
        status: if output.approval_request.is_some() {
            "pending_approval".to_owned()
        } else {
            "queued".to_owned()
        },
        approval_request_id: output.approval_request.map(|approval| approval.id),
    })
    .map_err(|error| {
        CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
    })?;
    Ok(CreateJobHttpOutput { job_id, body })
}

fn resolve_command_targets<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    request: &CreateCommandJobRequest,
) -> Result<Vec<String>, CreateCommandJobHttpError> {
    resolve_targets_from_parts(
        store,
        &request.target_agent_ids,
        request.selector.as_deref(),
        request.match_labels.as_ref(),
        "job_selector_resolved",
    )
}

fn resolve_drift_check_targets<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    request: &CreateDriftCheckJobRequest,
) -> Result<Vec<String>, CreateCommandJobHttpError> {
    resolve_targets_from_parts(
        store,
        &request.target_agent_ids,
        request.selector.as_deref(),
        request.match_labels.as_ref(),
        "drift_check_selector_resolved",
    )
}

fn resolve_runbook_targets_and_snapshot<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    request: &CreateRunbookJobRequest,
    runbook_selector: &Selector,
) -> Result<(Vec<String>, (String, String)), CreateCommandJobHttpError> {
    if !request.target_agent_ids.is_empty()
        || request.selector.is_some()
        || request.match_labels.is_some()
    {
        let target_agent_ids = resolve_targets_from_parts(
            store,
            &request.target_agent_ids,
            request.selector.as_deref(),
            request.match_labels.as_ref(),
            "runbook_selector_resolved",
        )?;
        let selector_snapshot = job_selector_snapshot(
            &request.target_agent_ids,
            request.selector.as_deref(),
            request.match_labels.as_ref(),
            &target_agent_ids,
        )?;
        return Ok((target_agent_ids, selector_snapshot));
    }

    let target_agent_ids = resolve_targets_from_selector(
        store,
        runbook_selector,
        "runbook_document_selector_resolved",
    )?;
    let selector_snapshot = runbook_selector_snapshot(runbook_selector)?;
    Ok((target_agent_ids, selector_snapshot))
}

fn resolve_targets_from_parts<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    target_agent_ids: &[String],
    selector: Option<&str>,
    match_labels: Option<&BTreeMap<String, String>>,
    log_event: &'static str,
) -> Result<Vec<String>, CreateCommandJobHttpError> {
    if !target_agent_ids.is_empty() {
        return Ok(target_agent_ids.to_vec());
    }
    let Some(selector) = selector_from_parts(selector, match_labels)
        .map_err(CreateCommandJobHttpError::BadRequest)?
    else {
        return Ok(Vec::new());
    };
    let repo = ControllerAgentInventoryRepository {
        store: store.into(),
    };
    let agents = ListInventoryAgents::execute(&repo)
        .map_err(|error| CreateCommandJobHttpError::Internal(ControllerError::Store(error)))?;
    let selection = select_dispatch_targets(&agents, &selector);
    tracing::debug!(
        resolution_kind = log_event,
        matched_count = selection.matched_count,
        selected_count = selection.targets.len(),
        disabled_count = selection.disabled_count,
        offline_count = selection.offline_count,
        "selector_resolved"
    );
    Ok(selection
        .targets
        .into_iter()
        .map(|agent| agent.id().as_str().to_owned())
        .collect())
}

fn resolve_targets_from_selector<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    selector: &Selector,
    log_event: &'static str,
) -> Result<Vec<String>, CreateCommandJobHttpError> {
    let repo = ControllerAgentInventoryRepository {
        store: store.into(),
    };
    let agents = ListInventoryAgents::execute(&repo)
        .map_err(|error| CreateCommandJobHttpError::Internal(ControllerError::Store(error)))?;
    let selection = select_dispatch_targets(&agents, selector);
    tracing::debug!(
        resolution_kind = log_event,
        matched_count = selection.matched_count,
        selected_count = selection.targets.len(),
        disabled_count = selection.disabled_count,
        offline_count = selection.offline_count,
        "selector_resolved"
    );
    Ok(selection
        .targets
        .into_iter()
        .map(|agent| agent.id().as_str().to_owned())
        .collect())
}

fn selector_from_parts(
    selector: Option<&str>,
    match_labels: Option<&BTreeMap<String, String>>,
) -> Result<Option<Selector>, String> {
    match (selector, match_labels) {
        (Some(_), Some(_)) => Err("selector and matchLabels cannot be used together".to_owned()),
        (Some(selector), None) => Selector::parse(selector)
            .map(Some)
            .map_err(|error| error.to_string()),
        (None, Some(labels)) => Selector::from_match_labels(labels.clone())
            .map(Some)
            .map_err(|error| error.to_string()),
        (None, None) => Ok(None),
    }
}

fn job_selector_snapshot(
    explicit_target_agent_ids: &[String],
    selector: Option<&str>,
    match_labels: Option<&BTreeMap<String, String>>,
    resolved_target_agent_ids: &[String],
) -> Result<(String, String), CreateCommandJobHttpError> {
    if !explicit_target_agent_ids.is_empty() {
        return Ok((
            "explicit_ids".to_owned(),
            serde_json::to_string(explicit_target_agent_ids).map_err(|error| {
                CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
            })?,
        ));
    }
    match (selector, match_labels) {
        (Some(selector), None) => Ok(("selector".to_owned(), selector.to_owned())),
        (None, Some(labels)) => Ok((
            "matchLabels".to_owned(),
            serde_json::to_string(labels).map_err(|error| {
                CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
            })?,
        )),
        (None, None) => Ok((
            "resolved_ids".to_owned(),
            serde_json::to_string(resolved_target_agent_ids).map_err(|error| {
                CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
            })?,
        )),
        (Some(_), Some(_)) => Err(CreateCommandJobHttpError::BadRequest(
            "selector and matchLabels cannot be used together".to_owned(),
        )),
    }
}

fn runbook_selector_snapshot(
    selector: &Selector,
) -> Result<(String, String), CreateCommandJobHttpError> {
    match selector {
        Selector::Agent(agent) => Ok(("runbook_selector".to_owned(), format!("agent:{agent}"))),
        Selector::Labels(labels) => {
            let labels = labels
                .iter()
                .map(|label| (label.key().to_owned(), label.value().to_owned()))
                .collect::<BTreeMap<_, _>>();
            Ok((
                "runbook_matchLabels".to_owned(),
                serde_json::to_string(&labels).map_err(|error| {
                    CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
                })?,
            ))
        }
    }
}

fn normalize_job_strategy(
    strategy: Option<&JobStrategyRequest>,
) -> Result<JobStrategyConfig, CreateCommandJobHttpError> {
    let concurrency = strategy
        .and_then(|strategy| strategy.concurrency)
        .unwrap_or(1);
    if concurrency == 0 {
        return Err(CreateCommandJobHttpError::BadRequest(
            "strategy.concurrency must be greater than zero".to_owned(),
        ));
    }
    let max_failures = strategy.and_then(|strategy| strategy.max_failures);
    if max_failures == Some(0) {
        return Err(CreateCommandJobHttpError::BadRequest(
            "strategy.maxFailures must be greater than zero when provided".to_owned(),
        ));
    }
    Ok(JobStrategyConfig {
        concurrency,
        max_failures,
    })
}

fn persist_job_strategy<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    job_id: &str,
    strategy: JobStrategyConfig,
) -> Result<(), CreateCommandJobHttpError> {
    let store = store.into();
    store
        .update_job_strategy(job_id, strategy.concurrency, strategy.max_failures)
        .map_err(|error| CreateCommandJobHttpError::Internal(ControllerError::Store(error)))?;
    Ok(())
}

fn persist_job_selector_snapshot<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    job_id: &str,
    selector_snapshot: (String, String),
) -> Result<(), CreateCommandJobHttpError> {
    let store = store.into();
    store
        .update_job_selector_snapshot(job_id, &selector_snapshot.0, &selector_snapshot.1)
        .map_err(|error| CreateCommandJobHttpError::Internal(ControllerError::Store(error)))?;
    Ok(())
}

fn list_job_output<'a>(
    job_id: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<String, ControllerError> {
    let repo = ControllerJobOutputRepository {
        store: store.into(),
    };
    let chunks = ListJobOutputForJob::execute(&repo, job_id)?;
    let response = chunks
        .into_iter()
        .map(|chunk| JobOutputChunkResponse {
            job_id: chunk.job_id,
            agent_id: chunk.agent_id,
            stream: job_output_stream_to_str(chunk.stream).to_owned(),
            sequence: chunk.sequence,
            data: chunk.body,
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&response).map_err(|error| ControllerError::Json(error.to_string()))
}

enum ArtifactHttpResult {
    Found(String),
    NotFound,
    Corrupt,
}

fn parse_job_artifact_path(path: &str) -> Option<(&str, &str)> {
    let rest = path.trim_start_matches("/api/jobs/");
    let (job_id, artifact_id) = rest.split_once("/artifacts/")?;
    if job_id.is_empty()
        || artifact_id.is_empty()
        || job_id.contains('/')
        || artifact_id.contains('/')
        || artifact_id.contains('\\')
    {
        return None;
    }
    Some((job_id, artifact_id.trim_end_matches('/')))
}

fn get_job_artifact<'a>(
    job_id: &str,
    artifact_id: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    artifact_store: &Mutex<LocalArtifactStore>,
) -> Result<ArtifactHttpResult, ControllerError> {
    let job_id = JobId::new(job_id).map_err(|error| {
        ControllerError::Store(fleet_store::StoreError::Domain(error.to_string()))
    })?;
    let artifact_id = ArtifactId::new(artifact_id).map_err(|error| {
        ControllerError::Store(fleet_store::StoreError::Domain(error.to_string()))
    })?;
    let store = store.into();
    let metadata = store
        .list_rendered_artifacts_for_job(&job_id)?
        .into_iter()
        .find(|metadata| metadata.id == artifact_id);
    let Some(metadata) = metadata else {
        return Ok(ArtifactHttpResult::NotFound);
    };

    let artifact_store = artifact_store.lock().map_err(|_| {
        ControllerError::Store(fleet_store::StoreError::Domain(
            "artifact store lock poisoned".to_owned(),
        ))
    })?;
    let verification =
        artifact_store.verify(&metadata.id, metadata.retention_class, &metadata.checksum)?;
    match verification {
        fleet_application::ArtifactVerification::Missing => {
            tracing::warn!(
                artifact_id = metadata.id.as_str(),
                checksum_prefix = artifact_checksum_prefix(&metadata.checksum),
                status = "missing",
                "artifact_retrieval_failed"
            );
            Ok(ArtifactHttpResult::NotFound)
        }
        fleet_application::ArtifactVerification::Corrupt { .. } => {
            tracing::warn!(
                artifact_id = metadata.id.as_str(),
                checksum_prefix = artifact_checksum_prefix(&metadata.checksum),
                status = "corrupt",
                "artifact_retrieval_failed"
            );
            Ok(ArtifactHttpResult::Corrupt)
        }
        fleet_application::ArtifactVerification::Verified(record) => {
            let Some(content_bytes) = artifact_store.get(&metadata.id, metadata.retention_class)?
            else {
                return Ok(ArtifactHttpResult::NotFound);
            };
            let response = JobArtifactBodyResponse {
                job_id: metadata.job_id.as_str().to_owned(),
                artifact_id: metadata.id.as_str().to_owned(),
                task_id: metadata.task_id.as_str().to_owned(),
                agent_id: metadata.agent_id.as_str().to_owned(),
                retention_class: metadata.retention_class.as_str().to_owned(),
                checksum_sha256: record.checksum.as_sha256().to_owned(),
                size_bytes: record.size_bytes,
                content_bytes,
            };
            serde_json::to_string(&response)
                .map(ArtifactHttpResult::Found)
                .map_err(|error| ControllerError::Json(error.to_string()))
        }
    }
}

fn artifact_checksum_prefix(checksum: &ArtifactChecksum) -> &str {
    let value = checksum.as_sha256();
    &value[..value.len().min(12)]
}

fn cancel_job<'a>(
    job_id: &str,
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
) -> Result<Option<String>, ControllerError> {
    let store = store.into();
    let Some(job_status) = store.find_job_status_value(job_id)? else {
        return Ok(None);
    };
    let request = if body.trim().is_empty() {
        CancelJobRequest { reason: None }
    } else {
        serde_json::from_str::<CancelJobRequest>(body)
            .map_err(|error| ControllerError::Json(error.to_string()))?
    };
    let reason = request
        .reason
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "operator requested cancel".to_owned());
    let now = SystemTime::now();
    let assignments = store.list_task_assignment_summaries_for_job(job_id)?;
    let mut canceled_count = 0;
    let mut cancel_delivered_count = 0;
    let mut first_canceled_assignment = None;

    for assignment in assignments
        .iter()
        .filter(|assignment| assignment_status_accepts_cancel(&assignment.status))
    {
        store.update_task_assignment_status(
            &assignment.task_id,
            AssignmentStatus::Canceled,
            now,
            Some(&reason),
        )?;
        canceled_count += 1;
        if first_canceled_assignment.is_none() {
            first_canceled_assignment = Some(assignment.clone());
        }
        if matches!(
            assignment.status.as_str(),
            "dispatched" | "accepted" | "started"
        ) && deliver_task_cancel(
            sessions,
            &assignment.agent_id,
            &assignment.job_id,
            &assignment.task_id,
            &reason,
        ) {
            cancel_delivered_count += 1;
        }
    }

    if !matches!(
        job_status.as_str(),
        "success" | "failed" | "canceled" | "expired"
    ) {
        store.recompute_job_status_from_assignments(job_id)?;
        audit_job(
            store,
            "job_canceled",
            job_id,
            AuditValue::Plain(format!(
                "reason={},canceled_count={canceled_count},cancel_delivered_count={cancel_delivered_count}",
                fleet_core::redact_secret(&reason)
            )),
        )?;
    }

    let current_assignment_status = first_canceled_assignment
        .as_ref()
        .and_then(|assignment| store.find_task_assignment_status(&assignment.task_id).ok())
        .flatten();
    let final_job_status = store
        .find_job_status_value(job_id)?
        .unwrap_or_else(|| job_status.clone());
    let response = CancelJobResponse {
        job_id: job_id.to_owned(),
        status: final_job_status,
        task_id: first_canceled_assignment
            .as_ref()
            .map(|assignment| assignment.task_id.clone()),
        agent_id: first_canceled_assignment
            .as_ref()
            .map(|assignment| assignment.agent_id.clone()),
        assignment_status: current_assignment_status,
        canceled_count,
        cancel_delivered_count,
        cancel_delivered: cancel_delivered_count > 0,
    };
    serde_json::to_string(&response)
        .map(Some)
        .map_err(|error| ControllerError::Json(error.to_string()))
}

fn assignment_status_accepts_cancel(status: &str) -> bool {
    matches!(status, "queued" | "dispatched" | "accepted" | "started")
}

fn deliver_task_cancel(
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
    agent_id: &str,
    job_id: &str,
    task_id: &str,
    reason: &str,
) -> bool {
    let Some(sessions) = sessions else {
        return false;
    };
    let Ok(sessions) = sessions.lock() else {
        return false;
    };
    let message = fleet_protocol::WireMessage::new(
        fleet_core::generate_prefixed_ulid("msg").unwrap_or_else(|_| "msg-task-cancel".to_owned()),
        fleet_core::generate_prefixed_ulid("corr")
            .unwrap_or_else(|_| "corr-task-cancel".to_owned()),
        None,
        system_time_to_millis(SystemTime::now()),
        fleet_protocol::WirePayload::TaskCancel {
            job_id: job_id.to_owned(),
            task_id: task_id.to_owned(),
            reason: reason.to_owned(),
        },
    );
    sessions.try_send(agent_id, message).is_ok()
}

fn job_output_stream_to_str(stream: JobOutputStream) -> &'static str {
    match stream {
        JobOutputStream::Stdout => "stdout",
        JobOutputStream::Stderr => "stderr",
    }
}

fn list_agents<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
) -> Result<String, ControllerError> {
    mark_stale_agents_offline_for_inventory(store)?;
    let repo = ControllerAgentInventoryRepository {
        store: store.into(),
    };
    let connected_agent_ids = connected_agent_ids(sessions);
    let agents = ListInventoryAgents::execute(&repo)?
        .iter()
        .map(|agent| agent_to_response_with_latest_facts(agent, store, &connected_agent_ids))
        .collect::<Result<Vec<_>, _>>()?;
    serde_json::to_string(&agents).map_err(|error| ControllerError::Json(error.to_string()))
}

fn get_agent<'a>(
    agent_id: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
) -> Result<Option<String>, ControllerError> {
    mark_stale_agents_offline_for_inventory(store)?;
    let agent_id = AgentId::new(agent_id).map_err(|error| ControllerError::Store(error.into()))?;
    let repo = ControllerAgentInventoryRepository {
        store: store.into(),
    };
    let Some(agent) = GetInventoryAgent::execute(&repo, agent_id)? else {
        return Ok(None);
    };
    let connected_agent_ids = connected_agent_ids(sessions);
    let response = agent_to_response_with_latest_facts(&agent, store, &connected_agent_ids)?;
    serde_json::to_string(&response)
        .map(Some)
        .map_err(|error| ControllerError::Json(error.to_string()))
}

fn list_policies<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<String, ControllerError> {
    let store = store.into();
    let policies = store
        .list_policies()?
        .into_iter()
        .map(app_policy_to_response)
        .collect::<Vec<_>>();
    serde_json::to_string(&policies).map_err(|error| ControllerError::Json(error.to_string()))
}

fn save_policy<'a>(
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    actor: &str,
) -> Result<String, ControllerError> {
    let store = store.into();
    let request: SavePolicyRequest =
        serde_json::from_str(body).map_err(|error| ControllerError::Json(error.to_string()))?;
    let now = SystemTime::now();
    let mut repo = ControllerPolicyRepository { store };
    let mut audit = ControllerAuditWriter { store };
    let policy = SavePolicy::execute(
        &mut repo,
        &mut audit,
        SavePolicyInput {
            source: request.source,
            actor: actor.to_owned(),
            now,
        },
    )
    .map_err(map_policy_use_case_error)?;
    let record = store.find_policy(&policy.id)?.ok_or_else(|| {
        ControllerError::Store(fleet_store::StoreError::Domain(
            "saved policy was not readable".to_owned(),
        ))
    })?;
    serde_json::to_string(&policy_to_response(record))
        .map_err(|error| ControllerError::Json(error.to_string()))
}

fn assign_policy<'a>(
    policy_id: &str,
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    actor: &str,
) -> Result<String, ControllerError> {
    let request: AssignPolicyRequest =
        serde_json::from_str(body).map_err(|error| ControllerError::Json(error.to_string()))?;
    let mut repo = ControllerPolicyRepository {
        store: store.into(),
    };
    let mut audit = ControllerAuditWriter {
        store: store.into(),
    };
    let assignment = AssignPolicyToAgent::execute(
        &mut repo,
        &mut audit,
        AssignPolicyToAgentInput {
            policy_id: policy_id.to_owned(),
            agent_id: request.agent_id,
            actor: actor.to_owned(),
            now: SystemTime::now(),
        },
    )
    .map_err(map_policy_use_case_error)?;
    serde_json::to_string(&PolicyAssignmentResponse {
        policy_id: assignment.policy_id,
        agent_id: assignment.agent_id,
        assigned_at_ms: system_time_to_millis(assignment.assigned_at),
    })
    .map_err(|error| ControllerError::Json(error.to_string()))
}

fn schedule_policy_drift<'a>(
    policy_id: &str,
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    actor: &str,
) -> Result<String, ControllerError> {
    let request: SchedulePolicyDriftRequest =
        serde_json::from_str(body).map_err(|error| ControllerError::Json(error.to_string()))?;
    let interval = Duration::from_secs(request.interval_seconds);
    if interval.is_zero() {
        return Err(ControllerError::Json(
            "interval_seconds must be positive".to_owned(),
        ));
    }
    let now = SystemTime::now();
    let next_due_at = now + interval;
    let mut repo = ControllerPolicyRepository {
        store: store.into(),
    };
    let mut audit = ControllerAuditWriter {
        store: store.into(),
    };
    SchedulePolicyDrift::execute(
        &mut repo,
        &mut audit,
        SchedulePolicyDriftInput {
            policy_id: policy_id.to_owned(),
            agent_id: request.agent_id.clone(),
            interval,
            next_due_at,
            actor: actor.to_owned(),
            now,
        },
    )
    .map_err(map_policy_use_case_error)?;
    serde_json::to_string(&ScheduledDriftResponse {
        policy_id: policy_id.to_owned(),
        agent_id: request.agent_id,
        interval_seconds: interval.as_secs(),
        next_due_at_ms: system_time_to_millis(next_due_at),
        last_checked_at_ms: None,
    })
    .map_err(|error| ControllerError::Json(error.to_string()))
}

fn list_due_scheduled_drift<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<String, ControllerError> {
    let repo = ControllerPolicyRepository {
        store: store.into(),
    };
    let records = ListDueScheduledDrift::execute(&repo, SystemTime::now(), 100)?;
    let response = records
        .into_iter()
        .map(scheduled_drift_to_response)
        .collect::<Vec<_>>();
    serde_json::to_string(&response).map_err(|error| ControllerError::Json(error.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ControllerSigningStagedRolloutWorkerOutput {
    loaded: bool,
    rollout_state: Option<String>,
    planned_count: usize,
    attempted_count: usize,
    updated_count: usize,
    skipped_count: usize,
    failed_count: usize,
    already_current_count: usize,
    unavailable_count: usize,
    pending_count: usize,
}

fn run_controller_signing_staged_rollout_once<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    sessions: &Arc<Mutex<AgentSessionRegistry>>,
    identity: &ControllerIdentity,
    metadata: &ControllerRuntimeMetadata,
    now: SystemTime,
) -> Result<ControllerSigningStagedRolloutWorkerOutput, ControllerError> {
    let store_ref = store.into();
    let Some(record) = store_ref
        .load_controller_signing_staged_rollout(DEFAULT_CONTROLLER_ID)
        .map_err(ControllerError::Store)?
    else {
        return Ok(ControllerSigningStagedRolloutWorkerOutput::default());
    };
    let mut output = ControllerSigningStagedRolloutWorkerOutput {
        loaded: true,
        rollout_state: Some(record.rollout.state().as_str().to_owned()),
        ..ControllerSigningStagedRolloutWorkerOutput::default()
    };
    let Some(rotation_record) = store_ref
        .load_signing_key_rotation(DEFAULT_CONTROLLER_ID)
        .map_err(controller_signing_rotation_load_error)?
    else {
        return Ok(output);
    };
    let previous_public_key_path = record
        .previous_fingerprint
        .as_ref()
        .map(|_| controller_signing_previous_public_key_backup_path(metadata))
        .transpose()?;
    let entries = controller_signing_trust_bundle_entries_from_rotation(
        &rotation_record.rotation,
        identity,
        previous_public_key_path.as_deref(),
    )?;
    let current_fingerprint = entries
        .iter()
        .find(|entry| entry.role == fleet_protocol::ControllerSigningTrustRoleWire::Current)
        .map(|entry| entry.fingerprint.clone())
        .ok_or_else(|| {
            ControllerError::SigningKeyRotation(
                "staged rollout worker requires current fingerprint".to_owned(),
            )
        })?;
    let previous_fingerprint = entries
        .iter()
        .find(|entry| entry.role == fleet_protocol::ControllerSigningTrustRoleWire::Previous)
        .map(|entry| entry.fingerprint.clone());
    if record.current_fingerprint != current_fingerprint
        || record.previous_fingerprint != previous_fingerprint
    {
        return Ok(output);
    }

    let mut staged_rollout = record.rollout;
    let target_ids = staged_rollout.snapshot().target_ids;
    let observations = {
        let sessions = sessions.lock().map_err(|_| {
            ControllerError::Store(fleet_store::StoreError::Domain(
                "session registry lock poisoned".to_owned(),
            ))
        })?;
        sessions.controller_signing_staged_rollout_targets(&target_ids, &current_fingerprint)
    };
    controller_signing_staged_rollout_observe_waiting(&mut staged_rollout, &observations, now)?;
    let (planned_agent_ids, already_current_count, unavailable_count, pending_count) =
        if staged_rollout.state().is_terminal()
            || staged_rollout.state()
                == fleet_domain::ControllerSigningStagedRolloutState::WaitingForAck
        {
            let (already_current_count, unavailable_count, pending_count) =
                controller_signing_staged_rollout_counts(&staged_rollout);
            (
                Vec::new(),
                already_current_count,
                unavailable_count,
                pending_count,
            )
        } else {
            let plan = staged_rollout
                .plan_next_batch(&observations, now)
                .map_err(controller_signing_staged_rollout_error)?;
            (
                plan.agent_ids,
                plan.already_current_count,
                plan.unavailable_count,
                plan.pending_count,
            )
        };
    let agent_results = if planned_agent_ids.is_empty() {
        Vec::new()
    } else {
        let results = dispatch_controller_signing_trust_bundle(
            sessions,
            &entries,
            planned_agent_ids.clone(),
            Some(planned_agent_ids.len()),
            now,
        )?;
        let sent_agent_ids = results
            .iter()
            .filter(|result| result.status == "sent")
            .map(|result| result.agent_id.clone())
            .collect::<Vec<_>>();
        if !sent_agent_ids.is_empty() {
            staged_rollout
                .batch_dispatched(&sent_agent_ids, now)
                .map_err(controller_signing_staged_rollout_error)?;
        }
        results
    };
    let attempted_count = agent_results.len();
    let updated_count = agent_results
        .iter()
        .filter(|result| result.status == "sent")
        .count();
    let dispatch_skipped_count = agent_results
        .iter()
        .filter(|result| result.status.starts_with("skipped"))
        .count();
    let dispatch_failed_count = attempted_count
        .saturating_sub(updated_count)
        .saturating_sub(dispatch_skipped_count);
    let failed_count = staged_rollout.snapshot().failed_agent_ids.len() + dispatch_failed_count;
    let skipped_count = already_current_count + unavailable_count + dispatch_skipped_count;
    let current_fingerprint_prefix =
        controller_signing_fingerprint_prefix(&current_fingerprint).to_owned();
    let previous_fingerprint_prefix = previous_fingerprint
        .as_deref()
        .map(|fingerprint| controller_signing_fingerprint_prefix(fingerprint).to_owned());
    let mut staged_store = store.into();
    staged_store.save_controller_signing_staged_rollout(ControllerSigningStagedRolloutRecord {
        controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
        current_fingerprint,
        previous_fingerprint,
        rollout: staged_rollout.clone(),
        updated_at: now,
    })?;
    output.rollout_state = Some(staged_rollout.state().as_str().to_owned());
    output.planned_count = planned_agent_ids.len();
    output.attempted_count = attempted_count;
    output.updated_count = updated_count;
    output.skipped_count = skipped_count;
    output.failed_count = failed_count;
    output.already_current_count = already_current_count;
    output.unavailable_count = unavailable_count;
    output.pending_count = pending_count;

    store.into().write_audit_event(AuditEvent {
        category: AuditCategory::Security,
        action: "controller_signing_trust_bundle_staged_rollout_worker".to_owned(),
        actor: AuditActor::new("staged-rollout-worker"),
        target: AuditTarget::new(DEFAULT_CONTROLLER_ID.to_owned()),
        value: AuditValue::Plain(format!(
            "mode=worker,rollout_state={},planned_count={},updated_count={},skipped_count={},failed_count={},already_current_count={},unavailable_count={},pending_count={},entries_count={},current_fingerprint_prefix={},previous_fingerprint_prefix={}",
            output.rollout_state.as_deref().unwrap_or("none"),
            output.planned_count,
            output.updated_count,
            output.skipped_count,
            output.failed_count,
            output.already_current_count,
            output.unavailable_count,
            output.pending_count,
            entries.len(),
            current_fingerprint_prefix,
            previous_fingerprint_prefix.unwrap_or_else(|| "none".to_owned())
        )),
        occurred_at: now,
    })?;
    Ok(output)
}

fn run_due_scheduled_drift_once<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    identity: &ControllerIdentity,
    now: SystemTime,
) -> Result<fleet_application::RunDueScheduledDriftOutput, ControllerError> {
    let store = store.into();
    let mut repo = ControllerJobRepository { store };
    let mut audit = ControllerAuditWriter { store };
    let mut signer = ControllerTaskSigner {
        private_key: &identity.private_key,
        signing_fingerprint: &identity.fingerprint,
    };
    RunDueScheduledDrift::execute(
        &mut repo,
        &mut audit,
        &mut signer,
        RunDueScheduledDriftInput {
            now,
            grace_duration: SCHEDULED_DRIFT_WORKER_GRACE,
            limit: SCHEDULED_DRIFT_WORKER_LIMIT,
            job_timeout: SCHEDULED_DRIFT_JOB_TIMEOUT,
            job_expires_in: SCHEDULED_DRIFT_JOB_EXPIRES_IN,
            actor: "scheduled-drift-worker".to_owned(),
            job_id_prefix: "scheduled-drift".to_owned(),
            nonce_prefix: "scheduled-drift-nonce".to_owned(),
        },
    )
    .map_err(map_run_due_scheduled_drift_error)
}

fn run_retention_cleanup_once<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    now: SystemTime,
) -> Result<fleet_application::RunRetentionCleanupOutput, ControllerError> {
    let store = store.into();
    let mut repo = ControllerRetentionRepository { store };
    let mut audit = ControllerAuditWriter { store };
    RunRetentionCleanup::execute(
        &mut repo,
        &mut audit,
        RunRetentionCleanupInput {
            now,
            policy: RetentionPolicy::mvp_defaults(),
            dry_run: false,
            actor: "retention-worker".to_owned(),
            target: "controller-store".to_owned(),
        },
    )
    .map_err(map_run_retention_cleanup_error)
}

fn list_agent_policies<'a>(
    agent_id: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<String, ControllerError> {
    let store = store.into();
    let assignments = store
        .policies_for_agent(agent_id)?
        .into_iter()
        .map(|assignment| PolicyAssignmentResponse {
            policy_id: assignment.policy_id,
            agent_id: assignment.agent_id,
            assigned_at_ms: system_time_to_millis(assignment.assigned_at),
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&assignments).map_err(|error| ControllerError::Json(error.to_string()))
}

fn policy_to_response(record: fleet_store::PolicyRecord) -> PolicyResponse {
    PolicyResponse {
        id: record.id,
        name: record.name,
        version: record.version,
        source: record.source,
        created_at_ms: system_time_to_millis(record.created_at),
        updated_at_ms: system_time_to_millis(record.updated_at),
    }
}

fn app_policy_to_response(record: fleet_application::PolicyRecord) -> PolicyResponse {
    PolicyResponse {
        id: record.id,
        name: record.name,
        version: record.version,
        source: record.source,
        created_at_ms: system_time_to_millis(record.created_at),
        updated_at_ms: system_time_to_millis(record.updated_at),
    }
}

fn scheduled_drift_to_response(
    record: fleet_application::ScheduledDriftRecord,
) -> ScheduledDriftResponse {
    ScheduledDriftResponse {
        policy_id: record.policy_id,
        agent_id: record.agent_id,
        interval_seconds: record.interval_seconds,
        next_due_at_ms: system_time_to_millis(record.next_due_at),
        last_checked_at_ms: record.last_checked_at.map(system_time_to_millis),
    }
}

fn mark_stale_agents_offline_for_inventory<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<(), ControllerError> {
    let store = store.into();
    let now = SystemTime::now();
    let cutoff = now
        .checked_sub(AGENT_OFFLINE_AFTER)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let changed = store.mark_stale_agents_offline(cutoff, now)?;
    if changed > 0 {
        tracing::info!(offline_count = changed, "stale_agents_marked_offline");
    }
    Ok(())
}

fn request_agent_certificate_issuance<'a>(
    agent_id: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
    actor: &str,
    now: SystemTime,
) -> Result<Option<String>, ControllerError> {
    let agent_id = AgentId::new(agent_id.to_owned())
        .map_err(|error| ControllerError::Json(error.to_string()))?;
    if store.into().find_agent_by_id(agent_id.as_str())?.is_none() {
        return Ok(None);
    }
    let mut repo = ControllerAgentCertificateLifecycleRepository {
        store: store.into(),
    };
    let mut audit = ControllerAuditWriter {
        store: store.into(),
    };
    let output = RequestAgentCertificateIssuance::execute(
        &mut repo,
        &mut audit,
        RequestAgentCertificateIssuanceInput {
            agent_id: agent_id.clone(),
            actor: actor.to_owned(),
            requested_at: now,
        },
    )
    .map_err(map_agent_certificate_lifecycle_error)?;
    let action = fleet_protocol::AgentCertificateLifecycleActionWire::RequestIssuance;
    let dispatch = if let Some(sessions) = sessions {
        dispatch_agent_certificate_lifecycle_update(
            sessions,
            action,
            &output.record,
            &fleet_core::generate_prefixed_ulid("corr")
                .map_err(|error| ControllerError::Json(error.to_string()))?,
            system_time_to_millis(now),
        )?
    } else {
        agent_certificate_lifecycle_dispatch_result(
            action,
            &output.record,
            AgentCertificateLifecycleDispatchStatus::NotConnected,
        )
    };
    let response = agent_certificate_issuance_request_response(&output, &dispatch);
    serde_json::to_string(&response)
        .map(Some)
        .map_err(|error| ControllerError::Json(error.to_string()))
}

fn get_agent_certificate_lifecycle_status<'a>(
    agent_id: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<Option<String>, ControllerError> {
    let agent_id = AgentId::new(agent_id.to_owned())
        .map_err(|error| ControllerError::Json(error.to_string()))?;
    let store = store.into();
    if store.find_agent_by_id(agent_id.as_str())?.is_none() {
        return Ok(None);
    }
    let record = store.load_agent_certificate_lifecycle(&agent_id)?;
    let response = agent_certificate_lifecycle_status_response(&agent_id, record.as_ref());
    serde_json::to_string(&response)
        .map(Some)
        .map_err(|error| ControllerError::Json(error.to_string()))
}

fn update_agent_labels<'a>(
    agent_id: &str,
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
    actor: &str,
) -> Result<Option<String>, ControllerError> {
    let request: UpdateAgentLabelsRequest =
        serde_json::from_str(body).map_err(|error| ControllerError::Json(error.to_string()))?;
    let labels = request
        .labels
        .into_iter()
        .map(|label| {
            AgentLabel::new(label.key, label.value)
                .map_err(|error| ControllerError::Store(error.into()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut repo = ControllerAgentInventoryRepository {
        store: store.into(),
    };
    let mut audit = ControllerAuditWriter {
        store: store.into(),
    };
    let Some(agent) = UpdateAgentLabels::execute(
        &mut repo,
        &mut audit,
        UpdateAgentLabelsInput {
            agent_id: agent_id.to_owned(),
            labels,
            actor: actor.to_owned(),
            occurred_at: SystemTime::now(),
        },
    )
    .map_err(map_update_agent_labels_error)?
    else {
        return Ok(None);
    };
    let connected_agent_ids = connected_agent_ids(sessions);
    let response = agent_to_response_with_latest_facts(&agent, store, &connected_agent_ids)?;
    serde_json::to_string(&response)
        .map(Some)
        .map_err(|error| ControllerError::Json(error.to_string()))
}

fn revoke_agent_key<'a>(
    agent_id: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
    actor: &str,
) -> Result<Option<String>, ControllerError> {
    let mut repo = ControllerAgentInventoryRepository {
        store: store.into(),
    };
    let mut audit = ControllerAuditWriter {
        store: store.into(),
    };
    let Some(agent) = RevokeAgentKey::execute(
        &mut repo,
        &mut audit,
        RevokeAgentKeyInput {
            agent_id: agent_id.to_owned(),
            actor: actor.to_owned(),
            occurred_at: SystemTime::now(),
        },
    )
    .map_err(map_revoke_agent_key_error)?
    else {
        return Ok(None);
    };
    if let Some(sessions) = sessions {
        let ended = {
            let mut sessions = sessions.lock().map_err(|_| {
                ControllerError::Store(fleet_store::StoreError::Domain(
                    "session registry lock poisoned".to_owned(),
                ))
            })?;
            sessions.close(agent_id, AgentSessionCloseReason::Revoked)
        };
        if let Some(ended) = ended {
            audit_agent_session_revoked_closed(store, &ended)?;
        }
    }
    let connected_agent_ids = connected_agent_ids(sessions);
    let response = agent_to_response_with_latest_facts(&agent, store, &connected_agent_ids)?;
    serde_json::to_string(&response)
        .map(Some)
        .map_err(|error| ControllerError::Json(error.to_string()))
}

fn latest_facts<'a>(
    agent_id: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<Option<String>, ControllerError> {
    let repo = ControllerFactsRepository {
        store: store.into(),
    };
    let Some(snapshot) = GetLatestFacts::execute(&repo, agent_id)? else {
        return Ok(None);
    };
    let body = serde_json::from_str(&snapshot.body)
        .map_err(|error| ControllerError::Json(error.to_string()))?;
    serde_json::to_string(&LatestFactsResponse {
        agent_id: snapshot.agent_id,
        collected_at_ms: system_time_to_millis(snapshot.collected_at),
        agent_system_time_ms: agent_system_time_ms_from_body(&body)
            .unwrap_or_else(|| system_time_to_millis(snapshot.collected_at)),
        body,
    })
    .map(Some)
    .map_err(|error| ControllerError::Json(error.to_string()))
}

fn list_facts_snapshots<'a>(
    agent_id: &str,
    raw_path: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<String, ControllerError> {
    let page = parse_snapshot_page_request(raw_path)?;
    let repo = ControllerFactsRepository {
        store: store.into(),
    };
    let mut snapshots =
        ListFactsSnapshots::execute(&repo, agent_id, page.fetch_limit(), page.before)?;
    let has_more = snapshots.len() > page.limit;
    if has_more {
        snapshots.truncate(page.limit);
    }
    let next_cursor =
        next_snapshot_cursor(snapshots.last().map(|snapshot| snapshot.cursor), has_more);
    let items = snapshots
        .into_iter()
        .map(|snapshot| {
            let body = serde_json::from_str(&snapshot.body)
                .map_err(|error| ControllerError::Json(error.to_string()))?;
            Ok(FactsSnapshotItemResponse {
                agent_id: snapshot.agent_id,
                collected_at_ms: system_time_to_millis(snapshot.collected_at),
                agent_system_time_ms: agent_system_time_ms_from_body(&body)
                    .unwrap_or_else(|| system_time_to_millis(snapshot.collected_at)),
                body,
                cursor: encode_snapshot_page_cursor(snapshot.cursor),
            })
        })
        .collect::<Result<Vec<_>, ControllerError>>()?;
    serde_json::to_string(&FactsSnapshotPageResponse { items, next_cursor })
        .map_err(|error| ControllerError::Json(error.to_string()))
}

fn list_jobs<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
) -> Result<String, ControllerError> {
    let store = store.into();
    let repo = ControllerJobQueryRepository { store };
    let jobs = ListJobSummaries::execute(&repo, 50)?;
    let connected_agent_ids = connected_agent_ids(sessions);
    let response = jobs
        .into_iter()
        .map(|job| job_summary_response_with_artifacts(job, &connected_agent_ids, store))
        .collect::<Result<Vec<_>, _>>()?;
    serde_json::to_string(&response).map_err(|error| ControllerError::Json(error.to_string()))
}

fn get_job<'a>(
    job_id: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
) -> Result<Option<String>, ControllerError> {
    let store = store.into();
    let repo = ControllerJobQueryRepository { store };
    let Some(job) = GetJobSummary::execute(&repo, job_id)? else {
        return Ok(None);
    };
    let connected_agent_ids = connected_agent_ids(sessions);
    serde_json::to_string(&job_summary_response_with_artifacts(
        job,
        &connected_agent_ids,
        store,
    )?)
    .map(Some)
    .map_err(|error| ControllerError::Json(error.to_string()))
}

fn list_approvals<'a>(
    raw_path: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<String, ControllerError> {
    let status = query_param(raw_path, "status");
    let repo = ControllerJobRepository {
        store: store.into(),
    };
    let approvals = ListApprovalRequests::execute(&repo, status, 100)?;
    let response = approvals
        .into_iter()
        .map(approval_response)
        .collect::<Vec<_>>();
    serde_json::to_string(&response).map_err(|error| ControllerError::Json(error.to_string()))
}

fn approve_approval<'a>(
    approval_id: &str,
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    actor: &str,
) -> Result<Option<(String, String)>, ControllerError> {
    let request: ApprovalDecisionRequest =
        serde_json::from_str(body).map_err(|error| ControllerError::Json(error.to_string()))?;
    let mut repo = ControllerJobRepository {
        store: store.into(),
    };
    let mut audit = ControllerAuditWriter {
        store: store.into(),
    };
    let output = ApproveApprovalRequest::execute(
        &mut repo,
        &mut audit,
        ApproveApprovalInput {
            approval_id: approval_id.to_owned(),
            approver: actor.to_owned(),
            reason: request.reason,
            occurred_at: SystemTime::now(),
        },
    )
    .map_err(map_approval_use_case_error)?;
    let job_id = output.approval.job_id.clone();
    let body = serde_json::to_string(&approval_response(output.approval))
        .map_err(|error| ControllerError::Json(error.to_string()))?;
    Ok(Some((body, job_id)))
}

fn reject_approval<'a>(
    approval_id: &str,
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    actor: &str,
) -> Result<Option<String>, ControllerError> {
    let request: ApprovalDecisionRequest =
        serde_json::from_str(body).map_err(|error| ControllerError::Json(error.to_string()))?;
    let mut repo = ControllerJobRepository {
        store: store.into(),
    };
    let mut audit = ControllerAuditWriter {
        store: store.into(),
    };
    let output = RejectApprovalRequest::execute(
        &mut repo,
        &mut audit,
        RejectApprovalInput {
            approval_id: approval_id.to_owned(),
            approver: actor.to_owned(),
            reason: request.reason,
            occurred_at: SystemTime::now(),
        },
    )
    .map_err(map_approval_use_case_error)?;
    serde_json::to_string(&approval_response(output.approval))
        .map(Some)
        .map_err(|error| ControllerError::Json(error.to_string()))
}

fn expire_approvals<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<String, ControllerError> {
    let mut repo = ControllerJobRepository {
        store: store.into(),
    };
    let mut audit = ControllerAuditWriter {
        store: store.into(),
    };
    let output = ExpireApprovalRequests::execute(
        &mut repo,
        &mut audit,
        ExpireApprovalsInput {
            occurred_at: SystemTime::now(),
        },
    )
    .map_err(map_approval_use_case_error)?;
    let approvals = output
        .expired
        .into_iter()
        .map(approval_response)
        .collect::<Vec<_>>();
    serde_json::to_string(&ExpireApprovalsResponse {
        expired_count: approvals.len(),
        approvals,
    })
    .map_err(|error| ControllerError::Json(error.to_string()))
}

fn approval_response(record: AppApprovalRequestRecord) -> ApprovalRequestResponse {
    ApprovalRequestResponse {
        id: record.id,
        job_id: record.job_id,
        requester: record.requester,
        approver: record.approver,
        reason: record.reason,
        status: record.status,
        expires_at_ms: system_time_to_millis(record.expires_at),
        created_at_ms: system_time_to_millis(record.created_at),
        decided_at_ms: record.decided_at.map(system_time_to_millis),
    }
}

fn remediation_response(record: RemediationRequestRecord) -> RemediationRequestResponse {
    RemediationRequestResponse {
        id: record.id,
        policy_id: record.policy_id,
        policy_name: record.policy_name,
        agent_id: record.agent_id,
        runbook_ref: record.runbook_ref,
        status: record.status,
        approval_required: record.approval_required,
        risk_summary: record.risk_summary,
        job_id: record.job_id,
        created_at_ms: system_time_to_millis(record.created_at),
        updated_at_ms: system_time_to_millis(record.updated_at),
    }
}

fn list_remediations<'a>(
    raw_path: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<String, ControllerError> {
    let store = store.into();
    let limit = match query_param(raw_path, "limit") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| ControllerError::Json("limit must be a positive integer".to_owned()))?,
        None => 100,
    };
    if limit == 0 {
        return Err(ControllerError::Json(
            "limit must be a positive integer".to_owned(),
        ));
    }
    let records = store.list_remediation_request_records(
        query_param(raw_path, "agent_id"),
        query_param(raw_path, "policy_id"),
        limit,
    )?;
    let response = records
        .into_iter()
        .map(remediation_response)
        .collect::<Vec<_>>();
    serde_json::to_string(&response).map_err(|error| ControllerError::Json(error.to_string()))
}

fn get_remediation<'a>(
    remediation_id: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<Option<String>, ControllerError> {
    let store = store.into();
    store
        .find_remediation_request_record(remediation_id)?
        .map(remediation_response)
        .map(|response| {
            serde_json::to_string(&response)
                .map_err(|error| ControllerError::Json(error.to_string()))
        })
        .transpose()
}

fn create_remediation_approval_request<'a>(
    remediation_id: &str,
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    actor: &str,
) -> Result<String, RemediationHttpError> {
    let request: CreateRemediationApprovalRequest = serde_json::from_str(body)
        .map_err(|error| RemediationHttpError::BadRequest(error.to_string()))?;
    let now = SystemTime::now();
    let approval_id = match request.approval_id {
        Some(value) => value,
        None => fleet_core::generate_prefixed_ulid("approval").map_err(|error| {
            RemediationHttpError::Internal(ControllerError::Json(error.to_string()))
        })?,
    };
    let job_id = match request.job_id {
        Some(value) => value,
        None => fleet_core::generate_prefixed_ulid("remediation-job").map_err(|error| {
            RemediationHttpError::Internal(ControllerError::Json(error.to_string()))
        })?,
    };
    let mut repo = ControllerJobRepository {
        store: store.into(),
    };
    let mut audit = ControllerAuditWriter {
        store: store.into(),
    };
    let output = RequestRemediationApproval::execute(
        &mut repo,
        &mut audit,
        RequestRemediationApprovalInput {
            remediation_id: remediation_id.to_owned(),
            approval_id,
            job_id,
            requester: actor.to_owned(),
            reason: request.reason,
            expires_at: now + Duration::from_secs(request.expires_in_seconds),
            now,
        },
    )
    .map_err(map_remediation_approval_request_error)?;
    serde_json::to_string(&CreateRemediationApprovalResponse {
        remediation: remediation_response(output.remediation),
        approval: approval_response(output.approval),
    })
    .map_err(|error| RemediationHttpError::Internal(ControllerError::Json(error.to_string())))
}

fn approve_remediation_job<'a>(
    remediation_id: &str,
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    identity: &ControllerIdentity,
    actor: &str,
) -> Result<(String, String), RemediationHttpError> {
    let request: ApproveRemediationJobRequest = serde_json::from_str(body)
        .map_err(|error| RemediationHttpError::BadRequest(error.to_string()))?;
    let issued_at = SystemTime::now();
    let expires_at = issued_at + Duration::from_secs(request.expires_in_seconds);
    let nonce_prefix = match request.nonce_prefix {
        Some(prefix) => prefix,
        None => fleet_core::generate_prefixed_ulid("nonce").map_err(|error| {
            RemediationHttpError::Internal(ControllerError::Json(error.to_string()))
        })?,
    };
    let job_id = request.job_id.clone();
    let mut repo = ControllerJobRepository {
        store: store.into(),
    };
    let mut audit = ControllerAuditWriter {
        store: store.into(),
    };
    let mut signer = ControllerTaskSigner {
        private_key: &identity.private_key,
        signing_fingerprint: &identity.fingerprint,
    };
    let output = ApproveRemediationRunbookJob::execute(
        &mut repo,
        &mut audit,
        &mut signer,
        ApproveRemediationRunbookJobInput {
            remediation_id: remediation_id.to_owned(),
            approval_id: request.approval_id,
            job_id: request.job_id,
            runbook_document: request.runbook_document,
            timeout: Duration::from_secs(request.timeout_seconds),
            approver: actor.to_owned(),
            approval_reason: request.reason,
            issued_at,
            expires_at,
            nonce_prefix,
        },
    )
    .map_err(map_approve_remediation_job_error)?;
    let body = serde_json::to_string(&ApproveRemediationJobResponse {
        remediation: remediation_response(output.remediation),
        approval: approval_response(output.approval),
        job_id: job_id.clone(),
        assignment_count: output.envelopes.len(),
        status: "job_created".to_owned(),
    })
    .map_err(|error| RemediationHttpError::Internal(ControllerError::Json(error.to_string())))?;
    Ok((body, job_id))
}

fn mark_remediation_running<'a>(
    remediation_id: &str,
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    actor: &str,
) -> Result<String, RemediationHttpError> {
    let request: RemediationJobRunningRequest = serde_json::from_str(body)
        .map_err(|error| RemediationHttpError::BadRequest(error.to_string()))?;
    let mut repo = ControllerJobRepository {
        store: store.into(),
    };
    let mut audit = ControllerAuditWriter {
        store: store.into(),
    };
    let output = MarkRemediationJobRunning::execute(
        &mut repo,
        &mut audit,
        MarkRemediationJobRunningInput {
            remediation_id: remediation_id.to_owned(),
            job_id: request.job_id,
            actor: actor.to_owned(),
            occurred_at: SystemTime::now(),
        },
    )
    .map_err(map_remediation_result_error)?;
    serde_json::to_string(&remediation_response(output.remediation))
        .map_err(|error| RemediationHttpError::Internal(ControllerError::Json(error.to_string())))
}

fn record_remediation_result<'a>(
    remediation_id: &str,
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    actor: &str,
) -> Result<String, RemediationHttpError> {
    let request: RemediationJobResultRequest = serde_json::from_str(body)
        .map_err(|error| RemediationHttpError::BadRequest(error.to_string()))?;
    let status = match request.status.as_str() {
        "succeeded" | "success" => RemediationJobResultStatus::Succeeded,
        "failed" | "failure" | "canceled" | "expired" => RemediationJobResultStatus::Failed,
        _ => {
            return Err(RemediationHttpError::BadRequest(
                "remediation result status must be succeeded or failed".to_owned(),
            ));
        }
    };
    let mut repo = ControllerJobRepository {
        store: store.into(),
    };
    let mut audit = ControllerAuditWriter {
        store: store.into(),
    };
    let output = RecordRemediationJobResult::execute(
        &mut repo,
        &mut audit,
        RecordRemediationJobResultInput {
            remediation_id: remediation_id.to_owned(),
            job_id: request.job_id,
            status,
            actor: actor.to_owned(),
            occurred_at: SystemTime::now(),
        },
    )
    .map_err(map_remediation_result_error)?;
    serde_json::to_string(&remediation_response(output.remediation))
        .map_err(|error| RemediationHttpError::Internal(ControllerError::Json(error.to_string())))
}

fn verify_remediation<'a>(
    remediation_id: &str,
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    actor: &str,
) -> Result<String, RemediationHttpError> {
    let request: RemediationVerifyRequest = serde_json::from_str(body)
        .map_err(|error| RemediationHttpError::BadRequest(error.to_string()))?;
    let mut repo = ControllerJobRepository {
        store: store.into(),
    };
    let mut audit = ControllerAuditWriter {
        store: store.into(),
    };
    let output = VerifyRemediationResolution::execute(
        &mut repo,
        &mut audit,
        VerifyRemediationResolutionInput {
            remediation_id: remediation_id.to_owned(),
            agent_id: request.agent_id,
            policy_id: request.policy_id,
            policy_name: request.policy_name,
            job_id: request.job_id,
            actor: actor.to_owned(),
            verified_at: SystemTime::now(),
        },
    )
    .map_err(map_remediation_result_error)?;
    serde_json::to_string(&remediation_response(output.remediation))
        .map_err(|error| RemediationHttpError::Internal(ControllerError::Json(error.to_string())))
}

fn connected_agent_ids(
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
) -> std::collections::BTreeSet<String> {
    sessions
        .and_then(|sessions| sessions.lock().ok())
        .map(|sessions| {
            sessions
                .snapshot()
                .into_iter()
                .map(|summary| summary.agent_id)
                .collect()
        })
        .unwrap_or_default()
}

fn job_summary_response(
    job: fleet_application::JobSummaryRecord,
    connected_agent_ids: &std::collections::BTreeSet<String>,
) -> JobSummaryResponse {
    let target_agents = job
        .target_agents
        .iter()
        .map(|target| JobTargetSummaryResponse {
            connected: connected_agent_ids.contains(&target.agent_id),
            agent_id: target.agent_id.clone(),
            name: target.agent_name.clone(),
            status: agent_status_for_job_target(
                &target.status,
                connected_agent_ids.contains(&target.agent_id),
            ),
            snapshot_status: target.status.clone(),
            labels: target
                .labels
                .iter()
                .map(|(key, value)| AgentLabelResponse {
                    key: key.clone(),
                    value: value.clone(),
                })
                .collect(),
            task_id: target.task_id.clone(),
            assignment_status: target.assignment_status.clone(),
            last_error: target.last_error.clone(),
            revoked: target.status == "disabled",
        })
        .collect::<Vec<_>>();
    let target_agent_ids = target_agents
        .iter()
        .map(|target| target.agent_id.clone())
        .collect::<Vec<_>>();
    let assignment_summary = assignment_summary_for_targets(&target_agents);
    let target_connected = target_agents.iter().any(|target| target.connected);
    let created_at_ms = system_time_to_millis(job.created_at);
    let dispatch_state = job_dispatch_state(&job.status, target_connected);
    JobSummaryResponse {
        id: job.id,
        status: job.status,
        dispatch_state,
        risk: job.risk,
        command_program: job.command_program,
        command_args: job.command_args,
        selector_kind: job.selector_kind,
        selector_source: job.selector_source,
        strategy: JobStrategyResponse {
            concurrency: job.strategy_concurrency,
            max_failures: job.strategy_max_failures,
        },
        target_count: job.target_count,
        target_agent_ids,
        target_agents,
        assignment_summary,
        rendered_artifacts: Vec::new(),
        target_connected,
        created_at_ms,
        updated_at_ms: created_at_ms,
        expires_at_ms: job.expires_at.map(system_time_to_millis),
        last_error: String::new(),
    }
}

fn job_summary_response_with_artifacts(
    job: fleet_application::JobSummaryRecord,
    connected_agent_ids: &std::collections::BTreeSet<String>,
    store: ControllerStoreRef<'_>,
) -> Result<JobSummaryResponse, ControllerError> {
    let artifacts = list_rendered_artifact_metadata_responses_for_job(store, &job.id)?;
    let mut response = job_summary_response(job, connected_agent_ids);
    response.rendered_artifacts = artifacts;
    Ok(response)
}

fn list_rendered_artifact_metadata_responses_for_job(
    store: ControllerStoreRef<'_>,
    job_id: &str,
) -> Result<Vec<RenderedArtifactMetadataResponse>, ControllerError> {
    let job_id = JobId::new(job_id.to_owned()).map_err(|error| {
        ControllerError::Store(fleet_store::StoreError::Domain(error.to_string()))
    })?;
    store
        .list_rendered_artifacts_for_job(&job_id)?
        .into_iter()
        .map(|artifact| {
            Ok(RenderedArtifactMetadataResponse {
                artifact_id: artifact.id.as_str().to_owned(),
                task_id: artifact.task_id.as_str().to_owned(),
                agent_id: artifact.agent_id.as_str().to_owned(),
                retention_class: artifact.retention_class.as_str().to_owned(),
                checksum_sha256: artifact.checksum.as_sha256().to_owned(),
                size_bytes: artifact.size_bytes,
            })
        })
        .collect()
}

fn assignment_summary_for_targets(
    targets: &[JobTargetSummaryResponse],
) -> JobAssignmentSummaryResponse {
    let mut summary = JobAssignmentSummaryResponse {
        queued: 0,
        dispatched: 0,
        accepted: 0,
        started: 0,
        succeeded: 0,
        failed: 0,
        rejected: 0,
        canceled: 0,
        expired: 0,
        skipped: 0,
        unknown: 0,
    };

    for target in targets {
        match target.assignment_status.as_deref() {
            Some("queued") => summary.queued += 1,
            Some("dispatched") => summary.dispatched += 1,
            Some("accepted") => summary.accepted += 1,
            Some("started") => summary.started += 1,
            Some("succeeded") => summary.succeeded += 1,
            Some("failed") => summary.failed += 1,
            Some("rejected") => summary.rejected += 1,
            Some("canceled") => {
                summary.canceled += 1;
                if target.last_error.contains("maxFailures") {
                    summary.skipped += 1;
                }
            }
            Some("expired") => summary.expired += 1,
            _ => summary.unknown += 1,
        }
    }

    summary
}

fn job_dispatch_state(status: &str, target_connected: bool) -> String {
    match status {
        "queued" if target_connected => "created",
        "queued" => "queued",
        "running" => "delivered",
        "success" => "completed",
        "failed" => "failed",
        "expired" => "expired",
        "canceled" => "canceled",
        value => value,
    }
    .to_owned()
}

fn agent_status_for_job_target(status: &str, connected: bool) -> String {
    if status == "disabled" {
        "offline".to_owned()
    } else if connected && matches!(status, "pending" | "offline" | "unknown") {
        "online".to_owned()
    } else {
        status.to_owned()
    }
}

fn facts_payload_is_degraded(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("degraded")
                .and_then(|degraded| degraded.get("status"))
                .and_then(serde_json::Value::as_bool)
        })
        .unwrap_or(false)
}

fn latest_metrics<'a>(
    agent_id: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<Option<String>, ControllerError> {
    let repo = ControllerMetricsRepository {
        store: store.into(),
    };
    let Some(snapshot) = GetLatestMetrics::execute(&repo, agent_id)? else {
        return Ok(None);
    };
    let body = serde_json::from_str(&snapshot.body)
        .map_err(|error| ControllerError::Json(error.to_string()))?;
    serde_json::to_string(&LatestMetricsResponse {
        agent_id: snapshot.agent_id,
        collected_at_ms: system_time_to_millis(snapshot.collected_at),
        agent_system_time_ms: agent_system_time_ms_from_body(&body)
            .unwrap_or_else(|| system_time_to_millis(snapshot.collected_at)),
        body,
    })
    .map(Some)
    .map_err(|error| ControllerError::Json(error.to_string()))
}

fn list_metrics_snapshots<'a>(
    agent_id: &str,
    raw_path: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<String, ControllerError> {
    let page = parse_snapshot_page_request(raw_path)?;
    let repo = ControllerMetricsRepository {
        store: store.into(),
    };
    let mut snapshots =
        ListMetricsSnapshots::execute(&repo, agent_id, page.fetch_limit(), page.before)?;
    let has_more = snapshots.len() > page.limit;
    if has_more {
        snapshots.truncate(page.limit);
    }
    let next_cursor =
        next_snapshot_cursor(snapshots.last().map(|snapshot| snapshot.cursor), has_more);
    let items = snapshots
        .into_iter()
        .map(|snapshot| {
            let body = serde_json::from_str(&snapshot.body)
                .map_err(|error| ControllerError::Json(error.to_string()))?;
            Ok(MetricsSnapshotItemResponse {
                agent_id: snapshot.agent_id,
                collected_at_ms: system_time_to_millis(snapshot.collected_at),
                agent_system_time_ms: agent_system_time_ms_from_body(&body)
                    .unwrap_or_else(|| system_time_to_millis(snapshot.collected_at)),
                body,
                cursor: encode_snapshot_page_cursor(snapshot.cursor),
            })
        })
        .collect::<Result<Vec<_>, ControllerError>>()?;
    serde_json::to_string(&MetricsSnapshotPageResponse { items, next_cursor })
        .map_err(|error| ControllerError::Json(error.to_string()))
}

fn list_agent_logs<'a>(
    agent_id: &str,
    raw_path: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<String, ControllerError> {
    let page = parse_snapshot_page_request(raw_path)?;
    let repo = ControllerAgentLogRepository {
        store: store.into(),
    };
    let mut chunks = ListAgentLogChunks::execute(&repo, agent_id, page.fetch_limit(), page.before)?;
    let has_more = chunks.len() > page.limit;
    if has_more {
        chunks.truncate(page.limit);
    }
    let next_cursor = next_snapshot_cursor(chunks.last().map(|chunk| chunk.cursor), has_more);
    let items = chunks
        .into_iter()
        .map(|chunk| AgentLogChunkItemResponse {
            agent_id: chunk.agent_id,
            collected_at_ms: system_time_to_millis(chunk.collected_at),
            line: chunk.line,
            cursor: encode_snapshot_page_cursor(chunk.cursor),
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&AgentLogChunkPageResponse { items, next_cursor })
        .map_err(|error| ControllerError::Json(error.to_string()))
}

fn latest_drift_report<'a>(
    agent_id: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<Option<String>, ControllerError> {
    let repo = ControllerDriftRepository {
        store: store.into(),
    };
    let Some(record) = GetLatestDrift::execute(&repo, agent_id)? else {
        return Ok(None);
    };
    serde_json::to_string(&LatestDriftReportResponse {
        agent_id: record.agent_id,
        checked_at_ms: system_time_to_millis(record.checked_at),
        agent_system_time_ms: system_time_to_millis(record.checked_at),
        policy_name: record.report.policy_name,
        status: drift_status_to_str(&record.report.status).to_owned(),
        severity: drift_severity_to_str(record.report.severity).to_owned(),
        acknowledged: record.report.acknowledgement.is_acknowledged(),
        acknowledged_by: drift_acknowledged_by(&record.report.acknowledgement),
        acknowledged_at_ms: drift_acknowledged_at(&record.report.acknowledgement)
            .map(system_time_to_millis),
        resolved: matches!(
            record.report.acknowledgement,
            DriftAcknowledgement::Resolved { .. }
        ),
        resolution_job_id: drift_resolution_job_id(&record.report.acknowledgement),
        resolved_at_ms: drift_resolved_at(&record.report.acknowledgement)
            .map(system_time_to_millis),
        expected: record.report.expected,
        actual: record.report.actual,
    })
    .map(Some)
    .map_err(|error| ControllerError::Json(error.to_string()))
}

fn list_drift_reports<'a>(
    agent_id: &str,
    raw_path: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<String, ControllerError> {
    let page = parse_snapshot_page_request(raw_path)?;
    let repo = ControllerDriftRepository {
        store: store.into(),
    };
    let mut reports = ListDriftReports::execute(&repo, agent_id, page.fetch_limit(), page.before)?;
    let has_more = reports.len() > page.limit;
    if has_more {
        reports.truncate(page.limit);
    }
    let next_cursor = next_snapshot_cursor(reports.last().map(|report| report.cursor), has_more);
    let items = reports
        .into_iter()
        .map(|record| DriftReportItemResponse {
            agent_id: record.agent_id,
            checked_at_ms: system_time_to_millis(record.checked_at),
            agent_system_time_ms: system_time_to_millis(record.checked_at),
            policy_name: record.report.policy_name,
            status: drift_status_to_str(&record.report.status).to_owned(),
            severity: drift_severity_to_str(record.report.severity).to_owned(),
            acknowledged: record.report.acknowledgement.is_acknowledged(),
            acknowledged_by: drift_acknowledged_by(&record.report.acknowledgement),
            acknowledged_at_ms: drift_acknowledged_at(&record.report.acknowledgement)
                .map(system_time_to_millis),
            resolved: matches!(
                record.report.acknowledgement,
                DriftAcknowledgement::Resolved { .. }
            ),
            resolution_job_id: drift_resolution_job_id(&record.report.acknowledgement),
            resolved_at_ms: drift_resolved_at(&record.report.acknowledgement)
                .map(system_time_to_millis),
            expected: record.report.expected,
            actual: record.report.actual,
            cursor: encode_snapshot_page_cursor(record.cursor),
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&DriftReportPageResponse { items, next_cursor })
        .map_err(|error| ControllerError::Json(error.to_string()))
}

fn list_audit_events<'a>(
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<String, ControllerError> {
    let repo = ControllerAuditRepository {
        store: store.into(),
    };
    let events = ListAuditEvents::execute(&repo, 50)?;
    let response = events
        .iter()
        .map(audit_event_to_response)
        .collect::<Vec<_>>();
    serde_json::to_string(&response).map_err(|error| ControllerError::Json(error.to_string()))
}

fn export_audit_events<'a>(
    raw_path: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<String, ControllerError> {
    let page = parse_snapshot_page_request(raw_path)?;
    let category = parse_audit_category_query(raw_path)?;
    let repo = ControllerAuditRepository {
        store: store.into(),
    };
    let mut records = ExportAuditEvents::execute(&repo, category, page.fetch_limit(), page.before)?;
    let has_more = records.len() > page.limit;
    if has_more {
        records.truncate(page.limit);
    }
    let next_cursor = next_snapshot_cursor(records.last().map(|record| record.cursor), has_more);
    let items = records
        .iter()
        .map(audit_event_to_export_response)
        .collect::<Vec<_>>();
    serde_json::to_string(&AuditExportPageResponse { items, next_cursor })
        .map_err(|error| ControllerError::Json(error.to_string()))
}

fn parse_audit_category_query(raw_path: &str) -> Result<Option<AuditCategory>, ControllerError> {
    let Some(category) = query_param(raw_path, "category") else {
        return Ok(None);
    };
    if category.is_empty() {
        return Ok(None);
    }
    AuditCategory::parse(category)
        .map(Some)
        .ok_or_else(|| ControllerError::Json(format!("unknown audit category: {category}")))
}

fn audit_event_to_response(event: &AuditEvent) -> AuditEventResponse {
    let (value_kind, value) = audit_value_to_response(&event.value);
    AuditEventResponse {
        category: event.category.as_str().to_owned(),
        action: event.action.clone(),
        actor: event.actor.as_str().to_owned(),
        target: event.target.as_str().to_owned(),
        value_kind: value_kind.to_owned(),
        value,
        occurred_at_ms: system_time_to_millis(event.occurred_at),
    }
}

fn audit_event_to_export_response(
    record: &fleet_application::AuditEventPageRecord,
) -> AuditExportEventResponse {
    let event = audit_event_to_response(&record.event);
    AuditExportEventResponse {
        category: event.category,
        action: event.action,
        actor: event.actor,
        target: event.target,
        value_kind: event.value_kind,
        value: event.value,
        occurred_at_ms: event.occurred_at_ms,
        cursor: encode_snapshot_page_cursor(record.cursor),
    }
}

fn audit_value_to_response(value: &AuditValue) -> (&'static str, String) {
    match value {
        AuditValue::Plain(value) => ("plain", value.clone()),
        AuditValue::SecretRef(_) => ("secret_ref", "secret_ref".to_owned()),
        AuditValue::Redacted => ("redacted", "redacted".to_owned()),
    }
}

#[derive(Debug, Clone, Default)]
struct AgentFactsSummary {
    hostname: Option<String>,
    os: Option<String>,
    arch: Option<String>,
}

fn agent_to_response_with_latest_facts<'a>(
    agent: &Agent,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    connected_agent_ids: &std::collections::BTreeSet<String>,
) -> Result<AgentResponse, ControllerError> {
    let store = store.into();
    let summary = store
        .latest_facts_snapshot(agent.id().as_str())?
        .and_then(|record| agent_facts_summary(&record.body));
    let assigned_policy_ids = store.assigned_policy_ids_for_agent(agent.id().as_str())?;
    let capability_snapshot = store.latest_agent_capability_snapshot(agent.id().as_str())?;
    let capabilities = capability_snapshot
        .as_ref()
        .and_then(AgentCapabilitySnapshot::profile)
        .map(|profile| {
            profile
                .capabilities()
                .iter()
                .map(|capability| capability.as_str().to_owned())
                .collect()
        })
        .unwrap_or_default();
    let capability_reported_at_ms = capability_snapshot
        .as_ref()
        .and_then(AgentCapabilitySnapshot::reported_at)
        .map(system_time_to_millis);
    Ok(agent_to_response(
        agent,
        summary.as_ref(),
        connected_agent_ids,
        assigned_policy_ids,
        capabilities,
        capability_reported_at_ms,
    ))
}

fn agent_facts_summary(body: &str) -> Option<AgentFactsSummary> {
    let value = serde_json::from_str::<serde_json::Value>(body).ok()?;
    Some(AgentFactsSummary {
        hostname: json_string_field(&value, "hostname"),
        os: json_string_field(&value, "os"),
        arch: json_string_field(&value, "arch"),
    })
}

fn json_string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

fn agent_system_time_ms_from_body(value: &serde_json::Value) -> Option<u64> {
    value
        .get("system_time_ms")
        .and_then(serde_json::Value::as_u64)
}

fn agent_to_response(
    agent: &Agent,
    facts: Option<&AgentFactsSummary>,
    connected_agent_ids: &std::collections::BTreeSet<String>,
    assigned_policy_ids: Vec<String>,
    capabilities: Vec<String>,
    capability_reported_at_ms: Option<u64>,
) -> AgentResponse {
    let last_seen_at = agent.last_seen_at();
    let revoked = agent.status() == AgentStatus::Disabled;
    let connected = !revoked && connected_agent_ids.contains(agent.id().as_str());
    AgentResponse {
        id: agent.id().as_str().to_owned(),
        name: agent.name().as_str().to_owned(),
        status: agent_status_for_inventory(agent.status(), connected, last_seen_at).to_owned(),
        connected,
        revoked,
        fingerprint: agent.identity().fingerprint.as_str().to_owned(),
        labels: agent
            .labels()
            .iter()
            .map(|label| AgentLabelResponse {
                key: label.key().to_owned(),
                value: label.value().to_owned(),
            })
            .collect(),
        assigned_policy_ids,
        capabilities,
        capability_reported_at_ms,
        last_seen_at_ms: last_seen_at.map(system_time_to_millis),
        last_seen_age_seconds: last_seen_at.map(system_time_age_seconds),
        hostname: facts.and_then(|summary| summary.hostname.clone()),
        os: facts.and_then(|summary| summary.os.clone()),
        arch: facts.and_then(|summary| summary.arch.clone()),
    }
}

fn agent_status_for_inventory(
    status: AgentStatus,
    connected: bool,
    last_seen_at: Option<SystemTime>,
) -> &'static str {
    if status == AgentStatus::Disabled {
        "offline"
    } else if connected {
        "online"
    } else if recently_seen(last_seen_at) {
        "reconnecting"
    } else {
        agent_status_to_str(status)
    }
}

fn recently_seen(last_seen_at: Option<SystemTime>) -> bool {
    last_seen_at
        .map(|last_seen_at| {
            SystemTime::now()
                .duration_since(last_seen_at)
                .unwrap_or_default()
                <= AGENT_RECENTLY_SEEN_AFTER
        })
        .unwrap_or(false)
}

fn system_time_age_seconds(value: SystemTime) -> u64 {
    SystemTime::now()
        .duration_since(value)
        .unwrap_or_default()
        .as_secs()
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

fn parse_drift_status(value: &str) -> DriftStatus {
    match value {
        "compliant" => DriftStatus::Compliant,
        "drifted" => DriftStatus::Drifted,
        _ => DriftStatus::Unknown,
    }
}

fn drift_status_to_str(status: &DriftStatus) -> &'static str {
    match status {
        DriftStatus::Compliant => "compliant",
        DriftStatus::Drifted => "drifted",
        DriftStatus::Unknown => "unknown",
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

fn drift_acknowledged_by(acknowledgement: &DriftAcknowledgement) -> Option<String> {
    match acknowledgement {
        DriftAcknowledgement::Acknowledged { by, .. } => Some(by.clone()),
        _ => None,
    }
}

fn drift_acknowledged_at(acknowledgement: &DriftAcknowledgement) -> Option<SystemTime> {
    match acknowledgement {
        DriftAcknowledgement::Acknowledged { at, .. } => Some(*at),
        _ => None,
    }
}

fn drift_resolution_job_id(acknowledgement: &DriftAcknowledgement) -> Option<String> {
    match acknowledgement {
        DriftAcknowledgement::Resolved { job_id, .. } => Some(job_id.clone()),
        _ => None,
    }
}

fn drift_resolved_at(acknowledgement: &DriftAcknowledgement) -> Option<SystemTime> {
    match acknowledgement {
        DriftAcknowledgement::Resolved { at, .. } => Some(*at),
        _ => None,
    }
}

fn map_create_command_job_error(
    error: CreateCommandJobError<
        fleet_store::StoreError,
        fleet_store::StoreError,
        fleet_core::IdentityError,
    >,
) -> CreateCommandJobHttpError {
    match error {
        CreateCommandJobError::Domain(error) => {
            CreateCommandJobHttpError::BadRequest(error.to_string())
        }
        CreateCommandJobError::Agent(error) => {
            CreateCommandJobHttpError::BadRequest(error.to_string())
        }
        CreateCommandJobError::NoTargets => CreateCommandJobHttpError::BadRequest(
            "command job requires at least one target".to_owned(),
        ),
        CreateCommandJobError::Repository(fleet_store::StoreError::ConstraintViolation(
            message,
        )) => CreateCommandJobHttpError::Conflict(message),
        CreateCommandJobError::Repository(error) | CreateCommandJobError::Audit(error) => {
            CreateCommandJobHttpError::Internal(ControllerError::Store(error))
        }
        CreateCommandJobError::Sign(error) => {
            CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
        }
    }
}

fn map_create_drift_check_job_error(
    error: CreateDriftCheckJobError<
        fleet_store::StoreError,
        fleet_store::StoreError,
        fleet_core::IdentityError,
    >,
) -> CreateCommandJobHttpError {
    match error {
        CreateDriftCheckJobError::Domain(error) => {
            CreateCommandJobHttpError::BadRequest(error.to_string())
        }
        CreateDriftCheckJobError::Agent(error) => {
            CreateCommandJobHttpError::BadRequest(error.to_string())
        }
        CreateDriftCheckJobError::NoTargets => CreateCommandJobHttpError::BadRequest(
            "drift check job requires at least one target".to_owned(),
        ),
        CreateDriftCheckJobError::Repository(fleet_store::StoreError::ConstraintViolation(
            message,
        )) => CreateCommandJobHttpError::Conflict(message),
        CreateDriftCheckJobError::Repository(error) | CreateDriftCheckJobError::Audit(error) => {
            CreateCommandJobHttpError::Internal(ControllerError::Store(error))
        }
        CreateDriftCheckJobError::Sign(error) => {
            CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
        }
    }
}

fn map_create_runbook_job_error(
    error: CreateRunbookJobError<
        fleet_store::StoreError,
        fleet_store::StoreError,
        fleet_core::IdentityError,
    >,
) -> CreateCommandJobHttpError {
    match error {
        CreateRunbookJobError::Domain(error) => {
            CreateCommandJobHttpError::BadRequest(error.to_string())
        }
        CreateRunbookJobError::Agent(error) => {
            CreateCommandJobHttpError::BadRequest(error.to_string())
        }
        CreateRunbookJobError::InvalidRunbook(error) => {
            CreateCommandJobHttpError::BadRequest(error)
        }
        CreateRunbookJobError::NoTargets => CreateCommandJobHttpError::BadRequest(
            "runbook job requires at least one target".to_owned(),
        ),
        CreateRunbookJobError::Repository(fleet_store::StoreError::ConstraintViolation(
            message,
        )) => CreateCommandJobHttpError::Conflict(message),
        CreateRunbookJobError::Repository(error) | CreateRunbookJobError::Audit(error) => {
            CreateCommandJobHttpError::Internal(ControllerError::Store(error))
        }
        CreateRunbookJobError::Sign(error) => {
            CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
        }
    }
}

fn remediation_http_error_response(error: RemediationHttpError) -> Result<String, ControllerError> {
    match error {
        RemediationHttpError::BadRequest(message) => Ok(response(
            400,
            "application/json",
            &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
        )),
        RemediationHttpError::NotFound => Ok(response(
            404,
            "application/json",
            "{\"error\":\"not_found\"}\n",
        )),
        RemediationHttpError::Conflict(message) => Ok(response(
            409,
            "application/json",
            &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
        )),
        RemediationHttpError::Internal(error) => Err(error),
    }
}

fn map_remediation_approval_request_error(
    error: RemediationApprovalRequestError<fleet_store::StoreError, fleet_store::StoreError>,
) -> RemediationHttpError {
    match error {
        RemediationApprovalRequestError::Domain(message) => {
            RemediationHttpError::BadRequest(message)
        }
        RemediationApprovalRequestError::NotFound(_) => RemediationHttpError::NotFound,
        RemediationApprovalRequestError::Repository(
            fleet_store::StoreError::ConstraintViolation(message),
        ) => RemediationHttpError::Conflict(message),
        RemediationApprovalRequestError::Repository(error)
        | RemediationApprovalRequestError::Audit(error) => {
            RemediationHttpError::Internal(ControllerError::Store(error))
        }
    }
}

fn map_approve_remediation_job_error(
    error: ApproveRemediationRunbookJobError<
        fleet_store::StoreError,
        fleet_store::StoreError,
        fleet_core::IdentityError,
    >,
) -> RemediationHttpError {
    match error {
        ApproveRemediationRunbookJobError::Domain(message)
        | ApproveRemediationRunbookJobError::InvalidRunbook(message) => {
            RemediationHttpError::BadRequest(message)
        }
        ApproveRemediationRunbookJobError::NoTargets => {
            RemediationHttpError::BadRequest("remediation job requires a target".to_owned())
        }
        ApproveRemediationRunbookJobError::NotFound(_) => RemediationHttpError::NotFound,
        ApproveRemediationRunbookJobError::Repository(
            fleet_store::StoreError::ConstraintViolation(message),
        ) => RemediationHttpError::Conflict(message),
        ApproveRemediationRunbookJobError::Repository(error)
        | ApproveRemediationRunbookJobError::Audit(error) => {
            RemediationHttpError::Internal(ControllerError::Store(error))
        }
        ApproveRemediationRunbookJobError::Sign(error) => {
            RemediationHttpError::Internal(ControllerError::Json(error.to_string()))
        }
    }
}

fn map_remediation_result_error(
    error: RemediationResultUseCaseError<fleet_store::StoreError, fleet_store::StoreError>,
) -> RemediationHttpError {
    match error {
        RemediationResultUseCaseError::Domain(message) => RemediationHttpError::BadRequest(message),
        RemediationResultUseCaseError::Mismatch(field) => {
            RemediationHttpError::BadRequest(format!("remediation evidence mismatch: {field}"))
        }
        RemediationResultUseCaseError::NotFound(_) => RemediationHttpError::NotFound,
        RemediationResultUseCaseError::Repository(error)
        | RemediationResultUseCaseError::Audit(error) => {
            RemediationHttpError::Internal(ControllerError::Store(error))
        }
    }
}

fn map_approval_use_case_error(
    error: ApprovalUseCaseError<fleet_store::StoreError, fleet_store::StoreError>,
) -> ControllerError {
    match error {
        ApprovalUseCaseError::Domain(error) => ControllerError::Json(error.to_string()),
        ApprovalUseCaseError::NotFound => ControllerError::Store(fleet_store::StoreError::NotFound),
        ApprovalUseCaseError::Repository(error) | ApprovalUseCaseError::Audit(error) => {
            ControllerError::Store(error)
        }
    }
}

fn map_update_agent_labels_error(
    error: UpdateAgentLabelsError<fleet_store::StoreError, fleet_store::StoreError>,
) -> ControllerError {
    match error {
        UpdateAgentLabelsError::Agent(error) => ControllerError::Store(error.into()),
        UpdateAgentLabelsError::Repository(error) | UpdateAgentLabelsError::Audit(error) => {
            ControllerError::Store(error)
        }
    }
}

fn map_revoke_agent_key_error(
    error: RevokeAgentKeyError<fleet_store::StoreError, fleet_store::StoreError>,
) -> ControllerError {
    match error {
        RevokeAgentKeyError::Agent(error) => ControllerError::Store(error.into()),
        RevokeAgentKeyError::Repository(error) | RevokeAgentKeyError::Audit(error) => {
            ControllerError::Store(error)
        }
    }
}

fn map_policy_use_case_error(
    error: fleet_application::PolicyUseCaseError<fleet_store::StoreError, fleet_store::StoreError>,
) -> ControllerError {
    match error {
        fleet_application::PolicyUseCaseError::Domain(message) => ControllerError::Json(message),
        fleet_application::PolicyUseCaseError::NotFound(_) => {
            ControllerError::Store(fleet_store::StoreError::NotFound)
        }
        fleet_application::PolicyUseCaseError::Repository(error)
        | fleet_application::PolicyUseCaseError::Audit(error) => ControllerError::Store(error),
    }
}

fn map_run_due_scheduled_drift_error(
    error: RunDueScheduledDriftError<
        fleet_store::StoreError,
        fleet_store::StoreError,
        fleet_core::IdentityError,
    >,
) -> ControllerError {
    match error {
        RunDueScheduledDriftError::Domain(message) => ControllerError::Json(message),
        RunDueScheduledDriftError::Repository(error) | RunDueScheduledDriftError::Audit(error) => {
            ControllerError::Store(error)
        }
        RunDueScheduledDriftError::Sign(error) => ControllerError::Json(error.to_string()),
    }
}

fn map_run_retention_cleanup_error(
    error: RunRetentionCleanupError<fleet_store::StoreError, fleet_store::StoreError>,
) -> ControllerError {
    match error {
        RunRetentionCleanupError::Domain(message) => ControllerError::Json(message),
        RunRetentionCleanupError::Repository(error) | RunRetentionCleanupError::Audit(error) => {
            ControllerError::Store(error)
        }
    }
}

struct ControllerAdminTokenRepository<'a> {
    store: ControllerStoreRef<'a>,
}

impl AdminTokenRepository for ControllerAdminTokenRepository<'_> {
    type Error = fleet_store::StoreError;

    fn admin_token_exists(&self) -> Result<bool, Self::Error> {
        self.store.admin_token_exists()
    }

    fn insert_admin_token_hash(&mut self, token_hash: &str) -> Result<(), Self::Error> {
        self.store.insert_admin_token_hash(token_hash)
    }

    fn verify_admin_token_hash(&self, token_hash: &str) -> Result<bool, Self::Error> {
        self.store.verify_admin_token_hash(token_hash)
    }

    fn find_admin_token_record(
        &self,
        token_hash: &str,
    ) -> Result<Option<fleet_application::AdminTokenRecord>, Self::Error> {
        self.store.find_admin_token_record(token_hash)
    }
}

struct ControllerAgentInventoryRepository<'a> {
    store: ControllerStoreRef<'a>,
}

impl AgentInventoryRepository for ControllerAgentInventoryRepository<'_> {
    type Error = fleet_store::StoreError;

    fn list_agents(&self) -> Result<Vec<Agent>, Self::Error> {
        self.store.list_agents()
    }

    fn find_agent_by_id(&self, id: &AgentId) -> Result<Option<Agent>, Self::Error> {
        self.store.find_agent_by_id(id.as_str())
    }

    fn revoke_agent_key(&mut self, id: &AgentId) -> Result<bool, Self::Error> {
        self.store.revoke_agent_key(id.as_str())
    }

    fn update_agent_labels(
        &mut self,
        id: &AgentId,
        labels: &[AgentLabel],
    ) -> Result<bool, Self::Error> {
        self.store.update_agent_labels(id.as_str(), labels)
    }
}

struct ControllerFactsRepository<'a> {
    store: ControllerStoreRef<'a>,
}

impl FactsRepository for ControllerFactsRepository<'_> {
    type Error = fleet_store::StoreError;

    fn insert_facts_snapshot(
        &mut self,
        agent_id: &str,
        body: &str,
        collected_at: SystemTime,
    ) -> Result<(), Self::Error> {
        self.store
            .insert_facts_snapshot(agent_id, body, collected_at)
    }

    fn latest_facts_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<fleet_application::FactsSnapshotRecord>, Self::Error> {
        Ok(self.store.latest_facts_snapshot(agent_id)?.map(|record| {
            fleet_application::FactsSnapshotRecord {
                agent_id: record.agent_id,
                body: record.body,
                collected_at: record.collected_at,
            }
        }))
    }

    fn list_facts_snapshots(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<fleet_application::FactsSnapshotPageRecord>, Self::Error> {
        Ok(self
            .store
            .list_facts_snapshots(agent_id, limit, before)?
            .into_iter()
            .map(|record| fleet_application::FactsSnapshotPageRecord {
                agent_id: record.agent_id,
                body: record.body,
                collected_at: record.collected_at,
                cursor: record.cursor,
            })
            .collect())
    }
}

struct ControllerMetricsRepository<'a> {
    store: ControllerStoreRef<'a>,
}

impl MetricsRepository for ControllerMetricsRepository<'_> {
    type Error = fleet_store::StoreError;

    fn insert_metrics_snapshot(
        &mut self,
        agent_id: &str,
        body: &str,
        collected_at: SystemTime,
    ) -> Result<(), Self::Error> {
        self.store
            .insert_metrics_snapshot(agent_id, body, collected_at)
    }

    fn latest_metrics_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<fleet_application::MetricsSnapshotRecord>, Self::Error> {
        Ok(self.store.latest_metrics_snapshot(agent_id)?.map(|record| {
            fleet_application::MetricsSnapshotRecord {
                agent_id: record.agent_id,
                body: record.body,
                collected_at: record.collected_at,
            }
        }))
    }

    fn list_metrics_snapshots(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<fleet_application::MetricsSnapshotPageRecord>, Self::Error> {
        Ok(self
            .store
            .list_metrics_snapshots(agent_id, limit, before)?
            .into_iter()
            .map(|record| fleet_application::MetricsSnapshotPageRecord {
                agent_id: record.agent_id,
                body: record.body,
                collected_at: record.collected_at,
                cursor: record.cursor,
            })
            .collect())
    }
}

struct ControllerAgentLogRepository<'a> {
    store: ControllerStoreRef<'a>,
}

impl AgentLogRepository for ControllerAgentLogRepository<'_> {
    type Error = fleet_store::StoreError;

    fn list_agent_log_chunks(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<fleet_application::AgentLogChunkPageRecord>, Self::Error> {
        Ok(self
            .store
            .list_agent_log_chunks_page(agent_id, limit, before)?
            .into_iter()
            .map(|record| fleet_application::AgentLogChunkPageRecord {
                agent_id: record.agent_id,
                line: record.line,
                collected_at: record.collected_at,
                cursor: record.cursor,
            })
            .collect())
    }
}

struct ControllerDriftRepository<'a> {
    store: ControllerStoreRef<'a>,
}

impl DriftRepository for ControllerDriftRepository<'_> {
    type Error = fleet_store::StoreError;

    fn insert_drift_report(
        &mut self,
        agent_id: &str,
        report: &DriftReport,
        checked_at: SystemTime,
    ) -> Result<(), Self::Error> {
        self.store.insert_drift_report(agent_id, report, checked_at)
    }

    fn latest_drift_report(
        &self,
        agent_id: &str,
    ) -> Result<Option<fleet_application::DriftReportRecord>, Self::Error> {
        Ok(self.store.latest_drift_report(agent_id)?.map(|record| {
            fleet_application::DriftReportRecord {
                agent_id: record.agent_id,
                report: record.report,
                checked_at: record.checked_at,
            }
        }))
    }

    fn list_drift_reports(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<fleet_application::DriftReportPageRecord>, Self::Error> {
        Ok(self
            .store
            .list_drift_reports(agent_id, limit, before)?
            .into_iter()
            .map(|record| fleet_application::DriftReportPageRecord {
                agent_id: record.agent_id,
                report: record.report,
                checked_at: record.checked_at,
                cursor: record.cursor,
            })
            .collect())
    }
}

struct ControllerPolicyRepository<'a> {
    store: ControllerStoreRef<'a>,
}

impl AppPolicyRepository for ControllerPolicyRepository<'_> {
    type Error = fleet_store::StoreError;

    fn save_policy_source(
        &mut self,
        policy_id: &str,
        name: &str,
        version: u32,
        source: &str,
    ) -> Result<(), Self::Error> {
        self.store
            .save_policy_source(policy_id, name, version, source)
    }

    fn list_policies(&self) -> Result<Vec<fleet_application::PolicyRecord>, Self::Error> {
        Ok(self
            .store
            .list_policies()?
            .into_iter()
            .map(|record| fleet_application::PolicyRecord {
                id: record.id,
                name: record.name,
                version: record.version,
                source: record.source,
                created_at: record.created_at,
                updated_at: record.updated_at,
            })
            .collect())
    }

    fn find_policy(
        &self,
        policy_id: &str,
    ) -> Result<Option<fleet_application::PolicyRecord>, Self::Error> {
        Ok(self
            .store
            .find_policy(policy_id)?
            .map(|record| fleet_application::PolicyRecord {
                id: record.id,
                name: record.name,
                version: record.version,
                source: record.source,
                created_at: record.created_at,
                updated_at: record.updated_at,
            }))
    }

    fn assign_policy_to_agent(
        &mut self,
        policy_id: &str,
        agent_id: &str,
        assigned_at: SystemTime,
    ) -> Result<(), Self::Error> {
        self.store
            .assign_policy_to_agent(policy_id, agent_id, assigned_at)
    }

    fn policies_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<fleet_application::PolicyAssignmentRecord>, Self::Error> {
        Ok(self
            .store
            .policies_for_agent(agent_id)?
            .into_iter()
            .map(|record| fleet_application::PolicyAssignmentRecord {
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
        self.store
            .upsert_policy_schedule(policy_id, agent_id, interval, next_due_at)
    }

    fn due_scheduled_drift_checks(
        &self,
        now: SystemTime,
        limit: usize,
    ) -> Result<Vec<fleet_application::ScheduledDriftRecord>, Self::Error> {
        Ok(self
            .store
            .due_scheduled_drift_checks(now, limit)?
            .into_iter()
            .map(|record| fleet_application::ScheduledDriftRecord {
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
        self.store
            .record_scheduled_drift_check(policy_id, agent_id, checked_at)
    }

    fn acknowledge_latest_drift_report(
        &mut self,
        agent_id: &str,
        policy_name: &str,
        actor: &str,
        acknowledged_at: SystemTime,
    ) -> Result<bool, Self::Error> {
        self.store
            .acknowledge_latest_drift_report(agent_id, policy_name, actor, acknowledged_at)
    }

    fn mark_latest_drift_resolved(
        &mut self,
        agent_id: &str,
        policy_name: &str,
        job_id: &str,
        resolved_at: SystemTime,
    ) -> Result<bool, Self::Error> {
        self.store
            .mark_latest_drift_resolved(agent_id, policy_name, job_id, resolved_at)
    }
}

struct ControllerAuditRepository<'a> {
    store: ControllerStoreRef<'a>,
}

impl fleet_application::AuditWriter for ControllerAuditRepository<'_> {
    type Error = fleet_store::StoreError;

    fn write(&mut self, event: AuditEvent) -> Result<(), Self::Error> {
        self.store.write_audit_event(event)
    }
}

impl fleet_application::AuditRepository for ControllerAuditRepository<'_> {
    fn list(&self, limit: usize) -> Result<Vec<AuditEvent>, Self::Error> {
        self.store.list_audit_events(limit)
    }

    fn list_by_category(
        &self,
        category: AuditCategory,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, Self::Error> {
        self.store.list_audit_events_by_category(category, limit)
    }

    fn export_page(
        &self,
        category: Option<AuditCategory>,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<fleet_application::AuditEventPageRecord>, Self::Error> {
        self.store.export_audit_events(category, limit, before)
    }
}

struct ControllerEnrollmentTokenRepository<'a> {
    store: ControllerStoreRef<'a>,
}

impl EnrollmentTokenRepository for ControllerEnrollmentTokenRepository<'_> {
    type Error = fleet_store::StoreError;

    fn insert_enrollment_token_hash(
        &mut self,
        id: &str,
        token_hash: &str,
        default_labels: &str,
        expires_at: SystemTime,
        max_uses: u32,
    ) -> Result<(), Self::Error> {
        self.store.insert_enrollment_token_hash(
            id,
            token_hash,
            default_labels,
            expires_at,
            max_uses,
        )
    }

    fn list_enrollment_tokens(
        &self,
    ) -> Result<Vec<fleet_application::EnrollmentTokenRecord>, Self::Error> {
        Ok(self
            .store
            .list_enrollment_tokens()?
            .into_iter()
            .map(|record| fleet_application::EnrollmentTokenRecord {
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
        self.store.revoke_enrollment_token(id)
    }

    fn consume_enrollment_token_hash(
        &mut self,
        token_hash: &str,
        now: SystemTime,
    ) -> Result<fleet_application::EnrollmentTokenRecord, Self::Error> {
        let record = self.store.consume_enrollment_token_hash(token_hash, now)?;
        Ok(fleet_application::EnrollmentTokenRecord {
            id: record.id,
            default_labels: record.default_labels,
            expires_at: record.expires_at,
            max_uses: record.max_uses,
            used_count: record.used_count,
            revoked: record.revoked,
        })
    }
}

struct ControllerJobRepository<'a> {
    store: ControllerStoreRef<'a>,
}

struct ControllerJobOutputRepository<'a> {
    store: ControllerStoreRef<'a>,
}

impl JobOutputRepository for ControllerJobOutputRepository<'_> {
    type Error = fleet_store::StoreError;

    fn append_output_chunk(&mut self, chunk: JobOutputChunk) -> Result<(), Self::Error> {
        self.store.append_job_output_chunk_record(&chunk)
    }

    fn list_output_chunks(
        &self,
        job_id: &str,
        agent_id: &str,
    ) -> Result<Vec<JobOutputChunk>, Self::Error> {
        self.store.list_job_output_chunks(job_id, agent_id)
    }

    fn list_output_chunks_for_job(&self, job_id: &str) -> Result<Vec<JobOutputChunk>, Self::Error> {
        self.store.list_job_output_chunks_for_job(job_id)
    }
}

struct ControllerJobQueryRepository<'a> {
    store: ControllerStoreRef<'a>,
}

impl JobQueryRepository for ControllerJobQueryRepository<'_> {
    type Error = fleet_store::StoreError;

    fn list_job_summaries(
        &self,
        limit: usize,
    ) -> Result<Vec<fleet_application::JobSummaryRecord>, Self::Error> {
        Ok(self
            .store
            .list_job_summaries(limit)?
            .into_iter()
            .map(|record| fleet_application::JobSummaryRecord {
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
                    .map(|target| fleet_application::JobTargetSummaryRecord {
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

    fn find_job_summary(
        &self,
        job_id: &str,
    ) -> Result<Option<fleet_application::JobSummaryRecord>, Self::Error> {
        Ok(self
            .store
            .find_job_summary(job_id)?
            .map(|record| fleet_application::JobSummaryRecord {
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
                    .map(|target| fleet_application::JobTargetSummaryRecord {
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
            }))
    }
}

impl JobRepository for ControllerJobRepository<'_> {
    type Error = fleet_store::StoreError;

    fn save(&mut self, job: Job) -> Result<(), Self::Error> {
        self.store.save_job_record(&job)
    }
}

impl fleet_application::AgentRepository for ControllerJobRepository<'_> {
    type Error = fleet_store::StoreError;

    fn save(&mut self, agent: Agent) -> Result<(), Self::Error> {
        self.store.save_agent(agent)
    }

    fn find_by_id(&self, id: &AgentId) -> Result<Option<Agent>, Self::Error> {
        self.store.find_agent_by_id(id.as_str())
    }

    fn list(&self) -> Result<Vec<Agent>, Self::Error> {
        self.store.list_agents()
    }
}

impl AppPolicyRepository for ControllerJobRepository<'_> {
    type Error = fleet_store::StoreError;

    fn save_policy_source(
        &mut self,
        policy_id: &str,
        name: &str,
        version: u32,
        source: &str,
    ) -> Result<(), Self::Error> {
        self.store
            .save_policy_source(policy_id, name, version, source)
    }

    fn list_policies(&self) -> Result<Vec<fleet_application::PolicyRecord>, Self::Error> {
        Ok(self
            .store
            .list_policies()?
            .into_iter()
            .map(|record| fleet_application::PolicyRecord {
                id: record.id,
                name: record.name,
                version: record.version,
                source: record.source,
                created_at: record.created_at,
                updated_at: record.updated_at,
            })
            .collect())
    }

    fn find_policy(
        &self,
        policy_id: &str,
    ) -> Result<Option<fleet_application::PolicyRecord>, Self::Error> {
        Ok(self
            .store
            .find_policy(policy_id)?
            .map(|record| fleet_application::PolicyRecord {
                id: record.id,
                name: record.name,
                version: record.version,
                source: record.source,
                created_at: record.created_at,
                updated_at: record.updated_at,
            }))
    }

    fn assign_policy_to_agent(
        &mut self,
        policy_id: &str,
        agent_id: &str,
        assigned_at: SystemTime,
    ) -> Result<(), Self::Error> {
        self.store
            .assign_policy_to_agent(policy_id, agent_id, assigned_at)
    }

    fn policies_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<fleet_application::PolicyAssignmentRecord>, Self::Error> {
        Ok(self
            .store
            .policies_for_agent(agent_id)?
            .into_iter()
            .map(|record| fleet_application::PolicyAssignmentRecord {
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
        self.store
            .upsert_policy_schedule(policy_id, agent_id, interval, next_due_at)
    }

    fn due_scheduled_drift_checks(
        &self,
        now: SystemTime,
        limit: usize,
    ) -> Result<Vec<fleet_application::ScheduledDriftRecord>, Self::Error> {
        Ok(self
            .store
            .due_scheduled_drift_checks(now, limit)?
            .into_iter()
            .map(|record| fleet_application::ScheduledDriftRecord {
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
        self.store
            .record_scheduled_drift_check(policy_id, agent_id, checked_at)
    }

    fn acknowledge_latest_drift_report(
        &mut self,
        agent_id: &str,
        policy_name: &str,
        actor: &str,
        acknowledged_at: SystemTime,
    ) -> Result<bool, Self::Error> {
        self.store
            .acknowledge_latest_drift_report(agent_id, policy_name, actor, acknowledged_at)
    }

    fn mark_latest_drift_resolved(
        &mut self,
        agent_id: &str,
        policy_name: &str,
        job_id: &str,
        resolved_at: SystemTime,
    ) -> Result<bool, Self::Error> {
        self.store
            .mark_latest_drift_resolved(agent_id, policy_name, job_id, resolved_at)
    }
}

impl TaskAssignmentRepository for ControllerJobRepository<'_> {
    type Error = fleet_store::StoreError;

    fn save_assignment(&mut self, envelope: TaskEnvelope) -> Result<(), Self::Error> {
        self.store.save_task_assignment_record(&envelope)
    }
}

impl RemediationRequestRepository for ControllerJobRepository<'_> {
    type Error = fleet_store::StoreError;

    fn save_remediation_request(
        &mut self,
        request: RemediationRequestRecord,
    ) -> Result<(), Self::Error> {
        self.store.save_remediation_request_record(&request)
    }

    fn find_remediation_request(
        &self,
        request_id: &str,
    ) -> Result<Option<RemediationRequestRecord>, Self::Error> {
        self.store.find_remediation_request_record(request_id)
    }

    fn list_remediation_requests(
        &self,
        agent_id: Option<&str>,
        policy_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RemediationRequestRecord>, Self::Error> {
        self.store
            .list_remediation_request_records(agent_id, policy_id, limit)
    }

    fn update_remediation_request_status(
        &mut self,
        request_id: &str,
        status: &str,
        job_id: Option<&str>,
        updated_at: SystemTime,
    ) -> Result<(), Self::Error> {
        self.store
            .update_remediation_request_status_record(request_id, status, job_id, updated_at)
    }
}

impl AppApprovalRepository for ControllerJobRepository<'_> {
    type Error = fleet_store::StoreError;

    fn insert_approval_request(
        &mut self,
        request: AppApprovalRequestRecord,
    ) -> Result<(), Self::Error> {
        self.store.insert_approval_request(request)
    }

    fn find_approval_request(
        &self,
        approval_id: &str,
    ) -> Result<Option<AppApprovalRequestRecord>, Self::Error> {
        self.store.find_approval_request(approval_id)
    }

    fn find_pending_approval_for_job(
        &self,
        job_id: &str,
    ) -> Result<Option<AppApprovalRequestRecord>, Self::Error> {
        self.store.find_pending_approval_for_job(job_id)
    }

    fn list_approval_requests(
        &self,
        status: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AppApprovalRequestRecord>, Self::Error> {
        self.store.list_approval_requests(status, limit)
    }

    fn update_approval_request(
        &mut self,
        request: AppApprovalRequestRecord,
    ) -> Result<bool, Self::Error> {
        self.store.update_approval_request(request)
    }

    fn update_job_status_for_approval(
        &mut self,
        job_id: &str,
        status: JobStatus,
    ) -> Result<bool, Self::Error> {
        self.store.update_job_status(job_id, status)
    }
}

impl CommandJobRepository for ControllerJobRepository<'_> {
    fn save_command_job(
        &mut self,
        job: Job,
        task: &fleet_domain::CommandTask,
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.store.save_command_job_record(&job, task)
    }

    fn save_command_job_with_assignments(
        &mut self,
        job: Job,
        task: &fleet_domain::CommandTask,
        assignments: &[TaskEnvelope],
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.store
            .save_command_job_with_assignments_record(&job, task, assignments)
    }
}

impl fleet_application::DriftCheckJobRepository for ControllerJobRepository<'_> {
    fn save_drift_check_job(
        &mut self,
        job: Job,
        task: &fleet_domain::DriftCheckTask,
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.store.save_drift_check_job_record(&job, task)
    }

    fn save_drift_check_job_with_assignments(
        &mut self,
        job: Job,
        task: &fleet_domain::DriftCheckTask,
        assignments: &[TaskEnvelope],
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.store
            .save_drift_check_job_with_assignments_record(&job, task, assignments)
    }
}

impl RunbookJobRepository for ControllerJobRepository<'_> {
    fn save_runbook_job(
        &mut self,
        job: Job,
        task: &fleet_domain::RunbookExecutionTask,
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.store.save_runbook_job_record(&job, task)
    }

    fn save_runbook_job_with_assignments(
        &mut self,
        job: Job,
        task: &fleet_domain::RunbookExecutionTask,
        assignments: &[TaskEnvelope],
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.store
            .save_runbook_job_with_assignments_record(&job, task, assignments)
    }
}

impl DispatchAssignmentRepository for ControllerJobRepository<'_> {
    type Error = fleet_store::StoreError;

    fn list_pending_assignments(
        &self,
        agent_id: Option<&AgentId>,
        job_id: Option<&JobId>,
        limit: usize,
    ) -> Result<Vec<PendingTaskAssignment>, Self::Error> {
        self.store
            .list_pending_dispatch_assignments(agent_id, job_id, limit)
    }

    fn find_dispatch_agent(&self, agent_id: &AgentId) -> Result<Option<Agent>, Self::Error> {
        self.store.find_agent_by_id(agent_id.as_str())
    }

    fn dispatch_gate(&self, job_id: &JobId) -> Result<JobDispatchGate, Self::Error> {
        let gate = self.store.job_dispatch_gate(job_id.as_str())?.unwrap_or(
            fleet_store::JobDispatchGateRecord {
                concurrency: 1,
                max_failures: None,
                active_count: 0,
                failure_count: 0,
            },
        );
        Ok(JobDispatchGate {
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
        self.store
            .latest_agent_capability_snapshot(agent_id.as_str())
    }

    fn mark_assignment_rejected(
        &mut self,
        task_id: &fleet_domain::TaskId,
        now: SystemTime,
        reason: &str,
    ) -> Result<(), Self::Error> {
        let job_id = self.store.find_task_assignment_job_id(task_id.as_str())?;
        let changed = self.store.update_active_task_assignment_status(
            task_id.as_str(),
            AssignmentStatus::Rejected,
            now,
            Some(reason),
        )?;
        if changed && let Some(job_id) = job_id {
            self.store.recompute_job_status_from_assignments(&job_id)?;
        }
        Ok(())
    }

    fn mark_assignment_dispatched(
        &mut self,
        task_id: &fleet_domain::TaskId,
        now: SystemTime,
    ) -> Result<(), Self::Error> {
        self.store.update_task_assignment_status(
            task_id.as_str(),
            AssignmentStatus::Dispatched,
            now,
            None,
        )?;
        Ok(())
    }

    fn claim_assignment_for_dispatch(
        &mut self,
        task_id: &fleet_domain::TaskId,
        now: SystemTime,
    ) -> Result<bool, Self::Error> {
        self.store
            .claim_task_assignment_for_dispatch(task_id.as_str(), now)
    }

    fn release_assignment_dispatch_claim(
        &mut self,
        task_id: &fleet_domain::TaskId,
        now: SystemTime,
        reason: &str,
    ) -> Result<(), Self::Error> {
        self.store
            .release_task_assignment_dispatch_claim(task_id.as_str(), now, reason)
    }

    fn mark_job_running(&mut self, job_id: &JobId, _now: SystemTime) -> Result<(), Self::Error> {
        self.store
            .update_job_status(job_id.as_str(), JobStatus::Running)?;
        Ok(())
    }

    fn mark_job_expired(&mut self, job_id: &JobId, _now: SystemTime) -> Result<(), Self::Error> {
        self.store
            .update_job_status(job_id.as_str(), JobStatus::Expired)?;
        Ok(())
    }
}

struct ControllerAuditWriter<'a> {
    store: ControllerStoreRef<'a>,
}

impl fleet_application::AuditWriter for ControllerAuditWriter<'_> {
    type Error = fleet_store::StoreError;

    fn write(&mut self, event: AuditEvent) -> Result<(), Self::Error> {
        self.store.write_audit_event(event)
    }
}

struct ControllerAgentCertificateLifecycleRepository<'a> {
    store: ControllerStoreRef<'a>,
}

impl AgentCertificateLifecycleRepository for ControllerAgentCertificateLifecycleRepository<'_> {
    type Error = fleet_store::StoreError;

    fn save_agent_certificate_lifecycle(
        &mut self,
        record: fleet_application::AgentCertificateLifecycleRecord,
    ) -> Result<(), Self::Error> {
        self.store.save_agent_certificate_lifecycle(record)
    }

    fn load_agent_certificate_lifecycle(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<fleet_application::AgentCertificateLifecycleRecord>, Self::Error> {
        self.store.load_agent_certificate_lifecycle(agent_id)
    }
}

struct ControllerRetentionRepository<'a> {
    store: ControllerStoreRef<'a>,
}

impl fleet_application::RetentionRepository for ControllerRetentionRepository<'_> {
    type Error = fleet_store::StoreError;

    fn cleanup_retention(
        &mut self,
        cutoffs: fleet_application::RetentionCutoffs,
        dry_run: bool,
    ) -> Result<fleet_application::RetentionCleanupSummary, Self::Error> {
        self.store.cleanup_retention(cutoffs, dry_run)
    }
}

struct ControllerTaskSigner<'a> {
    private_key: &'a str,
    signing_fingerprint: &'a str,
}

impl TaskEnvelopeSigner for ControllerTaskSigner<'_> {
    type Error = fleet_core::IdentityError;

    fn sign(&mut self, payload: &str) -> Result<String, Self::Error> {
        let _selected_signing_fingerprint = self.signing_fingerprint;
        fleet_core::sign_challenge(self.private_key, payload)
    }
}

fn enroll_agent<'a>(
    body: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
    identity: &ControllerIdentity,
) -> Result<String, ControllerError> {
    let store = store.into();
    let request: EnrollAgentRequest =
        serde_json::from_str(body).map_err(|error| ControllerError::Json(error.to_string()))?;
    let expected_fingerprint = fleet_core::fingerprint_public_key(&request.public_key)
        .map_err(|error| ControllerError::Json(error.to_string()))?;
    if expected_fingerprint != request.fingerprint {
        return Err(ControllerError::Store(fleet_store::StoreError::Domain(
            "agent fingerprint does not match public key".to_owned(),
        )));
    }
    let enrollment_token =
        store.consume_enrollment_token_hash(&hash_token(&request.token), SystemTime::now())?;
    let agent_id = request.agent_id.clone();

    let mut labels = parse_label_string(&enrollment_token.default_labels)?;
    for label in request.labels {
        let label = AgentLabel::new(label.key, label.value)
            .map_err(|error| ControllerError::Store(error.into()))?;
        labels.retain(|existing| existing.key() != label.key());
        labels.push(label);
    }
    let mut agent = Agent::new(
        AgentId::new(request.agent_id).map_err(|error| ControllerError::Store(error.into()))?,
        AgentName::new(request.name).map_err(|error| ControllerError::Store(error.into()))?,
        AgentIdentity {
            public_key: AgentPublicKey::new(request.public_key)
                .map_err(|error| ControllerError::Store(error.into()))?,
            fingerprint: AgentFingerprint::new(request.fingerprint)
                .map_err(|error| ControllerError::Store(error.into()))?,
        },
    );
    agent.set_labels(labels);
    agent.pin_controller(
        ControllerPublicKey::new(identity.public_key.clone())
            .map_err(|error| ControllerError::Store(error.into()))?,
    );

    store.save_agent(agent)?;
    store.write_audit_event(AuditEvent {
        category: AuditCategory::Enrollment,
        action: "enrollment_token_used".to_owned(),
        actor: AuditActor::new(agent_id.clone()),
        target: AuditTarget::new(enrollment_token.id),
        value: AuditValue::SecretRef("enrollment-token".to_owned()),
        occurred_at: SystemTime::now(),
    })?;

    serde_json::to_string(&EnrollAgentResponse {
        agent_id,
        controller_public_key: identity.public_key.clone(),
        controller_fingerprint: identity.fingerprint.clone(),
    })
    .map_err(|error| ControllerError::Json(error.to_string()))
}

fn parse_label_string(labels: &str) -> Result<Vec<AgentLabel>, ControllerError> {
    labels
        .split(',')
        .filter(|part| !part.trim().is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').ok_or_else(|| {
                ControllerError::Store(fleet_store::StoreError::Domain(format!(
                    "invalid default label, expected key=value: {part}"
                )))
            })?;
            AgentLabel::new(key, value).map_err(|error| ControllerError::Store(error.into()))
        })
        .collect()
}

fn request_body(request: &str) -> &str {
    request.split("\r\n\r\n").nth(1).unwrap_or_default()
}

fn authenticate_admin_request<'a>(
    request: &str,
    store: impl Into<ControllerStoreRef<'a>> + Copy,
) -> Result<Option<AdminRequestContext>, ControllerError> {
    let Some(token) = bearer_token(request) else {
        return Ok(None);
    };
    let repo = ControllerAdminTokenRepository {
        store: store.into(),
    };
    let Some(record) = AuthenticateAdminToken::execute(&repo, &hash_token(token))
        .map_err(ControllerError::from)?
    else {
        return Ok(None);
    };
    let role = AdminRole::parse(&record.role).ok_or_else(|| {
        ControllerError::Store(fleet_store::StoreError::Domain(format!(
            "invalid admin role stored for actor {}",
            record.actor_id
        )))
    })?;
    Ok(Some(AdminRequestContext {
        actor_id: record.actor_id,
        role,
    }))
}

fn required_permission_for_route(method: &str, route_path: &str) -> Option<AdminPermission> {
    match (method, route_path) {
        ("GET", "/api/agents") => Some(AdminPermission::AgentRead),
        ("GET", path) if path.starts_with("/api/agents/") => Some(AdminPermission::AgentRead),
        ("PATCH", path) if path.starts_with("/api/agents/") && path.ends_with("/labels") => {
            Some(AdminPermission::AgentWrite)
        }
        ("POST", path)
            if path.starts_with("/api/agents/")
                && path.ends_with("/certificate-lifecycle/request-issuance") =>
        {
            Some(AdminPermission::AgentWrite)
        }
        ("POST", path) if path.starts_with("/api/agents/") && path.ends_with("/revoke-key") => {
            Some(AdminPermission::AgentRevoke)
        }
        ("POST", "/api/selectors/preview") => Some(AdminPermission::AgentRead),
        ("GET", "/api/jobs") => Some(AdminPermission::JobRead),
        ("GET", path) if path.starts_with("/api/jobs/") => Some(AdminPermission::JobRead),
        ("POST", "/api/jobs/command")
        | ("POST", "/api/jobs/drift-check")
        | ("POST", "/api/jobs/runbook") => Some(AdminPermission::JobCreate),
        ("POST", path) if path.starts_with("/api/jobs/") && path.ends_with("/cancel") => {
            Some(AdminPermission::JobCancel)
        }
        ("GET", "/api/approvals") => Some(AdminPermission::ApprovalRead),
        ("POST", "/api/approvals/expire") => Some(AdminPermission::JobApprove),
        ("POST", path) if path.starts_with("/api/approvals/") && path.ends_with("/approve") => {
            Some(AdminPermission::JobApprove)
        }
        ("POST", path) if path.starts_with("/api/approvals/") && path.ends_with("/reject") => {
            Some(AdminPermission::JobApprove)
        }
        ("GET", "/api/remediations") => Some(AdminPermission::PolicyRead),
        ("GET", path) if path.starts_with("/api/remediations/") => {
            Some(AdminPermission::PolicyRead)
        }
        ("POST", path) if path.starts_with("/api/remediations/") => {
            Some(AdminPermission::JobApprove)
        }
        ("GET", "/api/enrollment-tokens") => Some(AdminPermission::EnrollmentTokenRead),
        ("POST", "/api/enrollment-tokens") => Some(AdminPermission::EnrollmentTokenCreate),
        ("DELETE", path) if path.starts_with("/api/enrollment-tokens/") => {
            Some(AdminPermission::EnrollmentTokenRevoke)
        }
        ("GET", "/api/controller/signing-rotation/status")
        | ("GET", "/api/controller/signing-rotation/restart-plan") => {
            Some(AdminPermission::AuditRead)
        }
        ("POST", path) if path.starts_with("/api/controller/signing-rotation/") => {
            Some(AdminPermission::SigningRotationWrite)
        }
        ("GET", "/api/audit") | ("GET", "/api/audit/export") => Some(AdminPermission::AuditRead),
        ("GET", "/api/policies") => Some(AdminPermission::PolicyRead),
        ("GET", path) if path.starts_with("/api/agents/") && path.ends_with("/policies") => {
            Some(AdminPermission::PolicyRead)
        }
        ("GET", "/api/drift/scheduled") => Some(AdminPermission::PolicyRead),
        ("POST", path) if path.starts_with("/api/policies") => Some(AdminPermission::PolicyWrite),
        _ => None,
    }
}

fn forbidden_response(permission: AdminPermission) -> String {
    response(
        403,
        "application/json",
        &format!(
            "{{\"error\":\"forbidden\",\"required_permission\":\"{}\"}}\n",
            permission.as_str()
        ),
    )
}

fn bearer_token(request: &str) -> Option<&str> {
    request.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("authorization") {
            value.trim().strip_prefix("Bearer ")
        } else {
            None
        }
    })
}

fn response(status: u16, content_type: &str, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        _ => "Internal Server Error",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn validate_transport(config: &ControllerServerConfig) -> Result<(), ControllerError> {
    match (&config.tls_cert_path, &config.tls_key_path) {
        (Some(cert_path), Some(key_path)) => {
            validate_tls_material(cert_path, key_path)?;
            if let Some(external_url) = &config.external_url
                && !external_url.starts_with("https://")
            {
                return Err(ControllerError::Tls(
                    "TLS controller external URL must start with https://".to_owned(),
                ));
            }
        }
        (None, None) => {}
        _ => {
            return Err(ControllerError::Tls(
                "--tls-cert and --tls-key must be provided together".to_owned(),
            ));
        }
    }
    if let Some(external_url) = &config.external_url {
        validate_external_url(external_url)?;
    }
    let agent_client_certificate = controller_agent_client_certificate_trust(config)?;
    ensure_agent_client_certificate_mtls_supported(&agent_client_certificate)?;
    Ok(())
}

fn validate_tls_material(cert_path: &Path, key_path: &Path) -> Result<(), ControllerError> {
    validate_tls_private_key_permissions(key_path)?;
    let certs = load_tls_certificates(cert_path)?;
    if certs.is_empty() {
        return Err(ControllerError::Tls(format!(
            "TLS certificate file has no certificates: {}",
            cert_path.display()
        )));
    }
    let _key = load_tls_private_key(key_path)?;
    Ok(())
}

fn build_tls_acceptor(cert_path: &Path, key_path: &Path) -> Result<TlsAcceptor, ControllerError> {
    ensure_rustls_crypto_provider();
    validate_tls_material(cert_path, key_path)?;
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            load_tls_certificates(cert_path)?,
            load_tls_private_key(key_path)?,
        )
        .map_err(|error| ControllerError::Tls(format!("invalid TLS certificate/key: {error}")))?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

fn ensure_rustls_crypto_provider() {
    let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
}

fn load_tls_certificates(path: &Path) -> Result<Vec<CertificateDer<'static>>, ControllerError> {
    let file = File::open(path).map_err(|error| {
        ControllerError::Tls(format!(
            "failed to open TLS certificate file {}: {error}",
            path.display()
        ))
    })?;
    let mut reader = BufReader::new(file);
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            ControllerError::Tls(format!(
                "failed to parse TLS certificate file {}: {error}",
                path.display()
            ))
        })
}

fn load_tls_private_key(path: &Path) -> Result<PrivateKeyDer<'static>, ControllerError> {
    let file = File::open(path).map_err(|error| {
        ControllerError::Tls(format!(
            "failed to open TLS private key file {}: {error}",
            path.display()
        ))
    })?;
    let mut reader = BufReader::new(file);
    rustls_pemfile::private_key(&mut reader)
        .map_err(|error| {
            ControllerError::Tls(format!(
                "failed to parse TLS private key file {}: {error}",
                path.display()
            ))
        })?
        .ok_or_else(|| {
            ControllerError::Tls(format!(
                "TLS private key file has no private key: {}",
                path.display()
            ))
        })
}

fn validate_tls_private_key_permissions(path: &Path) -> Result<(), ControllerError> {
    #[cfg(unix)]
    {
        let mode = std::fs::metadata(path)
            .map_err(ControllerError::Io)?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(ControllerError::Tls(format!(
                "TLS private key file must not be readable, writable, or executable by group/other: {}",
                path.display()
            )));
        }
    }

    #[cfg(not(unix))]
    {
        let _ = path;
    }

    Ok(())
}

fn validate_external_url(url: &str) -> Result<(), ControllerError> {
    if let Some(rest) = url.strip_prefix("https://") {
        let host = external_url_host(rest)?;
        if host.is_empty() {
            return Err(ControllerError::Json(
                "controller external URL host cannot be empty".to_owned(),
            ));
        }
        return Ok(());
    }

    if let Some(rest) = url.strip_prefix("http://") {
        let host = external_url_host(rest)?;
        if host.is_empty() {
            return Err(ControllerError::Json(
                "controller external URL host cannot be empty".to_owned(),
            ));
        }
        return Ok(());
    }

    Err(ControllerError::Json(
        "controller external URL must start with http:// or https://".to_owned(),
    ))
}

fn insecure_http_transport_target(config: &ControllerServerConfig) -> Option<String> {
    if let Some(external_url) = &config.external_url {
        return external_url
            .starts_with("http://")
            .then(|| external_url.clone());
    }

    if config.tls_cert_path.is_none() {
        return Some(format!("http://{}:{}", config.host, config.port));
    }

    None
}

fn external_url_host(rest: &str) -> Result<&str, ControllerError> {
    let authority = rest.split('/').next().unwrap_or(rest);
    if authority.is_empty() {
        return Err(ControllerError::Json(
            "controller external URL host cannot be empty".to_owned(),
        ));
    }
    if let Some(stripped) = authority.strip_prefix('[') {
        let (host, _) = stripped.split_once(']').ok_or_else(|| {
            ControllerError::Json("invalid bracketed IPv6 external URL host".to_owned())
        })?;
        return Ok(host);
    }
    Ok(authority.split(':').next().unwrap_or(authority))
}

pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn generate_token(prefix: &str) -> Result<String, ControllerError> {
    fleet_core::generate_prefixed_ulid(prefix)
        .map_err(|error| ControllerError::Json(error.to_string()))
}

pub fn heartbeat_signature(nonce: &str, fingerprint: &str) -> String {
    hash_token(&format!("{nonce}:{fingerprint}"))
}

fn default_job_expiration_seconds() -> u64 {
    300
}

fn default_drift_timeout_seconds() -> u64 {
    30
}

fn default_enrollment_token_max_uses() -> u32 {
    1
}

fn default_enrollment_token_expires_in_seconds() -> u64 {
    3600
}

fn default_confirmed_by() -> String {
    "admin".to_owned()
}

fn system_time_to_millis(value: SystemTime) -> u64 {
    value
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn millis_to_system_time(value: u64) -> SystemTime {
    UNIX_EPOCH
        .checked_add(Duration::from_millis(value))
        .unwrap_or(UNIX_EPOCH)
}

fn epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_store::SqliteStore;
    use std::io::{Read, Write};

    #[test]
    fn session_registry_registers_gets_and_unregisters_session() {
        let mut registry = AgentSessionRegistry::default();
        let (handle, _receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            vec!["persistent_session".to_owned()],
            Some(64),
        );

        let outcome = registry.register(handle);

        assert!(outcome.replaced.is_none());
        assert!(registry.has_active_session("agent-1"));
        let summary = registry.snapshot();
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].agent_id, "agent-1");
        assert_eq!(summary[0].connection_id, "conn-1");
        assert_eq!(summary[0].connected_at_ms, 1000);
        assert_eq!(summary[0].last_session_seen_at_ms, 1000);
        assert_eq!(summary[0].capabilities, vec!["persistent_session"]);
        assert_eq!(summary[0].queue_capacity, Some(64));

        assert_eq!(
            registry.unregister("agent-1", "conn-1", AgentSessionCloseReason::HandlerEnded),
            Some(AgentSessionEnded {
                agent_id: "agent-1".to_owned(),
                connection_id: "conn-1".to_owned(),
                close_reason: AgentSessionCloseReason::HandlerEnded,
            })
        );
        assert!(!registry.has_active_session("agent-1"));
    }

    #[test]
    fn session_registry_duplicate_session_uses_new_session_wins() {
        let mut registry = AgentSessionRegistry::default();
        let (old_handle, mut old_receiver) = session_handle(
            "agent-1",
            "conn-old",
            SystemTime::UNIX_EPOCH,
            Vec::new(),
            None,
        );
        let (new_handle, _new_receiver) = session_handle(
            "agent-1",
            "conn-new",
            SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            vec!["persistent_session".to_owned()],
            None,
        );
        registry.register(old_handle);

        let outcome = registry.register(new_handle);

        assert_eq!(
            outcome.replaced,
            Some(AgentSessionReplacement {
                agent_id: "agent-1".to_owned(),
                old_connection_id: "conn-old".to_owned(),
                new_connection_id: "conn-new".to_owned(),
                close_reason: AgentSessionCloseReason::ReplacedByNewSession,
            })
        );
        assert_eq!(
            old_receiver.try_recv().unwrap(),
            AgentSessionOutboundMessage::Close {
                reason: AgentSessionCloseReason::ReplacedByNewSession,
            }
        );
        assert_eq!(registry.get("agent-1").unwrap().connection_id(), "conn-new");
    }

    #[test]
    fn duplicate_session_replacement_is_audited() {
        let store = SqliteStore::in_memory().unwrap();
        let replacement = AgentSessionReplacement {
            agent_id: "agent-1".to_owned(),
            old_connection_id: "conn-old".to_owned(),
            new_connection_id: "conn-new".to_owned(),
            close_reason: AgentSessionCloseReason::ReplacedByNewSession,
        };

        audit_agent_session_replaced(&store, &replacement).unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Agent, 10)
            .unwrap();

        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].action, "agent_session_replaced");
        assert!(matches!(
            &audits[0].value,
            AuditValue::Plain(value)
                if value.contains("old_connection_id=conn-old")
                    && value.contains("new_connection_id=conn-new")
                    && value.contains("close_reason=replaced_by_new_session")
        ));
    }

    #[test]
    fn session_started_and_ended_are_audited_with_close_reason() {
        let store = SqliteStore::in_memory().unwrap();
        let ended = AgentSessionEnded {
            agent_id: "agent-1".to_owned(),
            connection_id: "conn-1".to_owned(),
            close_reason: AgentSessionCloseReason::HeartbeatTimeout,
        };

        audit_agent_session_started(&store, "agent-1", "conn-1").unwrap();
        audit_agent_session_ended(&store, &ended).unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Agent, 10)
            .unwrap();

        assert_eq!(audits.len(), 2);
        assert!(
            audits
                .iter()
                .any(|event| event.action == "agent_session_started")
        );
        assert!(matches!(
            &audits
                .iter()
                .find(|event| event.action == "agent_session_ended")
                .unwrap()
                .value,
            AuditValue::Plain(value)
                if value.contains("connection_id=conn-1")
                    && value.contains("close_reason=heartbeat_timeout")
        ));
    }

    #[test]
    fn session_registry_stale_unregister_does_not_remove_replacement() {
        let mut registry = AgentSessionRegistry::default();
        let (old_handle, _old_receiver) = session_handle(
            "agent-1",
            "conn-old",
            SystemTime::UNIX_EPOCH,
            Vec::new(),
            None,
        );
        let (new_handle, _new_receiver) = session_handle(
            "agent-1",
            "conn-new",
            SystemTime::UNIX_EPOCH,
            Vec::new(),
            None,
        );
        registry.register(old_handle);
        registry.register(new_handle);

        let removed =
            registry.unregister("agent-1", "conn-old", AgentSessionCloseReason::HandlerEnded);

        assert!(removed.is_none());
        assert_eq!(registry.get("agent-1").unwrap().connection_id(), "conn-new");
    }

    #[test]
    fn session_registry_close_removes_revoked_agent_session() {
        let mut registry = AgentSessionRegistry::default();
        let (handle, mut receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            Vec::new(),
            None,
        );
        registry.register(handle);

        let ended = registry.close("agent-1", AgentSessionCloseReason::Revoked);

        assert_eq!(
            ended,
            Some(AgentSessionEnded {
                agent_id: "agent-1".to_owned(),
                connection_id: "conn-1".to_owned(),
                close_reason: AgentSessionCloseReason::Revoked,
            })
        );
        assert_eq!(
            receiver.try_recv().unwrap(),
            AgentSessionOutboundMessage::Close {
                reason: AgentSessionCloseReason::Revoked,
            }
        );
        assert!(registry.snapshot().is_empty());
    }

    #[test]
    fn session_registry_snapshot_order_is_deterministic() {
        let mut registry = AgentSessionRegistry::default();
        let (agent_b, _receiver_b) = session_handle(
            "agent-b",
            "conn-b",
            SystemTime::UNIX_EPOCH,
            Vec::new(),
            None,
        );
        let (agent_a, _receiver_a) = session_handle(
            "agent-a",
            "conn-a",
            SystemTime::UNIX_EPOCH,
            Vec::new(),
            None,
        );
        registry.register(agent_b);
        registry.register(agent_a);

        let agent_ids = registry
            .snapshot()
            .into_iter()
            .map(|summary| summary.agent_id)
            .collect::<Vec<_>>();

        assert_eq!(agent_ids, vec!["agent-a", "agent-b"]);
    }

    #[test]
    fn session_registry_mark_seen_ignores_connection_id_mismatch() {
        let mut registry = AgentSessionRegistry::default();
        let (handle, _receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            Vec::new(),
            None,
        );
        registry.register(handle);

        assert!(!registry.mark_seen(
            "agent-1",
            "conn-stale",
            SystemTime::UNIX_EPOCH + Duration::from_secs(10)
        ));
        assert!(registry.mark_seen(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH + Duration::from_secs(11)
        ));

        assert_eq!(registry.snapshot()[0].last_session_seen_at_ms, 11000);
    }

    #[test]
    fn session_registry_records_controller_signing_trust_ack_for_matching_connection() {
        let mut registry = AgentSessionRegistry::default();
        let (handle, _receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            Vec::new(),
            None,
        );
        registry.register(handle);

        assert!(!registry.record_controller_signing_trust_ack(
            "agent-1",
            "conn-stale",
            AgentControllerSigningTrustAck {
                accepted: true,
                current_fingerprint: Some("controller-fp-stale".to_owned()),
                entries_count: 1,
                reason_code: None,
                acknowledged_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            }
        ));
        assert!(registry.record_controller_signing_trust_ack(
            "agent-1",
            "conn-1",
            AgentControllerSigningTrustAck {
                accepted: true,
                current_fingerprint: Some("controller-fp-new".to_owned()),
                entries_count: 2,
                reason_code: None,
                acknowledged_at: SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            }
        ));

        let summary = registry.snapshot();
        assert_eq!(
            summary[0].controller_signing_trust_current_fingerprint_prefix,
            Some(controller_signing_fingerprint_prefix("controller-fp-new").to_owned())
        );
        assert_eq!(summary[0].controller_signing_trust_accepted, Some(true));
        assert_eq!(summary[0].controller_signing_trust_entries_count, Some(2));
        assert_eq!(
            summary[0].controller_signing_trust_acknowledged_at_ms,
            Some(2000)
        );
        assert!(registry.controller_signing_trust_is_current("agent-1", "controller-fp-new"));
        assert!(!registry.controller_signing_trust_is_current("agent-1", "controller-fp-other"));
    }

    #[test]
    fn session_registry_records_agent_certificate_lifecycle_ack_for_matching_connection() {
        let mut registry = AgentSessionRegistry::default();
        let (handle, _receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            Vec::new(),
            None,
        );
        registry.register(handle);

        assert!(!registry.record_agent_certificate_lifecycle_ack(
            "agent-1",
            "conn-stale",
            AgentCertificateLifecycleRuntimeAck {
                accepted: true,
                state: fleet_protocol::AgentCertificateLifecycleStateWire::Issued,
                current_fingerprint: Some("cert-fp-stale".to_owned()),
                reason_code: None,
                acknowledged_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            }
        ));
        assert!(registry.record_agent_certificate_lifecycle_ack(
            "agent-1",
            "conn-1",
            AgentCertificateLifecycleRuntimeAck {
                accepted: false,
                state: fleet_protocol::AgentCertificateLifecycleStateWire::RenewalRequested,
                current_fingerprint: Some("cert-fp-current".to_owned()),
                reason_code: Some("certificate_lifecycle_runtime_not_implemented".to_owned()),
                acknowledged_at: SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            }
        ));

        let summary = registry.snapshot();
        assert_eq!(summary[0].agent_certificate_lifecycle_accepted, Some(false));
        assert_eq!(
            summary[0].agent_certificate_lifecycle_state.as_deref(),
            Some("renewal_requested")
        );
        assert_eq!(
            summary[0].agent_certificate_lifecycle_current_fingerprint_prefix,
            Some(controller_signing_fingerprint_prefix("cert-fp-current").to_owned())
        );
        assert_eq!(
            summary[0]
                .agent_certificate_lifecycle_reason_code
                .as_deref(),
            Some("certificate_lifecycle_runtime_not_implemented")
        );
        assert_eq!(
            summary[0].agent_certificate_lifecycle_acknowledged_at_ms,
            Some(2000)
        );
    }

    #[test]
    fn controller_trust_ack_from_wire_ignores_agent_mismatch() {
        let matching = fleet_protocol::WireMessage::new(
            "msg-trust-ack",
            "corr-trust-ack",
            Some("agent-1".to_owned()),
            1,
            fleet_protocol::WirePayload::ControllerSigningTrustBundleAck {
                agent_id: "agent-1".to_owned(),
                accepted: true,
                current_fingerprint: Some("controller-fp-new".to_owned()),
                entries_count: 1,
                reason_code: None,
            },
        );
        let mismatched = fleet_protocol::WireMessage::new(
            "msg-trust-ack",
            "corr-trust-ack",
            Some("agent-2".to_owned()),
            1,
            fleet_protocol::WirePayload::ControllerSigningTrustBundleAck {
                agent_id: "agent-2".to_owned(),
                accepted: true,
                current_fingerprint: Some("controller-fp-new".to_owned()),
                entries_count: 1,
                reason_code: None,
            },
        );

        let ack = agent_controller_signing_trust_ack_from_wire(
            "agent-1",
            &matching,
            SystemTime::UNIX_EPOCH + Duration::from_secs(3),
        )
        .expect("matching ack should parse");

        assert_eq!(
            ack.current_fingerprint.as_deref(),
            Some("controller-fp-new")
        );
        assert_eq!(
            ack.acknowledged_at,
            SystemTime::UNIX_EPOCH + Duration::from_secs(3)
        );
        assert!(
            agent_controller_signing_trust_ack_from_wire(
                "agent-1",
                &mismatched,
                SystemTime::UNIX_EPOCH
            )
            .is_none()
        );
    }

    #[test]
    fn agent_certificate_lifecycle_ack_from_wire_ignores_agent_mismatch() {
        let matching = fleet_protocol::WireMessage::new(
            "msg-agent-cert-ack",
            "corr-agent-cert-ack",
            Some("agent-1".to_owned()),
            1,
            fleet_protocol::WirePayload::AgentCertificateLifecycleAck {
                agent_id: "agent-1".to_owned(),
                accepted: false,
                state: fleet_protocol::AgentCertificateLifecycleStateWire::Issued,
                current_fingerprint: Some("cert-fp-current".to_owned()),
                reason_code: Some("certificate_lifecycle_runtime_not_implemented".to_owned()),
            },
        );
        let mismatched = fleet_protocol::WireMessage::new(
            "msg-agent-cert-ack",
            "corr-agent-cert-ack",
            Some("agent-2".to_owned()),
            1,
            fleet_protocol::WirePayload::AgentCertificateLifecycleAck {
                agent_id: "agent-2".to_owned(),
                accepted: false,
                state: fleet_protocol::AgentCertificateLifecycleStateWire::Issued,
                current_fingerprint: Some("cert-fp-current".to_owned()),
                reason_code: None,
            },
        );

        let ack = agent_certificate_lifecycle_ack_from_wire(
            "agent-1",
            &matching,
            SystemTime::UNIX_EPOCH + Duration::from_secs(3),
        )
        .expect("matching ack should parse");

        assert!(!ack.accepted);
        assert_eq!(ack.current_fingerprint.as_deref(), Some("cert-fp-current"));
        assert_eq!(
            ack.acknowledged_at,
            SystemTime::UNIX_EPOCH + Duration::from_secs(3)
        );
        assert!(
            agent_certificate_lifecycle_ack_from_wire(
                "agent-1",
                &mismatched,
                SystemTime::UNIX_EPOCH
            )
            .is_none()
        );
    }

    #[test]
    fn controller_signing_trust_bundle_ack_audit_omits_material() {
        let store = SqliteStore::in_memory().unwrap();
        let message = fleet_protocol::WireMessage::new(
            "msg-trust-ack",
            "corr-trust-ack",
            Some("agent-1".to_owned()),
            1,
            fleet_protocol::WirePayload::ControllerSigningTrustBundleAck {
                agent_id: "agent-1".to_owned(),
                accepted: false,
                current_fingerprint: Some("controller-fp-new".to_owned()),
                entries_count: 2,
                reason_code: Some("token=admin-token private_key=/secret/key".to_owned()),
            },
        );

        handle_agent_task_data_message(&store, "agent-1", message).unwrap();

        let audits = store
            .list_audit_events_by_category(AuditCategory::Security, 10)
            .unwrap();
        let dump = format!("{audits:?}");

        assert!(
            audits
                .iter()
                .any(|event| event.action == "controller_signing_trust_bundle_acknowledged")
        );
        assert!(dump.contains("accepted=false"));
        assert!(dump.contains(&format!(
            "current_fingerprint_prefix={}",
            controller_signing_fingerprint_prefix("controller-fp-new")
        )));
        assert!(dump.contains("entries_count=2"));
        assert!(dump.contains("reason_code=redacted"));
        assert!(!dump.contains("admin-token"));
        assert!(!dump.contains("/secret/key"));
        assert!(!dump.contains("private_key"));
        assert!(!dump.contains("public_key"));
    }

    #[test]
    fn agent_certificate_lifecycle_ack_audit_omits_material() {
        let store = SqliteStore::in_memory().unwrap();
        let message = fleet_protocol::WireMessage::new(
            "msg-agent-cert-ack",
            "corr-agent-cert-ack",
            Some("agent-1".to_owned()),
            1,
            fleet_protocol::WirePayload::AgentCertificateLifecycleAck {
                agent_id: "agent-1".to_owned(),
                accepted: false,
                state: fleet_protocol::AgentCertificateLifecycleStateWire::Issued,
                current_fingerprint: Some("cert-fp-current".to_owned()),
                reason_code: Some("token=admin-token private_key=/secret/key".to_owned()),
            },
        );

        handle_agent_task_data_message(&store, "agent-1", message).unwrap();

        let audits = store
            .list_audit_events_by_category(AuditCategory::Security, 10)
            .unwrap();
        let dump = format!("{audits:?}");

        assert!(
            audits
                .iter()
                .any(|event| event.action == "agent_certificate_lifecycle_acknowledged")
        );
        assert!(dump.contains("accepted=false"));
        assert!(dump.contains("state=issued"));
        assert!(dump.contains(&format!(
            "current_fingerprint_prefix={}",
            controller_signing_fingerprint_prefix("cert-fp-current")
        )));
        assert!(dump.contains("reason_code=redacted"));
        assert!(!dump.contains("admin-token"));
        assert!(!dump.contains("/secret/key"));
        assert!(!dump.contains("private_key"));
        assert!(!dump.contains("certificate_body"));
        assert!(!dump.contains("ca_path"));
    }

    #[test]
    fn agent_certificate_lifecycle_dispatch_sends_public_update_to_connected_session() {
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        let (handle, mut receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            vec!["split_writer".to_owned()],
            Some(4),
        );
        sessions.lock().unwrap().register(handle);
        let record = controller_test_agent_certificate_lifecycle_record();

        let result = dispatch_agent_certificate_lifecycle_update(
            &sessions,
            fleet_protocol::AgentCertificateLifecycleActionWire::ActivateRenewal,
            &record,
            "corr-agent-cert",
            90_000,
        )
        .unwrap();

        let outbound = receiver.try_recv().expect("update should be queued");
        let AgentSessionOutboundMessage::Wire(message) = outbound else {
            panic!("expected wire message");
        };
        let encoded = fleet_protocol::encode_message(&message).unwrap();
        let fleet_protocol::WirePayload::AgentCertificateLifecycleUpdate {
            agent_id,
            action,
            state,
            current_certificate,
            next_certificate,
            grace_until_ms,
            reason_code,
        } = message.payload
        else {
            panic!("expected lifecycle update");
        };

        assert_eq!(result.status, AgentCertificateLifecycleDispatchStatus::Sent);
        assert_eq!(result.agent_id, "agent-1");
        assert_eq!(agent_id, "agent-1");
        assert_eq!(
            action,
            fleet_protocol::AgentCertificateLifecycleActionWire::ActivateRenewal
        );
        assert_eq!(
            state,
            fleet_protocol::AgentCertificateLifecycleStateWire::DualCertificateActive
        );
        assert_eq!(
            current_certificate
                .as_ref()
                .map(|certificate| certificate.fingerprint.as_str()),
            Some("0123456789abcdef")
        );
        assert_eq!(
            next_certificate
                .as_ref()
                .map(|certificate| certificate.fingerprint.as_str()),
            Some("fedcba9876543210")
        );
        assert_eq!(grace_until_ms, Some(91_000));
        assert!(reason_code.is_none());
        assert!(!encoded.contains("private_key"));
        assert!(!encoded.contains("certificate_body"));
        assert!(!encoded.contains("ca_path"));
        assert!(!encoded.contains("runtime_env"));
        assert!(!encoded.contains("websocket_handle"));
    }

    #[test]
    fn agent_certificate_lifecycle_dispatch_reports_not_connected_without_persisting_handles() {
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        let record = controller_test_agent_certificate_lifecycle_record();

        let result = dispatch_agent_certificate_lifecycle_update(
            &sessions,
            fleet_protocol::AgentCertificateLifecycleActionWire::ActivateRenewal,
            &record,
            "corr-agent-cert",
            90_000,
        )
        .unwrap();

        assert_eq!(
            result.status,
            AgentCertificateLifecycleDispatchStatus::NotConnected
        );
        assert_eq!(result.agent_id, "agent-1");
        assert_eq!(
            result.current_fingerprint_prefix.as_deref(),
            Some(controller_signing_fingerprint_prefix("0123456789abcdef"))
        );
        assert_eq!(
            result.next_fingerprint_prefix.as_deref(),
            Some(controller_signing_fingerprint_prefix("fedcba9876543210"))
        );
    }

    #[test]
    fn agent_certificate_issuance_request_persists_and_dispatches_public_update_without_material() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        let (handle, mut receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            vec!["agent_certificate_lifecycle".to_owned()],
            Some(8),
        );
        sessions.lock().unwrap().register(handle);

        let response = route_request_with_identity_and_sessions(
            &admin_json_post(
                "/api/agents/agent-1/certificate-lifecycle/request-issuance",
                "{}",
            ),
            &store,
            None,
            &ControllerIdentity::dev_insecure(),
            &ControllerRuntimeMetadata::default(),
            Some(&sessions),
        )
        .unwrap();
        let body: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let stored =
            <SqliteStore as AgentCertificateLifecycleRepository>::load_agent_certificate_lifecycle(
                &store,
                &AgentId::new("agent-1").unwrap(),
            )
            .unwrap()
            .expect("lifecycle should be persisted");
        let outbound = receiver
            .try_recv()
            .expect("connected agent should receive lifecycle update");
        let AgentSessionOutboundMessage::Wire(message) = outbound else {
            panic!("expected lifecycle wire message");
        };
        let fleet_protocol::WirePayload::AgentCertificateLifecycleUpdate {
            agent_id,
            action,
            state,
            current_certificate,
            next_certificate,
            grace_until_ms,
            reason_code,
        } = message.payload
        else {
            panic!("expected agent certificate lifecycle update");
        };
        let audits = store
            .list_audit_events_by_category(AuditCategory::Security, 10)
            .unwrap();
        let dump = format!("{response}{audits:?}");

        assert!(response.starts_with("HTTP/1.1 202 Accepted"));
        assert_eq!(body["agent_id"], "agent-1");
        assert_eq!(body["action"], "request_issuance");
        assert_eq!(body["lifecycle_state"], "issuance_requested");
        assert_eq!(body["dispatch_status"], "sent");
        assert_eq!(
            body["audit_event_action"],
            "agent_certificate_issuance_requested"
        );
        assert_eq!(
            stored.lifecycle.state,
            fleet_domain::AgentCertificateLifecycleState::IssuanceRequested
        );
        assert_eq!(agent_id, "agent-1");
        assert_eq!(
            action,
            fleet_protocol::AgentCertificateLifecycleActionWire::RequestIssuance
        );
        assert_eq!(
            state,
            fleet_protocol::AgentCertificateLifecycleStateWire::IssuanceRequested
        );
        assert!(current_certificate.is_none());
        assert!(next_certificate.is_none());
        assert!(grace_until_ms.is_none());
        assert!(reason_code.is_none());
        assert!(
            audits
                .iter()
                .any(|event| event.action == "agent_certificate_issuance_requested")
        );
        assert!(!dump.contains("certificate_body"));
        assert!(!dump.contains("private_key"));
        assert!(!dump.contains("ca_path"));
        assert!(!dump.contains("admin-token"));
        assert!(!dump.contains("runtime_env"));
        assert!(!dump.contains("websocket_handle"));
    }

    #[test]
    fn agent_certificate_issuance_request_requires_agent_write_permission() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash_with_identity(
                &hash_token("viewer-token"),
                "viewer-1",
                "viewer",
            )
            .unwrap();
        save_test_agent(&store, "agent-1");

        let response = route_request(
            "POST /api/agents/agent-1/certificate-lifecycle/request-issuance HTTP/1.1\r\nAuthorization: Bearer viewer-token\r\nContent-Length: 2\r\n\r\n{}",
            &store,
        )
        .unwrap();
        let stored =
            <SqliteStore as AgentCertificateLifecycleRepository>::load_agent_certificate_lifecycle(
                &store,
                &AgentId::new("agent-1").unwrap(),
            )
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 403 Forbidden"));
        assert!(response.contains("\"required_permission\":\"agent_write\""));
        assert!(stored.is_none());
    }

    #[test]
    fn agent_certificate_lifecycle_status_returns_not_issued_for_known_agent_without_record() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash_with_identity(
                &hash_token("viewer-token"),
                "viewer-1",
                "viewer",
            )
            .unwrap();
        save_test_agent(&store, "agent-1");

        let response = route_request(
            "GET /api/agents/agent-1/certificate-lifecycle/status HTTP/1.1\r\nAuthorization: Bearer viewer-token\r\n\r\n",
            &store,
        )
        .unwrap();
        let body: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(body["agent_id"], "agent-1");
        assert_eq!(body["record_present"], false);
        assert_eq!(body["lifecycle_state"], "not_issued");
        assert!(body["current_fingerprint_prefix"].is_null());
        assert!(body["next_fingerprint_prefix"].is_null());
    }

    #[test]
    fn agent_certificate_lifecycle_status_returns_public_prefixes_without_material() {
        let mut store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        let record = controller_test_agent_certificate_lifecycle_record();
        <SqliteStore as AgentCertificateLifecycleRepository>::save_agent_certificate_lifecycle(
            &mut store, record,
        )
        .unwrap();

        let response = route_request(
            "GET /api/agents/agent-1/certificate-lifecycle/status HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        let body: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let dump = response.to_string();

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(body["record_present"], true);
        assert_eq!(body["lifecycle_state"], "dual_certificate_active");
        assert_eq!(
            body["current_fingerprint_prefix"],
            controller_signing_fingerprint_prefix("0123456789abcdef")
        );
        assert_eq!(
            body["next_fingerprint_prefix"],
            controller_signing_fingerprint_prefix("fedcba9876543210")
        );
        assert!(body["updated_at_ms"].as_u64().unwrap() > 0);
        assert!(!dump.contains("certificate_body"));
        assert!(!dump.contains("private_key"));
        assert!(!dump.contains("ca_path"));
        assert!(!dump.contains("admin-token"));
        assert!(!dump.contains("runtime_env"));
        assert!(!dump.contains("websocket_handle"));
    }

    #[test]
    fn websocket_session_outbound_channel_delivers_task_assignment() {
        let mut registry = AgentSessionRegistry::default();
        let (handle, mut receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            vec!["split_writer".to_owned()],
            Some(2),
        );
        registry.register(handle);
        let message = task_assignment_wire_message("agent-1");

        registry.try_send("agent-1", message.clone()).unwrap();

        assert_eq!(
            receiver.try_recv().unwrap(),
            AgentSessionOutboundMessage::Wire(Box::new(message))
        );
    }

    #[test]
    fn websocket_session_outbound_channel_overflow_is_reported() {
        let mut registry = AgentSessionRegistry::default();
        let (handle, _receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            vec!["split_writer".to_owned()],
            Some(1),
        );
        registry.register(handle);

        registry
            .try_send("agent-1", task_assignment_wire_message("agent-1"))
            .unwrap();
        let result = registry.try_send("agent-1", task_assignment_wire_message("agent-1"));

        assert_eq!(result, Err(AgentSessionSendError::QueueFull));
        let summary = registry.snapshot();
        assert_eq!(summary[0].queue_capacity, Some(1));
        assert_eq!(summary[0].queue_depth, 1);
    }

    #[test]
    fn websocket_session_write_failure_cleanup_removes_matching_session() {
        let mut registry = AgentSessionRegistry::default();
        let (handle, _receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            Vec::new(),
            None,
        );
        registry.register(handle);

        let ended = registry.unregister("agent-1", "conn-1", AgentSessionCloseReason::WriteFailure);

        assert_eq!(
            ended,
            Some(AgentSessionEnded {
                agent_id: "agent-1".to_owned(),
                connection_id: "conn-1".to_owned(),
                close_reason: AgentSessionCloseReason::WriteFailure,
            })
        );
        assert!(!registry.has_active_session("agent-1"));
    }

    #[test]
    fn session_read_failures_map_to_bounded_close_reasons() {
        assert_eq!(
            close_reason_from_session_read_result(&Err(ControllerError::Store(
                fleet_store::StoreError::Domain("slow store write".to_owned()),
            ))),
            AgentSessionCloseReason::StoreError
        );
        assert_eq!(
            close_reason_from_session_read_result(&Err(ControllerError::Json(
                "agent session outbound queue is full".to_owned(),
            ))),
            AgentSessionCloseReason::WriteQueueOverflow
        );
        assert_eq!(
            close_reason_from_session_read_result(&Err(ControllerError::Protocol(
                fleet_protocol::ProtocolError::Json(
                    "duplicate output chunk body mismatch".to_owned()
                ),
            ))),
            AgentSessionCloseReason::ProtocolError
        );
        assert_eq!(
            close_reason_from_session_read_result(&Ok(AgentSessionCloseReason::NormalShutdown)),
            AgentSessionCloseReason::NormalShutdown
        );
    }

    #[test]
    fn agent_disconnect_does_not_fail_running_job_without_task_result() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-1");
        store
            .update_job_status("job-1", JobStatus::Running)
            .unwrap();
        let ended = AgentSessionEnded {
            agent_id: "agent-1".to_owned(),
            connection_id: "conn-1".to_owned(),
            close_reason: AgentSessionCloseReason::NormalShutdown,
        };

        audit_agent_session_ended(&store, &ended).unwrap();

        assert_eq!(
            store.find_job_status_value("job-1").unwrap().unwrap(),
            "running"
        );
    }

    #[test]
    fn websocket_agent_id_mismatch_payload_is_security_audit() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        let message = fleet_protocol::WireMessage::new(
            "msg-facts",
            "corr-facts",
            Some("agent-2".to_owned()),
            1,
            fleet_protocol::WirePayload::FactsSnapshot {
                agent_id: "agent-2".to_owned(),
                body: "{\"os\":\"linux\"}".to_owned(),
            },
        );

        let finished = handle_agent_task_data_message(&store, "agent-1", message).unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Security, 10)
            .unwrap();
        let latest_facts = store.latest_facts_snapshot("agent-1").unwrap();

        assert!(!finished);
        assert!(latest_facts.is_none());
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].action, "websocket_facts_agent_mismatch");
    }

    #[test]
    fn health_endpoint_is_public() {
        let store = SqliteStore::in_memory().unwrap();
        let response = route_request("GET /healthz HTTP/1.1\r\n\r\n", &store).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"status\":\"ok\""));
    }

    #[test]
    fn controller_identity_endpoint_is_public() {
        let store = SqliteStore::in_memory().unwrap();
        let response =
            route_request("GET /api/controller/identity HTTP/1.1\r\n\r\n", &store).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"controller_fingerprint\":\"dev-controller-fingerprint\""));
        assert!(
            response.contains("\"controller_signing_fingerprint\":\"dev-controller-fingerprint\"")
        );
        assert!(response.contains("\"tls_endpoint\""));
    }

    #[test]
    fn controller_identity_endpoint_separates_tls_metadata() {
        let store = SqliteStore::in_memory().unwrap();
        let identity = ControllerIdentity::dev_insecure();
        let metadata = ControllerRuntimeMetadata {
            external_url: Some("https://fleet.example.com".to_owned()),
            tls_enabled: true,
            ..ControllerRuntimeMetadata::default()
        };

        let response = route_request_with_identity(
            "GET /api/controller/identity HTTP/1.1\r\n\r\n",
            &store,
            &identity,
            &metadata,
        )
        .unwrap();

        assert!(response.contains("\"controller_signing_public_key\""));
        assert!(response.contains("\"controller_signing_fingerprint\""));
        assert!(response.contains("\"external_url\":\"https://fleet.example.com\""));
        assert!(response.contains("\"tls_enabled\":true"));
    }

    #[test]
    fn admin_can_get_controller_signing_rotation_status_without_material_leak() {
        let mut store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let old_key = fleet_core::generate_agent_key_pair().unwrap();
        let new_key = fleet_core::generate_agent_key_pair().unwrap();
        let now = SystemTime::now();
        let requested_at = now - Duration::from_secs(30);
        let validated_at = now - Duration::from_secs(20);
        let activated_at = now - Duration::from_secs(10);
        let old_key_verifies_until = now + Duration::from_secs(60);
        let mut rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new(old_key.fingerprint.clone()).unwrap(),
        );
        rotation
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new(new_key.fingerprint.clone()).unwrap(),
                requested_at,
                old_key_verifies_until,
            )
            .unwrap();
        rotation.validate_new_material(validated_at).unwrap();
        rotation.activate_dual_trust(activated_at).unwrap();
        <SqliteStore as SigningKeyRotationRepository>::save_signing_key_rotation(
            &mut store,
            SigningKeyRotationRecord {
                controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
                rotation,
                updated_at: activated_at,
            },
        )
        .unwrap();
        let identity = controller_identity_from_key_pair(&new_key);

        let response = route_request_with_identity(
            "GET /api/controller/signing-rotation/status HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
            &identity,
            &ControllerRuntimeMetadata::default(),
        )
        .unwrap();
        let body: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let dump = body.to_string();

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(body["controller_id"], DEFAULT_CONTROLLER_ID);
        assert_eq!(body["persisted_state"], "dual_trust_active");
        assert_eq!(body["readiness"], "dual_trust_active_agents_migrating");
        assert_eq!(body["bootstrap_guard"], "active_matches_selected");
        assert_eq!(body["agent_trust_rollout"], "agents_migrating");
        assert!(body["old_key_verifies_until_ms"].as_u64().unwrap() > 0);
        assert!(!dump.contains(&old_key.public_key_hex));
        assert!(!dump.contains(&new_key.public_key_hex));
        assert!(!dump.contains(&new_key.private_key_hex));
        assert!(!dump.contains("controller_private.key"));
    }

    #[test]
    fn controller_signing_rotation_status_requires_admin_auth() {
        let store = SqliteStore::in_memory().unwrap();

        let response = route_request(
            "GET /api/controller/signing-rotation/status HTTP/1.1\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 401 Unauthorized"));
    }

    #[test]
    fn controller_signing_rotation_restart_plan_reports_no_restart_for_steady_state() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let active_key = fleet_core::generate_agent_key_pair().unwrap();
        let identity = controller_identity_from_key_pair(&active_key);

        let response = route_request_with_identity(
            "GET /api/controller/signing-rotation/restart-plan HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
            &identity,
            &ControllerRuntimeMetadata::default(),
        )
        .unwrap();
        let body: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let dump = body.to_string();

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(body["controller_id"], DEFAULT_CONTROLLER_ID);
        assert_eq!(body["restart_required"], false);
        assert_eq!(body["reload_supported"], false);
        assert_eq!(body["recommended_action"], "none");
        assert_eq!(body["bootstrap_guard"], "active_matches_selected");
        assert_eq!(body["readiness"], "steady_ready");
        assert!(
            body["verification_commands"]
                .as_array()
                .unwrap()
                .iter()
                .any(|command| command
                    .as_str()
                    .unwrap()
                    .contains("signing-rotation-status"))
        );
        assert!(!dump.contains(&active_key.public_key_hex));
        assert!(!dump.contains(&active_key.private_key_hex));
        assert!(!dump.contains("controller_private.key"));
        assert!(!dump.contains("admin-token"));
    }

    #[test]
    fn controller_signing_rotation_restart_plan_reports_restart_for_selected_mismatch() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let old_key = fleet_core::generate_agent_key_pair().unwrap();
        let new_key = fleet_core::generate_agent_key_pair().unwrap();
        let now = SystemTime::now();
        let mut rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new(old_key.fingerprint.clone()).unwrap(),
        );
        rotation
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new(new_key.fingerprint.clone()).unwrap(),
                now - Duration::from_secs(30),
                now + Duration::from_secs(60),
            )
            .unwrap();
        rotation
            .validate_new_material(now - Duration::from_secs(20))
            .unwrap();
        rotation
            .activate_dual_trust(now - Duration::from_secs(10))
            .unwrap();
        let mut rotation_repo = &store;
        <&SqliteStore as SigningKeyRotationRepository>::save_signing_key_rotation(
            &mut rotation_repo,
            SigningKeyRotationRecord {
                controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
                rotation: rotation.clone(),
                updated_at: now - Duration::from_secs(10),
            },
        )
        .unwrap();
        let identity = controller_identity_from_key_pair(&old_key);

        let response = route_request_with_identity(
            "GET /api/controller/signing-rotation/restart-plan HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
            &identity,
            &ControllerRuntimeMetadata::default(),
        )
        .unwrap();
        let body: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let persisted = <SqliteStore as SigningKeyRotationRepository>::load_signing_key_rotation(
            &store,
            DEFAULT_CONTROLLER_ID,
        )
        .unwrap()
        .expect("rotation state remains persisted");
        let dump = body.to_string();

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(body["restart_required"], true);
        assert_eq!(body["reload_supported"], false);
        assert_eq!(body["recommended_action"], "restart_controller_process");
        assert_eq!(body["bootstrap_guard"], "active_mismatch_selected");
        assert!(
            body["blocked_reason"]
                .as_str()
                .unwrap()
                .contains("active signer")
        );
        assert_eq!(
            persisted.rotation.state().as_str(),
            rotation.state().as_str()
        );
        assert!(!dump.contains(&old_key.public_key_hex));
        assert!(!dump.contains(&new_key.public_key_hex));
        assert!(!dump.contains(&new_key.private_key_hex));
        assert!(!dump.contains("controller_private.key"));
        assert!(!dump.contains("admin-token"));
    }

    #[test]
    fn controller_signing_rotation_restart_plan_requires_admin_auth() {
        let store = SqliteStore::in_memory().unwrap();

        let response = route_request(
            "GET /api/controller/signing-rotation/restart-plan HTTP/1.1\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 401 Unauthorized"));
    }

    #[test]
    fn controller_signing_rotation_restart_action_audits_external_restart_without_material_leak() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let old_key = fleet_core::generate_agent_key_pair().unwrap();
        let new_key = fleet_core::generate_agent_key_pair().unwrap();
        let now = SystemTime::now();
        let mut rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new(old_key.fingerprint.clone()).unwrap(),
        );
        rotation
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new(new_key.fingerprint.clone()).unwrap(),
                now - Duration::from_secs(30),
                now + Duration::from_secs(60),
            )
            .unwrap();
        rotation
            .validate_new_material(now - Duration::from_secs(20))
            .unwrap();
        rotation
            .activate_dual_trust(now - Duration::from_secs(10))
            .unwrap();
        let mut rotation_repo = &store;
        <&SqliteStore as SigningKeyRotationRepository>::save_signing_key_rotation(
            &mut rotation_repo,
            SigningKeyRotationRecord {
                controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
                rotation,
                updated_at: now - Duration::from_secs(10),
            },
        )
        .unwrap();
        let identity = controller_identity_from_key_pair(&old_key);
        let body = serde_json::json!({
            "confirm_external_restart": true,
            "reason": "operator approved token=admin-token private_key=/secret/controller_private.key"
        })
        .to_string();

        let response = route_request_with_identity(
            &admin_json_post("/api/controller/signing-rotation/restart-action", &body),
            &store,
            &identity,
            &ControllerRuntimeMetadata::default(),
        )
        .unwrap();
        let response_body: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Security, 10)
            .unwrap();
        let dump = format!("{response}{audits:?}");

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(response_body["action"], "external_service_manager_restart");
        assert_eq!(
            response_body["action_status"],
            "audit_recorded_external_restart_required"
        );
        assert_eq!(response_body["restart_required"], true);
        assert_eq!(response_body["reload_supported"], false);
        assert_eq!(response_body["bootstrap_guard"], "active_mismatch_selected");
        assert_eq!(
            response_body["service_command"],
            "sponzey controller restart-service --dry-run"
        );
        assert!(
            audits
                .iter()
                .any(|event| event.action == "controller_signing_rotation_restart_action_requested")
        );
        assert!(!dump.contains(&old_key.public_key_hex));
        assert!(!dump.contains(&new_key.public_key_hex));
        assert!(!dump.contains(&old_key.private_key_hex));
        assert!(!dump.contains(&new_key.private_key_hex));
        assert!(!dump.contains("admin-token"));
        assert!(!dump.contains("/secret/controller_private.key"));
    }

    #[test]
    fn controller_signing_rotation_restart_action_rejects_when_restart_not_required_without_audit()
    {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let active_key = fleet_core::generate_agent_key_pair().unwrap();
        let identity = controller_identity_from_key_pair(&active_key);
        let body = serde_json::json!({
            "confirm_external_restart": true,
            "reason": "operator approved token=admin-token private_key=/secret/controller_private.key"
        })
        .to_string();

        let response = route_request_with_identity(
            &admin_json_post("/api/controller/signing-rotation/restart-action", &body),
            &store,
            &identity,
            &ControllerRuntimeMetadata::default(),
        )
        .unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Security, 10)
            .unwrap();
        let dump = format!("{response}{audits:?}");

        assert!(response.starts_with("HTTP/1.1 409 Conflict"));
        assert!(
            audits
                .iter()
                .all(|event| event.action != "controller_signing_rotation_restart_action_requested")
        );
        assert!(!dump.contains(&active_key.public_key_hex));
        assert!(!dump.contains(&active_key.private_key_hex));
        assert!(!dump.contains("admin-token"));
        assert!(!dump.contains("/secret/controller_private.key"));
    }

    #[test]
    fn admin_can_rollout_controller_signing_trust_bundle_to_connected_sessions_without_material_leak()
     {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let dir = artifact_test_root("signing-trust-rollout");
        let old_pair = write_controller_signing_key_pair(&dir.join("old"), "old");
        let new_pair = write_controller_signing_key_pair(&dir.join("new"), "new");
        let now = SystemTime::now();
        let mut rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new(old_pair.key_pair.fingerprint.clone())
                .unwrap(),
        );
        rotation
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new(new_pair.key_pair.fingerprint.clone())
                    .unwrap(),
                now - Duration::from_secs(30),
                now + Duration::from_secs(3600),
            )
            .unwrap();
        rotation
            .validate_new_material(now - Duration::from_secs(20))
            .unwrap();
        rotation
            .activate_dual_trust(now - Duration::from_secs(10))
            .unwrap();
        let mut rotation_repo = &store;
        <&SqliteStore as SigningKeyRotationRepository>::save_signing_key_rotation(
            &mut rotation_repo,
            SigningKeyRotationRecord {
                controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
                rotation,
                updated_at: now - Duration::from_secs(10),
            },
        )
        .unwrap();
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        let (handle, mut receiver) = session_handle(
            "agent-1",
            "conn-1",
            now,
            vec!["persistent_session".to_owned()],
            Some(8),
        );
        sessions.lock().unwrap().register(handle);
        let body = serde_json::json!({
            "previous_public_key_path": old_pair.files.public_key_path,
            "agent_ids": ["agent-1", "agent-2"]
        })
        .to_string();
        let identity = controller_identity_from_key_pair(&new_pair.key_pair);

        let response = route_request_with_identity_and_sessions(
            &admin_json_post(
                "/api/controller/signing-rotation/rollout-trust-bundle",
                &body,
            ),
            &store,
            None,
            &identity,
            &ControllerRuntimeMetadata::default(),
            Some(&sessions),
        )
        .unwrap();
        let response_body: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let message = receiver
            .try_recv()
            .expect("connected agent should receive update");
        let AgentSessionOutboundMessage::Wire(message) = message else {
            panic!("expected wire message");
        };
        let fleet_protocol::WirePayload::ControllerSigningTrustBundleUpdate { entries } =
            &message.payload
        else {
            panic!("expected trust bundle update");
        };
        let audits = store
            .list_audit_events_by_category(AuditCategory::Security, 10)
            .unwrap();
        let dump = format!("{response}{audits:?}");

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(response_body["attempted_count"], 2);
        assert_eq!(response_body["updated_count"], 1);
        assert_eq!(response_body["skipped_count"], 1);
        assert_eq!(response_body["failed_count"], 0);
        assert_eq!(response_body["entries_count"], 2);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].role,
            fleet_protocol::ControllerSigningTrustRoleWire::Current
        );
        assert_eq!(entries[0].public_key, new_pair.key_pair.public_key_hex);
        assert_eq!(
            entries[1].role,
            fleet_protocol::ControllerSigningTrustRoleWire::Previous
        );
        assert_eq!(entries[1].public_key, old_pair.key_pair.public_key_hex);
        assert!(
            audits
                .iter()
                .any(|event| event.action == "controller_signing_trust_bundle_rollout")
        );
        assert!(!dump.contains(&old_pair.key_pair.public_key_hex));
        assert!(!dump.contains(&new_pair.key_pair.public_key_hex));
        assert!(!dump.contains(&old_pair.key_pair.private_key_hex));
        assert!(!dump.contains(&new_pair.key_pair.private_key_hex));
        assert!(!dump.contains("old_controller_public.key"));
        assert!(!dump.contains("admin-token"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn controller_signing_trust_bundle_rollout_skips_already_current_acknowledged_agent() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let dir = artifact_test_root("signing-trust-rollout-already-current");
        let old_pair = write_controller_signing_key_pair(&dir.join("old"), "old");
        let new_pair = write_controller_signing_key_pair(&dir.join("new"), "new");
        let now = SystemTime::now();
        let mut rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new(old_pair.key_pair.fingerprint.clone())
                .unwrap(),
        );
        rotation
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new(new_pair.key_pair.fingerprint.clone())
                    .unwrap(),
                now - Duration::from_secs(30),
                now + Duration::from_secs(3600),
            )
            .unwrap();
        rotation
            .validate_new_material(now - Duration::from_secs(20))
            .unwrap();
        rotation
            .activate_dual_trust(now - Duration::from_secs(10))
            .unwrap();
        let mut rotation_repo = &store;
        <&SqliteStore as SigningKeyRotationRepository>::save_signing_key_rotation(
            &mut rotation_repo,
            SigningKeyRotationRecord {
                controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
                rotation,
                updated_at: now - Duration::from_secs(10),
            },
        )
        .unwrap();
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        let (handle, mut receiver) = session_handle(
            "agent-1",
            "conn-1",
            now,
            vec!["persistent_session".to_owned()],
            Some(8),
        );
        {
            let mut guard = sessions.lock().unwrap();
            guard.register(handle);
            assert!(guard.record_controller_signing_trust_ack(
                "agent-1",
                "conn-1",
                AgentControllerSigningTrustAck {
                    accepted: true,
                    current_fingerprint: Some(new_pair.key_pair.fingerprint.clone()),
                    entries_count: 2,
                    reason_code: None,
                    acknowledged_at: now,
                }
            ));
        }
        let body = serde_json::json!({
            "previous_public_key_path": old_pair.files.public_key_path,
            "agent_ids": ["agent-1"]
        })
        .to_string();
        let identity = controller_identity_from_key_pair(&new_pair.key_pair);

        let response = route_request_with_identity_and_sessions(
            &admin_json_post(
                "/api/controller/signing-rotation/rollout-trust-bundle",
                &body,
            ),
            &store,
            None,
            &identity,
            &ControllerRuntimeMetadata::default(),
            Some(&sessions),
        )
        .unwrap();
        let response_body: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Security, 10)
            .unwrap();
        let dump = format!("{response}{audits:?}");

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(response_body["attempted_count"], 1);
        assert_eq!(response_body["updated_count"], 0);
        assert_eq!(response_body["skipped_count"], 1);
        assert_eq!(response_body["failed_count"], 0);
        assert_eq!(response_body["agent_results"][0]["agent_id"], "agent-1");
        assert_eq!(
            response_body["agent_results"][0]["status"],
            "skipped_already_current"
        );
        assert!(receiver.try_recv().is_err());
        assert!(!dump.contains(&old_pair.key_pair.public_key_hex));
        assert!(!dump.contains(&new_pair.key_pair.public_key_hex));
        assert!(!dump.contains(&old_pair.key_pair.private_key_hex));
        assert!(!dump.contains(&new_pair.key_pair.private_key_hex));
        assert!(!dump.contains("old_controller_public.key"));
        assert!(!dump.contains("admin-token"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn controller_signing_trust_bundle_staged_rollout_uses_ack_state_and_batch_limit() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let dir = artifact_test_root("signing-trust-staged-rollout");
        let old_pair = write_controller_signing_key_pair(&dir.join("old"), "old");
        let new_pair = write_controller_signing_key_pair(&dir.join("new"), "new");
        let now = SystemTime::now();
        let mut rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new(old_pair.key_pair.fingerprint.clone())
                .unwrap(),
        );
        rotation
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new(new_pair.key_pair.fingerprint.clone())
                    .unwrap(),
                now - Duration::from_secs(30),
                now + Duration::from_secs(3600),
            )
            .unwrap();
        rotation
            .validate_new_material(now - Duration::from_secs(20))
            .unwrap();
        rotation
            .activate_dual_trust(now - Duration::from_secs(10))
            .unwrap();
        let mut rotation_repo = &store;
        <&SqliteStore as SigningKeyRotationRepository>::save_signing_key_rotation(
            &mut rotation_repo,
            SigningKeyRotationRecord {
                controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
                rotation,
                updated_at: now - Duration::from_secs(10),
            },
        )
        .unwrap();
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        let (agent_1_handle, mut agent_1_receiver) = session_handle(
            "agent-1",
            "conn-1",
            now,
            vec!["persistent_session".to_owned()],
            Some(8),
        );
        let (agent_2_handle, mut agent_2_receiver) = session_handle(
            "agent-2",
            "conn-2",
            now,
            vec!["persistent_session".to_owned()],
            Some(8),
        );
        let (agent_3_handle, mut agent_3_receiver) = session_handle(
            "agent-3",
            "conn-3",
            now,
            vec!["persistent_session".to_owned()],
            Some(8),
        );
        {
            let mut guard = sessions.lock().unwrap();
            guard.register(agent_1_handle);
            guard.register(agent_2_handle);
            guard.register(agent_3_handle);
            assert!(guard.record_controller_signing_trust_ack(
                "agent-1",
                "conn-1",
                AgentControllerSigningTrustAck {
                    accepted: true,
                    current_fingerprint: Some(new_pair.key_pair.fingerprint.clone()),
                    entries_count: 2,
                    reason_code: None,
                    acknowledged_at: now,
                }
            ));
        }
        let body = serde_json::json!({
            "previous_public_key_path": old_pair.files.public_key_path,
            "agent_ids": ["agent-1", "agent-2", "agent-3"],
            "batch_size": 1,
            "max_failures": 1,
            "ack_timeout_seconds": 30
        })
        .to_string();
        let identity = controller_identity_from_key_pair(&new_pair.key_pair);

        let response = route_request_with_identity_and_sessions(
            &admin_json_post(
                "/api/controller/signing-rotation/rollout-trust-bundle/staged",
                &body,
            ),
            &store,
            None,
            &identity,
            &ControllerRuntimeMetadata::default(),
            Some(&sessions),
        )
        .unwrap();
        let response_body: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let agent_2_message = agent_2_receiver
            .try_recv()
            .expect("first non-current connected agent should receive staged update");
        let AgentSessionOutboundMessage::Wire(agent_2_message) = agent_2_message else {
            panic!("expected staged trust bundle message");
        };
        let audits = store
            .list_audit_events_by_category(AuditCategory::Security, 10)
            .unwrap();
        let dump = format!("{response}{audits:?}");

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(response_body["rollout_state"], "waiting_for_ack");
        assert_eq!(response_body["planned_count"], 1);
        assert_eq!(response_body["updated_count"], 1);
        assert_eq!(response_body["already_current_count"], 1);
        assert_eq!(response_body["pending_count"], 1);
        assert_eq!(response_body["unavailable_count"], 0);
        assert_eq!(response_body["agent_results"][0]["agent_id"], "agent-2");
        assert_eq!(response_body["agent_results"][0]["status"], "sent");
        assert!(matches!(
            agent_2_message.payload,
            fleet_protocol::WirePayload::ControllerSigningTrustBundleUpdate { .. }
        ));
        assert!(agent_1_receiver.try_recv().is_err());
        assert!(agent_3_receiver.try_recv().is_err());
        assert!(
            audits
                .iter()
                .any(|event| event.action == "controller_signing_trust_bundle_staged_rollout")
        );
        assert!(!dump.contains(&old_pair.key_pair.public_key_hex));
        assert!(!dump.contains(&new_pair.key_pair.public_key_hex));
        assert!(!dump.contains(&old_pair.key_pair.private_key_hex));
        assert!(!dump.contains(&new_pair.key_pair.private_key_hex));
        assert!(!dump.contains("old_controller_public.key"));
        assert!(!dump.contains("admin-token"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn controller_signing_trust_bundle_staged_rollout_persists_waiting_state_between_ticks() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let dir = artifact_test_root("signing-trust-staged-rollout-persisted");
        let old_pair = write_controller_signing_key_pair(&dir.join("old"), "old");
        let new_pair = write_controller_signing_key_pair(&dir.join("new"), "new");
        let now = SystemTime::now();
        let mut rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new(old_pair.key_pair.fingerprint.clone())
                .unwrap(),
        );
        rotation
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new(new_pair.key_pair.fingerprint.clone())
                    .unwrap(),
                now - Duration::from_secs(30),
                now + Duration::from_secs(3600),
            )
            .unwrap();
        rotation
            .validate_new_material(now - Duration::from_secs(20))
            .unwrap();
        rotation
            .activate_dual_trust(now - Duration::from_secs(10))
            .unwrap();
        let mut rotation_repo = &store;
        <&SqliteStore as SigningKeyRotationRepository>::save_signing_key_rotation(
            &mut rotation_repo,
            SigningKeyRotationRecord {
                controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
                rotation,
                updated_at: now - Duration::from_secs(10),
            },
        )
        .unwrap();
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        let (agent_1_handle, mut agent_1_receiver) = session_handle(
            "agent-1",
            "conn-1",
            now,
            vec!["persistent_session".to_owned()],
            Some(8),
        );
        let (agent_2_handle, mut agent_2_receiver) = session_handle(
            "agent-2",
            "conn-2",
            now,
            vec!["persistent_session".to_owned()],
            Some(8),
        );
        {
            let mut guard = sessions.lock().unwrap();
            guard.register(agent_1_handle);
            guard.register(agent_2_handle);
        }
        let body = serde_json::json!({
            "previous_public_key_path": old_pair.files.public_key_path,
            "agent_ids": ["agent-1", "agent-2"],
            "batch_size": 1,
            "max_failures": 1,
            "ack_timeout_seconds": 30
        })
        .to_string();
        let identity = controller_identity_from_key_pair(&new_pair.key_pair);

        let first_response = route_request_with_identity_and_sessions(
            &admin_json_post(
                "/api/controller/signing-rotation/rollout-trust-bundle/staged",
                &body,
            ),
            &store,
            None,
            &identity,
            &ControllerRuntimeMetadata::default(),
            Some(&sessions),
        )
        .unwrap();
        let first_body: serde_json::Value =
            serde_json::from_str(first_response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let agent_1_message = agent_1_receiver
            .try_recv()
            .expect("first tick should dispatch only the first staged target");
        let AgentSessionOutboundMessage::Wire(_) = agent_1_message else {
            panic!("expected staged trust bundle message");
        };
        let persisted_after_first =
            <SqliteStore as ControllerSigningStagedRolloutRepository>::load_controller_signing_staged_rollout(
                &store,
                DEFAULT_CONTROLLER_ID,
            )
            .unwrap()
            .expect("staged rollout state should persist after first tick");

        assert!(first_response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(first_body["rollout_state"], "waiting_for_ack");
        assert_eq!(first_body["agent_results"][0]["agent_id"], "agent-1");
        assert_eq!(
            persisted_after_first.rollout.snapshot().in_flight[0].agent_id,
            "agent-1"
        );

        let second_response = route_request_with_identity_and_sessions(
            &admin_json_post(
                "/api/controller/signing-rotation/rollout-trust-bundle/staged",
                &body,
            ),
            &store,
            None,
            &identity,
            &ControllerRuntimeMetadata::default(),
            Some(&sessions),
        )
        .unwrap();
        let second_body: serde_json::Value =
            serde_json::from_str(second_response.split("\r\n\r\n").nth(1).unwrap()).unwrap();

        assert!(second_response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(second_body["rollout_state"], "waiting_for_ack");
        assert_eq!(second_body["planned_count"], 0);
        assert_eq!(second_body["attempted_count"], 0);
        assert!(second_body["agent_results"].as_array().unwrap().is_empty());
        assert!(agent_1_receiver.try_recv().is_err());
        assert!(agent_2_receiver.try_recv().is_err());

        {
            let mut guard = sessions.lock().unwrap();
            assert!(guard.record_controller_signing_trust_ack(
                "agent-1",
                "conn-1",
                AgentControllerSigningTrustAck {
                    accepted: true,
                    current_fingerprint: Some(new_pair.key_pair.fingerprint.clone()),
                    entries_count: 2,
                    reason_code: None,
                    acknowledged_at: now + Duration::from_secs(1),
                }
            ));
        }

        let third_response = route_request_with_identity_and_sessions(
            &admin_json_post(
                "/api/controller/signing-rotation/rollout-trust-bundle/staged",
                &body,
            ),
            &store,
            None,
            &identity,
            &ControllerRuntimeMetadata::default(),
            Some(&sessions),
        )
        .unwrap();
        let third_body: serde_json::Value =
            serde_json::from_str(third_response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let agent_2_message = agent_2_receiver
            .try_recv()
            .expect("third tick should dispatch the next target after persisted ack observation");
        let AgentSessionOutboundMessage::Wire(_) = agent_2_message else {
            panic!("expected staged trust bundle message");
        };
        let persisted_after_third =
            <SqliteStore as ControllerSigningStagedRolloutRepository>::load_controller_signing_staged_rollout(
                &store,
                DEFAULT_CONTROLLER_ID,
            )
            .unwrap()
            .expect("staged rollout state should persist after third tick");
        let third_snapshot = persisted_after_third.rollout.snapshot();
        let dump = format!("{first_response}{second_response}{third_response}{third_snapshot:?}");

        assert!(third_response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(third_body["rollout_state"], "waiting_for_ack");
        assert_eq!(third_body["already_current_count"], 1);
        assert_eq!(third_body["planned_count"], 1);
        assert_eq!(third_body["agent_results"][0]["agent_id"], "agent-2");
        assert_eq!(
            third_snapshot.acknowledged_agent_ids,
            vec!["agent-1".to_owned()]
        );
        assert_eq!(third_snapshot.in_flight[0].agent_id, "agent-2");
        assert!(!dump.contains(&old_pair.key_pair.public_key_hex));
        assert!(!dump.contains(&new_pair.key_pair.public_key_hex));
        assert!(!dump.contains(&old_pair.key_pair.private_key_hex));
        assert!(!dump.contains(&new_pair.key_pair.private_key_hex));
        assert!(!dump.contains("old_controller_public.key"));
        assert!(!dump.contains("admin-token"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn controller_signing_staged_rollout_worker_continues_persisted_state_without_request_body() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let dir = artifact_test_root("signing-trust-staged-rollout-worker");
        let old_pair = write_controller_signing_key_pair(&dir.join("old"), "old");
        let new_pair = write_controller_signing_key_pair(&dir.join("new"), "new");
        let active_dir = dir.join("active");
        std::fs::create_dir_all(&active_dir).unwrap();
        let active_public_key_path = active_dir.join("controller_public.key");
        let previous_public_key_path = active_dir.join("controller_public.key.bak");
        std::fs::write(&active_public_key_path, &new_pair.key_pair.public_key_hex).unwrap();
        std::fs::write(&previous_public_key_path, &old_pair.key_pair.public_key_hex).unwrap();
        let now = SystemTime::now();
        let mut rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new(old_pair.key_pair.fingerprint.clone())
                .unwrap(),
        );
        rotation
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new(new_pair.key_pair.fingerprint.clone())
                    .unwrap(),
                now - Duration::from_secs(30),
                now + Duration::from_secs(3600),
            )
            .unwrap();
        rotation
            .validate_new_material(now - Duration::from_secs(20))
            .unwrap();
        rotation
            .activate_dual_trust(now - Duration::from_secs(10))
            .unwrap();
        let mut rotation_repo = &store;
        <&SqliteStore as SigningKeyRotationRepository>::save_signing_key_rotation(
            &mut rotation_repo,
            SigningKeyRotationRecord {
                controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
                rotation,
                updated_at: now - Duration::from_secs(10),
            },
        )
        .unwrap();
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        let (agent_1_handle, mut agent_1_receiver) = session_handle(
            "agent-1",
            "conn-1",
            now,
            vec!["persistent_session".to_owned()],
            Some(8),
        );
        let (agent_2_handle, mut agent_2_receiver) = session_handle(
            "agent-2",
            "conn-2",
            now,
            vec!["persistent_session".to_owned()],
            Some(8),
        );
        {
            let mut guard = sessions.lock().unwrap();
            guard.register(agent_1_handle);
            guard.register(agent_2_handle);
        }
        let body = serde_json::json!({
            "previous_public_key_path": old_pair.files.public_key_path,
            "agent_ids": ["agent-1", "agent-2"],
            "batch_size": 1,
            "max_failures": 1,
            "ack_timeout_seconds": 30
        })
        .to_string();
        let identity = controller_identity_from_key_pair(&new_pair.key_pair);
        let metadata = ControllerRuntimeMetadata {
            controller_signing_public_key_path: Some(active_public_key_path),
            ..ControllerRuntimeMetadata::default()
        };

        let start_response = route_request_with_identity_and_sessions(
            &admin_json_post(
                "/api/controller/signing-rotation/rollout-trust-bundle/staged",
                &body,
            ),
            &store,
            None,
            &identity,
            &metadata,
            Some(&sessions),
        )
        .unwrap();
        let AgentSessionOutboundMessage::Wire(_) = agent_1_receiver
            .try_recv()
            .expect("operator tick should dispatch first staged target")
        else {
            panic!("expected staged trust bundle message");
        };
        {
            let mut guard = sessions.lock().unwrap();
            assert!(guard.record_controller_signing_trust_ack(
                "agent-1",
                "conn-1",
                AgentControllerSigningTrustAck {
                    accepted: true,
                    current_fingerprint: Some(new_pair.key_pair.fingerprint.clone()),
                    entries_count: 2,
                    reason_code: None,
                    acknowledged_at: now + Duration::from_secs(1),
                }
            ));
        }

        let output = run_controller_signing_staged_rollout_once(
            &store,
            &sessions,
            &identity,
            &metadata,
            now + Duration::from_secs(2),
        )
        .unwrap();
        let AgentSessionOutboundMessage::Wire(agent_2_message) = agent_2_receiver
            .try_recv()
            .expect("worker tick should dispatch next persisted target")
        else {
            panic!("expected staged trust bundle message");
        };
        let audits = store
            .list_audit_events_by_category(AuditCategory::Security, 10)
            .unwrap();
        let dump = format!("{start_response}{output:?}{audits:?}");

        assert_eq!(output.rollout_state.as_deref(), Some("waiting_for_ack"));
        assert_eq!(output.planned_count, 1);
        assert_eq!(output.updated_count, 1);
        assert_eq!(output.already_current_count, 1);
        assert!(matches!(
            agent_2_message.payload,
            fleet_protocol::WirePayload::ControllerSigningTrustBundleUpdate { .. }
        ));
        assert!(
            audits.iter().any(
                |event| event.action == "controller_signing_trust_bundle_staged_rollout_worker"
            )
        );
        assert!(!dump.contains(&old_pair.key_pair.public_key_hex));
        assert!(!dump.contains(&new_pair.key_pair.public_key_hex));
        assert!(!dump.contains(&old_pair.key_pair.private_key_hex));
        assert!(!dump.contains(&new_pair.key_pair.private_key_hex));
        assert!(!dump.contains("controller_public.key.bak"));
        assert!(!dump.contains("admin-token"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn controller_signing_trust_bundle_staged_rollout_rejects_invalid_config_without_leak() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let dir = artifact_test_root("signing-trust-staged-rollout-invalid");
        let old_pair = write_controller_signing_key_pair(&dir.join("old"), "old");
        let new_pair = write_controller_signing_key_pair(&dir.join("new"), "new");
        let now = SystemTime::now();
        let mut rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new(old_pair.key_pair.fingerprint.clone())
                .unwrap(),
        );
        rotation
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new(new_pair.key_pair.fingerprint.clone())
                    .unwrap(),
                now - Duration::from_secs(30),
                now + Duration::from_secs(3600),
            )
            .unwrap();
        rotation
            .validate_new_material(now - Duration::from_secs(20))
            .unwrap();
        rotation
            .activate_dual_trust(now - Duration::from_secs(10))
            .unwrap();
        let mut rotation_repo = &store;
        <&SqliteStore as SigningKeyRotationRepository>::save_signing_key_rotation(
            &mut rotation_repo,
            SigningKeyRotationRecord {
                controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
                rotation,
                updated_at: now - Duration::from_secs(10),
            },
        )
        .unwrap();
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        let (handle, mut receiver) = session_handle(
            "agent-1",
            "conn-1",
            now,
            vec!["persistent_session".to_owned()],
            Some(8),
        );
        sessions.lock().unwrap().register(handle);
        let body = serde_json::json!({
            "previous_public_key_path": old_pair.files.public_key_path,
            "agent_ids": ["agent-1"],
            "batch_size": 0,
            "max_failures": 1,
            "ack_timeout_seconds": 30
        })
        .to_string();
        let identity = controller_identity_from_key_pair(&new_pair.key_pair);

        let response = route_request_with_identity_and_sessions(
            &admin_json_post(
                "/api/controller/signing-rotation/rollout-trust-bundle/staged",
                &body,
            ),
            &store,
            None,
            &identity,
            &ControllerRuntimeMetadata::default(),
            Some(&sessions),
        )
        .unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Security, 10)
            .unwrap();
        let dump = format!("{response}{audits:?}");

        assert!(response.starts_with("HTTP/1.1 409 Conflict"));
        assert!(receiver.try_recv().is_err());
        assert!(
            audits
                .iter()
                .all(|event| event.action != "controller_signing_trust_bundle_staged_rollout")
        );
        assert!(!dump.contains(&old_pair.key_pair.public_key_hex));
        assert!(!dump.contains(&new_pair.key_pair.public_key_hex));
        assert!(!dump.contains(&old_pair.key_pair.private_key_hex));
        assert!(!dump.contains(&new_pair.key_pair.private_key_hex));
        assert!(!dump.contains("old_controller_public.key"));
        assert!(!dump.contains("admin-token"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn controller_signing_trust_bundle_staged_rollout_requires_write_permission() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash_with_identity(
                &hash_token("viewer-token"),
                "viewer-1",
                "viewer",
            )
            .unwrap();
        let body = serde_json::json!({
            "agent_ids": ["agent-1"],
            "batch_size": 1,
            "max_failures": 1,
            "ack_timeout_seconds": 30
        })
        .to_string();

        let response = route_request(
            &format!(
                "POST /api/controller/signing-rotation/rollout-trust-bundle/staged HTTP/1.1\r\nAuthorization: Bearer viewer-token\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            ),
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 403 Forbidden"));
        assert!(response.contains("\"required_permission\":\"signing_rotation_write\""));
    }

    #[test]
    fn controller_signing_trust_bundle_rollout_requires_restart_and_valid_state() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let dir = artifact_test_root("signing-trust-rollout-conflict");
        let old_pair = write_controller_signing_key_pair(&dir.join("old"), "old");
        let new_pair = write_controller_signing_key_pair(&dir.join("new"), "new");
        let now = SystemTime::now();
        let mut rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new(old_pair.key_pair.fingerprint.clone())
                .unwrap(),
        );
        rotation
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new(new_pair.key_pair.fingerprint.clone())
                    .unwrap(),
                now - Duration::from_secs(30),
                now + Duration::from_secs(3600),
            )
            .unwrap();
        rotation
            .validate_new_material(now - Duration::from_secs(20))
            .unwrap();
        rotation
            .activate_dual_trust(now - Duration::from_secs(10))
            .unwrap();
        let mut rotation_repo = &store;
        <&SqliteStore as SigningKeyRotationRepository>::save_signing_key_rotation(
            &mut rotation_repo,
            SigningKeyRotationRecord {
                controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
                rotation,
                updated_at: now - Duration::from_secs(10),
            },
        )
        .unwrap();
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        let (handle, mut receiver) = session_handle("agent-1", "conn-1", now, Vec::new(), Some(8));
        sessions.lock().unwrap().register(handle);
        let body = serde_json::json!({
            "previous_public_key_path": old_pair.files.public_key_path,
            "agent_ids": ["agent-1"]
        })
        .to_string();
        let old_identity = controller_identity_from_key_pair(&old_pair.key_pair);

        let response = route_request_with_identity_and_sessions(
            &admin_json_post(
                "/api/controller/signing-rotation/rollout-trust-bundle",
                &body,
            ),
            &store,
            None,
            &old_identity,
            &ControllerRuntimeMetadata::default(),
            Some(&sessions),
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 409 Conflict"));
        assert!(receiver.try_recv().is_err());
        assert!(!response.contains(&old_pair.key_pair.public_key_hex));
        assert!(!response.contains(&new_pair.key_pair.public_key_hex));
        assert!(!response.contains("old_controller_public.key"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn controller_signing_trust_bundle_rollout_requires_write_permission() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash_with_identity(
                &hash_token("viewer-token"),
                "viewer-1",
                "viewer",
            )
            .unwrap();

        let response = route_request(
            "POST /api/controller/signing-rotation/rollout-trust-bundle HTTP/1.1\r\nAuthorization: Bearer viewer-token\r\nContent-Length: 2\r\n\r\n{}",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 403 Forbidden"));
        assert!(response.contains("\"required_permission\":\"signing_rotation_write\""));
    }

    #[test]
    fn controller_signing_trust_bundle_retry_limits_batch_and_skips_disconnected_without_leak() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let dir = artifact_test_root("signing-trust-rollout-retry");
        let old_pair = write_controller_signing_key_pair(&dir.join("old"), "old");
        let new_pair = write_controller_signing_key_pair(&dir.join("new"), "new");
        let now = SystemTime::now();
        let mut rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new(old_pair.key_pair.fingerprint.clone())
                .unwrap(),
        );
        rotation
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new(new_pair.key_pair.fingerprint.clone())
                    .unwrap(),
                now - Duration::from_secs(30),
                now + Duration::from_secs(3600),
            )
            .unwrap();
        rotation
            .validate_new_material(now - Duration::from_secs(20))
            .unwrap();
        rotation
            .activate_dual_trust(now - Duration::from_secs(10))
            .unwrap();
        let mut rotation_repo = &store;
        <&SqliteStore as SigningKeyRotationRepository>::save_signing_key_rotation(
            &mut rotation_repo,
            SigningKeyRotationRecord {
                controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
                rotation,
                updated_at: now - Duration::from_secs(10),
            },
        )
        .unwrap();
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        let (agent_1_handle, mut agent_1_receiver) = session_handle(
            "agent-1",
            "conn-1",
            now,
            vec!["persistent_session".to_owned()],
            Some(8),
        );
        let (agent_3_handle, mut agent_3_receiver) = session_handle(
            "agent-3",
            "conn-3",
            now,
            vec!["persistent_session".to_owned()],
            Some(8),
        );
        {
            let mut guard = sessions.lock().unwrap();
            guard.register(agent_1_handle);
            guard.register(agent_3_handle);
        }
        let body = serde_json::json!({
            "previous_public_key_path": old_pair.files.public_key_path,
            "agent_ids": ["agent-1", "agent-2", "agent-3"],
            "max_agent_count": 2
        })
        .to_string();
        let identity = controller_identity_from_key_pair(&new_pair.key_pair);

        let response = route_request_with_identity_and_sessions(
            &admin_json_post(
                "/api/controller/signing-rotation/rollout-trust-bundle/retry",
                &body,
            ),
            &store,
            None,
            &identity,
            &ControllerRuntimeMetadata::default(),
            Some(&sessions),
        )
        .unwrap();
        let response_body: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let message = agent_1_receiver
            .try_recv()
            .expect("first connected agent in batch should receive update");
        let AgentSessionOutboundMessage::Wire(message) = message else {
            panic!("expected wire message");
        };
        let fleet_protocol::WirePayload::ControllerSigningTrustBundleUpdate { entries } =
            &message.payload
        else {
            panic!("expected trust bundle update");
        };
        let audits = store
            .list_audit_events_by_category(AuditCategory::Security, 10)
            .unwrap();
        let dump = format!("{response}{audits:?}");

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(response_body["attempted_count"], 2);
        assert_eq!(response_body["updated_count"], 1);
        assert_eq!(response_body["skipped_count"], 1);
        assert_eq!(response_body["failed_count"], 0);
        assert_eq!(response_body["agent_results"][0]["agent_id"], "agent-1");
        assert_eq!(response_body["agent_results"][0]["status"], "sent");
        assert_eq!(response_body["agent_results"][1]["agent_id"], "agent-2");
        assert_eq!(
            response_body["agent_results"][1]["status"],
            "skipped_not_connected"
        );
        assert_eq!(entries.len(), 2);
        assert!(agent_3_receiver.try_recv().is_err());
        assert!(
            audits
                .iter()
                .any(|event| event.action == "controller_signing_trust_bundle_rollout_retry")
        );
        assert!(!dump.contains(&old_pair.key_pair.public_key_hex));
        assert!(!dump.contains(&new_pair.key_pair.public_key_hex));
        assert!(!dump.contains(&old_pair.key_pair.private_key_hex));
        assert!(!dump.contains(&new_pair.key_pair.private_key_hex));
        assert!(!dump.contains("old_controller_public.key"));
        assert!(!dump.contains("admin-token"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn controller_signing_trust_bundle_retry_rejects_zero_batch_without_material_leak() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let dir = artifact_test_root("signing-trust-rollout-retry-invalid");
        let old_pair = write_controller_signing_key_pair(&dir.join("old"), "old");
        let new_pair = write_controller_signing_key_pair(&dir.join("new"), "new");
        let now = SystemTime::now();
        let mut rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new(old_pair.key_pair.fingerprint.clone())
                .unwrap(),
        );
        rotation
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new(new_pair.key_pair.fingerprint.clone())
                    .unwrap(),
                now - Duration::from_secs(30),
                now + Duration::from_secs(3600),
            )
            .unwrap();
        rotation
            .validate_new_material(now - Duration::from_secs(20))
            .unwrap();
        rotation
            .activate_dual_trust(now - Duration::from_secs(10))
            .unwrap();
        let mut rotation_repo = &store;
        <&SqliteStore as SigningKeyRotationRepository>::save_signing_key_rotation(
            &mut rotation_repo,
            SigningKeyRotationRecord {
                controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
                rotation,
                updated_at: now - Duration::from_secs(10),
            },
        )
        .unwrap();
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        let body = serde_json::json!({
            "previous_public_key_path": old_pair.files.public_key_path,
            "agent_ids": ["agent-1"],
            "max_agent_count": 0
        })
        .to_string();
        let identity = controller_identity_from_key_pair(&new_pair.key_pair);

        let response = route_request_with_identity_and_sessions(
            &admin_json_post(
                "/api/controller/signing-rotation/rollout-trust-bundle/retry",
                &body,
            ),
            &store,
            None,
            &identity,
            &ControllerRuntimeMetadata::default(),
            Some(&sessions),
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 409 Conflict"));
        assert!(!response.contains(&old_pair.key_pair.public_key_hex));
        assert!(!response.contains(&new_pair.key_pair.public_key_hex));
        assert!(!response.contains(&old_pair.key_pair.private_key_hex));
        assert!(!response.contains("old_controller_public.key"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn admin_can_request_controller_signing_rotation_without_material_leak() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let old_key = fleet_core::generate_agent_key_pair().unwrap();
        let new_key = fleet_core::generate_agent_key_pair().unwrap();
        let identity = controller_identity_from_key_pair(&old_key);
        let body = serde_json::json!({
            "new_fingerprint": new_key.fingerprint,
            "old_key_verifies_for_seconds": 60,
            "reason": "operator requested rotation"
        })
        .to_string();

        let response = route_request_with_identity(
            &admin_json_post("/api/controller/signing-rotation/request", &body),
            &store,
            &identity,
            &ControllerRuntimeMetadata::default(),
        )
        .unwrap();
        let response_body: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let loaded = <SqliteStore as SigningKeyRotationRepository>::load_signing_key_rotation(
            &store,
            DEFAULT_CONTROLLER_ID,
        )
        .unwrap()
        .expect("rotation state should be persisted");
        let audits = store
            .list_audit_events_by_category(AuditCategory::Security, 10)
            .unwrap();
        let dump = format!("{response}{loaded:?}{audits:?}");

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(response_body["persisted_state"], "rotation_requested");
        assert_eq!(
            response_body["readiness"],
            "rotation_requested_not_validated"
        );
        assert_eq!(loaded.rotation.state().as_str(), "rotation_requested");
        assert!(
            audits
                .iter()
                .any(|event| event.action == "controller_signing_key_rotation_requested")
        );
        assert!(!dump.contains(&new_key.public_key_hex));
        assert!(!dump.contains(&new_key.private_key_hex));
        assert!(!dump.contains("controller_private.key"));
    }

    #[test]
    fn controller_signing_rotation_validate_rejects_key_body_fields_without_leak() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let body = serde_json::json!({
            "candidate_public_key_path": "/tmp/controller_public.key",
            "candidate_private_key_path": "/tmp/controller_private.key",
            "private_key": "PRIVATE KEY SECRET",
            "task_payload_body": "{\"program\":\"uptime\"}"
        })
        .to_string();

        let response = route_request(
            &admin_json_post("/api/controller/signing-rotation/validate", &body),
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 400 Bad Request"));
        assert!(response.contains("\"error\":\"invalid_signing_rotation_request\""));
        assert!(!response.contains("PRIVATE KEY SECRET"));
        assert!(!response.contains("controller_private.key"));
        assert!(!response.contains("uptime"));
    }

    #[test]
    fn admin_can_validate_controller_signing_rotation_from_candidate_paths() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let dir = artifact_test_root("signing-rotation-validate");
        let active = write_default_controller_signing_key_pair(&dir);
        let candidate = controller_signing_test_pair(&dir.join("candidate"), "candidate");
        let identity = controller_identity_from_key_pair(&active.key_pair);
        let metadata = signing_rotation_test_metadata(&active.files, None);
        let request_body = serde_json::json!({
            "new_fingerprint": candidate.key_pair.fingerprint,
            "old_key_verifies_for_seconds": 60
        })
        .to_string();
        route_request_with_identity(
            &admin_json_post("/api/controller/signing-rotation/request", &request_body),
            &store,
            &identity,
            &metadata,
        )
        .unwrap();
        let validate_body = serde_json::json!({
            "candidate_public_key_path": candidate.files.public_key_path,
            "candidate_private_key_path": candidate.files.private_key_path,
            "reason": "validated by controller-side file boundary"
        })
        .to_string();

        let response = route_request_with_identity(
            &admin_json_post("/api/controller/signing-rotation/validate", &validate_body),
            &store,
            &identity,
            &metadata,
        )
        .unwrap();
        let response_body: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let loaded = <SqliteStore as SigningKeyRotationRepository>::load_signing_key_rotation(
            &store,
            DEFAULT_CONTROLLER_ID,
        )
        .unwrap()
        .expect("rotation state should be persisted");
        let dump = response_body.to_string();

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(response_body["persisted_state"], "new_material_validated");
        assert_eq!(
            response_body["readiness"],
            "new_material_validated_waiting_activation"
        );
        assert_eq!(loaded.rotation.state().as_str(), "new_material_validated");
        assert!(!dump.contains(&candidate.key_pair.public_key_hex));
        assert!(!dump.contains(&candidate.key_pair.private_key_hex));
        assert!(!dump.contains("candidate_controller_private.key"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn controller_signing_rotation_activate_retire_and_fail_are_state_machine_bound() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let old_key = fleet_core::generate_agent_key_pair().unwrap();
        let new_key = fleet_core::generate_agent_key_pair().unwrap();
        let identity = controller_identity_from_key_pair(&old_key);
        let now = SystemTime::now();
        let mut rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new(old_key.fingerprint.clone()).unwrap(),
        );
        rotation
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new(new_key.fingerprint.clone()).unwrap(),
                now - Duration::from_secs(30),
                now + Duration::from_secs(60),
            )
            .unwrap();
        rotation
            .validate_new_material(now - Duration::from_secs(20))
            .unwrap();
        let mut rotation_repo = &store;
        <&SqliteStore as SigningKeyRotationRepository>::save_signing_key_rotation(
            &mut rotation_repo,
            SigningKeyRotationRecord {
                controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
                rotation,
                updated_at: now,
            },
        )
        .unwrap();

        let activate = route_request_with_identity(
            &admin_json_post("/api/controller/signing-rotation/activate", "{}"),
            &store,
            &identity,
            &ControllerRuntimeMetadata::default(),
        )
        .unwrap();
        let activate_body: serde_json::Value =
            serde_json::from_str(activate.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert!(activate.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(activate_body["persisted_state"], "dual_trust_active");
        assert_eq!(
            activate_body["readiness"],
            "dual_trust_active_agents_migrating"
        );
        assert_eq!(activate_body["bootstrap_guard"], "active_mismatch_selected");

        let early_retire = route_request_with_identity(
            &admin_json_post("/api/controller/signing-rotation/retire", "{}"),
            &store,
            &identity,
            &ControllerRuntimeMetadata::default(),
        )
        .unwrap();
        assert!(early_retire.starts_with("HTTP/1.1 409 Conflict"));

        let mut ready_to_retire = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new(old_key.fingerprint.clone()).unwrap(),
        );
        ready_to_retire
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new(new_key.fingerprint.clone()).unwrap(),
                now - Duration::from_secs(120),
                now - Duration::from_secs(10),
            )
            .unwrap();
        ready_to_retire
            .validate_new_material(now - Duration::from_secs(110))
            .unwrap();
        ready_to_retire
            .activate_dual_trust(now - Duration::from_secs(100))
            .unwrap();
        let mut rotation_repo = &store;
        <&SqliteStore as SigningKeyRotationRepository>::save_signing_key_rotation(
            &mut rotation_repo,
            SigningKeyRotationRecord {
                controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
                rotation: ready_to_retire,
                updated_at: now - Duration::from_secs(100),
            },
        )
        .unwrap();
        let retire = route_request_with_identity(
            &admin_json_post("/api/controller/signing-rotation/retire", "{}"),
            &store,
            &identity,
            &ControllerRuntimeMetadata::default(),
        )
        .unwrap();
        let retire_body: serde_json::Value =
            serde_json::from_str(retire.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert!(retire.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(retire_body["persisted_state"], "old_key_retired");
        assert_eq!(retire_body["readiness"], "terminal_retired");

        let fail_body = serde_json::json!({
            "reason": "PRIVATE KEY failed at controller_private.key"
        })
        .to_string();
        let fail = route_request_with_identity(
            &admin_json_post("/api/controller/signing-rotation/fail", &fail_body),
            &store,
            &identity,
            &ControllerRuntimeMetadata::default(),
        )
        .unwrap();
        assert!(fail.starts_with("HTTP/1.1 409 Conflict"));
        assert!(!fail.contains("PRIVATE KEY"));
        assert!(!fail.contains("controller_private.key"));
    }

    #[test]
    fn controller_signing_rotation_fail_records_terminal_state_without_reason_leak() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let old_key = fleet_core::generate_agent_key_pair().unwrap();
        let new_key = fleet_core::generate_agent_key_pair().unwrap();
        let identity = controller_identity_from_key_pair(&old_key);
        let request_body = serde_json::json!({
            "new_fingerprint": new_key.fingerprint,
            "old_key_verifies_for_seconds": 60
        })
        .to_string();
        route_request_with_identity(
            &admin_json_post("/api/controller/signing-rotation/request", &request_body),
            &store,
            &identity,
            &ControllerRuntimeMetadata::default(),
        )
        .unwrap();
        let fail_body = serde_json::json!({
            "reason": "PRIVATE KEY failed at /secret/controller_private.key"
        })
        .to_string();

        let response = route_request_with_identity(
            &admin_json_post("/api/controller/signing-rotation/fail", &fail_body),
            &store,
            &identity,
            &ControllerRuntimeMetadata::default(),
        )
        .unwrap();
        let response_body: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let loaded = <SqliteStore as SigningKeyRotationRepository>::load_signing_key_rotation(
            &store,
            DEFAULT_CONTROLLER_ID,
        )
        .unwrap()
        .expect("rotation state should be persisted");
        let audits = store
            .list_audit_events_by_category(AuditCategory::Security, 10)
            .unwrap();
        let dump = format!("{response}{loaded:?}{audits:?}");

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(response_body["persisted_state"], "rotation_failed");
        assert_eq!(response_body["readiness"], "terminal_failed");
        assert_eq!(loaded.rotation.state().as_str(), "rotation_failed");
        assert!(
            audits
                .iter()
                .any(|event| event.action == "controller_signing_key_rotation_failed")
        );
        assert!(!dump.contains("PRIVATE KEY"));
        assert!(!dump.contains("controller_private.key"));
        assert!(!dump.contains("/secret"));
    }

    #[test]
    fn controller_signing_rotation_mutation_requires_write_permission() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash_with_identity(
                &hash_token("viewer-token"),
                "viewer-1",
                "viewer",
            )
            .unwrap();
        let body = serde_json::json!({
            "new_fingerprint": "new-fingerprint",
            "old_key_verifies_for_seconds": 60
        })
        .to_string();

        let response = route_request(
            &format!(
                "POST /api/controller/signing-rotation/request HTTP/1.1\r\nAuthorization: Bearer viewer-token\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            ),
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 403 Forbidden"));
        assert!(response.contains("\"required_permission\":\"signing_rotation_write\""));
    }

    #[test]
    fn admin_static_assets_are_served_by_controller() {
        let store = SqliteStore::in_memory().unwrap();

        let index = route_request("GET /admin HTTP/1.1\r\n\r\n", &store).unwrap();
        let index_with_query =
            route_request("GET /admin?admin-token=redacted HTTP/1.1\r\n\r\n", &store).unwrap();
        let css = route_request("GET /admin/styles.css HTTP/1.1\r\n\r\n", &store).unwrap();
        let js = route_request("GET /admin/app.js HTTP/1.1\r\n\r\n", &store).unwrap();
        let js_with_query =
            route_request("GET /admin/app.js?v=1 HTTP/1.1\r\n\r\n", &store).unwrap();
        let client = route_request("GET /admin/api-client.js HTTP/1.1\r\n\r\n", &store).unwrap();
        let schema = route_request("GET /admin/api.schema.json HTTP/1.1\r\n\r\n", &store).unwrap();
        let missing = route_request("GET /admin/missing.js HTTP/1.1\r\n\r\n", &store).unwrap();
        let favicon = route_request("GET /favicon.ico HTTP/1.1\r\n\r\n", &store).unwrap();

        assert!(index.starts_with("HTTP/1.1 200"));
        assert!(index.contains("Content-Type: text/html; charset=utf-8"));
        assert!(index.contains("Sponzey Fleet Admin"));
        assert!(index.contains("/admin/app.js"));
        assert!(index_with_query.starts_with("HTTP/1.1 200"));
        assert!(index_with_query.contains("Sponzey Fleet Admin"));
        assert!(css.starts_with("HTTP/1.1 200"));
        assert!(css.contains("Content-Type: text/css; charset=utf-8"));
        assert!(css.contains("color-scheme"));
        assert!(js.starts_with("HTTP/1.1 200"));
        assert!(js.contains("Content-Type: application/javascript; charset=utf-8"));
        assert!(js.contains("./api-client.js"));
        assert!(js_with_query.starts_with("HTTP/1.1 200"));
        assert!(js_with_query.contains("createApiClient"));
        assert!(client.starts_with("HTTP/1.1 200"));
        assert!(client.contains("/api/agents"));
        assert!(schema.starts_with("HTTP/1.1 200"));
        assert!(schema.contains("\"schema_version\": \"mvp-1\""));
        assert!(favicon.starts_with("HTTP/1.1 204"));
        assert!(missing.starts_with("HTTP/1.1 404"));
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ApiSurface {
        Public,
        Admin,
        AgentProtocol,
    }

    #[derive(Clone, Copy, Debug)]
    struct RestApiRouteContract {
        method: &'static str,
        path: &'static str,
        surface: ApiSurface,
    }

    const REST_API_ROUTE_CONTRACT: &[RestApiRouteContract] = &[
        RestApiRouteContract {
            method: "GET",
            path: "/healthz",
            surface: ApiSurface::Public,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/openapi.json",
            surface: ApiSurface::Public,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/swagger-ui",
            surface: ApiSurface::Public,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/controller/identity",
            surface: ApiSurface::Public,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/controller/signing-rotation/status",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/controller/signing-rotation/restart-plan",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/controller/signing-rotation/restart-action",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/controller/signing-rotation/request",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/controller/signing-rotation/validate",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/controller/signing-rotation/activate",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/controller/signing-rotation/retire",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/controller/signing-rotation/fail",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/controller/signing-rotation/rollout-trust-bundle",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/controller/signing-rotation/rollout-trust-bundle/staged",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/controller/signing-rotation/rollout-trust-bundle/retry",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/agents/enroll",
            surface: ApiSurface::AgentProtocol,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/agents",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/agents/{agent_id}",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "PATCH",
            path: "/api/agents/{agent_id}/labels",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/agents/{agent_id}/revoke-key",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/agents/{agent_id}/facts/latest",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/agents/{agent_id}/facts",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/agents/{agent_id}/metrics/latest",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/agents/{agent_id}/metrics",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/agents/{agent_id}/logs",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/agents/{agent_id}/drift/latest",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/agents/{agent_id}/drift",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/agents/{agent_id}/policies",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/policies",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/policies",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/policies/{policy_id}/assignments",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/policies/{policy_id}/schedules",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/drift/scheduled",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/jobs",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/jobs/{job_id}",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/jobs/command",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/jobs/drift-check",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/jobs/runbook",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/jobs/{job_id}/cancel",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/jobs/{job_id}/output",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/jobs/{job_id}/artifacts/{artifact_id}",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/approvals",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/approvals/expire",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/approvals/{approval_id}/approve",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/approvals/{approval_id}/reject",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/remediations",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/remediations/{remediation_id}",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/remediations/{remediation_id}/approval-request",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/remediations/{remediation_id}/approve",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/remediations/{remediation_id}/running",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/remediations/{remediation_id}/result",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/remediations/{remediation_id}/verify",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/audit",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/audit/export",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "GET",
            path: "/api/enrollment-tokens",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/enrollment-tokens",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "DELETE",
            path: "/api/enrollment-tokens/{id}",
            surface: ApiSurface::Admin,
        },
        RestApiRouteContract {
            method: "POST",
            path: "/api/selectors/preview",
            surface: ApiSurface::Admin,
        },
    ];

    fn openapi_operation_pointer(path: &str, method: &str) -> String {
        let escaped_path = path.replace('~', "~0").replace('/', "~1");
        format!("/paths/{escaped_path}/{}", method.to_ascii_lowercase())
    }

    fn openapi_has_bearer_auth(operation: &serde_json::Value) -> bool {
        operation
            .get("security")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|entries| {
                entries.iter().any(|entry| {
                    entry
                        .as_object()
                        .is_some_and(|object| object.contains_key("bearerAuth"))
                })
            })
    }

    #[test]
    fn openapi_and_swagger_ui_are_public() {
        let store = SqliteStore::in_memory().unwrap();

        let openapi = route_request("GET /openapi.json HTTP/1.1\r\n\r\n", &store).unwrap();
        let swagger = route_request("GET /swagger-ui?try=1 HTTP/1.1\r\n\r\n", &store).unwrap();

        assert!(openapi.starts_with("HTTP/1.1 200"));
        assert!(openapi.contains("Content-Type: application/json; charset=utf-8"));
        let body = openapi.split("\r\n\r\n").nth(1).unwrap_or_default();
        let document: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(document["openapi"], "3.1.0");
        assert_eq!(
            document
                .pointer("/components/securitySchemes/bearerAuth/type")
                .and_then(serde_json::Value::as_str),
            Some("http")
        );
        assert!(
            document
                .pointer("/components/responses/Forbidden")
                .is_some()
        );
        assert_eq!(
            document
                .pointer("/components/schemas/ApprovalDecisionRequest/required")
                .and_then(serde_json::Value::as_array),
            None
        );
        assert!(document.pointer("/paths/~1healthz").is_some());
        assert!(document.pointer("/paths/~1api~1agents").is_some());
        assert!(
            document
                .pointer("/paths/~1api~1agents~1{agent_id}~1facts~1latest")
                .is_some()
        );
        assert!(
            document
                .pointer("/paths/~1api~1agents~1{agent_id}~1facts")
                .is_some()
        );
        assert!(
            document
                .pointer("/paths/~1api~1agents~1{agent_id}~1metrics")
                .is_some()
        );
        assert!(
            document
                .pointer("/paths/~1api~1agents~1{agent_id}~1logs")
                .is_some()
        );
        assert!(
            document
                .pointer("/paths/~1api~1agents~1{agent_id}~1drift")
                .is_some()
        );
        assert!(document.pointer("/paths/~1api~1jobs~1command").is_some());
        assert!(document.pointer("/paths/~1api~1audit").is_some());

        assert!(swagger.starts_with("HTTP/1.1 200"));
        assert!(swagger.contains("Content-Type: text/html; charset=utf-8"));
        assert!(swagger.contains("SwaggerUIBundle"));
        assert!(swagger.contains("/openapi.json"));
        assert!(swagger.contains("HTTP is test-only"));
    }

    #[test]
    fn openapi_documents_public_admin_and_agent_rest_contract() {
        let document: serde_json::Value = serde_json::from_str(OPENAPI_JSON).unwrap();
        let mut public_routes = 0;
        let mut admin_routes = 0;
        let mut agent_protocol_routes = 0;

        for route in REST_API_ROUTE_CONTRACT {
            let pointer = openapi_operation_pointer(route.path, route.method);
            let operation = document.pointer(&pointer).unwrap_or_else(|| {
                panic!(
                    "OpenAPI must document {} {} at pointer {pointer}",
                    route.method, route.path
                )
            });

            match route.surface {
                ApiSurface::Public => {
                    public_routes += 1;
                    assert!(
                        !openapi_has_bearer_auth(operation),
                        "public route must not require bearer auth: {} {}",
                        route.method,
                        route.path
                    );
                }
                ApiSurface::Admin => {
                    admin_routes += 1;
                    assert!(
                        openapi_has_bearer_auth(operation),
                        "admin route must require bearer auth: {} {}",
                        route.method,
                        route.path
                    );
                }
                ApiSurface::AgentProtocol => {
                    agent_protocol_routes += 1;
                    assert!(
                        !openapi_has_bearer_auth(operation),
                        "agent protocol route must not use admin bearer auth: {} {}",
                        route.method,
                        route.path
                    );
                }
            }
        }

        assert!(public_routes >= 4);
        assert!(admin_routes >= 30);
        assert_eq!(agent_protocol_routes, 1);
        assert!(
            document.pointer("/paths/~1api~1agents~1ws").is_none(),
            "WebSocket protocol is documented in docs/protocol.md, not REST OpenAPI"
        );
        assert!(
            document.pointer("/paths/~1admin").is_none(),
            "Web Admin static assets are not public REST API"
        );
        let remediation_properties = document
            .pointer("/components/schemas/RemediationRequestResponse/properties")
            .and_then(serde_json::Value::as_object)
            .expect("remediation response schema must expose properties");
        for field in [
            "runbook_document",
            "rendered_body",
            "command_output",
            "secret_value",
            "token",
        ] {
            assert!(
                !remediation_properties.contains_key(field),
                "remediation metadata response must not expose {field}"
            );
        }
        assert!(
            document
                .pointer(
                    "/components/schemas/ApproveRemediationJobRequest/properties/runbook_document"
                )
                .is_some(),
            "approve remediation request must document the request-only runbook document"
        );
    }

    #[test]
    fn openapi_documents_common_error_latest_and_paging_contracts() {
        let document: serde_json::Value = serde_json::from_str(OPENAPI_JSON).unwrap();

        assert_eq!(
            document.pointer("/components/schemas/ErrorResponse/required/0"),
            Some(&serde_json::Value::String("error".to_owned()))
        );
        assert!(
            document
                .pointer("/components/responses/Unauthorized/content/application~1json/schema/$ref")
                .is_some()
        );
        assert!(
            document
                .pointer("/components/responses/Forbidden/content/application~1json/schema/$ref")
                .is_some()
        );
        assert!(
            document
                .pointer("/components/responses/NotFound/content/application~1json/schema/$ref")
                .is_some()
        );
        assert!(
            document
                .pointer("/components/responses/Conflict/content/application~1json/schema/$ref")
                .is_some()
        );

        for path in [
            "/api/agents/{agent_id}/facts/latest",
            "/api/agents/{agent_id}/metrics/latest",
            "/api/agents/{agent_id}/drift/latest",
        ] {
            let pointer = format!(
                "{}/responses/200/content/application~1json/schema/anyOf/1/type",
                openapi_operation_pointer(path, "GET")
            );
            assert_eq!(
                document.pointer(&pointer),
                Some(&serde_json::Value::String("null".to_owned())),
                "{path} latest response must document empty latest result as 200 null"
            );
        }

        for path in [
            "/api/agents/{agent_id}/facts",
            "/api/agents/{agent_id}/metrics",
            "/api/agents/{agent_id}/logs",
            "/api/agents/{agent_id}/drift",
        ] {
            let operation_pointer = openapi_operation_pointer(path, "GET");
            let parameters = document
                .pointer(&format!("{operation_pointer}/parameters"))
                .and_then(serde_json::Value::as_array)
                .unwrap_or_else(|| panic!("{path} must document query parameters"));
            assert!(
                parameters.iter().any(|parameter| {
                    parameter.get("name").and_then(serde_json::Value::as_str) == Some("limit")
                        && parameter
                            .pointer("/schema/minimum")
                            .and_then(serde_json::Value::as_i64)
                            == Some(1)
                        && parameter
                            .pointer("/schema/maximum")
                            .and_then(serde_json::Value::as_i64)
                            == Some(500)
                }),
                "{path} must document bounded limit paging"
            );
            assert!(
                parameters.iter().any(|parameter| {
                    parameter.get("name").and_then(serde_json::Value::as_str) == Some("before")
                }),
                "{path} must document cursor paging"
            );
        }

        for schema in [
            "FactsSnapshotPage",
            "MetricsSnapshotPage",
            "AgentLogChunkPage",
            "DriftReportPage",
        ] {
            assert_eq!(
                document.pointer(&format!("/components/schemas/{schema}/required/0")),
                Some(&serde_json::Value::String("items".to_owned()))
            );
            assert_eq!(
                document.pointer(&format!("/components/schemas/{schema}/required/1")),
                Some(&serde_json::Value::String("next_cursor".to_owned()))
            );
        }

        for (path, method, field) in [
            ("/api/agents/enroll", "POST", "token"),
            ("/api/enrollment-tokens", "POST", "labels"),
            ("/api/jobs/command", "POST", "job_id"),
            ("/api/jobs/runbook", "POST", "runbook_document"),
            ("/api/jobs/drift-check", "POST", "policy_document"),
            ("/api/policies", "POST", "source"),
            ("/api/policies/{policy_id}/assignments", "POST", "agent_id"),
            (
                "/api/policies/{policy_id}/schedules",
                "POST",
                "interval_seconds",
            ),
        ] {
            let pointer = format!(
                "{}/requestBody/content/application~1json/example/{field}",
                openapi_operation_pointer(path, method)
            );
            assert!(
                document.pointer(&pointer).is_some(),
                "{method} {path} must document request example field {field}"
            );
        }
    }

    #[test]
    fn protected_api_requires_admin_token() {
        let store = SqliteStore::in_memory().unwrap();
        let response =
            route_request("POST /api/enrollment-tokens HTTP/1.1\r\n\r\n", &store).unwrap();
        assert!(response.starts_with("HTTP/1.1 401"));
        assert!(response.contains("\"error\":\"unauthorized\""));
    }

    #[test]
    fn latest_optional_resources_return_null_instead_of_not_found() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();

        for path in [
            "/api/agents/agent-missing/facts/latest",
            "/api/agents/agent-missing/metrics/latest",
            "/api/agents/agent-missing/drift/latest",
        ] {
            let response = route_request(
                &format!("GET {path} HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n"),
                &store,
            )
            .unwrap();
            assert!(response.starts_with("HTTP/1.1 200"), "{path}");
            assert_eq!(response.split("\r\n\r\n").nth(1), Some("null\n"));
        }

        let missing_agent = route_request(
            "GET /api/agents/agent-missing HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        assert!(missing_agent.starts_with("HTTP/1.1 404"));
        assert!(missing_agent.contains("\"error\":\"not_found\""));
    }

    #[test]
    fn admin_token_maps_to_bootstrap_actor() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();

        let context = authenticate_admin_request(
            "GET /api/agents HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap()
        .unwrap();

        assert_eq!(context.actor_id, "bootstrap-admin");
        assert_eq!(context.role, AdminRole::Owner);
        assert!(context.allows(AdminPermission::AgentRevoke));
        assert!(context.allows(AdminPermission::EnrollmentTokenCreate));
    }

    #[test]
    fn permission_matrix_allows_operator_execution_but_denies_agent_revoke() {
        let operator = AdminRequestContext {
            actor_id: "operator-1".to_owned(),
            role: AdminRole::Operator,
        };
        let viewer = AdminRequestContext {
            actor_id: "viewer-1".to_owned(),
            role: AdminRole::Viewer,
        };

        assert!(operator.allows(AdminPermission::JobCreate));
        assert!(operator.allows(AdminPermission::JobApprove));
        assert!(operator.allows(AdminPermission::JobCancel));
        assert!(!operator.allows(AdminPermission::AgentRevoke));
        assert!(!operator.allows(AdminPermission::EnrollmentTokenCreate));
        assert!(viewer.allows(AdminPermission::AgentRead));
        assert!(viewer.allows(AdminPermission::JobRead));
        assert!(!viewer.allows(AdminPermission::JobCreate));
        assert!(!viewer.allows(AdminPermission::JobApprove));
    }

    #[test]
    fn forbidden_response_contract_names_required_permission() {
        let response = forbidden_response(AdminPermission::JobApprove);

        assert!(response.starts_with("HTTP/1.1 403 Forbidden"));
        assert!(response.contains("\"error\":\"forbidden\""));
        assert!(response.contains("\"required_permission\":\"job_approve\""));
    }

    #[test]
    fn approval_approve_requires_permission() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash_with_identity(
                &hash_token("viewer-token"),
                "viewer-1",
                "viewer",
            )
            .unwrap();

        let response = route_request(
            "POST /api/approvals/approval-1/approve HTTP/1.1\r\nAuthorization: Bearer viewer-token\r\nContent-Length: 2\r\n\r\n{}",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 403"));
        assert!(response.contains("\"required_permission\":\"job_approve\""));
    }

    #[test]
    fn remediation_approval_requires_job_approve_permission() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash_with_identity(
                &hash_token("viewer-token"),
                "viewer-1",
                "viewer",
            )
            .unwrap();

        let response = route_request(
            "POST /api/remediations/rem-1/approval-request HTTP/1.1\r\nAuthorization: Bearer viewer-token\r\nContent-Length: 2\r\n\r\n{}",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 403"));
        assert!(response.contains("\"required_permission\":\"job_approve\""));
    }

    #[test]
    fn enrollment_token_create_requires_permission() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash_with_identity(
                &hash_token("viewer-token"),
                "viewer-1",
                "viewer",
            )
            .unwrap();

        let response = route_request(
            "POST /api/enrollment-tokens HTTP/1.1\r\nAuthorization: Bearer viewer-token\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 403"));
        assert!(response.contains("\"required_permission\":\"enrollment_token_create\""));
    }

    #[test]
    fn agent_revoke_requires_permission() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash_with_identity(
                &hash_token("viewer-token"),
                "viewer-1",
                "viewer",
            )
            .unwrap();

        let response = route_request(
            "POST /api/agents/agent-1/revoke-key HTTP/1.1\r\nAuthorization: Bearer viewer-token\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 403"));
        assert!(response.contains("\"required_permission\":\"agent_revoke\""));
    }

    #[test]
    fn admin_token_can_create_enrollment_token() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();

        let response = route_request(
            "POST /api/enrollment-tokens HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert!(response.contains("\"token\":\"enroll-"));
        assert_eq!(store.list_enrollment_tokens().unwrap().len(), 1);
        let audits = store
            .list_audit_events_by_category(AuditCategory::Enrollment, 10)
            .unwrap();
        assert_eq!(audits[0].actor.as_str(), "bootstrap-admin");
    }

    #[test]
    fn custom_admin_token_actor_is_used_in_audit() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash_with_identity(
                &hash_token("admin-token"),
                "ops-admin-1",
                "admin",
            )
            .unwrap();

        let response = route_request(
            "POST /api/enrollment-tokens HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Enrollment, 10)
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert_eq!(audits[0].actor.as_str(), "ops-admin-1");
    }

    #[test]
    fn admin_token_create_enrollment_token_accepts_scope() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();

        let response = route_request(
            "POST /api/enrollment-tokens HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n{\"labels\":\"role=web,env=prod\",\"max_uses\":3,\"expires_in_seconds\":900}",
            &store,
        )
        .unwrap();
        let records = store.list_enrollment_tokens().unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert!(response.contains("\"expires_in_seconds\":900"));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].default_labels, "role=web,env=prod");
        assert_eq!(records[0].max_uses, 3);
    }

    #[test]
    fn admin_token_create_enrollment_token_rejects_invalid_scope() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();

        let response = route_request(
            "POST /api/enrollment-tokens HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n{\"max_uses\":0}",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 400"));
        assert!(response.contains("max_uses"));
        assert_eq!(store.list_enrollment_tokens().unwrap().len(), 0);
    }

    #[test]
    fn enrollment_token_create_is_audited_without_raw_token() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();

        let response = route_request(
            "POST /api/enrollment-tokens HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Enrollment, 10)
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].action, "enrollment_token_created");
        assert!(!audits[0].contains_secret_plaintext());
    }

    #[test]
    fn enrollment_token_revoke_is_audited() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        store
            .insert_enrollment_token_hash(
                "et-1",
                &hash_token("enroll-token"),
                "",
                SystemTime::now() + Duration::from_secs(60),
                1,
            )
            .unwrap();

        let response = route_request(
            "DELETE /api/enrollment-tokens/et-1 HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Enrollment, 10)
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 204"));
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].action, "enrollment_token_revoked");
    }

    #[test]
    fn admin_can_create_command_job_with_signed_assignment() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        let body = serde_json::to_string(&CreateCommandJobRequest {
            job_id: "job-1".to_owned(),
            target_agent_ids: vec!["agent-1".to_owned()],
            selector: None,
            match_labels: None,
            strategy: None,
            program: "uptime".to_owned(),
            args: Vec::new(),
            timeout_seconds: 30,
            confirmed_high_risk: true,
            confirmed_by: "operator-1".to_owned(),
            expires_in_seconds: 60,
            nonce_prefix: Some("nonce".to_owned()),
        })
        .unwrap();
        let request = format!(
            "POST /api/jobs/command HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = route_request(&request, &store).unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Job, 10)
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert!(response.contains("\"target_count\":1"));
        assert!(response.contains("\"assignment_count\":1"));
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].action, "job_created");
        assert_eq!(audits[0].actor.as_str(), "bootstrap-admin");
        assert_eq!(
            audits[0].value,
            AuditValue::Plain(
                "confirmed_high_risk=true,confirmed_by=bootstrap-admin,target_count=1".to_owned()
            )
        );
    }

    #[test]
    fn dispatch_created_command_job_to_active_session_immediately() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        let (handle, mut receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            vec!["persistent_session".to_owned()],
            Some(64),
        );
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        sessions.lock().unwrap().register(handle);
        let request = command_job_request("job-1", "agent-1", true);

        let response = route_request_with_sessions(&request, &store, &sessions).unwrap();
        let sent = receiver
            .try_recv()
            .expect("task assignment should be queued");
        let status = store.find_job_status_value("job-1").unwrap().unwrap();
        let task_id = match &sent {
            AgentSessionOutboundMessage::Wire(message) => match &message.payload {
                fleet_protocol::WirePayload::TaskAssignment { envelope, .. } => {
                    envelope.task_id.clone()
                }
                _ => panic!("expected task assignment"),
            },
            _ => panic!("expected task assignment"),
        };
        let assignment_status = store
            .find_task_assignment_status(&task_id)
            .unwrap()
            .unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Job, 10)
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert!(matches!(
            sent,
            AgentSessionOutboundMessage::Wire(message)
                if matches!(&message.payload, fleet_protocol::WirePayload::TaskAssignment { .. })
        ));
        assert_eq!(status, "running");
        assert_eq!(assignment_status, "dispatched");
        assert!(audits.iter().any(|event| event.action == "job_created"));
        assert!(audits.iter().any(|event| event.action == "task_dispatched"));
    }

    #[test]
    fn dispatch_rejects_unsupported_capability_without_websocket_write() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        store
            .save_agent_capability_snapshot(
                "agent-1",
                AgentCapabilitySnapshot::reported(
                    AgentRuntimeProfile::new(PrivilegeLevel::Unprivileged, None, None, Vec::new()),
                    SystemTime::UNIX_EPOCH,
                ),
            )
            .unwrap();
        let (handle, mut receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            vec!["persistent_session".to_owned()],
            Some(64),
        );
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        sessions.lock().unwrap().register(handle);

        let response = route_request_with_sessions(
            &command_job_request("job-capability", "agent-1", true),
            &store,
            &sessions,
        )
        .unwrap();
        let assignment = store
            .find_task_assignment_state_for_job("job-capability")
            .unwrap()
            .unwrap();
        let assignment_summary = store
            .list_task_assignment_summaries_for_job("job-capability")
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Job, 10)
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert_eq!(assignment.status, "rejected");
        assert!(
            assignment_summary
                .last_error
                .contains("capability_unsupported")
        );
        assert!(receiver.try_recv().is_err());
        assert!(
            audits
                .iter()
                .any(|event| event.action == "assignment_rejected_capability")
        );
    }

    #[test]
    fn dispatch_created_runbook_and_drift_jobs_use_active_session_service() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-runbook");
        save_test_agent(&store, "agent-drift");
        let (runbook_handle, mut runbook_receiver) = session_handle(
            "agent-runbook",
            "conn-runbook",
            SystemTime::UNIX_EPOCH,
            vec!["persistent_session".to_owned()],
            Some(64),
        );
        let (drift_handle, mut drift_receiver) = session_handle(
            "agent-drift",
            "conn-drift",
            SystemTime::UNIX_EPOCH,
            vec!["persistent_session".to_owned()],
            Some(64),
        );
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        {
            let mut registry = sessions.lock().unwrap();
            registry.register(runbook_handle);
            registry.register(drift_handle);
        }

        let runbook_response = route_request_with_sessions(
            &runbook_job_request("job-runbook", "agent-runbook"),
            &store,
            &sessions,
        )
        .unwrap();
        let drift_response = route_request_with_sessions(
            &drift_check_job_request("job-drift", "agent-drift"),
            &store,
            &sessions,
        )
        .unwrap();
        approve_pending_job_with_sessions(&store, &sessions, "job-runbook");

        assert!(runbook_response.starts_with("HTTP/1.1 201"));
        assert!(drift_response.starts_with("HTTP/1.1 201"));
        assert!(matches!(
            runbook_receiver.try_recv().unwrap(),
            AgentSessionOutboundMessage::Wire(message)
                if matches!(
                    &message.payload,
                    fleet_protocol::WirePayload::TaskAssignment {
                        task: fleet_protocol::TaskWire::RunbookExecution(_),
                        ..
                    }
                )
        ));
        assert!(matches!(
            drift_receiver.try_recv().unwrap(),
            AgentSessionOutboundMessage::Wire(message)
                if matches!(
                    &message.payload,
                    fleet_protocol::WirePayload::TaskAssignment {
                        task: fleet_protocol::TaskWire::DriftCheck(_),
                        ..
                    }
                )
        ));
        assert_eq!(
            store.find_job_status_value("job-runbook").unwrap().unwrap(),
            "running"
        );
        assert_eq!(
            store.find_job_status_value("job-drift").unwrap().unwrap(),
            "running"
        );
    }

    #[test]
    fn dispatch_created_command_job_keeps_disconnected_agent_queued() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        let request = command_job_request("job-queued", "agent-1", true);

        let response = route_request_with_sessions(&request, &store, &sessions).unwrap();
        let status = store.find_job_status_value("job-queued").unwrap().unwrap();
        let pending = store
            .list_pending_dispatch_assignments(
                Some(&AgentId::new("agent-1").unwrap()),
                Some(&JobId::new("job-queued").unwrap()),
                10,
            )
            .unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Job, 10)
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert_eq!(status, "queued");
        assert_eq!(pending.len(), 1);
        assert!(!audits.iter().any(|event| event.action == "task_dispatched"));
    }

    #[test]
    fn fanout_command_job_creates_assignment_per_target_snapshot() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent_with_labels(&store, "agent-1", vec![("role", "web")]);
        save_test_agent_with_labels(&store, "agent-2", vec![("role", "web")]);
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        let request = command_selector_job_request(
            "job-fanout",
            "label:role=web",
            Some(JobStrategyRequest {
                concurrency: Some(2),
                max_failures: Some(1),
            }),
        );

        let response = route_request_with_sessions(&request, &store, &sessions).unwrap();
        let summary = store.find_job_summary("job-fanout").unwrap().unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert!(response.contains("\"target_count\":2"));
        assert!(response.contains("\"assignment_count\":2"));
        assert_eq!(summary.target_count, 2);
        assert_eq!(summary.strategy_concurrency, 2);
        assert_eq!(summary.strategy_max_failures, Some(1));
        assert_eq!(
            summary
                .target_agents
                .iter()
                .filter(|target| target.assignment_status.as_deref() == Some("queued"))
                .count(),
            2
        );
    }

    #[test]
    fn fanout_concurrency_one_dispatches_only_one_active_target() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent_with_labels(&store, "agent-1", vec![("role", "web")]);
        save_test_agent_with_labels(&store, "agent-2", vec![("role", "web")]);
        let (handle_1, mut receiver_1) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            vec!["persistent_session".to_owned()],
            Some(64),
        );
        let (handle_2, mut receiver_2) = session_handle(
            "agent-2",
            "conn-2",
            SystemTime::UNIX_EPOCH,
            vec!["persistent_session".to_owned()],
            Some(64),
        );
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        {
            let mut registry = sessions.lock().unwrap();
            registry.register(handle_1);
            registry.register(handle_2);
        }
        let request = command_selector_job_request(
            "job-concurrency-one",
            "label:role=web",
            Some(JobStrategyRequest {
                concurrency: Some(1),
                max_failures: None,
            }),
        );

        let response = route_request_with_sessions(&request, &store, &sessions).unwrap();
        approve_pending_job_with_sessions(&store, &sessions, "job-concurrency-one");
        let first = receiver_1.try_recv().ok();
        let second = receiver_2.try_recv().ok();
        let delivered_count = usize::from(first.is_some()) + usize::from(second.is_some());
        let summary = store
            .find_job_summary("job-concurrency-one")
            .unwrap()
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert_eq!(delivered_count, 1);
        assert_eq!(
            summary
                .target_agents
                .iter()
                .filter(|target| target.assignment_status.as_deref() == Some("dispatched"))
                .count(),
            1
        );
        assert_eq!(
            summary
                .target_agents
                .iter()
                .filter(|target| target.assignment_status.as_deref() == Some("queued"))
                .count(),
            1
        );
    }

    #[test]
    fn fanout_concurrency_n_dispatches_at_most_n_targets() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        for agent_id in ["agent-1", "agent-2", "agent-3"] {
            save_test_agent_with_labels(&store, agent_id, vec![("role", "web")]);
        }
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        let mut receivers = Vec::new();
        for agent_id in ["agent-1", "agent-2", "agent-3"] {
            let (handle, receiver) = session_handle(
                agent_id,
                &format!("conn-{agent_id}"),
                SystemTime::UNIX_EPOCH,
                vec!["persistent_session".to_owned()],
                Some(64),
            );
            sessions.lock().unwrap().register(handle);
            receivers.push(receiver);
        }
        let request = command_selector_job_request(
            "job-concurrency-two",
            "label:role=web",
            Some(JobStrategyRequest {
                concurrency: Some(2),
                max_failures: None,
            }),
        );

        let response = route_request_with_sessions(&request, &store, &sessions).unwrap();
        approve_pending_job_with_sessions(&store, &sessions, "job-concurrency-two");
        let mut delivered_count = 0;
        for receiver in &mut receivers {
            if receiver.try_recv().is_ok() {
                delivered_count += 1;
            }
        }

        assert!(response.starts_with("HTTP/1.1 201"));
        assert_eq!(delivered_count, 2);
        assert_eq!(
            store
                .find_job_summary("job-concurrency-two")
                .unwrap()
                .unwrap()
                .target_agents
                .iter()
                .filter(|target| target.assignment_status.as_deref() == Some("queued"))
                .count(),
            1
        );
    }

    #[test]
    fn dispatch_failure_keeps_assignment_queued_and_audits() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        let (handle, _receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            vec!["persistent_session".to_owned()],
            Some(1),
        );
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        {
            let mut registry = sessions.lock().unwrap();
            registry.register(handle);
            registry
                .try_send("agent-1", task_assignment_wire_message("agent-1"))
                .unwrap();
        }
        let request = command_job_request("job-fail-dispatch", "agent-1", true);

        let response = route_request_with_sessions(&request, &store, &sessions).unwrap();
        let status = store
            .find_job_status_value("job-fail-dispatch")
            .unwrap()
            .unwrap();
        let pending = store
            .list_pending_dispatch_assignments(
                Some(&AgentId::new("agent-1").unwrap()),
                Some(&JobId::new("job-fail-dispatch").unwrap()),
                10,
            )
            .unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Job, 10)
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert_eq!(status, "queued");
        assert_eq!(pending.len(), 1);
        assert!(audits.iter().any(|event| {
            event.action == "task_dispatch_failed"
                && matches!(&event.value, AuditValue::Plain(value) if value.contains("failure_reason="))
        }));
    }

    #[test]
    fn dispatch_reconnect_drains_one_queued_assignment_for_agent() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        route_request(
            &command_job_request("job-reconnect", "agent-1", true),
            &store,
        )
        .unwrap();
        let (handle, mut receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            vec!["persistent_session".to_owned()],
            Some(64),
        );
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        sessions.lock().unwrap().register(handle);

        let output = dispatch_pending_assignments(
            &store,
            &sessions,
            Some(AgentId::new("agent-1").unwrap()),
            None,
            1,
        )
        .unwrap();
        let sent = receiver.try_recv().expect("queued assignment should drain");

        assert_eq!(output.dispatched_count, 1);
        assert!(matches!(
            sent,
            AgentSessionOutboundMessage::Wire(message)
                if matches!(&message.payload, fleet_protocol::WirePayload::TaskAssignment { .. })
        ));
        assert_eq!(
            store
                .find_job_status_value("job-reconnect")
                .unwrap()
                .unwrap(),
            "running"
        );
    }

    #[test]
    fn dispatch_skips_revoked_agent_even_with_active_session() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_disabled_test_agent_with_labels(&store, "agent-revoked", vec![("role", "web")]);
        let (handle, mut receiver) = session_handle(
            "agent-revoked",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            vec!["persistent_session".to_owned()],
            Some(64),
        );
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        sessions.lock().unwrap().register(handle);
        let request = command_job_request("job-revoked", "agent-revoked", true);

        let response = route_request_with_sessions(&request, &store, &sessions).unwrap();
        let output = receiver.try_recv();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert!(output.is_err());
        assert_eq!(
            store.find_job_status_value("job-revoked").unwrap().unwrap(),
            "queued"
        );
    }

    #[test]
    fn dispatch_does_not_bypass_pending_approval() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        let (handle, mut receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            vec!["persistent_session".to_owned()],
            Some(64),
        );
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        sessions.lock().unwrap().register(handle);

        let body = serde_json::to_string(&CreateCommandJobRequest {
            job_id: "job-needs-approval".to_owned(),
            target_agent_ids: vec!["agent-1".to_owned()],
            selector: None,
            match_labels: None,
            strategy: None,
            program: "bash".to_owned(),
            args: vec!["-lc".to_owned(), "uptime".to_owned()],
            timeout_seconds: 30,
            confirmed_high_risk: false,
            confirmed_by: "operator-1".to_owned(),
            expires_in_seconds: 60,
            nonce_prefix: Some("nonce-needs-approval".to_owned()),
        })
        .unwrap();
        let request = format!(
            "POST /api/jobs/command HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = route_request_with_sessions(&request, &store, &sessions).unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert!(response.contains("\"status\":\"pending_approval\""));
        assert!(receiver.try_recv().is_err());
        assert_eq!(
            store
                .find_job_status_value("job-needs-approval")
                .unwrap()
                .unwrap(),
            "pending_approval"
        );
        assert!(
            store
                .find_pending_approval_for_job("job-needs-approval")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn admin_can_list_and_approve_pending_approval_then_dispatch() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        let (handle, mut receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            vec!["persistent_session".to_owned()],
            Some(64),
        );
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        sessions.lock().unwrap().register(handle);

        let create_response = route_request_with_sessions(
            &high_risk_command_job_request("job-approval", "agent-1", "bash"),
            &store,
            &sessions,
        )
        .unwrap();
        let list_response = route_request(
            "GET /api/approvals?status=pending HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        let approval = store
            .find_pending_approval_for_job("job-approval")
            .unwrap()
            .expect("pending approval should exist");
        let body = "{\"reason\":\"approved maintenance window\"}".to_owned();
        let approve_request = format!(
            "POST /api/approvals/{}/approve HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            approval.id,
            body.len(),
            body
        );

        let approve_response =
            route_request_with_sessions(&approve_request, &store, &sessions).unwrap();
        let sent = receiver
            .try_recv()
            .expect("approved assignment should dispatch over active session");
        let audits = store
            .list_audit_events_by_category(AuditCategory::Approval, 10)
            .unwrap();

        assert!(create_response.starts_with("HTTP/1.1 201"));
        assert!(create_response.contains("\"status\":\"pending_approval\""));
        assert!(list_response.starts_with("HTTP/1.1 200"));
        assert!(list_response.contains("\"status\":\"pending\""));
        assert!(approve_response.starts_with("HTTP/1.1 200"));
        assert!(approve_response.contains("\"status\":\"approved\""));
        assert!(approve_response.contains("\"approver\":\"bootstrap-admin\""));
        assert!(matches!(
            sent,
            AgentSessionOutboundMessage::Wire(message)
                if matches!(&message.payload, fleet_protocol::WirePayload::TaskAssignment { .. })
        ));
        assert_eq!(
            store
                .find_job_status_value("job-approval")
                .unwrap()
                .unwrap(),
            "running"
        );
        assert!(
            audits
                .iter()
                .any(|event| event.action == "approval_requested")
        );
        assert!(
            audits
                .iter()
                .any(|event| event.action == "approval_approved"
                    && event.actor.as_str() == "bootstrap-admin")
        );
    }

    #[test]
    fn admin_can_reject_pending_approval_without_dispatch() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        route_request(
            &high_risk_command_job_request("job-reject-approval", "agent-1", "bash"),
            &store,
        )
        .unwrap();
        let approval = store
            .find_pending_approval_for_job("job-reject-approval")
            .unwrap()
            .expect("pending approval should exist");
        let body = serde_json::to_string(&ApprovalDecisionRequest {
            actor: "manager-1".to_owned(),
            reason: "outside maintenance window".to_owned(),
        })
        .unwrap();
        let reject_request = format!(
            "POST /api/approvals/{}/reject HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            approval.id,
            body.len(),
            body
        );

        let response = route_request(&reject_request, &store).unwrap();
        let pending = store
            .list_pending_command_assignments_for_agent("agent-1")
            .unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Approval, 10)
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"status\":\"rejected\""));
        assert_eq!(
            store
                .find_job_status_value("job-reject-approval")
                .unwrap()
                .unwrap(),
            "failed"
        );
        assert!(pending.is_empty());
        assert!(
            audits
                .iter()
                .any(|event| event.action == "approval_rejected"
                    && event.actor.as_str() == "bootstrap-admin")
        );
    }

    #[test]
    fn admin_can_expire_due_approval_requests() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_job(&store, "job-expire-approval");
        store
            .insert_approval_request(AppApprovalRequestRecord {
                id: "approval-expired".to_owned(),
                job_id: "job-expire-approval".to_owned(),
                requester: "admin".to_owned(),
                approver: None,
                reason: "approval required".to_owned(),
                status: "pending".to_owned(),
                expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
                created_at: SystemTime::UNIX_EPOCH,
                decided_at: None,
            })
            .unwrap();

        let response = route_request(
            "POST /api/approvals/expire HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Approval, 10)
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"expired_count\":1"));
        assert!(response.contains("\"status\":\"expired\""));
        assert_eq!(
            store
                .find_job_status_value("job-expire-approval")
                .unwrap()
                .unwrap(),
            "expired"
        );
        assert!(
            audits
                .iter()
                .any(|event| event.action == "approval_expired")
        );
    }

    #[test]
    fn admin_can_list_and_get_remediation_metadata_without_payload_bodies() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        save_test_agent(&store, "agent-2");
        store
            .save_remediation_request_record(&controller_remediation_request_record(
                "rem-1",
                "agent-1",
                "nginx-running",
                "proposed",
                None,
            ))
            .unwrap();
        store
            .save_remediation_request_record(&controller_remediation_request_record(
                "rem-other",
                "agent-2",
                "ssh-running",
                "proposed",
                None,
            ))
            .unwrap();

        let list_response = route_request(
            "GET /api/remediations?agent_id=agent-1&policy_id=nginx-running&limit=10 HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        let detail_response = route_request(
            "GET /api/remediations/rem-1 HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        let missing_response = route_request(
            "GET /api/remediations/missing HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        let bad_limit_response = route_request(
            "GET /api/remediations?limit=0 HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(list_response.starts_with("HTTP/1.1 200"));
        assert!(list_response.contains("\"id\":\"rem-1\""));
        assert!(!list_response.contains("rem-other"));
        assert!(detail_response.starts_with("HTTP/1.1 200"));
        assert!(detail_response.contains("\"runbook_ref\":\"runbooks/remediate.yml\""));
        assert!(missing_response.starts_with("HTTP/1.1 404"));
        assert!(bad_limit_response.starts_with("HTTP/1.1 400"));
        assert_remediation_surface_excludes_payloads(&list_response);
        assert_remediation_surface_excludes_payloads(&detail_response);
    }

    #[test]
    fn admin_can_request_approval_and_approve_remediation_job_without_body_leak() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        store
            .save_remediation_request_record(&controller_remediation_request_record(
                "rem-approve",
                "agent-1",
                "nginx-running",
                "proposed",
                None,
            ))
            .unwrap();
        let approval_body = serde_json::json!({
            "approval_id": "approval-rem-approve",
            "job_id": "job-rem-approve",
            "reason": "approved maintenance window",
            "expires_in_seconds": 600
        })
        .to_string();
        let approve_body = serde_json::json!({
            "approval_id": "approval-rem-approve",
            "job_id": "job-rem-approve",
            "runbook_document": remediation_runbook_document_with_secret_marker(),
            "timeout_seconds": 30,
            "expires_in_seconds": 600,
            "nonce_prefix": "nonce-rem-approve",
            "reason": "approved remediation"
        })
        .to_string();

        let approval_response = route_request(
            &admin_json_post(
                "/api/remediations/rem-approve/approval-request",
                &approval_body,
            ),
            &store,
        )
        .unwrap();
        assert!(
            approval_response.starts_with("HTTP/1.1 201"),
            "{approval_response}"
        );
        assert!(approval_response.contains("\"status\":\"pending_approval\""));
        let approve_response = route_request(
            &admin_json_post("/api/remediations/rem-approve/approve", &approve_body),
            &store,
        )
        .unwrap();
        assert!(
            approve_response.starts_with("HTTP/1.1 200"),
            "{approve_response}"
        );
        assert!(approve_response.contains("\"status\":\"job_created\""));
        assert!(approve_response.contains("\"assignment_count\":1"));
        let remediation = store
            .find_remediation_request_record("rem-approve")
            .unwrap()
            .unwrap();
        let approval = store
            .find_approval_request("approval-rem-approve")
            .unwrap()
            .unwrap();
        let assignments = store
            .list_pending_runbook_assignments_for_agent("agent-1")
            .unwrap();
        let audits = store.list_audit_events(20).unwrap();

        assert_eq!(remediation.status, "job_created");
        assert_eq!(remediation.job_id.as_deref(), Some("job-rem-approve"));
        assert_eq!(approval.status, "approved");
        assert_eq!(approval.approver.as_deref(), Some("bootstrap-admin"));
        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].envelope.job_id.as_str(), "job-rem-approve");
        assert!(assignments[0].envelope.signature.is_some());
        assert_remediation_surface_excludes_payloads(&approval_response);
        assert_remediation_surface_excludes_payloads(&approve_response);
        assert_audit_values_exclude_payloads(&audits);
    }

    #[test]
    fn remediation_api_rejects_terminal_approval_request() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        store
            .save_remediation_request_record(&controller_remediation_request_record(
                "rem-resolved",
                "agent-1",
                "nginx-running",
                "resolved",
                Some("job-rem-resolved"),
            ))
            .unwrap();
        let body = serde_json::json!({
            "approval_id": "approval-rem-resolved",
            "job_id": "job-rem-resolved",
            "reason": "should be rejected",
            "expires_in_seconds": 600
        })
        .to_string();

        let response = route_request(
            &admin_json_post("/api/remediations/rem-resolved/approval-request", &body),
            &store,
        )
        .unwrap();
        let remediation = store
            .find_remediation_request_record("rem-resolved")
            .unwrap()
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 400"));
        assert!(response.contains("InvalidTransition"));
        assert_eq!(remediation.status, "resolved");
    }

    #[test]
    fn admin_can_record_remediation_result_and_verify_resolution() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        store
            .save_remediation_request_record(&controller_remediation_request_record(
                "rem-result",
                "agent-1",
                "nginx-running",
                "job_created",
                Some("job-rem-result"),
            ))
            .unwrap();
        store
            .insert_drift_report(
                "agent-1",
                &DriftReport::drifted("nginx-running", "service nginx running", "stopped"),
                SystemTime::UNIX_EPOCH + Duration::from_secs(10),
            )
            .unwrap();
        let running_body = serde_json::json!({
            "job_id": "job-rem-result"
        })
        .to_string();
        let result_body = serde_json::json!({
            "job_id": "job-rem-result",
            "status": "succeeded"
        })
        .to_string();
        let verify_body = serde_json::json!({
            "agent_id": "agent-1",
            "policy_id": "nginx-running",
            "policy_name": "nginx-running",
            "job_id": "job-rem-result"
        })
        .to_string();

        let running_response = route_request(
            &admin_json_post("/api/remediations/rem-result/running", &running_body),
            &store,
        )
        .unwrap();
        let result_response = route_request(
            &admin_json_post("/api/remediations/rem-result/result", &result_body),
            &store,
        )
        .unwrap();
        let before_verify = store.latest_drift_report("agent-1").unwrap().unwrap();
        let verify_response = route_request(
            &admin_json_post("/api/remediations/rem-result/verify", &verify_body),
            &store,
        )
        .unwrap();
        let remediation = store
            .find_remediation_request_record("rem-result")
            .unwrap()
            .unwrap();
        let drift = store.latest_drift_report("agent-1").unwrap().unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Policy, 10)
            .unwrap();

        assert!(running_response.starts_with("HTTP/1.1 200"));
        assert!(running_response.contains("\"status\":\"running\""));
        assert!(result_response.starts_with("HTTP/1.1 200"));
        assert!(result_response.contains("\"status\":\"succeeded_pending_verify\""));
        assert!(matches!(
            &before_verify.report.acknowledgement,
            DriftAcknowledgement::Open
        ));
        assert!(verify_response.starts_with("HTTP/1.1 200"));
        assert!(verify_response.contains("\"status\":\"resolved\""));
        assert_eq!(remediation.status, "resolved");
        assert!(matches!(
            &drift.report.acknowledgement,
            DriftAcknowledgement::Resolved { job_id, .. } if job_id == "job-rem-result"
        ));
        assert!(
            audits
                .iter()
                .any(|event| event.action == "remediation_resolved")
        );
    }

    #[test]
    fn remediation_verify_rejects_mismatched_evidence_without_state_change() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        store
            .save_remediation_request_record(&controller_remediation_request_record(
                "rem-mismatch",
                "agent-1",
                "nginx-running",
                "succeeded_pending_verify",
                Some("job-rem-mismatch"),
            ))
            .unwrap();
        store
            .insert_drift_report(
                "agent-1",
                &DriftReport::drifted("nginx-running", "service nginx running", "stopped"),
                SystemTime::UNIX_EPOCH + Duration::from_secs(10),
            )
            .unwrap();
        let verify_body = serde_json::json!({
            "agent_id": "agent-1",
            "policy_id": "wrong-policy",
            "policy_name": "nginx-running",
            "job_id": "job-rem-mismatch"
        })
        .to_string();

        let response = route_request(
            &admin_json_post("/api/remediations/rem-mismatch/verify", &verify_body),
            &store,
        )
        .unwrap();
        let remediation = store
            .find_remediation_request_record("rem-mismatch")
            .unwrap()
            .unwrap();
        let drift = store.latest_drift_report("agent-1").unwrap().unwrap();

        assert!(response.starts_with("HTTP/1.1 400"));
        assert!(response.contains("remediation evidence mismatch: policy_id"));
        assert_eq!(remediation.status, "succeeded_pending_verify");
        assert!(matches!(
            &drift.report.acknowledgement,
            DriftAcknowledgement::Open
        ));
    }

    #[test]
    fn admin_can_create_runbook_job_with_signed_assignment() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        let body = serde_json::to_string(&CreateRunbookJobRequest {
            job_id: "job-runbook-1".to_owned(),
            target_agent_ids: vec!["agent-1".to_owned()],
            selector: None,
            match_labels: None,
            strategy: None,
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
            timeout_seconds: 30,
            confirmed_high_risk: true,
            confirmed_by: "operator-1".to_owned(),
            expires_in_seconds: 60,
            nonce_prefix: Some("nonce-runbook".to_owned()),
        })
        .unwrap();
        let request = format!(
            "POST /api/jobs/runbook HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = route_request(&request, &store).unwrap();
        let assignments = store
            .list_pending_runbook_assignments_for_agent("agent-1")
            .unwrap();
        let approval_audits = store
            .list_audit_events_by_category(AuditCategory::Approval, 10)
            .unwrap();
        let job_audits = store
            .list_audit_events_by_category(AuditCategory::Job, 10)
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert!(response.contains("\"target_count\":1"));
        assert!(response.contains("\"status\":\"pending_approval\""));
        assert_eq!(assignments.len(), 0);
        assert_eq!(
            store
                .find_job_status_value("job-runbook-1")
                .unwrap()
                .unwrap(),
            "pending_approval"
        );
        assert_eq!(approval_audits.len(), 1);
        assert_eq!(approval_audits[0].action, "approval_requested");
        assert_eq!(job_audits.len(), 1);
        assert_eq!(job_audits[0].action, "runbook_job_created");
    }

    #[test]
    fn runbook_job_uses_document_selector_when_request_has_no_target() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent_with_labels(&store, "agent-web", vec![("role", "web")]);
        save_test_agent_with_labels(&store, "agent-db", vec![("role", "db")]);
        let body = serde_json::to_string(&CreateRunbookJobRequest {
            job_id: "job-runbook-selector".to_owned(),
            target_agent_ids: Vec::new(),
            selector: None,
            match_labels: None,
            strategy: None,
            runbook_document: r#"
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Runbook
name: nginx-basic
matchLabels:
  role: web
steps:
  - id: nginx-package
    package:
      name: nginx
      state: present
"#
            .to_owned(),
            timeout_seconds: 30,
            confirmed_high_risk: true,
            confirmed_by: "operator-1".to_owned(),
            expires_in_seconds: 60,
            nonce_prefix: Some("nonce-runbook-selector".to_owned()),
        })
        .unwrap();
        let request = format!(
            "POST /api/jobs/runbook HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = route_request(&request, &store).unwrap();
        let summary = store
            .find_job_summary("job-runbook-selector")
            .unwrap()
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert!(response.contains("\"target_count\":1"));
        assert_eq!(summary.selector_kind, "runbook_matchLabels");
        assert!(summary.selector_source.contains("\"role\":\"web\""));
    }

    #[test]
    fn command_job_can_target_agents_by_selector() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent_with_labels(&store, "agent-1", vec![("role", "web")]);
        let body = serde_json::to_string(&CreateCommandJobRequest {
            job_id: "job-1".to_owned(),
            target_agent_ids: Vec::new(),
            selector: Some("role=web".to_owned()),
            match_labels: None,
            strategy: None,
            program: "uptime".to_owned(),
            args: Vec::new(),
            timeout_seconds: 30,
            confirmed_high_risk: true,
            confirmed_by: "operator-1".to_owned(),
            expires_in_seconds: 60,
            nonce_prefix: Some("nonce".to_owned()),
        })
        .unwrap();
        let request = format!(
            "POST /api/jobs/command HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = route_request(&request, &store).unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert!(response.contains("\"target_count\":1"));
        assert_eq!(
            store
                .list_pending_command_assignments_for_agent("agent-1")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn selector_preview_reports_matches_and_warnings() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent_with_labels(&store, "agent-enabled", vec![("role", "web")]);
        save_disabled_test_agent_with_labels(&store, "agent-disabled", vec![("role", "web")]);
        let body = serde_json::to_string(&SelectorPreviewRequest {
            selector: Some("label:role=web".to_owned()),
            match_labels: None,
        })
        .unwrap();
        let request = format!(
            "POST /api/selectors/preview HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = route_request(&request, &store).unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"matched_count\":2"));
        assert!(response.contains("\"selected_count\":1"));
        assert!(response.contains("\"disabled_agents_excluded\""));
        assert!(response.contains("\"selected_for_dispatch\":false"));
    }

    #[test]
    fn command_job_can_target_agents_by_match_labels_selector() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent_with_labels(&store, "agent-1", vec![("role", "web"), ("env", "prod")]);
        save_test_agent_with_labels(&store, "agent-2", vec![("role", "web"), ("env", "dev")]);
        let mut match_labels = BTreeMap::new();
        match_labels.insert("role".to_owned(), "web".to_owned());
        match_labels.insert("env".to_owned(), "prod".to_owned());
        let body = serde_json::to_string(&CreateCommandJobRequest {
            job_id: "job-match-labels".to_owned(),
            target_agent_ids: Vec::new(),
            selector: None,
            match_labels: Some(match_labels),
            strategy: None,
            program: "uptime".to_owned(),
            args: Vec::new(),
            timeout_seconds: 30,
            confirmed_high_risk: true,
            confirmed_by: "operator-1".to_owned(),
            expires_in_seconds: 60,
            nonce_prefix: Some("nonce-match-labels".to_owned()),
        })
        .unwrap();
        let request = format!(
            "POST /api/jobs/command HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = route_request(&request, &store).unwrap();
        let summary = store.find_job_summary("job-match-labels").unwrap().unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert_eq!(summary.selector_kind, "matchLabels");
        assert!(summary.selector_source.contains("\"env\":\"prod\""));
        assert_eq!(
            store
                .list_pending_command_assignments_for_agent("agent-1")
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .list_pending_command_assignments_for_agent("agent-2")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn admin_can_create_drift_check_job_with_signed_assignment() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent_with_labels(&store, "agent-1", vec![("role", "web")]);
        let policy_document = r#"
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
"#;
        let body = serde_json::to_string(&CreateDriftCheckJobRequest {
            job_id: "drift-job-1".to_owned(),
            target_agent_ids: Vec::new(),
            selector: Some("role=web".to_owned()),
            match_labels: None,
            strategy: None,
            policy_document: policy_document.to_owned(),
            timeout_seconds: 30,
            created_by: "operator-1".to_owned(),
            expires_in_seconds: 60,
            nonce_prefix: Some("nonce-drift".to_owned()),
        })
        .unwrap();
        let request = format!(
            "POST /api/jobs/drift-check HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = route_request(&request, &store).unwrap();
        let assignments = store
            .list_pending_drift_check_assignments_for_agent("agent-1")
            .unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Drift, 10)
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert!(response.contains("\"target_count\":1"));
        assert!(response.contains("\"assignment_count\":1"));
        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].envelope.job_id.as_str(), "drift-job-1");
        assert!(
            assignments[0]
                .drift_check
                .policy_document()
                .contains("nginx-running")
        );
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].action, "drift_check_job_created");
        assert_eq!(audits[0].actor.as_str(), "bootstrap-admin");
    }

    #[test]
    fn command_job_selector_excludes_disabled_agents() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent_with_labels(&store, "agent-enabled", vec![("role", "web")]);
        save_disabled_test_agent_with_labels(&store, "agent-disabled", vec![("role", "web")]);
        let body = serde_json::to_string(&CreateCommandJobRequest {
            job_id: "job-1".to_owned(),
            target_agent_ids: Vec::new(),
            selector: Some("role=web".to_owned()),
            match_labels: None,
            strategy: None,
            program: "uptime".to_owned(),
            args: Vec::new(),
            timeout_seconds: 30,
            confirmed_high_risk: true,
            confirmed_by: "operator-1".to_owned(),
            expires_in_seconds: 60,
            nonce_prefix: Some("nonce".to_owned()),
        })
        .unwrap();
        let request = format!(
            "POST /api/jobs/command HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = route_request(&request, &store).unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert!(response.contains("\"target_count\":1"));
        assert_eq!(
            store
                .list_pending_command_assignments_for_agent("agent-enabled")
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .list_pending_command_assignments_for_agent("agent-disabled")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn safe_command_without_confirmation_queues_normally() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        let body = serde_json::to_string(&CreateCommandJobRequest {
            job_id: "job-1".to_owned(),
            target_agent_ids: vec!["agent-1".to_owned()],
            selector: None,
            match_labels: None,
            strategy: None,
            program: "uptime".to_owned(),
            args: Vec::new(),
            timeout_seconds: 30,
            confirmed_high_risk: false,
            confirmed_by: "operator-1".to_owned(),
            expires_in_seconds: 60,
            nonce_prefix: Some("nonce".to_owned()),
        })
        .unwrap();
        let request = format!(
            "POST /api/jobs/command HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = route_request(&request, &store).unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert!(response.contains("\"status\":\"queued\""));
        assert!(response.contains("\"approval_request_id\":null"));
    }

    #[test]
    fn task_output_chunk_is_stored_as_job_output() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-1");
        let message = fleet_protocol::WireMessage::new(
            "msg-output",
            "corr-output",
            Some("agent-1".to_owned()),
            1,
            fleet_protocol::WirePayload::OutputChunk {
                job_id: "job-1".to_owned(),
                task_id: "task-1".to_owned(),
                stream: fleet_protocol::OutputStream::Stdout,
                sequence: 0,
                data: "ok".to_owned(),
            },
        );

        let finished = handle_agent_task_data_message(&store, "agent-1", message).unwrap();
        let chunks = store.list_job_output_chunks("job-1", "agent-1").unwrap();

        assert!(!finished);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].body, "ok");
    }

    #[test]
    fn duplicate_output_chunk_with_same_body_is_idempotent_at_controller_boundary() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-1");
        let message = fleet_protocol::WireMessage::new(
            "msg-output",
            "corr-output",
            Some("agent-1".to_owned()),
            1,
            fleet_protocol::WirePayload::OutputChunk {
                job_id: "job-1".to_owned(),
                task_id: "task-1".to_owned(),
                stream: fleet_protocol::OutputStream::Stdout,
                sequence: 0,
                data: "ok".to_owned(),
            },
        );

        handle_agent_task_data_message(&store, "agent-1", message.clone()).unwrap();
        let finished = handle_agent_task_data_message(&store, "agent-1", message).unwrap();
        let chunks = store.list_job_output_chunks("job-1", "agent-1").unwrap();

        assert!(!finished);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].body, "ok");
    }

    #[test]
    fn duplicate_output_chunk_body_mismatch_is_audited_as_protocol_conflict() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-1");
        let first = fleet_protocol::WireMessage::new(
            "msg-output-1",
            "corr-output",
            Some("agent-1".to_owned()),
            1,
            fleet_protocol::WirePayload::OutputChunk {
                job_id: "job-1".to_owned(),
                task_id: "task-1".to_owned(),
                stream: fleet_protocol::OutputStream::Stdout,
                sequence: 0,
                data: "ok".to_owned(),
            },
        );
        let conflicting = fleet_protocol::WireMessage::new(
            "msg-output-2",
            "corr-output",
            Some("agent-1".to_owned()),
            2,
            fleet_protocol::WirePayload::OutputChunk {
                job_id: "job-1".to_owned(),
                task_id: "task-1".to_owned(),
                stream: fleet_protocol::OutputStream::Stdout,
                sequence: 0,
                data: "changed".to_owned(),
            },
        );

        handle_agent_task_data_message(&store, "agent-1", first).unwrap();
        let result = handle_agent_task_data_message(&store, "agent-1", conflicting);
        let chunks = store.list_job_output_chunks("job-1", "agent-1").unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Security, 10)
            .unwrap();

        assert!(matches!(result, Err(ControllerError::Protocol(_))));
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].body, "ok");
        assert!(audits.iter().any(|event| {
            event.action == "websocket_output_chunk_conflict"
                && matches!(&event.value, AuditValue::Plain(value)
                    if value.contains("job_id=job-1")
                        && value.contains("stream=stdout")
                        && value.contains("sequence=0")
                        && value.contains("reason=duplicate_body_mismatch")
                        && !value.contains("changed"))
        }));
    }

    #[test]
    fn admin_can_poll_job_output_chunks() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-1");
        store
            .append_job_output_chunk_record(&JobOutputChunk {
                job_id: "job-1".to_owned(),
                agent_id: "agent-1".to_owned(),
                stream: JobOutputStream::Stdout,
                sequence: 0,
                body: "ok".to_owned(),
            })
            .unwrap();
        store
            .append_job_output_chunk_record(&JobOutputChunk {
                job_id: "job-1".to_owned(),
                agent_id: "agent-1".to_owned(),
                stream: JobOutputStream::Stderr,
                sequence: 1,
                body: "real 0m0.001s\nuser 0m0.000s\nsys 0m0.001s\n".to_owned(),
            })
            .unwrap();

        let response = route_request(
            "GET /api/jobs/job-1/output HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"job_id\":\"job-1\""));
        assert!(response.contains("\"agent_id\":\"agent-1\""));
        assert!(response.contains("\"stream\":\"stdout\""));
        assert!(response.contains("\"data\":\"ok\""));
        assert!(response.contains("\"stream\":\"stderr\""));
        assert!(response.contains("real 0m0.001s\\nuser 0m0.000s\\nsys 0m0.001s\\n"));
    }

    #[test]
    fn admin_can_list_agents() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        store
            .mark_agent_online("agent-1", SystemTime::now() - Duration::from_secs(5))
            .unwrap();
        store
            .insert_facts_snapshot(
                "agent-1",
                "{\"hostname\":\"web-01\",\"os\":\"linux\",\"arch\":\"x86_64\"}",
                SystemTime::now(),
            )
            .unwrap();
        store
            .save_agent_capability_snapshot(
                "agent-1",
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
                    SystemTime::UNIX_EPOCH + Duration::from_secs(2),
                ),
            )
            .unwrap();
        let (handle, _receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::now(),
            vec!["persistent_session".to_owned()],
            Some(64),
        );
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        sessions.lock().unwrap().register(handle);

        let response = route_request_with_sessions(
            "GET /api/agents HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
            &sessions,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"id\":\"agent-1\""));
        assert!(response.contains("\"name\":\"agent-1\""));
        assert!(response.contains("\"status\":\"online\""));
        assert!(response.contains("\"connected\":true"));
        assert!(response.contains("\"revoked\":false"));
        assert!(response.contains("\"fingerprint\""));
        assert!(response.contains("\"hostname\":\"web-01\""));
        assert!(response.contains("\"os\":\"linux\""));
        assert!(response.contains("\"arch\":\"x86_64\""));
        assert!(
            response.contains("\"capabilities\":[\"persistent_session\",\"command_execution\"]")
        );
        assert!(response.contains("\"capability_reported_at_ms\":2000"));
        assert!(response.contains("\"last_seen_age_seconds\""));
    }

    #[test]
    fn admin_agent_inventory_reports_recent_disconnected_agents_as_reconnecting() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-reconnecting");
        store
            .mark_agent_online(
                "agent-reconnecting",
                SystemTime::now() - Duration::from_secs(5),
            )
            .unwrap();

        let response = route_request(
            "GET /api/agents HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"id\":\"agent-reconnecting\""));
        assert!(response.contains("\"status\":\"reconnecting\""));
        assert!(response.contains("\"connected\":false"));
        assert!(response.contains("\"revoked\":false"));
    }

    #[test]
    fn admin_agent_inventory_reports_revoked_agents_as_offline() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_disabled_test_agent_with_labels(&store, "agent-revoked", vec![("role", "web")]);

        let response = route_request(
            "GET /api/agents HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"id\":\"agent-revoked\""));
        assert!(response.contains("\"status\":\"offline\""));
        assert!(response.contains("\"connected\":false"));
        assert!(response.contains("\"revoked\":true"));
    }

    #[test]
    fn admin_can_revoke_agent_key_and_agent_becomes_offline_revoked() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        store
            .mark_agent_online("agent-1", SystemTime::now())
            .unwrap();

        let response = route_request(
            "POST /api/agents/agent-1/revoke-key HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        let agent = store.find_agent_by_id("agent-1").unwrap().unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Agent, 10)
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"id\":\"agent-1\""));
        assert!(response.contains("\"status\":\"offline\""));
        assert!(response.contains("\"revoked\":true"));
        assert_eq!(agent.status(), AgentStatus::Disabled);
        assert!(store.find_agent_identity("agent-1").unwrap().is_none());
        assert!(
            !store
                .mark_agent_online("agent-1", SystemTime::now())
                .unwrap()
        );
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].action, "agent_key_revoked");
    }

    #[test]
    fn revoke_agent_key_closes_active_session_and_audits_reason() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        store
            .mark_agent_online("agent-1", SystemTime::now())
            .unwrap();
        let (handle, mut receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::now(),
            vec!["persistent_session".to_owned()],
            Some(64),
        );
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        sessions.lock().unwrap().register(handle);

        let response = route_request_with_sessions(
            "POST /api/agents/agent-1/revoke-key HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
            &sessions,
        )
        .unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Agent, 10)
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"status\":\"offline\""));
        assert!(response.contains("\"connected\":false"));
        assert!(response.contains("\"revoked\":true"));
        assert!(!sessions.lock().unwrap().has_active_session("agent-1"));
        assert_eq!(
            receiver.try_recv().unwrap(),
            AgentSessionOutboundMessage::Close {
                reason: AgentSessionCloseReason::Revoked,
            }
        );
        assert!(
            audits
                .iter()
                .any(|event| event.action == "agent_session_revoked_closed"
                    && matches!(&event.value, AuditValue::Plain(value) if value.contains("close_reason=agent_revoked")))
        );
    }

    #[test]
    fn admin_agent_inventory_marks_stale_online_agents_offline() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-stale");
        store
            .mark_agent_online(
                "agent-stale",
                SystemTime::now() - AGENT_OFFLINE_AFTER - Duration::from_secs(5),
            )
            .unwrap();

        let response = route_request(
            "GET /api/agents HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"id\":\"agent-stale\""));
        assert!(response.contains("\"status\":\"offline\""));
    }

    #[test]
    fn admin_can_get_agent_detail() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");

        let response = route_request(
            "GET /api/agents/agent-1 HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"id\":\"agent-1\""));
        assert!(response.contains("\"labels\""));
    }

    #[test]
    fn missing_agent_detail_is_not_found() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();

        let response = route_request(
            "GET /api/agents/missing HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 404"));
    }

    #[test]
    fn admin_can_update_agent_labels_and_audit() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        let body = serde_json::to_string(&UpdateAgentLabelsRequest {
            labels: vec![
                AgentLabelResponse {
                    key: "role".to_owned(),
                    value: "api".to_owned(),
                },
                AgentLabelResponse {
                    key: "env".to_owned(),
                    value: "prod".to_owned(),
                },
            ],
        })
        .unwrap();
        let request = format!(
            "PATCH /api/agents/agent-1/labels HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = route_request(&request, &store).unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Agent, 10)
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"key\":\"role\""));
        assert!(response.contains("\"value\":\"api\""));
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].action, "agent_labels_updated");
        assert_eq!(
            audits[0].value,
            AuditValue::Plain("label_count=2".to_owned())
        );
    }

    #[test]
    fn invalid_agent_label_update_is_rejected() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        let body = serde_json::to_string(&UpdateAgentLabelsRequest {
            labels: vec![AgentLabelResponse {
                key: "role!".to_owned(),
                value: "api".to_owned(),
            }],
        })
        .unwrap();
        let request = format!(
            "PATCH /api/agents/agent-1/labels HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = route_request(&request, &store).unwrap();

        assert!(response.starts_with("HTTP/1.1 400"));
        assert!(response.contains("invalid agent label"));
    }

    #[test]
    fn unauthorized_agent_label_update_is_rejected() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        let body = serde_json::to_string(&UpdateAgentLabelsRequest {
            labels: vec![AgentLabelResponse {
                key: "role".to_owned(),
                value: "api".to_owned(),
            }],
        })
        .unwrap();
        let request = format!(
            "PATCH /api/agents/agent-1/labels HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = route_request(&request, &store).unwrap();

        assert!(response.starts_with("HTTP/1.1 401"));
    }

    #[test]
    fn task_result_updates_job_status_and_audit() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-1");
        save_test_assignment(&store, "job-1", "task-1", "agent-1");
        let message = fleet_protocol::WireMessage::new(
            "msg-result",
            "corr-result",
            Some("agent-1".to_owned()),
            1,
            fleet_protocol::WirePayload::TaskResult {
                job_id: "job-1".to_owned(),
                task_id: "task-1".to_owned(),
                exit_code: 0,
                status: Some(fleet_protocol::TaskResultStatus::Succeeded),
                reason: String::new(),
                artifacts: Vec::new(),
            },
        );

        let finished = handle_agent_task_data_message(&store, "agent-1", message).unwrap();
        let status = store.find_job_status_value("job-1").unwrap().unwrap();
        let assignment_status = store
            .find_task_assignment_status("task-1")
            .unwrap()
            .unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Job, 10)
            .unwrap();

        assert!(!finished);
        assert_eq!(status, "success");
        assert_eq!(assignment_status, "succeeded");
        assert_eq!(audits[0].action, "job_completed");
    }

    #[test]
    fn task_result_with_artifacts_stores_rendered_artifact_metadata() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-artifact");
        save_test_assignment(&store, "job-artifact", "task-artifact", "agent-1");
        let message = fleet_protocol::WireMessage::new(
            "msg-artifact-result",
            "corr-artifact-result",
            Some("agent-1".to_owned()),
            1,
            fleet_protocol::WirePayload::TaskResult {
                job_id: "job-artifact".to_owned(),
                task_id: "task-artifact".to_owned(),
                exit_code: 0,
                status: Some(fleet_protocol::TaskResultStatus::Succeeded),
                reason: String::new(),
                artifacts: vec![fleet_protocol::TaskResultArtifactWire {
                    artifact_id: "artifact-template-1".to_owned(),
                    step_id: "template:template".to_owned(),
                    destination: "/etc/app.conf".to_owned(),
                    checksum_sha256:
                        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                            .to_owned(),
                    size_bytes: 42,
                    retention_class: "rendered_template".to_owned(),
                    content_bytes: None,
                }],
            },
        );

        handle_agent_task_data_message(&store, "agent-1", message).unwrap();

        let artifacts =
            <SqliteStore as fleet_application::ArtifactMetadataRepository>::list_rendered_artifacts_for_job(
                &store,
                &fleet_domain::JobId::new("job-artifact").unwrap(),
            )
            .unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].id.as_str(), "artifact-template-1");
        assert_eq!(artifacts[0].task_id.as_str(), "task-artifact");
        assert_eq!(artifacts[0].destination, "/etc/app.conf");
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
    fn job_summary_includes_artifact_metadata_without_body_or_paths() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-artifact-summary");
        save_test_assignment(
            &store,
            "job-artifact-summary",
            "task-artifact-summary",
            "agent-1",
        );
        let metadata = RenderedArtifactMetadata::new(
            ArtifactId::new("artifact-summary-1").unwrap(),
            JobId::new("job-artifact-summary").unwrap(),
            AgentId::new("agent-1").unwrap(),
            TaskId::new("task-artifact-summary").unwrap(),
            "/etc/app.conf",
            ArtifactChecksum::sha256(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap(),
            13,
            ArtifactRetentionClass::RenderedTemplate,
            SystemTime::UNIX_EPOCH,
        )
        .unwrap();
        store
            .save_rendered_artifact_metadata_record(&metadata)
            .unwrap();

        let response = route_request(
            "GET /api/jobs/job-artifact-summary HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        let job: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert_eq!(
            job["rendered_artifacts"][0]["artifact_id"],
            "artifact-summary-1"
        );
        assert_eq!(
            job["rendered_artifacts"][0]["task_id"],
            "task-artifact-summary"
        );
        assert_eq!(job["rendered_artifacts"][0]["agent_id"], "agent-1");
        assert_eq!(
            job["rendered_artifacts"][0]["retention_class"],
            "rendered_template"
        );
        assert_eq!(job["rendered_artifacts"][0]["size_bytes"], 13);
        assert!(job["rendered_artifacts"][0].get("content_bytes").is_none());
        assert!(job["rendered_artifacts"][0].get("destination").is_none());
        assert!(!response.contains("/etc/app.conf"));
        assert!(!response.contains("rendered body"));
    }

    #[test]
    fn task_result_with_artifact_body_stores_body_and_allows_verified_retrieval() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-artifact-body");
        save_test_assignment(&store, "job-artifact-body", "task-artifact-body", "agent-1");
        let root = artifact_test_root("artifact-body-ingest");
        let artifact_store = Mutex::new(LocalArtifactStore::new(&root).unwrap());
        let bytes = b"rendered body".to_vec();
        let checksum = controller_test_artifact_checksum(&bytes);
        let message = fleet_protocol::WireMessage::new(
            "msg-artifact-body-result",
            "corr-artifact-body-result",
            Some("agent-1".to_owned()),
            1,
            fleet_protocol::WirePayload::TaskResult {
                job_id: "job-artifact-body".to_owned(),
                task_id: "task-artifact-body".to_owned(),
                exit_code: 0,
                status: Some(fleet_protocol::TaskResultStatus::Succeeded),
                reason: String::new(),
                artifacts: vec![fleet_protocol::TaskResultArtifactWire {
                    artifact_id: "artifact-body-1".to_owned(),
                    step_id: "template:template".to_owned(),
                    destination: "/etc/app.conf".to_owned(),
                    checksum_sha256: checksum.as_sha256().to_owned(),
                    size_bytes: bytes.len() as u64,
                    retention_class: "rendered_template".to_owned(),
                    content_bytes: Some(bytes),
                }],
            },
        );

        handle_agent_task_data_message_with_artifact_store(
            &store,
            "agent-1",
            message,
            Some(&artifact_store),
        )
        .unwrap();
        let response = route_request_with_artifact_store(
            "GET /api/jobs/job-artifact-body/artifacts/artifact-body-1 HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
            &artifact_store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"artifact_id\":\"artifact-body-1\""));
        assert!(
            response
                .contains("\"content_bytes\":[114,101,110,100,101,114,101,100,32,98,111,100,121]")
        );
        assert!(!response.contains(root.to_string_lossy().as_ref()));
        assert!(!response.contains("/etc/app.conf"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn task_result_with_artifact_checksum_mismatch_does_not_store_body() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-artifact-mismatch");
        save_test_assignment(
            &store,
            "job-artifact-mismatch",
            "task-artifact-mismatch",
            "agent-1",
        );
        let root = artifact_test_root("artifact-body-mismatch");
        let artifact_store = Mutex::new(LocalArtifactStore::new(&root).unwrap());
        let expected = controller_test_artifact_checksum(b"expected body");
        let message = fleet_protocol::WireMessage::new(
            "msg-artifact-mismatch-result",
            "corr-artifact-mismatch-result",
            Some("agent-1".to_owned()),
            1,
            fleet_protocol::WirePayload::TaskResult {
                job_id: "job-artifact-mismatch".to_owned(),
                task_id: "task-artifact-mismatch".to_owned(),
                exit_code: 0,
                status: Some(fleet_protocol::TaskResultStatus::Succeeded),
                reason: String::new(),
                artifacts: vec![fleet_protocol::TaskResultArtifactWire {
                    artifact_id: "artifact-mismatch-1".to_owned(),
                    step_id: "template:template".to_owned(),
                    destination: "/etc/app.conf".to_owned(),
                    checksum_sha256: expected.as_sha256().to_owned(),
                    size_bytes: 13,
                    retention_class: "rendered_template".to_owned(),
                    content_bytes: Some(b"tampered body".to_vec()),
                }],
            },
        );

        let result = handle_agent_task_data_message_with_artifact_store(
            &store,
            "agent-1",
            message,
            Some(&artifact_store),
        );
        let response = route_request_with_artifact_store(
            "GET /api/jobs/job-artifact-mismatch/artifacts/artifact-mismatch-1 HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
            &artifact_store,
        )
        .unwrap();

        assert!(result.is_err());
        assert!(response.starts_with("HTTP/1.1 404"));
        assert!(!response.contains("tampered body"));
        assert!(!response.contains(root.to_string_lossy().as_ref()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn task_result_with_oversized_artifact_body_is_rejected_without_body_leak() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-artifact-large");
        save_test_assignment(
            &store,
            "job-artifact-large",
            "task-artifact-large",
            "agent-1",
        );
        let root = artifact_test_root("artifact-body-large");
        let artifact_store = Mutex::new(LocalArtifactStore::new(&root).unwrap());
        let bytes = vec![b'x'; DEFAULT_MAX_ARTIFACT_BODY_BYTES + 1];
        let checksum = controller_test_artifact_checksum(&bytes);
        let message = fleet_protocol::WireMessage::new(
            "msg-artifact-large-result",
            "corr-artifact-large-result",
            Some("agent-1".to_owned()),
            1,
            fleet_protocol::WirePayload::TaskResult {
                job_id: "job-artifact-large".to_owned(),
                task_id: "task-artifact-large".to_owned(),
                exit_code: 0,
                status: Some(fleet_protocol::TaskResultStatus::Succeeded),
                reason: String::new(),
                artifacts: vec![fleet_protocol::TaskResultArtifactWire {
                    artifact_id: "artifact-large-1".to_owned(),
                    step_id: "template:template".to_owned(),
                    destination: "/etc/app.conf".to_owned(),
                    checksum_sha256: checksum.as_sha256().to_owned(),
                    size_bytes: bytes.len() as u64,
                    retention_class: "rendered_template".to_owned(),
                    content_bytes: Some(bytes),
                }],
            },
        );

        let result = handle_agent_task_data_message_with_artifact_store(
            &store,
            "agent-1",
            message,
            Some(&artifact_store),
        );

        assert!(result.is_err());
        let error = result.unwrap_err().to_string();
        assert!(error.contains("artifact body exceeds max size"));
        assert!(!error.contains("/etc/app.conf"));
        assert!(!error.contains(root.to_string_lossy().as_ref()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn artifact_retrieval_requires_admin_token() {
        let store = SqliteStore::in_memory().unwrap();
        let root = artifact_test_root("artifact-retrieval-unauthorized");
        let artifact_store = Mutex::new(LocalArtifactStore::new(&root).unwrap());

        let response = route_request_with_artifact_store(
            "GET /api/jobs/job-artifact/artifacts/artifact-template-1 HTTP/1.1\r\n\r\n",
            &store,
            &artifact_store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 401"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn artifact_retrieval_returns_verified_body_without_local_path() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-artifact");
        save_test_assignment(&store, "job-artifact", "task-artifact", "agent-1");
        let root = artifact_test_root("artifact-retrieval-body");
        let mut local_store = LocalArtifactStore::new(&root).unwrap();
        let bytes = b"rendered body".to_vec();
        let checksum = controller_test_artifact_checksum(&bytes);
        let metadata = RenderedArtifactMetadata::new(
            ArtifactId::new("artifact-template-1").unwrap(),
            JobId::new("job-artifact").unwrap(),
            AgentId::new("agent-1").unwrap(),
            TaskId::new("task-artifact").unwrap(),
            "/etc/app.conf",
            checksum.clone(),
            bytes.len() as u64,
            ArtifactRetentionClass::RenderedTemplate,
            SystemTime::UNIX_EPOCH,
        )
        .unwrap();
        store
            .save_rendered_artifact_metadata_record(&metadata)
            .unwrap();
        local_store
            .put(fleet_application::ArtifactStorePut {
                id: metadata.id.clone(),
                retention_class: metadata.retention_class,
                expected_checksum: checksum,
                bytes: bytes.clone(),
            })
            .unwrap();
        let artifact_store = Mutex::new(local_store);

        let response = route_request_with_artifact_store(
            "GET /api/jobs/job-artifact/artifacts/artifact-template-1 HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
            &artifact_store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"artifact_id\":\"artifact-template-1\""));
        assert!(
            response
                .contains("\"content_bytes\":[114,101,110,100,101,114,101,100,32,98,111,100,121]")
        );
        assert!(!response.contains(root.to_string_lossy().as_ref()));
        assert!(!response.contains("/etc/app.conf"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn artifact_retrieval_rejects_missing_or_corrupt_body_without_body_leak() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-artifact");
        save_test_assignment(&store, "job-artifact", "task-artifact", "agent-1");
        let root = artifact_test_root("artifact-retrieval-corrupt");
        let mut local_store = LocalArtifactStore::new(&root).unwrap();
        let expected = controller_test_artifact_checksum(b"expected body");
        let metadata = RenderedArtifactMetadata::new(
            ArtifactId::new("artifact-template-1").unwrap(),
            JobId::new("job-artifact").unwrap(),
            AgentId::new("agent-1").unwrap(),
            TaskId::new("task-artifact").unwrap(),
            "/etc/app.conf",
            expected.clone(),
            13,
            ArtifactRetentionClass::RenderedTemplate,
            SystemTime::UNIX_EPOCH,
        )
        .unwrap();
        store
            .save_rendered_artifact_metadata_record(&metadata)
            .unwrap();
        local_store
            .put(fleet_application::ArtifactStorePut {
                id: metadata.id.clone(),
                retention_class: metadata.retention_class,
                expected_checksum: controller_test_artifact_checksum(b"tampered body"),
                bytes: b"tampered body".to_vec(),
            })
            .unwrap();
        let artifact_store = Mutex::new(local_store);

        let response = route_request_with_artifact_store(
            "GET /api/jobs/job-artifact/artifacts/artifact-template-1 HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
            &artifact_store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 409"));
        assert!(response.contains("\"error\":\"artifact_corrupt\""));
        assert!(!response.contains("tampered body"));
        assert!(!response.contains(root.to_string_lossy().as_ref()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn artifact_retrieval_request_cannot_specify_local_path() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        let root = artifact_test_root("artifact-retrieval-path");
        let artifact_store = Mutex::new(LocalArtifactStore::new(&root).unwrap());

        let response = route_request_with_artifact_store(
            "GET /api/jobs/job-artifact/artifacts/../secret HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
            &artifact_store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 404"));
        assert!(!response.contains(root.to_string_lossy().as_ref()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn task_result_timeout_marks_assignment_expired() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-timeout");
        save_test_assignment(&store, "job-timeout", "task-timeout", "agent-1");
        store
            .update_task_assignment_status(
                "task-timeout",
                AssignmentStatus::Started,
                SystemTime::UNIX_EPOCH,
                None,
            )
            .unwrap();
        let message = fleet_protocol::WireMessage::new(
            "msg-timeout",
            "corr-timeout",
            Some("agent-1".to_owned()),
            1,
            fleet_protocol::WirePayload::TaskResult {
                job_id: "job-timeout".to_owned(),
                task_id: "task-timeout".to_owned(),
                exit_code: -1,
                status: Some(fleet_protocol::TaskResultStatus::TimedOut),
                reason: "command timed out".to_owned(),
                artifacts: Vec::new(),
            },
        );

        handle_agent_task_data_message(&store, "agent-1", message).unwrap();

        assert_eq!(
            store.find_job_status_value("job-timeout").unwrap().unwrap(),
            "expired"
        );
        assert_eq!(
            store
                .find_task_assignment_status("task-timeout")
                .unwrap()
                .unwrap(),
            "expired"
        );
    }

    #[test]
    fn max_failures_cancels_remaining_queued_assignments_and_marks_failed() {
        let store = SqliteStore::in_memory().unwrap();
        for agent_id in ["agent-1", "agent-2", "agent-3"] {
            save_test_agent(&store, agent_id);
        }
        save_test_job(&store, "job-maxfail");
        store
            .update_job_strategy("job-maxfail", 1, Some(1))
            .unwrap();
        save_test_assignment(&store, "job-maxfail", "task-1", "agent-1");
        save_test_assignment(&store, "job-maxfail", "task-2", "agent-2");
        save_test_assignment(&store, "job-maxfail", "task-3", "agent-3");

        handle_agent_task_data_message(
            &store,
            "agent-1",
            fleet_protocol::WireMessage::new(
                "msg-failed",
                "corr-failed",
                Some("agent-1".to_owned()),
                1,
                fleet_protocol::WirePayload::TaskResult {
                    job_id: "job-maxfail".to_owned(),
                    task_id: "task-1".to_owned(),
                    exit_code: 1,
                    status: Some(fleet_protocol::TaskResultStatus::Failed),
                    reason: "command failed".to_owned(),
                    artifacts: Vec::new(),
                },
            ),
        )
        .unwrap();

        let assignments = store
            .list_task_assignment_summaries_for_job("job-maxfail")
            .unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Job, 10)
            .unwrap();

        assert_eq!(
            store.find_job_status_value("job-maxfail").unwrap().unwrap(),
            "failed"
        );
        assert_eq!(
            assignments
                .iter()
                .map(|assignment| assignment.status.as_str())
                .collect::<Vec<_>>(),
            vec!["failed", "canceled", "canceled"]
        );
        let repo = ControllerJobQueryRepository {
            store: (&store).into(),
        };
        let job_record = repo
            .list_job_summaries(10)
            .unwrap()
            .into_iter()
            .find(|job| job.id == "job-maxfail")
            .unwrap();
        let response = job_summary_response(job_record, &std::collections::BTreeSet::new());
        assert_eq!(response.assignment_summary.failed, 1);
        assert_eq!(response.assignment_summary.canceled, 2);
        assert_eq!(response.assignment_summary.skipped, 2);
        assert!(
            audits
                .iter()
                .any(|event| event.action == "job_max_failures_reached")
        );
    }

    #[test]
    fn mixed_fanout_results_mark_job_partial_success() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        save_test_agent(&store, "agent-2");
        save_test_job(&store, "job-partial");
        save_test_assignment(&store, "job-partial", "task-1", "agent-1");
        save_test_assignment(&store, "job-partial", "task-2", "agent-2");

        handle_agent_task_data_message(
            &store,
            "agent-1",
            fleet_protocol::WireMessage::new(
                "msg-success",
                "corr-success",
                Some("agent-1".to_owned()),
                1,
                fleet_protocol::WirePayload::TaskResult {
                    job_id: "job-partial".to_owned(),
                    task_id: "task-1".to_owned(),
                    exit_code: 0,
                    status: Some(fleet_protocol::TaskResultStatus::Succeeded),
                    reason: String::new(),
                    artifacts: Vec::new(),
                },
            ),
        )
        .unwrap();
        assert_eq!(
            store.find_job_status_value("job-partial").unwrap().unwrap(),
            "running"
        );

        handle_agent_task_data_message(
            &store,
            "agent-2",
            fleet_protocol::WireMessage::new(
                "msg-failed",
                "corr-failed",
                Some("agent-2".to_owned()),
                2,
                fleet_protocol::WirePayload::TaskResult {
                    job_id: "job-partial".to_owned(),
                    task_id: "task-2".to_owned(),
                    exit_code: 1,
                    status: Some(fleet_protocol::TaskResultStatus::Failed),
                    reason: "command failed".to_owned(),
                    artifacts: Vec::new(),
                },
            ),
        )
        .unwrap();

        assert_eq!(
            store.find_job_status_value("job-partial").unwrap().unwrap(),
            "partial_success"
        );
    }

    #[test]
    fn late_success_result_does_not_override_canceled_assignment() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-canceled");
        save_test_assignment(&store, "job-canceled", "task-canceled", "agent-1");
        store
            .update_task_assignment_status(
                "task-canceled",
                AssignmentStatus::Canceled,
                SystemTime::UNIX_EPOCH,
                Some("operator requested cancel"),
            )
            .unwrap();
        store
            .update_job_status("job-canceled", JobStatus::Canceled)
            .unwrap();

        handle_agent_task_data_message(
            &store,
            "agent-1",
            fleet_protocol::WireMessage::new(
                "msg-late-success",
                "corr-late-success",
                Some("agent-1".to_owned()),
                1,
                fleet_protocol::WirePayload::TaskResult {
                    job_id: "job-canceled".to_owned(),
                    task_id: "task-canceled".to_owned(),
                    exit_code: 0,
                    status: Some(fleet_protocol::TaskResultStatus::Succeeded),
                    reason: String::new(),
                    artifacts: Vec::new(),
                },
            ),
        )
        .unwrap();

        assert_eq!(
            store
                .find_job_status_value("job-canceled")
                .unwrap()
                .unwrap(),
            "canceled"
        );
        assert_eq!(
            store
                .find_task_assignment_status("task-canceled")
                .unwrap()
                .unwrap(),
            "canceled"
        );
    }

    #[test]
    fn cancel_job_cancels_queued_assignment_without_session() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-cancel-queued");
        save_test_assignment(&store, "job-cancel-queued", "task-cancel-queued", "agent-1");

        let response = route_request(
            "POST /api/jobs/job-cancel-queued/cancel HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: 0\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"status\":\"canceled\""));
        assert!(response.contains("\"cancel_delivered\":false"));
        assert_eq!(
            store
                .find_task_assignment_status("task-cancel-queued")
                .unwrap()
                .unwrap(),
            "canceled"
        );
    }

    #[test]
    fn cancel_job_sends_cancel_to_dispatched_session() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-cancel-dispatched");
        save_test_assignment(
            &store,
            "job-cancel-dispatched",
            "task-cancel-dispatched",
            "agent-1",
        );
        store
            .update_task_assignment_status(
                "task-cancel-dispatched",
                AssignmentStatus::Dispatched,
                SystemTime::UNIX_EPOCH,
                None,
            )
            .unwrap();
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        let (handle, mut receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            vec!["persistent_session".to_owned()],
            Some(8),
        );
        sessions.lock().unwrap().register(handle);

        let response = route_request_with_sessions(
            "POST /api/jobs/job-cancel-dispatched/cancel HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: 0\r\n\r\n",
            &store,
            &sessions,
        )
        .unwrap();
        let outbound = receiver.try_recv().unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"cancel_delivered\":true"));
        assert!(matches!(
            outbound,
            AgentSessionOutboundMessage::Wire(message)
                if matches!(
                    message.payload,
                    fleet_protocol::WirePayload::TaskCancel { ref task_id, .. }
                        if task_id == "task-cancel-dispatched"
                )
        ));
        assert_eq!(
            store
                .find_task_assignment_status("task-cancel-dispatched")
                .unwrap()
                .unwrap(),
            "canceled"
        );
    }

    #[test]
    fn cancel_job_sends_cancel_to_started_session() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-cancel-started");
        save_test_assignment(
            &store,
            "job-cancel-started",
            "task-cancel-started",
            "agent-1",
        );
        store
            .update_task_assignment_status(
                "task-cancel-started",
                AssignmentStatus::Started,
                SystemTime::UNIX_EPOCH,
                None,
            )
            .unwrap();
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        let (handle, mut receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            vec!["persistent_session".to_owned()],
            Some(8),
        );
        sessions.lock().unwrap().register(handle);

        let response = route_request_with_sessions(
            "POST /api/jobs/job-cancel-started/cancel HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: 0\r\n\r\n",
            &store,
            &sessions,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            AgentSessionOutboundMessage::Wire(message)
                if matches!(
                    message.payload,
                    fleet_protocol::WirePayload::TaskCancel { ref task_id, .. }
                        if task_id == "task-cancel-started"
                )
        ));
        assert_eq!(
            store
                .find_task_assignment_status("task-cancel-started")
                .unwrap()
                .unwrap(),
            "canceled"
        );
    }

    #[test]
    fn cancel_job_after_controller_restart_uses_persisted_assignment_state() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-restart-cancel");
        save_test_assignment(
            &store,
            "job-restart-cancel",
            "task-restart-cancel",
            "agent-1",
        );
        store
            .update_task_assignment_status(
                "task-restart-cancel",
                AssignmentStatus::Dispatched,
                SystemTime::UNIX_EPOCH,
                None,
            )
            .unwrap();

        let response = route_request(
            "POST /api/jobs/job-restart-cancel/cancel HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: 0\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"cancel_delivered\":false"));
        assert_eq!(
            store
                .find_task_assignment_status("task-restart-cancel")
                .unwrap()
                .unwrap(),
            "canceled"
        );
    }

    #[test]
    fn disconnect_does_not_mark_dispatched_assignment_success() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-disconnect");
        save_test_assignment(&store, "job-disconnect", "task-disconnect", "agent-1");
        store
            .update_task_assignment_status(
                "task-disconnect",
                AssignmentStatus::Dispatched,
                SystemTime::UNIX_EPOCH,
                None,
            )
            .unwrap();
        let mut sessions = AgentSessionRegistry::default();
        let (handle, _receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            Vec::new(),
            Some(8),
        );
        sessions.register(handle);
        sessions.unregister("agent-1", "conn-1", AgentSessionCloseReason::NormalShutdown);

        assert_eq!(
            store
                .find_task_assignment_status("task-disconnect")
                .unwrap()
                .unwrap(),
            "dispatched"
        );
        assert_ne!(
            store
                .find_job_status_value("job-disconnect")
                .unwrap()
                .unwrap(),
            "success"
        );
    }

    #[test]
    fn task_ack_started_and_rejected_update_assignment_status() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-ack");
        save_test_assignment(&store, "job-ack", "task-ack", "agent-1");
        save_test_job(&store, "job-reject");
        save_test_assignment(&store, "job-reject", "task-reject", "agent-1");

        handle_agent_task_data_message(
            &store,
            "agent-1",
            fleet_protocol::WireMessage::new(
                "msg-ack",
                "corr-ack",
                Some("agent-1".to_owned()),
                1,
                fleet_protocol::WirePayload::TaskAck {
                    job_id: "job-ack".to_owned(),
                    task_id: "task-ack".to_owned(),
                },
            ),
        )
        .unwrap();
        assert_eq!(
            store.find_task_assignment_status("task-ack").unwrap(),
            Some("accepted".to_owned())
        );

        handle_agent_task_data_message(
            &store,
            "agent-1",
            fleet_protocol::WireMessage::new(
                "msg-started",
                "corr-started",
                Some("agent-1".to_owned()),
                2,
                fleet_protocol::WirePayload::TaskStarted {
                    job_id: "job-ack".to_owned(),
                    task_id: "task-ack".to_owned(),
                },
            ),
        )
        .unwrap();
        assert_eq!(
            store.find_task_assignment_status("task-ack").unwrap(),
            Some("started".to_owned())
        );

        handle_agent_task_data_message(
            &store,
            "agent-1",
            fleet_protocol::WireMessage::new(
                "msg-rejected",
                "corr-rejected",
                Some("agent-1".to_owned()),
                3,
                fleet_protocol::WirePayload::TaskRejected {
                    job_id: "job-reject".to_owned(),
                    task_id: "task-reject".to_owned(),
                    reason_code: fleet_protocol::TaskRejectionReasonCode::InvalidSignature,
                    reason: "invalid signature".to_owned(),
                },
            ),
        )
        .unwrap();
        assert_eq!(
            store.find_task_assignment_status("task-reject").unwrap(),
            Some("rejected".to_owned())
        );
        assert_eq!(
            store.find_job_status_value("job-reject").unwrap(),
            Some("failed".to_owned())
        );
    }

    #[test]
    fn output_chunk_does_not_mark_assignment_success() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-output");
        save_test_assignment(&store, "job-output", "task-output", "agent-1");
        store
            .update_task_assignment_status(
                "task-output",
                AssignmentStatus::Started,
                SystemTime::UNIX_EPOCH,
                None,
            )
            .unwrap();

        handle_agent_task_data_message(
            &store,
            "agent-1",
            fleet_protocol::WireMessage::new(
                "msg-output",
                "corr-output",
                Some("agent-1".to_owned()),
                1,
                fleet_protocol::WirePayload::OutputChunk {
                    job_id: "job-output".to_owned(),
                    task_id: "task-output".to_owned(),
                    stream: fleet_protocol::OutputStream::Stdout,
                    sequence: 0,
                    data: "partial output".to_owned(),
                },
            ),
        )
        .unwrap();

        assert_eq!(
            store.find_task_assignment_status("task-output").unwrap(),
            Some("started".to_owned())
        );
        assert_eq!(
            store.find_job_status_value("job-output").unwrap(),
            Some("queued".to_owned())
        );
    }

    #[test]
    fn agent_security_event_is_audited() {
        let store = SqliteStore::in_memory().unwrap();
        let message = fleet_protocol::WireMessage::new(
            "msg-security",
            "corr-security",
            Some("agent-1".to_owned()),
            1,
            fleet_protocol::WirePayload::SecurityEvent {
                agent_id: "agent-1".to_owned(),
                action: "task_verification_failed".to_owned(),
                detail: "invalid signature".to_owned(),
            },
        );

        let finished = handle_agent_task_data_message(&store, "agent-1", message).unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Security, 10)
            .unwrap();

        assert!(!finished);
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].action, "task_verification_failed");
        assert!(!audits[0].contains_secret_plaintext());
    }

    #[test]
    fn facts_snapshot_message_is_stored() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        let message = fleet_protocol::WireMessage::new(
            "msg-facts",
            "corr-facts",
            Some("agent-1".to_owned()),
            1000,
            fleet_protocol::WirePayload::FactsSnapshot {
                agent_id: "agent-1".to_owned(),
                body: "{\"os\":\"linux\",\"arch\":\"x86_64\"}".to_owned(),
            },
        );

        let finished = handle_agent_task_data_message(&store, "agent-1", message).unwrap();
        let snapshot = store.latest_facts_snapshot("agent-1").unwrap().unwrap();

        assert!(!finished);
        assert_eq!(
            snapshot.collected_at,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1)
        );
        assert!(snapshot.body.contains("\"os\":\"linux\""));
    }

    #[test]
    fn degraded_facts_snapshot_marks_agent_degraded() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        store
            .mark_agent_online("agent-1", SystemTime::UNIX_EPOCH + Duration::from_secs(1))
            .unwrap();
        let message = fleet_protocol::WireMessage::new(
            "msg-facts",
            "corr-facts",
            Some("agent-1".to_owned()),
            1,
            fleet_protocol::WirePayload::FactsSnapshot {
                agent_id: "agent-1".to_owned(),
                body: "{\"degraded\":{\"status\":true,\"signals\":[\"disk_usage_unavailable\"]}}"
                    .to_owned(),
            },
        );

        handle_agent_task_data_message(&store, "agent-1", message).unwrap();

        let agent = store.find_agent_by_id("agent-1").unwrap().unwrap();
        assert_eq!(agent.status(), AgentStatus::Degraded);
    }

    #[test]
    fn metrics_snapshot_message_is_stored() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        let message = fleet_protocol::WireMessage::new(
            "msg-metrics",
            "corr-metrics",
            Some("agent-1".to_owned()),
            1000,
            fleet_protocol::WirePayload::MetricsSnapshot {
                agent_id: "agent-1".to_owned(),
                body: "{\"cpu\":{\"logical_count\":4}}".to_owned(),
            },
        );

        let finished = handle_agent_task_data_message(&store, "agent-1", message).unwrap();
        let snapshot = store.latest_metrics_snapshot("agent-1").unwrap().unwrap();

        assert!(!finished);
        assert_eq!(
            snapshot.collected_at,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1)
        );
        assert!(snapshot.body.contains("\"logical_count\":4"));
    }

    #[test]
    fn capability_snapshot_message_is_stored_and_audited() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        let message = fleet_protocol::WireMessage::new(
            "msg-capability",
            "corr-capability",
            Some("agent-1".to_owned()),
            1000,
            fleet_protocol::WirePayload::CapabilitySnapshot {
                agent_id: "agent-1".to_owned(),
                privilege_level: fleet_protocol::CapabilityPrivilegeLevelWire::SudoAvailable,
                package_manager: Some(fleet_protocol::PackageManagerWire::Apt),
                service_manager: Some(fleet_protocol::ServiceManagerWire::Systemd),
                capabilities: vec![
                    "persistent_session".to_owned(),
                    "command_execution".to_owned(),
                ],
                reported_at_ms: 1000,
            },
        );

        let finished = handle_agent_task_data_message(&store, "agent-1", message).unwrap();
        let snapshot = store
            .latest_agent_capability_snapshot("agent-1")
            .unwrap()
            .unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Agent, 10)
            .unwrap();

        assert!(!finished);
        assert_eq!(
            snapshot
                .evaluate(fleet_domain::RuntimePrimitive::Command)
                .status,
            fleet_domain::CapabilitySnapshotStatus::Compatible
        );
        assert_eq!(audits[0].action, "agent_capability_reported");
        assert!(matches!(
            &audits[0].value,
            AuditValue::Plain(value) if value.contains("capability_count=2")
        ));
    }

    #[test]
    fn log_chunk_message_is_stored_and_redacted() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        let message = fleet_protocol::WireMessage::new(
            "msg-log",
            "corr-log",
            Some("agent-1".to_owned()),
            1000,
            fleet_protocol::WirePayload::LogChunk {
                agent_id: "agent-1".to_owned(),
                line: "level=info event=agent_heartbeat_completed token=secret".to_owned(),
            },
        );

        let finished = handle_agent_task_data_message(&store, "agent-1", message).unwrap();
        let chunks = store.list_agent_log_chunks("agent-1", 10).unwrap();

        assert!(!finished);
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0].collected_at,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1)
        );
        assert!(chunks[0].line.contains("agent_heartbeat_completed"));
        assert!(!chunks[0].line.contains("secret"));
    }

    #[test]
    fn drift_report_message_is_stored_and_audited() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        let message = fleet_protocol::WireMessage::new(
            "msg-drift",
            "corr-drift",
            Some("agent-1".to_owned()),
            1000,
            fleet_protocol::WirePayload::DriftReport {
                agent_id: "agent-1".to_owned(),
                status: "drifted".to_owned(),
                expected: "service nginx running".to_owned(),
                actual: "stopped".to_owned(),
            },
        );

        let finished = handle_agent_task_data_message(&store, "agent-1", message).unwrap();
        let record = store.latest_drift_report("agent-1").unwrap().unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Drift, 10)
            .unwrap();

        assert!(!finished);
        assert_eq!(
            record.checked_at,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1)
        );
        assert_eq!(record.report.status, DriftStatus::Drifted);
        assert_eq!(record.report.actual, "stopped");
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].action, "drift_report_received");
    }

    #[test]
    fn admin_can_get_latest_facts() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        store
            .insert_facts_snapshot(
                "agent-1",
                "{\"os\":\"linux\",\"arch\":\"x86_64\"}",
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();

        let response = route_request(
            "GET /api/agents/agent-1/facts/latest HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"agent_id\":\"agent-1\""));
        assert!(response.contains("\"collected_at_ms\":1000"));
        assert!(response.contains("\"agent_system_time_ms\":1000"));
        assert!(response.contains("\"os\":\"linux\""));
    }

    #[test]
    fn admin_can_get_latest_metrics() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        store
            .insert_metrics_snapshot(
                "agent-1",
                "{\"cpu\":{\"logical_count\":4}}",
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();

        let response = route_request(
            "GET /api/agents/agent-1/metrics/latest HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"agent_id\":\"agent-1\""));
        assert!(response.contains("\"collected_at_ms\":1000"));
        assert!(response.contains("\"agent_system_time_ms\":1000"));
        assert!(response.contains("\"logical_count\":4"));
    }

    #[test]
    fn admin_can_get_latest_drift_report() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        store
            .insert_drift_report(
                "agent-1",
                &DriftReport {
                    policy_name: "nginx-running".to_owned(),
                    status: DriftStatus::Drifted,
                    severity: DriftSeverity::Warning,
                    acknowledgement: DriftAcknowledgement::Open,
                    expected: "service nginx running".to_owned(),
                    actual: "stopped".to_owned(),
                },
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();

        let response = route_request(
            "GET /api/agents/agent-1/drift/latest HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"agent_id\":\"agent-1\""));
        assert!(response.contains("\"checked_at_ms\":1000"));
        assert!(response.contains("\"agent_system_time_ms\":1000"));
        assert!(response.contains("\"status\":\"drifted\""));
        assert!(response.contains("\"severity\":\"warning\""));
        assert!(response.contains("\"actual\":\"stopped\""));
    }

    #[test]
    fn admin_can_save_assign_and_schedule_policy() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent_with_labels(&store, "agent-1", vec![("role", "web")]);
        let policy_source = r#"
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
"#;
        let save_body = serde_json::json!({ "source": policy_source }).to_string();
        let save_request = format!(
            "POST /api/policies HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            save_body.len(),
            save_body
        );

        let save_response = route_request(&save_request, &store).unwrap();

        assert!(save_response.starts_with("HTTP/1.1 201"));
        assert!(save_response.contains("\"id\":\"nginx-running\""));

        let list_response = route_request(
            "GET /api/policies HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        assert!(list_response.contains("\"name\":\"nginx-running\""));

        let assign_body = serde_json::json!({ "agent_id": "agent-1" }).to_string();
        let assign_request = format!(
            "POST /api/policies/nginx-running/assignments HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            assign_body.len(),
            assign_body
        );
        let assign_response = route_request(&assign_request, &store).unwrap();
        assert!(assign_response.starts_with("HTTP/1.1 201"));
        assert!(assign_response.contains("\"policy_id\":\"nginx-running\""));

        let agent_response = route_request(
            "GET /api/agents/agent-1 HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        assert!(agent_response.contains("\"assigned_policy_ids\":[\"nginx-running\"]"));

        let agent_policies_response = route_request(
            "GET /api/agents/agent-1/policies HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        assert!(agent_policies_response.contains("\"policy_id\":\"nginx-running\""));

        let schedule_body =
            serde_json::json!({ "agent_id": "agent-1", "interval_seconds": 300 }).to_string();
        let schedule_request = format!(
            "POST /api/policies/nginx-running/schedules HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            schedule_body.len(),
            schedule_body
        );
        let schedule_response = route_request(&schedule_request, &store).unwrap();
        assert!(schedule_response.starts_with("HTTP/1.1 201"));
        assert!(schedule_response.contains("\"interval_seconds\":300"));
    }

    #[test]
    fn scheduled_drift_worker_creates_queued_drift_job_from_due_schedule() {
        let store = SqliteStore::in_memory().unwrap();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(600);
        save_test_agent_with_labels(&store, "agent-1", vec![("role", "web")]);
        let policy_source = r#"
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
"#;
        store
            .save_policy_source("nginx-running", "nginx-running", 1, policy_source)
            .unwrap();
        store
            .upsert_policy_schedule(
                "nginx-running",
                "agent-1",
                Duration::from_secs(300),
                now - Duration::from_secs(1),
            )
            .unwrap();
        let identity = ControllerIdentity::dev_insecure();

        let output = run_due_scheduled_drift_once(&store, &identity, now).unwrap();
        let pending = store
            .list_pending_dispatch_assignments(None, None, 10)
            .unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Drift, 10)
            .unwrap();

        assert_eq!(output.created_count, 1);
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].task,
            fleet_domain::TaskKind::DriftCheck(_)
        ));
        assert!(
            store
                .due_scheduled_drift_checks(now + Duration::from_secs(299), 10)
                .unwrap()
                .is_empty()
        );
        assert!(
            audits
                .iter()
                .any(|event| event.action == "drift_check_job_created")
        );
        assert!(
            audits
                .iter()
                .any(|event| event.action == "scheduled_drift_job_created")
        );
    }

    #[test]
    fn retention_worker_cleanup_removes_bounded_artifacts_and_keeps_audit() {
        let store = SqliteStore::in_memory().unwrap();
        save_test_agent(&store, "agent-1");
        save_test_job(&store, "job-1");
        store
            .append_job_output_chunk_record(&JobOutputChunk {
                job_id: "job-1".to_owned(),
                agent_id: "agent-1".to_owned(),
                stream: JobOutputStream::Stdout,
                sequence: 0,
                body: "artifact body must not be logged".to_owned(),
            })
            .unwrap();
        store
            .insert_facts_snapshot("agent-1", "{\"hostname\":\"web-01\"}", SystemTime::now())
            .unwrap();
        store
            .insert_metrics_snapshot("agent-1", "{\"cpu\":0.1}", SystemTime::now())
            .unwrap();
        store
            .insert_agent_log_chunk("agent-1", "level=info event=old", SystemTime::now())
            .unwrap();
        store
            .write_audit_event(AuditEvent::security("retention_guard", "controller-store"))
            .unwrap();

        let now = SystemTime::now() + Duration::from_secs(40 * 86_400);
        let output = run_retention_cleanup_once(&store, now).unwrap();
        let remaining = store
            .cleanup_retention_with_cutoffs(RetentionPolicy::mvp_defaults().cutoffs(now), true)
            .unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Security, 10)
            .unwrap();

        assert_eq!(output.summary.total(), 4);
        assert_eq!(remaining.total(), 0);
        assert!(audits.iter().any(|event| event.action == "retention_guard"));
        assert!(
            audits
                .iter()
                .any(|event| event.action == "retention_cleanup")
        );
    }

    #[test]
    fn admin_latest_facts_prefers_payload_system_time_when_present() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        store
            .insert_facts_snapshot(
                "agent-1",
                "{\"system_time_ms\":123456,\"os\":\"linux\"}",
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();

        let response = route_request(
            "GET /api/agents/agent-1/facts/latest HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"collected_at_ms\":1000"));
        assert!(response.contains("\"agent_system_time_ms\":123456"));
    }

    #[test]
    fn admin_can_page_facts_metrics_logs_and_drift_reports() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        for body in ["{\"seq\":1}", "{\"seq\":2}", "{\"seq\":3}"] {
            store
                .insert_facts_snapshot(
                    "agent-1",
                    body,
                    SystemTime::UNIX_EPOCH + Duration::from_secs(1),
                )
                .unwrap();
        }
        for (seconds, body) in [
            (1, "{\"cpu\":{\"logical_count\":1}}"),
            (2, "{\"cpu\":{\"logical_count\":2}}"),
            (3, "{\"cpu\":{\"logical_count\":3}}"),
        ] {
            store
                .insert_metrics_snapshot(
                    "agent-1",
                    body,
                    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds),
                )
                .unwrap();
        }
        for (seconds, line) in [
            (1, "level=info event=agent_log_uploaded sequence=1"),
            (2, "level=info event=agent_log_uploaded sequence=2"),
            (3, "level=info event=agent_log_uploaded sequence=3"),
        ] {
            store
                .insert_agent_log_chunk(
                    "agent-1",
                    line,
                    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds),
                )
                .unwrap();
        }
        for (seconds, status, actual) in [
            (1, DriftStatus::Unknown, "unknown"),
            (2, DriftStatus::Compliant, "running"),
            (3, DriftStatus::Drifted, "stopped"),
        ] {
            store
                .insert_drift_report(
                    "agent-1",
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

        let facts_first = route_request(
            "GET /api/agents/agent-1/facts?limit=2 HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        assert!(facts_first.starts_with("HTTP/1.1 200"));
        let facts_first: serde_json::Value =
            serde_json::from_str(facts_first.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(facts_first["items"].as_array().unwrap().len(), 2);
        assert_eq!(facts_first["items"][0]["body"]["seq"], 3);
        assert_eq!(facts_first["items"][0]["agent_system_time_ms"], 1000);
        let facts_cursor = facts_first["next_cursor"].as_str().unwrap();
        let encoded_facts_cursor = facts_cursor.replace(':', "%3A");
        let facts_second = route_request(
            &format!(
                "GET /api/agents/agent-1/facts?limit=2&before={encoded_facts_cursor} HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n"
            ),
            &store,
        )
        .unwrap();
        let facts_second: serde_json::Value =
            serde_json::from_str(facts_second.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(facts_second["items"].as_array().unwrap().len(), 1);
        assert_eq!(facts_second["items"][0]["body"]["seq"], 1);
        assert!(facts_second["next_cursor"].is_null());

        let metrics_first = route_request(
            "GET /api/agents/agent-1/metrics?limit=2 HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        let metrics_first: serde_json::Value =
            serde_json::from_str(metrics_first.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(metrics_first["items"][0]["body"]["cpu"]["logical_count"], 3);
        assert_eq!(metrics_first["items"][0]["agent_system_time_ms"], 3000);
        let metrics_cursor = metrics_first["next_cursor"].as_str().unwrap();
        let metrics_second = route_request(
            &format!(
                "GET /api/agents/agent-1/metrics?limit=2&before={metrics_cursor} HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n"
            ),
            &store,
        )
        .unwrap();
        let metrics_second: serde_json::Value =
            serde_json::from_str(metrics_second.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(
            metrics_second["items"][0]["body"]["cpu"]["logical_count"],
            1
        );

        let logs_first = route_request(
            "GET /api/agents/agent-1/logs?limit=2 HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        let logs_first: serde_json::Value =
            serde_json::from_str(logs_first.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(
            logs_first["items"][0]["line"],
            "level=info event=agent_log_uploaded sequence=3"
        );
        assert_eq!(logs_first["items"][0]["collected_at_ms"], 3000);
        let logs_cursor = logs_first["next_cursor"].as_str().unwrap();
        let logs_second = route_request(
            &format!(
                "GET /api/agents/agent-1/logs?limit=2&before={logs_cursor} HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n"
            ),
            &store,
        )
        .unwrap();
        let logs_second: serde_json::Value =
            serde_json::from_str(logs_second.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(
            logs_second["items"][0]["line"],
            "level=info event=agent_log_uploaded sequence=1"
        );

        let drift_first = route_request(
            "GET /api/agents/agent-1/drift?limit=2 HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        let drift_first: serde_json::Value =
            serde_json::from_str(drift_first.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(drift_first["items"][0]["status"], "drifted");
        assert_eq!(drift_first["items"][0]["agent_system_time_ms"], 3000);
        let drift_cursor = drift_first["next_cursor"].as_str().unwrap();
        let drift_second = route_request(
            &format!(
                "GET /api/agents/agent-1/drift?limit=2&before={drift_cursor} HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n"
            ),
            &store,
        )
        .unwrap();
        let drift_second: serde_json::Value =
            serde_json::from_str(drift_second.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(drift_second["items"][0]["status"], "unknown");
    }

    #[test]
    fn admin_snapshot_page_rejects_invalid_query() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();

        let bad_limit = route_request(
            "GET /api/agents/agent-1/facts?limit=0 HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        let bad_cursor = route_request(
            "GET /api/agents/agent-1/metrics?before=not-a-cursor HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(bad_limit.starts_with("HTTP/1.1 400"));
        assert!(bad_cursor.starts_with("HTTP/1.1 400"));
    }

    #[test]
    fn admin_snapshot_page_omits_next_cursor_when_no_more_rows_exist() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        for body in ["{\"seq\":1}", "{\"seq\":2}"] {
            store
                .insert_facts_snapshot(
                    "agent-1",
                    body,
                    SystemTime::UNIX_EPOCH + Duration::from_secs(1),
                )
                .unwrap();
        }

        let response = route_request(
            "GET /api/agents/agent-1/facts?limit=2 HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        let response: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();

        assert_eq!(response["items"].as_array().unwrap().len(), 2);
        assert!(response["next_cursor"].is_null());
    }

    #[test]
    fn admin_latest_optional_agent_data_returns_null_when_missing() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");

        for path in [
            "/api/agents/agent-1/facts/latest",
            "/api/agents/agent-1/metrics/latest",
            "/api/agents/agent-1/drift/latest",
        ] {
            let response = route_request(
                &format!("GET {path} HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n"),
                &store,
            )
            .unwrap();

            assert!(
                response.starts_with("HTTP/1.1 200"),
                "{path} should return a successful empty optional response"
            );
            assert!(
                response.ends_with("\r\n\r\nnull\n"),
                "{path} should return JSON null when no latest record exists"
            );
        }
    }

    #[test]
    fn admin_can_list_jobs() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        let body = serde_json::to_string(&CreateCommandJobRequest {
            job_id: "job-history-1".to_owned(),
            target_agent_ids: vec!["agent-1".to_owned()],
            selector: None,
            match_labels: None,
            strategy: None,
            program: "uptime".to_owned(),
            args: vec!["-a".to_owned()],
            timeout_seconds: 30,
            confirmed_high_risk: true,
            confirmed_by: "admin-token".to_owned(),
            expires_in_seconds: 60,
            nonce_prefix: Some("job-history".to_owned()),
        })
        .unwrap();
        let request = format!(
            "POST /api/jobs/command HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        route_request_with_identity(
            &request,
            &store,
            &ControllerIdentity::dev_insecure(),
            &ControllerRuntimeMetadata::default(),
        )
        .unwrap();

        let response = route_request(
            "GET /api/jobs HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        let jobs: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let job = &jobs[0];

        assert!(response.starts_with("HTTP/1.1 200"));
        assert_eq!(job["id"], "job-history-1");
        assert_eq!(job["status"], "queued");
        assert_eq!(job["dispatch_state"], "queued");
        assert_eq!(job["command_program"], "uptime");
        assert_eq!(job["strategy"]["concurrency"], 1);
        assert_eq!(job["strategy"]["maxFailures"], serde_json::Value::Null);
        assert_eq!(job["assignment_summary"]["queued"], 1);
        assert_eq!(job["assignment_summary"]["canceled"], 0);
        assert_eq!(job["assignment_summary"]["expired"], 0);
        assert_eq!(job["assignment_summary"]["skipped"], 0);
        assert_eq!(job["target_count"], 1);
        assert_eq!(job["target_agent_ids"], serde_json::json!(["agent-1"]));
        assert_eq!(job["target_agents"][0]["agent_id"], "agent-1");
        assert_eq!(job["target_agents"][0]["assignment_status"], "queued");
        assert_eq!(job["target_agents"][0]["last_error"], "");
        assert_eq!(job["target_agents"][0]["connected"], false);
        assert!(job["expires_at_ms"].as_u64().is_some());
    }

    #[test]
    fn job_detail_api_includes_dispatch_and_connected_target_state() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        let (handle, _receiver) = session_handle(
            "agent-1",
            "conn-1",
            SystemTime::UNIX_EPOCH,
            vec!["persistent_session".to_owned()],
            Some(64),
        );
        let sessions = Arc::new(Mutex::new(AgentSessionRegistry::default()));
        sessions.lock().unwrap().register(handle);
        route_request_with_sessions(
            &command_job_request("job-detail-1", "agent-1", true),
            &store,
            &sessions,
        )
        .unwrap();

        let response = route_request_with_sessions(
            "GET /api/jobs/job-detail-1 HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
            &sessions,
        )
        .unwrap();
        let job: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert_eq!(job["id"], "job-detail-1");
        assert_eq!(job["status"], "running");
        assert_eq!(job["dispatch_state"], "delivered");
        assert_eq!(job["target_connected"], true);
        assert_eq!(job["target_agents"][0]["connected"], true);
        assert_eq!(job["target_agents"][0]["status"], "online");
        assert_eq!(job["target_agents"][0]["assignment_status"], "dispatched");
        assert!(job["target_agents"][0]["task_id"].as_str().is_some());
        assert_eq!(job["last_error"], "");
    }

    #[test]
    fn job_status_transition_response_separates_status_and_dispatch_state() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        route_request(
            &command_job_request("job-status-1", "agent-1", true),
            &store,
        )
        .unwrap();
        store
            .update_job_status("job-status-1", JobStatus::Expired)
            .unwrap();

        let response = route_request(
            "GET /api/jobs/job-status-1 HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();
        let job: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();

        assert_eq!(job["status"], "expired");
        assert_eq!(job["dispatch_state"], "expired");
        assert_eq!(job["target_connected"], false);
    }

    #[test]
    fn admin_can_list_audit_events_without_secret_values() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        store
            .write_audit_event(AuditEvent {
                category: AuditCategory::Security,
                action: "invalid_signature".to_owned(),
                actor: AuditActor::new("system"),
                target: AuditTarget::new("agent-1"),
                value: AuditValue::SecretRef("token=raw-secret".to_owned()),
                occurred_at: SystemTime::UNIX_EPOCH,
            })
            .unwrap();

        let response = route_request(
            "GET /api/audit HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"category\":\"security\""));
        assert!(response.contains("\"action\":\"invalid_signature\""));
        assert!(response.contains("\"value_kind\":\"secret_ref\""));
        assert!(!response.contains("raw-secret"));
    }

    #[test]
    fn admin_can_export_audit_events_with_category_cursor_and_redaction() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
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

        let response = route_request(
            "GET /api/audit/export?category=security&limit=1 HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"category\":\"security\""));
        assert!(response.contains("\"action\":\"invalid_signature\""));
        assert!(response.contains("\"value_kind\":\"secret_ref\""));
        assert!(!response.contains("raw-secret"));
        assert!(!response.contains("job_created"));

        let page: serde_json::Value =
            serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let next_cursor = page["next_cursor"].as_str().expect("next cursor");
        let next_response = route_request(
            &format!(
                "GET /api/audit/export?category=security&limit=1&before={} HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
                next_cursor.replace(':', "%3A")
            ),
            &store,
        )
        .unwrap();
        assert!(next_response.contains("\"action\":\"insecure_http_transport_enabled\""));
        assert!(!next_response.contains("invalid_signature"));
    }

    #[test]
    fn audit_route_contract_does_not_expose_update_or_delete() {
        assert!(!REST_API_ROUTE_CONTRACT.iter().any(|route| {
            route.path.starts_with("/api/audit")
                && matches!(route.method, "DELETE" | "PATCH" | "PUT" | "POST")
        }));
    }

    #[test]
    fn task_assignment_wire_includes_command_payload() {
        let envelope = TaskEnvelope {
            job_id: fleet_domain::JobId::new("job-1").unwrap(),
            task_id: fleet_domain::TaskId::new("task-1").unwrap(),
            target_agent_id: AgentId::new("agent-1").unwrap(),
            issued_at: SystemTime::UNIX_EPOCH,
            expires_at: fleet_domain::TaskExpiry::new(
                SystemTime::UNIX_EPOCH + Duration::from_secs(60),
            ),
            nonce: fleet_domain::TaskNonce::new("nonce-1").unwrap(),
            payload_hash: "hash".to_owned(),
            signature: Some(fleet_domain::TaskSignature::new("sig").unwrap()),
        };
        let command =
            fleet_domain::CommandTask::new("uptime", Vec::new(), Duration::from_secs(30)).unwrap();

        let envelope = task_envelope_to_wire(&envelope);
        let task = command_task_to_wire(&command);

        assert_eq!(envelope.job_id, "job-1");
        assert!(matches!(task, fleet_protocol::TaskWire::Command(_)));
    }

    #[test]
    fn agent_enroll_consumes_token_and_registers_agent() {
        let store = SqliteStore::in_memory().unwrap();
        let key_pair = fleet_core::generate_agent_key_pair().unwrap();
        store
            .insert_enrollment_token_hash(
                "et-1",
                &hash_token("enroll-token"),
                "role=web",
                SystemTime::now() + Duration::from_secs(60),
                1,
            )
            .unwrap();

        let body = serde_json::to_string(&EnrollAgentRequest {
            token: "enroll-token".to_owned(),
            agent_id: "agent-web-01".to_owned(),
            name: "web-01".to_owned(),
            public_key: key_pair.public_key_hex,
            fingerprint: key_pair.fingerprint,
            labels: vec![EnrollAgentLabel {
                key: "role".to_owned(),
                value: "web".to_owned(),
            }],
        })
        .unwrap();
        let request = format!(
            "POST /api/agents/enroll HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = route_request(&request, &store).unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert!(response.contains("\"controller_fingerprint\":\"dev-controller-fingerprint\""));
        assert_eq!(store.agent_count().unwrap(), 1);
        assert_eq!(store.list_enrollment_tokens().unwrap()[0].used_count, 1);
        let audits = store
            .list_audit_events_by_category(AuditCategory::Enrollment, 10)
            .unwrap();
        assert!(audits.iter().any(|event| {
            event.action == "enrollment_token_used"
                && event.actor.as_str() == "agent-web-01"
                && event.target.as_str() == "et-1"
                && !event.contains_secret_plaintext()
        }));
    }

    #[test]
    fn agent_enroll_applies_default_labels_from_token() {
        let store = SqliteStore::in_memory().unwrap();
        let key_pair = fleet_core::generate_agent_key_pair().unwrap();
        store
            .insert_enrollment_token_hash(
                "et-1",
                &hash_token("enroll-token"),
                "role=web,env=dev",
                SystemTime::now() + Duration::from_secs(60),
                1,
            )
            .unwrap();

        let body = serde_json::to_string(&EnrollAgentRequest {
            token: "enroll-token".to_owned(),
            agent_id: "agent-web-01".to_owned(),
            name: "web-01".to_owned(),
            public_key: key_pair.public_key_hex,
            fingerprint: key_pair.fingerprint,
            labels: vec![EnrollAgentLabel {
                key: "zone".to_owned(),
                value: "a".to_owned(),
            }],
        })
        .unwrap();
        let request = format!(
            "POST /api/agents/enroll HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = route_request(&request, &store).unwrap();
        let labels = store.list_agents().unwrap()[0]
            .labels()
            .iter()
            .map(|label| format!("{}={}", label.key(), label.value()))
            .collect::<Vec<_>>();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert_eq!(labels, ["role=web", "env=dev", "zone=a"]);
    }

    #[test]
    fn explicit_agent_label_overrides_token_default_label() {
        let store = SqliteStore::in_memory().unwrap();
        let key_pair = fleet_core::generate_agent_key_pair().unwrap();
        store
            .insert_enrollment_token_hash(
                "et-1",
                &hash_token("enroll-token"),
                "role=default,env=dev",
                SystemTime::now() + Duration::from_secs(60),
                1,
            )
            .unwrap();

        let body = serde_json::to_string(&EnrollAgentRequest {
            token: "enroll-token".to_owned(),
            agent_id: "agent-web-01".to_owned(),
            name: "web-01".to_owned(),
            public_key: key_pair.public_key_hex,
            fingerprint: key_pair.fingerprint,
            labels: vec![EnrollAgentLabel {
                key: "role".to_owned(),
                value: "web".to_owned(),
            }],
        })
        .unwrap();
        let request = format!(
            "POST /api/agents/enroll HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = route_request(&request, &store).unwrap();
        let labels = store.list_agents().unwrap()[0]
            .labels()
            .iter()
            .map(|label| format!("{}={}", label.key(), label.value()))
            .collect::<Vec<_>>();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert_eq!(labels, ["env=dev", "role=web"]);
    }

    #[test]
    fn agent_enroll_rejects_invalid_token() {
        let store = SqliteStore::in_memory().unwrap();
        let key_pair = fleet_core::generate_agent_key_pair().unwrap();
        let body = serde_json::to_string(&EnrollAgentRequest {
            token: "bad-token".to_owned(),
            agent_id: "agent-web-01".to_owned(),
            name: "web-01".to_owned(),
            public_key: key_pair.public_key_hex,
            fingerprint: key_pair.fingerprint,
            labels: Vec::new(),
        })
        .unwrap();
        let request = format!(
            "POST /api/agents/enroll HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = route_request(&request, &store).unwrap();

        assert!(response.starts_with("HTTP/1.1 401"));
    }

    #[test]
    fn agent_enroll_rejects_fingerprint_public_key_mismatch() {
        let store = SqliteStore::in_memory().unwrap();
        let key_pair = fleet_core::generate_agent_key_pair().unwrap();
        store
            .insert_enrollment_token_hash(
                "et-1",
                &hash_token("enroll-token"),
                "",
                SystemTime::now() + Duration::from_secs(60),
                1,
            )
            .unwrap();

        let body = serde_json::to_string(&EnrollAgentRequest {
            token: "enroll-token".to_owned(),
            agent_id: "agent-web-01".to_owned(),
            name: "web-01".to_owned(),
            public_key: key_pair.public_key_hex,
            fingerprint: "0123456789abcdef".to_owned(),
            labels: Vec::new(),
        })
        .unwrap();
        let request = format!(
            "POST /api/agents/enroll HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = route_request(&request, &store).unwrap();

        assert!(response.starts_with("HTTP/1.1 400"));
        assert!(response.contains("fingerprint does not match public key"));
    }

    #[test]
    fn duplicate_agent_name_is_conflict() {
        let store = SqliteStore::in_memory().unwrap();
        let first = fleet_core::generate_agent_key_pair().unwrap();
        let second = fleet_core::generate_agent_key_pair().unwrap();
        for (id, token, key_pair) in [
            ("agent-web-01", "enroll-token-1", first),
            ("agent-web-02", "enroll-token-2", second),
        ] {
            store
                .insert_enrollment_token_hash(
                    &format!("et-{id}"),
                    &hash_token(token),
                    "",
                    SystemTime::now() + Duration::from_secs(60),
                    1,
                )
                .unwrap();
            let body = serde_json::to_string(&EnrollAgentRequest {
                token: token.to_owned(),
                agent_id: id.to_owned(),
                name: "web-01".to_owned(),
                public_key: key_pair.public_key_hex,
                fingerprint: key_pair.fingerprint,
                labels: Vec::new(),
            })
            .unwrap();
            let request = format!(
                "POST /api/agents/enroll HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let response = route_request(&request, &store).unwrap();
            if id == "agent-web-01" {
                assert!(response.starts_with("HTTP/1.1 201"));
            } else {
                assert!(response.starts_with("HTTP/1.1 409"));
            }
        }
    }

    #[test]
    fn controller_resolves_default_database_settings_once_from_data_dir() {
        let config = ControllerServerConfig {
            host: "127.0.0.1".to_owned(),
            port: 7700,
            external_url: None,
            tls_cert_path: None,
            tls_key_path: None,
            agent_client_ca_cert_path: None,
            data_dir: PathBuf::from("/var/lib/sponzey-fleet"),
            database: None,
            secret_provider: None,
        };

        let database = controller_database_settings(&config).unwrap();
        assert_eq!(database.backend_name(), "sqlite");
        assert_eq!(
            database.sqlite_path(),
            Some(Path::new("/var/lib/sponzey-fleet/controller/fleet.db"))
        );
    }

    #[test]
    fn controller_resolves_default_artifact_store_settings_once_from_data_dir() {
        let config = ControllerServerConfig {
            host: "127.0.0.1".to_owned(),
            port: 7700,
            external_url: None,
            tls_cert_path: None,
            tls_key_path: None,
            agent_client_ca_cert_path: None,
            data_dir: PathBuf::from("/var/lib/sponzey-fleet"),
            database: None,
            secret_provider: None,
        };

        let settings = controller_artifact_store_settings(&config).unwrap();
        assert_eq!(settings.backend_name(), "local");
        assert_eq!(
            settings.local_root(),
            Path::new("/var/lib/sponzey-fleet/controller/artifacts")
        );

        let store = open_controller_artifact_store(&settings).unwrap();
        assert_eq!(
            store.root(),
            Path::new("/var/lib/sponzey-fleet/controller/artifacts")
        );
    }

    #[test]
    fn controller_secret_provider_factory_builds_disabled_from_default_settings() {
        let config = ControllerServerConfig {
            host: "127.0.0.1".to_owned(),
            port: 7700,
            external_url: None,
            tls_cert_path: None,
            tls_key_path: None,
            agent_client_ca_cert_path: None,
            data_dir: PathBuf::from("/var/lib/sponzey-fleet"),
            database: None,
            secret_provider: None,
        };

        let settings = controller_secret_provider_settings(&config);
        let provider = build_controller_secret_provider(&settings, None)
            .expect("default disabled provider should build");
        let reference = fleet_domain::SecretRef::parse("secret://app/disabled-token").unwrap();
        let error = provider.resolve_secret(&reference).unwrap_err();

        assert_eq!(settings.backend_name(), "disabled");
        assert_eq!(provider.mode(), "disabled");
        assert!(matches!(error, SecretProviderError::Denied { .. }));
        assert!(!error.to_string().contains("disabled-token"));
        assert!(!format!("{error:?}").contains("disabled-token"));
    }

    #[test]
    fn controller_secret_provider_factory_builds_static_test_from_explicit_source() {
        let reference = fleet_domain::SecretRef::parse("secret://app/api-token").unwrap();
        let settings =
            fleet_core::SecretProviderSettings::static_test(PathBuf::from("fixtures/secrets.json"))
                .unwrap();
        let provider_source =
            StaticSecretProvider::new().with_secret(reference.clone(), "raw-static-fixture-secret");

        let provider = build_controller_secret_provider(&settings, Some(provider_source))
            .expect("static-test provider should build with explicit source");
        let resolved = provider
            .resolve_secret(&reference)
            .expect("static provider should resolve");

        assert_eq!(provider.mode(), "static-test");
        assert_eq!(
            resolved.expose_secret_for_rendering(),
            "raw-static-fixture-secret"
        );
        assert!(!resolved.to_string().contains("raw-static-fixture-secret"));
        assert!(!format!("{resolved:?}").contains("raw-static-fixture-secret"));
    }

    #[test]
    fn controller_secret_provider_factory_rejects_static_test_without_source_redacted() {
        let settings =
            fleet_core::SecretProviderSettings::static_test(PathBuf::from("fixtures/secrets.json"))
                .unwrap();

        let error = build_controller_secret_provider(&settings, None)
            .expect_err("static-test provider should require explicit source");

        assert_eq!(
            error,
            ControllerSecretProviderConstructionError::StaticTestFixtureSourceRequired
        );
        assert!(!error.to_string().contains("fixtures/secrets.json"));
        assert!(!format!("{error:?}").contains("fixtures/secrets.json"));
    }

    #[test]
    fn controller_store_boundary_wraps_sqlite_backend() {
        let db_path = unique_test_dir("controller-store-boundary").join("fleet.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let database = fleet_core::DatabaseSettings::sqlite(db_path).unwrap();

        let store = open_controller_store(&database).unwrap();

        assert_eq!(store.backend_name(), "sqlite");
    }

    #[test]
    fn controller_repository_adapters_use_store_boundary() {
        let store = ControllerStore::sqlite(SqliteStore::in_memory().unwrap());
        let mut repo = ControllerAdminTokenRepository {
            store: (&store).into(),
        };

        repo.insert_admin_token_hash("hash-boundary").unwrap();

        assert!(repo.verify_admin_token_hash("hash-boundary").unwrap());
    }

    #[test]
    fn controller_repository_adapters_hold_store_boundary_ref() {
        fn assert_store_ref(_: ControllerStoreRef<'_>) {}

        let store = ControllerStore::sqlite(SqliteStore::in_memory().unwrap());
        let repo = ControllerAdminTokenRepository {
            store: (&store).into(),
        };

        assert_store_ref(repo.store);
    }

    #[cfg(not(feature = "postgres"))]
    #[test]
    fn controller_rejects_postgres_backend_at_bootstrap_without_url_leak() {
        let config = ControllerServerConfig {
            host: "127.0.0.1".to_owned(),
            port: 7700,
            external_url: None,
            tls_cert_path: None,
            tls_key_path: None,
            agent_client_ca_cert_path: None,
            data_dir: unique_test_dir("controller-postgres-unsupported"),
            database: Some(
                fleet_core::DatabaseSettings::postgres(
                    "postgresql://fleet:secret@db.example.com/fleet".to_owned(),
                )
                .unwrap(),
            ),
            secret_provider: None,
        };

        let error = start_controller_server_until(config, || true)
            .expect_err("postgres backend is not implemented yet");
        let message = error.to_string();
        assert!(matches!(
            error,
            ControllerError::UnsupportedDatabaseBackend(ref backend) if backend == "postgres"
        ));
        assert!(!message.contains("secret"));
        assert!(!message.contains("db.example.com"));
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn controller_postgres_feature_exposes_store_boundary() {
        fn assert_postgres_constructor(_: fn(fleet_store::PostgresStore) -> ControllerStore) {}

        assert_postgres_constructor(ControllerStore::postgres);
        assert_eq!(ControllerStore::postgres_backend_name(), "postgres");
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn controller_store_ref_postgres_variant_holds_store_reference() {
        fn assert_postgres_ref_variant(store_ref: ControllerStoreRef<'_>) {
            if let ControllerStoreRef::Postgres(_) = store_ref {}
        }

        let _ = assert_postgres_ref_variant;
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn controller_postgres_connect_failure_redacts_url() {
        let database = fleet_core::DatabaseSettings::postgres(
            "postgresql://fleet:secret@127.0.0.1:1/fleet".to_owned(),
        )
        .unwrap();

        let error = match open_controller_store(&database) {
            Ok(_) => panic!("postgres connection should fail"),
            Err(error) => error,
        };
        let message = error.to_string();

        assert!(!message.contains("secret"));
        assert!(!message.contains("127.0.0.1"));
        assert!(!message.contains("fleet:"));
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn controller_postgres_tls_sslmode_connection_failure_redacts_url() {
        let database = fleet_core::DatabaseSettings::postgres(
            "postgresql://fleet:secret@127.0.0.1:1/fleet?sslmode=require".to_owned(),
        )
        .unwrap();

        let error = match open_controller_store(&database) {
            Ok(_) => panic!("postgres TLS connection should fail"),
            Err(error) => error,
        };
        let message = error.to_string();

        assert!(message.contains("postgres connection failed"));
        assert!(!message.contains("secret"));
        assert!(!message.contains("127.0.0.1"));
        assert!(!message.contains("fleet:"));
    }

    #[test]
    fn controller_allows_remote_bind_with_https_external_url() {
        let config = ControllerServerConfig {
            host: "0.0.0.0".to_owned(),
            port: 7700,
            external_url: Some("https://fleet.example.com".to_owned()),
            tls_cert_path: None,
            tls_key_path: None,
            agent_client_ca_cert_path: None,
            data_dir: PathBuf::from(".sponzey"),
            database: None,
            secret_provider: None,
        };

        assert!(validate_transport(&config).is_ok());
    }

    #[test]
    fn controller_allows_remote_http_external_url() {
        let config = ControllerServerConfig {
            host: "127.0.0.1".to_owned(),
            port: 7700,
            external_url: Some("http://192.168.0.10:7700".to_owned()),
            tls_cert_path: None,
            tls_key_path: None,
            agent_client_ca_cert_path: None,
            data_dir: PathBuf::from(".sponzey"),
            database: None,
            secret_provider: None,
        };

        assert!(validate_transport(&config).is_ok());
        assert_eq!(
            insecure_http_transport_target(&config).as_deref(),
            Some("http://192.168.0.10:7700")
        );
    }

    #[test]
    fn controller_marks_plain_http_listener_without_external_url() {
        let config = ControllerServerConfig {
            host: "127.0.0.1".to_owned(),
            port: 7700,
            external_url: None,
            tls_cert_path: None,
            tls_key_path: None,
            agent_client_ca_cert_path: None,
            data_dir: PathBuf::from(".sponzey"),
            database: None,
            secret_provider: None,
        };

        assert_eq!(
            insecure_http_transport_target(&config).as_deref(),
            Some("http://127.0.0.1:7700")
        );
    }

    #[test]
    fn controller_does_not_mark_https_external_url_behind_proxy_as_insecure() {
        let config = ControllerServerConfig {
            host: "127.0.0.1".to_owned(),
            port: 7700,
            external_url: Some("https://fleet.example.com".to_owned()),
            tls_cert_path: None,
            tls_key_path: None,
            agent_client_ca_cert_path: None,
            data_dir: PathBuf::from(".sponzey"),
            database: None,
            secret_provider: None,
        };

        assert_eq!(insecure_http_transport_target(&config), None);
    }

    #[test]
    fn controller_bind_error_explains_unassigned_host() {
        let error = controller_bind_error(
            "192.168.20.19:7700",
            std::io::Error::new(
                std::io::ErrorKind::AddrNotAvailable,
                "address not available",
            ),
        );

        let message = error.to_string();
        assert!(message.contains("failed to bind controller listener on 192.168.20.19:7700"));
        assert!(message.contains("--host is an IP address assigned to this machine"));
        assert!(message.contains("--host 0.0.0.0"));
        assert!(message.contains("--external-url"));
    }

    #[test]
    fn controller_requires_tls_cert_and_key_together() {
        let config = ControllerServerConfig {
            host: "127.0.0.1".to_owned(),
            port: 7700,
            external_url: Some("https://127.0.0.1:7700".to_owned()),
            tls_cert_path: Some(PathBuf::from("cert.pem")),
            tls_key_path: None,
            agent_client_ca_cert_path: None,
            data_dir: PathBuf::from(".sponzey"),
            database: None,
            secret_provider: None,
        };

        assert!(matches!(
            validate_transport(&config),
            Err(ControllerError::Tls(message)) if message.contains("--tls-cert and --tls-key")
        ));
    }

    #[test]
    fn controller_accepts_valid_tls_material() {
        let dir = unique_test_dir("controller-valid-tls");
        let (cert_path, key_path) = write_test_tls_material(&dir);
        let config = ControllerServerConfig {
            host: "127.0.0.1".to_owned(),
            port: 7700,
            external_url: Some("https://127.0.0.1:7700".to_owned()),
            tls_cert_path: Some(cert_path),
            tls_key_path: Some(key_path),
            agent_client_ca_cert_path: None,
            data_dir: PathBuf::from(".sponzey"),
            database: None,
            secret_provider: None,
        };

        assert!(validate_transport(&config).is_ok());
    }

    #[test]
    fn controller_rejects_agent_client_ca_cert_until_mtls_enforcement_exists() {
        let config = ControllerServerConfig {
            host: "127.0.0.1".to_owned(),
            port: 7700,
            external_url: Some("https://127.0.0.1:7700".to_owned()),
            tls_cert_path: None,
            tls_key_path: None,
            agent_client_ca_cert_path: Some(PathBuf::from("/etc/sponzey/agent-client-ca.pem")),
            data_dir: PathBuf::from(".sponzey"),
            database: None,
            secret_provider: None,
        };

        let error = validate_transport(&config).expect_err(
            "agent client certificate trust should be rejected until mTLS enforcement exists",
        );
        let message = error.to_string();

        assert!(message.contains("agent client certificate mTLS enforcement is not implemented"));
        assert!(!message.contains("agent-client-ca.pem"));
        assert!(!message.contains("/etc/sponzey"));
    }

    #[test]
    fn controller_trust_settings_separates_tls_and_signing_identity_paths() {
        let dir = unique_test_dir("controller-trust-separation");
        let tls_dir = dir.join("tls");
        let data_dir = dir.join("data");
        let (cert_path, key_path) = write_test_tls_material(&tls_dir);
        let config = ControllerServerConfig {
            host: "127.0.0.1".to_owned(),
            port: 7700,
            external_url: Some("https://127.0.0.1:7700".to_owned()),
            tls_cert_path: Some(cert_path.clone()),
            tls_key_path: Some(key_path.clone()),
            agent_client_ca_cert_path: None,
            data_dir: data_dir.clone(),
            database: None,
            secret_provider: None,
        };

        let settings =
            controller_trust_settings(&config).expect("trust settings should be constructed");

        assert_eq!(settings.tls_server().unwrap().cert_path(), cert_path);
        assert_eq!(settings.tls_server().unwrap().key_path(), key_path);
        assert_eq!(
            settings.controller_signing().public_key_path(),
            data_dir.join("controller").join("controller_public.key")
        );
        assert_eq!(
            settings.controller_signing().private_key_path(),
            data_dir.join("controller").join("controller_private.key")
        );
    }

    #[test]
    fn controller_trust_settings_reject_tls_key_reused_as_signing_key_without_path_leak() {
        let dir = unique_test_dir("controller-trust-key-reuse");
        let controller_dir = dir.join("controller");
        std::fs::create_dir_all(&controller_dir).unwrap();
        let cert_path = controller_dir.join("server.crt");
        let shared_key_path = controller_dir.join("controller_private.key");
        let config = ControllerServerConfig {
            host: "127.0.0.1".to_owned(),
            port: 7700,
            external_url: Some("https://127.0.0.1:7700".to_owned()),
            tls_cert_path: Some(cert_path),
            tls_key_path: Some(shared_key_path),
            agent_client_ca_cert_path: None,
            data_dir: dir.clone(),
            database: None,
            secret_provider: None,
        };

        let error = controller_trust_settings(&config)
            .expect_err("TLS key reuse as signing key should fail");
        let message = error.to_string();

        assert!(message.contains("must be separate files"));
        assert!(!message.contains("controller_private.key"));
        assert!(!message.contains(dir.to_string_lossy().as_ref()));
    }

    #[test]
    fn controller_trust_settings_do_not_treat_tls_cert_as_signing_public_key() {
        let dir = unique_test_dir("controller-trust-cert-reuse");
        let controller_dir = dir.join("controller");
        std::fs::create_dir_all(&controller_dir).unwrap();
        let shared_public_path = controller_dir.join("controller_public.key");
        let tls_key_path = controller_dir.join("server.key");
        let config = ControllerServerConfig {
            host: "127.0.0.1".to_owned(),
            port: 7700,
            external_url: Some("https://127.0.0.1:7700".to_owned()),
            tls_cert_path: Some(shared_public_path),
            tls_key_path: Some(tls_key_path),
            agent_client_ca_cert_path: None,
            data_dir: dir.clone(),
            database: None,
            secret_provider: None,
        };

        let error = controller_trust_settings(&config)
            .expect_err("TLS cert reuse as signing public key should fail");
        let message = error.to_string();

        assert!(message.contains("must be separate files"));
        assert!(!message.contains("controller_public.key"));
        assert!(!message.contains(dir.to_string_lossy().as_ref()));
    }

    #[test]
    fn controller_task_signer_uses_explicit_signing_fingerprint_context() {
        let key_pair = fleet_core::generate_agent_key_pair().unwrap();
        let signer = ControllerTaskSigner {
            private_key: &key_pair.private_key_hex,
            signing_fingerprint: &key_pair.fingerprint,
        };

        assert_eq!(signer.signing_fingerprint, key_pair.fingerprint);
        assert!(!signer.signing_fingerprint.contains("PRIVATE KEY"));
        assert!(
            !signer
                .signing_fingerprint
                .contains(&key_pair.private_key_hex)
        );
    }

    #[test]
    fn controller_signing_runtime_guard_accepts_matching_steady_state() {
        let key_pair = fleet_core::generate_agent_key_pair().unwrap();
        let identity = controller_identity_from_key_pair(&key_pair);
        let rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new(key_pair.fingerprint.clone()).unwrap(),
        );
        let record = fleet_application::SigningKeyRotationRecord {
            controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
            rotation,
            updated_at: SystemTime::UNIX_EPOCH,
        };

        let guarded = guard_controller_signing_runtime_identity(
            identity.clone(),
            Some(record),
            SystemTime::UNIX_EPOCH,
        )
        .unwrap();

        assert_eq!(guarded.fingerprint, identity.fingerprint);
    }

    #[test]
    fn controller_signing_runtime_identity_accepts_missing_rotation_state_as_active_steady() {
        let dir = unique_test_dir("controller-signing-runtime-missing-state");
        let active_pair = write_default_controller_signing_key_pair(&dir);
        let store = ControllerStore::sqlite(SqliteStore::in_memory().unwrap());

        let identity =
            load_controller_signing_runtime_identity(&dir, &store, SystemTime::UNIX_EPOCH).unwrap();

        assert_eq!(identity.fingerprint, active_pair.key_pair.fingerprint);
    }

    #[test]
    fn controller_signing_runtime_identity_rejects_invalid_active_material_without_leak() {
        let dir = unique_test_dir("controller-signing-runtime-invalid-active");
        let public_pair = fleet_core::generate_agent_key_pair().unwrap();
        let private_pair = fleet_core::generate_agent_key_pair().unwrap();
        let controller_dir = dir.join("controller");
        std::fs::create_dir_all(&controller_dir).unwrap();
        std::fs::write(
            controller_dir.join("controller_public.key"),
            &public_pair.public_key_hex,
        )
        .unwrap();
        std::fs::write(
            controller_dir.join("controller_private.key"),
            &private_pair.private_key_hex,
        )
        .unwrap();
        set_secure_test_permissions(&controller_dir.join("controller_private.key"));
        let store = ControllerStore::sqlite(SqliteStore::in_memory().unwrap());

        let error = load_controller_signing_runtime_identity(&dir, &store, SystemTime::UNIX_EPOCH)
            .unwrap_err();
        let message = error.to_string();

        assert!(message.contains("active controller signing material failed validation"));
        assert!(!message.contains("controller_private.key"));
        assert!(!message.contains(&public_pair.private_key_hex));
        assert!(!message.contains(&private_pair.private_key_hex));
    }

    #[test]
    fn controller_signing_runtime_guard_accepts_dual_trust_selected_new_key() {
        let old_pair = fleet_core::generate_agent_key_pair().unwrap();
        let new_pair = fleet_core::generate_agent_key_pair().unwrap();
        let identity = controller_identity_from_key_pair(&new_pair);
        let mut rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new(old_pair.fingerprint).unwrap(),
        );
        rotation
            .request_rotation(
                fleet_domain::SigningKeyFingerprint::new(new_pair.fingerprint.clone()).unwrap(),
                SystemTime::UNIX_EPOCH + Duration::from_secs(10),
                SystemTime::UNIX_EPOCH + Duration::from_secs(40),
            )
            .unwrap();
        rotation
            .validate_new_material(SystemTime::UNIX_EPOCH + Duration::from_secs(11))
            .unwrap();
        rotation
            .activate_dual_trust(SystemTime::UNIX_EPOCH + Duration::from_secs(20))
            .unwrap();
        let record = fleet_application::SigningKeyRotationRecord {
            controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
            rotation,
            updated_at: SystemTime::UNIX_EPOCH + Duration::from_secs(20),
        };

        let guarded = guard_controller_signing_runtime_identity(
            identity,
            Some(record),
            SystemTime::UNIX_EPOCH + Duration::from_secs(21),
        )
        .unwrap();

        assert_eq!(guarded.fingerprint, new_pair.fingerprint);
    }

    #[test]
    fn controller_signing_runtime_guard_rejects_mismatched_active_material_without_leak() {
        let active_pair = fleet_core::generate_agent_key_pair().unwrap();
        let expected_pair = fleet_core::generate_agent_key_pair().unwrap();
        let identity = ControllerIdentity {
            public_key: active_pair.public_key_hex.clone(),
            fingerprint: active_pair.fingerprint.clone(),
            private_key: format!("secret-private-{}", active_pair.private_key_hex),
        };
        let rotation = fleet_domain::ControllerSigningKeyRotation::steady(
            fleet_domain::SigningKeyFingerprint::new(expected_pair.fingerprint.clone()).unwrap(),
        );
        let record = fleet_application::SigningKeyRotationRecord {
            controller_id: DEFAULT_CONTROLLER_ID.to_owned(),
            rotation,
            updated_at: SystemTime::UNIX_EPOCH,
        };

        let error = guard_controller_signing_runtime_identity(
            identity,
            Some(record),
            SystemTime::UNIX_EPOCH,
        )
        .unwrap_err();
        let message = error.to_string();

        assert!(message.contains("does not match persisted signing rotation state"));
        assert!(!message.contains(&active_pair.private_key_hex));
        assert!(!message.contains(&expected_pair.private_key_hex));
        assert!(!message.contains("controller_private.key"));
    }

    #[test]
    fn controller_signing_runtime_load_error_is_redacted_for_corrupt_persisted_state() {
        let error = controller_signing_rotation_load_error(fleet_store::StoreError::Domain(
            "invalid signing key rotation state in store from controller_private.key secret-body"
                .to_owned(),
        ));
        let message = error.to_string();

        assert!(message.contains("persisted controller signing rotation state"));
        assert!(!message.contains("controller_private.key"));
        assert!(!message.contains("secret-body"));
    }

    #[test]
    fn controller_signing_candidate_files_validate_and_return_expected_fingerprint() {
        let dir = unique_test_dir("controller-signing-candidate-valid");
        let active_pair = controller_signing_test_pair(&dir.join("active"), "active");
        let candidate_key_pair =
            write_controller_signing_key_pair(&dir.join("candidate"), "candidate");

        let candidate =
            validate_controller_signing_key_candidate(&ControllerSigningKeyCandidateInput {
                candidate: candidate_key_pair.files.clone(),
                active: active_pair.files,
                disallowed_paths: Vec::new(),
                challenge: "controller-signing-rotation-validation".to_owned(),
            })
            .unwrap();

        assert_eq!(
            candidate.fingerprint,
            candidate_key_pair.key_pair.fingerprint
        );
    }

    #[cfg(unix)]
    #[test]
    fn controller_signing_candidate_private_key_insecure_permissions_are_rejected_without_path_leak()
     {
        let dir = unique_test_dir("controller-signing-candidate-insecure");
        let active_pair = controller_signing_test_pair(&dir.join("active"), "active");
        let candidate_pair = write_controller_signing_key_pair(&dir.join("candidate"), "candidate");
        std::fs::set_permissions(
            &candidate_pair.files.private_key_path,
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();

        let error =
            validate_controller_signing_key_candidate(&ControllerSigningKeyCandidateInput {
                candidate: candidate_pair.files,
                active: active_pair.files,
                disallowed_paths: Vec::new(),
                challenge: "controller-signing-rotation-validation".to_owned(),
            })
            .unwrap_err();
        let message = error.to_string();

        assert!(message.contains("permissions are insecure"));
        assert!(!message.contains("controller_private.key"));
        assert!(!message.contains(dir.to_string_lossy().as_ref()));
        assert!(!message.contains(&candidate_pair.key_pair.private_key_hex));
    }

    #[test]
    fn controller_signing_candidate_rejects_active_or_transport_path_reuse_without_path_leak() {
        let dir = unique_test_dir("controller-signing-candidate-path-reuse");
        let active_pair = controller_signing_test_pair(&dir.join("active"), "active");
        let candidate_pair = write_controller_signing_key_pair(&dir.join("candidate"), "candidate");
        let tls_key_path = candidate_pair.files.private_key_path.clone();

        let error =
            validate_controller_signing_key_candidate(&ControllerSigningKeyCandidateInput {
                candidate: candidate_pair.files,
                active: active_pair.files,
                disallowed_paths: vec![tls_key_path],
                challenge: "controller-signing-rotation-validation".to_owned(),
            })
            .unwrap_err();
        let message = error.to_string();

        assert!(message.contains("must be separate"));
        assert!(!message.contains("controller_private.key"));
        assert!(!message.contains(dir.to_string_lossy().as_ref()));
    }

    #[test]
    fn controller_signing_key_file_swap_replaces_active_files_and_writes_backup() {
        let dir = unique_test_dir("controller-signing-swap-success");
        let active_pair = write_controller_signing_key_pair(&dir.join("active"), "active");
        let candidate_pair = write_controller_signing_key_pair(&dir.join("candidate"), "candidate");
        let backup_dir = dir.join("backup");

        let outcome = swap_controller_signing_key_files(&ControllerSigningKeySwapInput {
            candidate: candidate_pair.files.clone(),
            active: active_pair.files.clone(),
            backup_dir: backup_dir.clone(),
            disallowed_paths: Vec::new(),
            challenge: "controller-signing-rotation-validation".to_owned(),
        })
        .unwrap();

        assert_eq!(
            outcome.final_state,
            ControllerSigningKeySwapState::Completed
        );
        assert_eq!(outcome.fingerprint, candidate_pair.key_pair.fingerprint);
        assert_eq!(
            std::fs::read_to_string(&active_pair.files.public_key_path)
                .unwrap()
                .trim(),
            candidate_pair.key_pair.public_key_hex
        );
        assert_eq!(
            std::fs::read_to_string(&active_pair.files.private_key_path)
                .unwrap()
                .trim(),
            candidate_pair.key_pair.private_key_hex
        );
        assert_eq!(
            std::fs::read_to_string(backup_dir.join("controller_public.key.bak"))
                .unwrap()
                .trim(),
            active_pair.key_pair.public_key_hex
        );
    }

    #[test]
    fn controller_signing_key_file_swap_rolls_back_after_public_swap_failure() {
        let dir = unique_test_dir("secret-controller-signing-swap-failure");
        let active_pair = write_controller_signing_key_pair(&dir.join("active"), "active");
        let candidate_pair = write_controller_signing_key_pair(&dir.join("candidate"), "candidate");
        let backup_dir = dir.join("backup");

        let error = swap_controller_signing_key_files_inner(
            &ControllerSigningKeySwapInput {
                candidate: candidate_pair.files,
                active: active_pair.files.clone(),
                backup_dir,
                disallowed_paths: Vec::new(),
                challenge: "controller-signing-rotation-validation".to_owned(),
            },
            Some(ControllerSigningKeySwapFault::AfterPublicKeySwap),
        )
        .unwrap_err();
        let message = error.to_string();

        assert!(message.contains("rollback completed"));
        assert!(!message.contains("secret-controller-signing-swap-failure"));
        assert!(!message.contains(&active_pair.key_pair.private_key_hex));
        assert_eq!(
            std::fs::read_to_string(&active_pair.files.public_key_path)
                .unwrap()
                .trim(),
            active_pair.key_pair.public_key_hex
        );
        assert_eq!(
            std::fs::read_to_string(&active_pair.files.private_key_path)
                .unwrap()
                .trim(),
            active_pair.key_pair.private_key_hex
        );
    }

    #[test]
    fn controller_rejects_invalid_tls_material() {
        let dir = unique_test_dir("controller-invalid-tls");
        std::fs::create_dir_all(&dir).unwrap();
        let cert_path = dir.join("cert.pem");
        let key_path = dir.join("key.pem");
        std::fs::write(&cert_path, "not a certificate").unwrap();
        std::fs::write(&key_path, "not a key").unwrap();
        set_secure_test_permissions(&key_path);

        assert!(matches!(
            validate_tls_material(&cert_path, &key_path),
            Err(ControllerError::Tls(message)) if message.contains("no certificates")
                || message.contains("parse TLS certificate")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn controller_rejects_group_readable_tls_private_key() {
        let dir = unique_test_dir("controller-insecure-tls-key");
        let (cert_path, key_path) = write_test_tls_material(&dir);
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert!(matches!(
            validate_tls_material(&cert_path, &key_path),
            Err(ControllerError::Tls(message)) if message.contains("must not be readable")
        ));
    }

    #[test]
    fn controller_server_starts_and_stops_on_shutdown_signal() {
        let data_dir = unique_test_dir("controller-shutdown");
        write_default_controller_signing_key_pair(&data_dir);

        let Some(port) = free_loopback_port() else {
            return;
        };
        let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_shutdown = shutdown.clone();
        let handle = std::thread::spawn(move || {
            start_controller_server_until(
                ControllerServerConfig {
                    host: "127.0.0.1".to_owned(),
                    port,
                    external_url: Some(format!("http://127.0.0.1:{port}")),
                    tls_cert_path: None,
                    tls_key_path: None,
                    agent_client_ca_cert_path: None,
                    data_dir,
                    database: None,
                    secret_provider: None,
                },
                move || thread_shutdown.load(std::sync::atomic::Ordering::SeqCst),
            )
            .unwrap();
        });

        let response = poll_http_get(port, "/healthz");
        shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        handle.join().unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"status\":\"ok\""));
    }

    #[test]
    fn controller_https_server_starts_with_self_signed_fixture() {
        let data_dir = unique_test_dir("controller-https-shutdown");
        write_default_controller_signing_key_pair(&data_dir);
        let (cert_path, key_path) = write_test_tls_material(&data_dir.join("tls"));

        let Some(port) = free_loopback_port() else {
            return;
        };
        let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_shutdown = shutdown.clone();
        let thread_cert_path = cert_path.clone();
        let handle = std::thread::spawn(move || {
            start_controller_server_until(
                ControllerServerConfig {
                    host: "127.0.0.1".to_owned(),
                    port,
                    external_url: Some(format!("https://localhost:{port}")),
                    tls_cert_path: Some(thread_cert_path),
                    tls_key_path: Some(key_path),
                    agent_client_ca_cert_path: None,
                    data_dir,
                    database: None,
                    secret_provider: None,
                },
                move || thread_shutdown.load(std::sync::atomic::Ordering::SeqCst),
            )
            .unwrap();
        });

        let response = poll_https_get(port, "/healthz", &cert_path);
        shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        handle.join().unwrap();

        assert!(response.contains("\"status\":\"ok\""));
    }

    #[test]
    fn auth_failure_writes_security_audit() {
        let store = SqliteStore::in_memory().unwrap();

        audit_security(&store, "websocket_invalid_signature", "agent-1").unwrap();

        assert_eq!(
            store
                .audit_count_by_category(fleet_domain::AuditCategory::Security)
                .unwrap(),
            1
        );
    }

    #[test]
    fn insecure_http_transport_start_is_audited() {
        let store = SqliteStore::in_memory().unwrap();

        audit_insecure_http_transport_enabled(&store, "http://192.168.0.10:7700").unwrap();

        let audits = store
            .list_audit_events_by_category(AuditCategory::Security, 10)
            .unwrap();
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].action, "insecure_http_transport_enabled");
        assert_eq!(audits[0].actor.as_str(), "controller");
        assert_eq!(audits[0].target.as_str(), "http://192.168.0.10:7700");
        assert_eq!(
            audits[0].value,
            AuditValue::Plain("http_without_tls".to_owned())
        );
    }

    #[test]
    fn invalid_agent_signature_is_rejected() {
        let key_pair = fleet_core::generate_agent_key_pair().unwrap();
        let signature = fleet_core::sign_challenge(&key_pair.private_key_hex, "nonce-1").unwrap();

        assert!(!verify_agent_auth_response(
            &key_pair.public_key_hex,
            "nonce-2",
            "nonce-2",
            &signature
        ));
    }

    #[test]
    fn unknown_agent_id_is_rejected_and_audited() {
        let store = SqliteStore::in_memory().unwrap();

        let result = validate_agent_ws_hello(&store, "agent-missing", "fingerprint").unwrap();

        assert!(result.is_none());
        assert_eq!(
            store
                .audit_count_by_category(fleet_domain::AuditCategory::Security)
                .unwrap(),
            1
        );
    }

    #[test]
    fn enrollment_token_cannot_authenticate_websocket_channel() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_enrollment_token_hash(
                "et-1",
                &hash_token("enroll-token"),
                "",
                SystemTime::now() + Duration::from_secs(60),
                1,
            )
            .unwrap();

        let result = validate_agent_ws_hello(&store, "enroll-token", "0123456789abcdef").unwrap();

        assert!(result.is_none());
        assert_eq!(
            store
                .audit_count_by_category(fleet_domain::AuditCategory::Security)
                .unwrap(),
            1
        );
        assert_eq!(store.list_enrollment_tokens().unwrap()[0].used_count, 0);
    }

    #[test]
    fn mismatched_agent_fingerprint_is_rejected_and_audited() {
        let store = SqliteStore::in_memory().unwrap();
        let key_pair = fleet_core::generate_agent_key_pair().unwrap();
        store
            .save_agent(agent_fixture(
                "agent-web-01",
                "web-01",
                &key_pair.public_key_hex,
                &key_pair.fingerprint,
            ))
            .unwrap();

        let result = validate_agent_ws_hello(&store, "agent-web-01", "0123456789abcdef").unwrap();

        assert!(result.is_none());
        assert_eq!(
            store
                .audit_count_by_category(fleet_domain::AuditCategory::Security)
                .unwrap(),
            1
        );
    }

    #[test]
    fn revoked_agent_reconnect_is_rejected_and_audited_as_auth_failed() {
        let store = SqliteStore::in_memory().unwrap();
        save_disabled_test_agent_with_labels(&store, "agent-revoked", vec![("role", "web")]);

        let result = validate_agent_ws_hello(&store, "agent-revoked", "fingerprint").unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Security, 10)
            .unwrap();

        assert!(result.is_none());
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].action, "agent_session_auth_failed");
        assert!(matches!(
            &audits[0].value,
            AuditValue::Plain(value) if value.contains("reason=revoked")
        ));
    }

    fn agent_fixture(id: &str, name: &str, public_key: &str, fingerprint: &str) -> Agent {
        Agent::new(
            AgentId::new(id).unwrap(),
            AgentName::new(name).unwrap(),
            AgentIdentity {
                public_key: AgentPublicKey::new(public_key).unwrap(),
                fingerprint: AgentFingerprint::new(fingerprint).unwrap(),
            },
        )
    }

    fn route_request_with_sessions(
        request: &str,
        store: &SqliteStore,
        sessions: &Arc<Mutex<AgentSessionRegistry>>,
    ) -> Result<String, ControllerError> {
        route_request_with_identity_and_sessions(
            request,
            store,
            None,
            &ControllerIdentity::dev_insecure(),
            &ControllerRuntimeMetadata::default(),
            Some(sessions),
        )
    }

    fn approve_pending_job_with_sessions(
        store: &SqliteStore,
        sessions: &Arc<Mutex<AgentSessionRegistry>>,
        job_id: &str,
    ) {
        let approval = store
            .find_pending_approval_for_job(job_id)
            .unwrap()
            .expect("pending approval should exist");
        let body = serde_json::to_string(&ApprovalDecisionRequest {
            actor: "approver-1".to_owned(),
            reason: "approved in test".to_owned(),
        })
        .unwrap();
        let request = format!(
            "POST /api/approvals/{}/approve HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            approval.id,
            body.len(),
            body
        );
        let response = route_request_with_sessions(&request, store, sessions).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
    }

    fn command_job_request(job_id: &str, agent_id: &str, confirmed_high_risk: bool) -> String {
        let body = serde_json::to_string(&CreateCommandJobRequest {
            job_id: job_id.to_owned(),
            target_agent_ids: vec![agent_id.to_owned()],
            selector: None,
            match_labels: None,
            strategy: None,
            program: "uptime".to_owned(),
            args: Vec::new(),
            timeout_seconds: 30,
            confirmed_high_risk,
            confirmed_by: "operator-1".to_owned(),
            expires_in_seconds: 60,
            nonce_prefix: Some(format!("nonce-{job_id}")),
        })
        .unwrap();
        format!(
            "POST /api/jobs/command HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn high_risk_command_job_request(job_id: &str, agent_id: &str, program: &str) -> String {
        let body = serde_json::to_string(&CreateCommandJobRequest {
            job_id: job_id.to_owned(),
            target_agent_ids: vec![agent_id.to_owned()],
            selector: None,
            match_labels: None,
            strategy: None,
            program: program.to_owned(),
            args: vec!["-lc".to_owned(), "uptime".to_owned()],
            timeout_seconds: 30,
            confirmed_high_risk: false,
            confirmed_by: "operator-1".to_owned(),
            expires_in_seconds: 60,
            nonce_prefix: Some(format!("nonce-{job_id}")),
        })
        .unwrap();
        format!(
            "POST /api/jobs/command HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn command_selector_job_request(
        job_id: &str,
        selector: &str,
        strategy: Option<JobStrategyRequest>,
    ) -> String {
        let body = serde_json::to_string(&CreateCommandJobRequest {
            job_id: job_id.to_owned(),
            target_agent_ids: Vec::new(),
            selector: Some(selector.to_owned()),
            match_labels: None,
            strategy,
            program: "uptime".to_owned(),
            args: Vec::new(),
            timeout_seconds: 30,
            confirmed_high_risk: true,
            confirmed_by: "operator-1".to_owned(),
            expires_in_seconds: 60,
            nonce_prefix: Some(format!("nonce-{job_id}")),
        })
        .unwrap();
        format!(
            "POST /api/jobs/command HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn runbook_job_request(job_id: &str, agent_id: &str) -> String {
        let body = serde_json::to_string(&CreateRunbookJobRequest {
            job_id: job_id.to_owned(),
            target_agent_ids: vec![agent_id.to_owned()],
            selector: None,
            match_labels: None,
            strategy: None,
            runbook_document: r#"
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Runbook
metadata:
  name: task006-runbook
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
            timeout_seconds: 30,
            confirmed_high_risk: true,
            confirmed_by: "operator-1".to_owned(),
            expires_in_seconds: 60,
            nonce_prefix: Some(format!("nonce-{job_id}")),
        })
        .unwrap();
        format!(
            "POST /api/jobs/runbook HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn drift_check_job_request(job_id: &str, agent_id: &str) -> String {
        let body = serde_json::to_string(&CreateDriftCheckJobRequest {
            job_id: job_id.to_owned(),
            target_agent_ids: vec![agent_id.to_owned()],
            selector: None,
            match_labels: None,
            strategy: None,
            policy_document: r#"
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Policy
metadata:
  name: task006-policy
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
            .to_owned(),
            timeout_seconds: 30,
            created_by: "operator-1".to_owned(),
            expires_in_seconds: 60,
            nonce_prefix: Some(format!("nonce-{job_id}")),
        })
        .unwrap();
        format!(
            "POST /api/jobs/drift-check HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn admin_json_post(path: &str, body: &str) -> String {
        format!(
            "POST {path} HTTP/1.1\r\nAuthorization: Bearer admin-token\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn controller_remediation_request_record(
        id: &str,
        agent_id: &str,
        policy_id: &str,
        status: &str,
        job_id: Option<&str>,
    ) -> RemediationRequestRecord {
        RemediationRequestRecord {
            id: id.to_owned(),
            policy_id: policy_id.to_owned(),
            policy_name: policy_id.to_owned(),
            agent_id: agent_id.to_owned(),
            runbook_ref: "runbooks/remediate.yml".to_owned(),
            status: status.to_owned(),
            approval_required: true,
            risk_summary: "drifted policy requires approved remediation".to_owned(),
            job_id: job_id.map(str::to_owned),
            created_at: SystemTime::UNIX_EPOCH,
            updated_at: SystemTime::UNIX_EPOCH,
        }
    }

    fn remediation_runbook_document_with_secret_marker() -> String {
        r#"
# secret-value-should-not-leak
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Runbook
metadata:
  name: remediation-runbook
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

    fn assert_remediation_surface_excludes_payloads(response: &str) {
        for needle in [
            "kind: Runbook",
            "runbook_document",
            "command_output",
            "rendered_body",
            "secret-value-should-not-leak",
        ] {
            assert!(
                !response.contains(needle),
                "remediation HTTP surface leaked {needle}: {response}"
            );
        }
    }

    fn assert_audit_values_exclude_payloads(events: &[AuditEvent]) {
        for event in events {
            let value = match &event.value {
                AuditValue::Plain(value) | AuditValue::SecretRef(value) => value.as_str(),
                AuditValue::Redacted => "",
            };
            for needle in [
                "kind: Runbook",
                "runbook_document",
                "command_output",
                "rendered_body",
                "secret-value-should-not-leak",
            ] {
                assert!(
                    !value.contains(needle),
                    "remediation audit value leaked {needle}: {value}"
                );
            }
        }
    }

    fn save_test_agent(store: &SqliteStore, id: &str) {
        let key_pair = fleet_core::generate_agent_key_pair().unwrap();
        store
            .save_agent(agent_fixture(
                id,
                id,
                &key_pair.public_key_hex,
                &key_pair.fingerprint,
            ))
            .unwrap();
    }

    fn session_handle(
        agent_id: &str,
        connection_id: &str,
        connected_at: SystemTime,
        capabilities: Vec<String>,
        queue_capacity: Option<usize>,
    ) -> (
        AgentSessionHandle,
        mpsc::Receiver<AgentSessionOutboundMessage>,
    ) {
        let capacity = queue_capacity.unwrap_or(AGENT_SESSION_OUTBOUND_QUEUE_CAPACITY);
        let (sender, receiver) = mpsc::channel(capacity);
        (
            AgentSessionHandle::new(
                agent_id,
                connection_id,
                connected_at,
                capabilities,
                sender,
                queue_capacity,
            ),
            receiver,
        )
    }

    fn controller_test_agent_certificate_lifecycle_record()
    -> fleet_application::AgentCertificateLifecycleRecord {
        let agent_id = AgentId::new("agent-1").unwrap();
        let mut lifecycle = fleet_domain::AgentCertificateLifecycle::new(agent_id.clone());
        let policy = fleet_domain::AgentCertificateRenewalPolicy::new(
            Duration::from_secs(30),
            Duration::from_secs(10),
        )
        .unwrap();
        lifecycle
            .request_issuance(UNIX_EPOCH + Duration::from_secs(10))
            .unwrap();
        lifecycle
            .issue(
                controller_test_agent_certificate("serial-1", "0123456789abcdef", 10, 110),
                UNIX_EPOCH + Duration::from_secs(11),
            )
            .unwrap();
        lifecycle
            .request_renewal(UNIX_EPOCH + Duration::from_secs(80), &policy)
            .unwrap();
        lifecycle
            .activate_renewal(
                controller_test_agent_certificate("serial-2", "fedcba9876543210", 80, 200),
                UNIX_EPOCH + Duration::from_secs(81),
                &policy,
            )
            .unwrap();
        fleet_application::AgentCertificateLifecycleRecord {
            agent_id,
            lifecycle: lifecycle.snapshot(),
            updated_at: UNIX_EPOCH + Duration::from_secs(82),
        }
    }

    fn controller_test_agent_certificate(
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

    fn task_assignment_wire_message(agent_id: &str) -> fleet_protocol::WireMessage {
        fleet_protocol::WireMessage::new(
            "msg-task",
            "corr-task",
            Some(agent_id.to_owned()),
            1,
            fleet_protocol::WirePayload::TaskAssignment {
                envelope: fleet_protocol::SignedTaskEnvelopeWire {
                    job_id: "job-1".to_owned(),
                    task_id: "task-1".to_owned(),
                    target_agent_id: agent_id.to_owned(),
                    issued_at_ms: 1,
                    expires_at_ms: 60_000,
                    nonce: "nonce-1".to_owned(),
                    payload_hash: "hash".to_owned(),
                    signature: "sig".to_owned(),
                },
                task: fleet_protocol::TaskWire::Command(fleet_protocol::CommandTaskWire {
                    program: "uptime".to_owned(),
                    args: Vec::new(),
                    timeout_ms: 30_000,
                    max_output_bytes: 1024,
                }),
            },
        )
    }

    fn save_test_agent_with_labels(store: &SqliteStore, id: &str, labels: Vec<(&str, &str)>) {
        let key_pair = fleet_core::generate_agent_key_pair().unwrap();
        let mut agent = agent_fixture(id, id, &key_pair.public_key_hex, &key_pair.fingerprint);
        agent.set_labels(
            labels
                .into_iter()
                .map(|(key, value)| AgentLabel::new(key, value).unwrap())
                .collect(),
        );
        store.save_agent(agent).unwrap();
    }

    fn save_disabled_test_agent_with_labels(
        store: &SqliteStore,
        id: &str,
        labels: Vec<(&str, &str)>,
    ) {
        let key_pair = fleet_core::generate_agent_key_pair().unwrap();
        let mut agent = agent_fixture(id, id, &key_pair.public_key_hex, &key_pair.fingerprint);
        agent.set_labels(
            labels
                .into_iter()
                .map(|(key, value)| AgentLabel::new(key, value).unwrap())
                .collect(),
        );
        agent.disable();
        store.save_agent(agent).unwrap();
    }

    fn save_test_job(store: &SqliteStore, id: &str) {
        let mut job = Job::new(
            fleet_domain::JobId::new(id).unwrap(),
            fleet_domain::TaskRisk::High,
            fleet_domain::ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        job.queue(true).unwrap();
        store.save_job_record(&job).unwrap();
    }

    fn save_test_assignment(store: &SqliteStore, job_id: &str, task_id: &str, agent_id: &str) {
        store
            .save_task_assignment_record(&TaskEnvelope {
                job_id: JobId::new(job_id).unwrap(),
                task_id: fleet_domain::TaskId::new(task_id).unwrap(),
                target_agent_id: AgentId::new(agent_id).unwrap(),
                issued_at: SystemTime::UNIX_EPOCH,
                expires_at: fleet_domain::TaskExpiry::new(
                    SystemTime::UNIX_EPOCH + Duration::from_secs(60),
                ),
                nonce: fleet_domain::TaskNonce::new(format!("{task_id}-nonce")).unwrap(),
                payload_hash: "hash".to_owned(),
                signature: Some(fleet_domain::TaskSignature::new("sig").unwrap()),
            })
            .unwrap();
    }

    fn write_test_tls_material(dir: &Path) -> (PathBuf, PathBuf) {
        std::fs::create_dir_all(dir).unwrap();
        let cert = rcgen::generate_simple_self_signed(vec![
            "localhost".to_owned(),
            "127.0.0.1".to_owned(),
        ])
        .unwrap();
        let cert_path = dir.join("cert.pem");
        let key_path = dir.join("key.pem");
        std::fs::write(&cert_path, cert.serialize_pem().unwrap()).unwrap();
        std::fs::write(&key_path, cert.serialize_private_key_pem()).unwrap();
        set_secure_test_permissions(&key_path);
        (cert_path, key_path)
    }

    fn set_secure_test_permissions(path: &Path) {
        #[cfg(unix)]
        {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        #[cfg(not(unix))]
        {
            let _ = path;
        }
    }

    struct ControllerSigningKeyTestPair {
        files: ControllerSigningKeyFilePair,
        key_pair: fleet_core::AgentKeyPair,
    }

    fn controller_signing_test_pair(dir: &Path, name: &str) -> ControllerSigningKeyTestPair {
        write_controller_signing_key_pair(dir, name)
    }

    fn write_default_controller_signing_key_pair(data_dir: &Path) -> ControllerSigningKeyTestPair {
        let pair = write_controller_signing_key_pair(&data_dir.join("controller"), "controller");
        std::fs::rename(
            &pair.files.public_key_path,
            data_dir.join("controller").join("controller_public.key"),
        )
        .unwrap();
        std::fs::rename(
            &pair.files.private_key_path,
            data_dir.join("controller").join("controller_private.key"),
        )
        .unwrap();
        let files = ControllerSigningKeyFilePair {
            public_key_path: data_dir.join("controller").join("controller_public.key"),
            private_key_path: data_dir.join("controller").join("controller_private.key"),
        };
        ControllerSigningKeyTestPair {
            files,
            key_pair: pair.key_pair,
        }
    }

    fn write_controller_signing_key_pair(dir: &Path, name: &str) -> ControllerSigningKeyTestPair {
        std::fs::create_dir_all(dir).unwrap();
        let key_pair = fleet_core::generate_agent_key_pair().unwrap();
        let files = ControllerSigningKeyFilePair {
            public_key_path: dir.join(format!("{name}_controller_public.key")),
            private_key_path: dir.join(format!("{name}_controller_private.key")),
        };
        std::fs::write(
            &files.public_key_path,
            format!("{}\n", key_pair.public_key_hex),
        )
        .unwrap();
        std::fs::write(
            &files.private_key_path,
            format!("{}\n", key_pair.private_key_hex),
        )
        .unwrap();
        set_secure_test_permissions(&files.private_key_path);
        ControllerSigningKeyTestPair { files, key_pair }
    }

    fn controller_identity_from_key_pair(
        key_pair: &fleet_core::AgentKeyPair,
    ) -> ControllerIdentity {
        ControllerIdentity {
            public_key: key_pair.public_key_hex.clone(),
            fingerprint: key_pair.fingerprint.clone(),
            private_key: key_pair.private_key_hex.clone(),
        }
    }

    fn signing_rotation_test_metadata(
        active: &ControllerSigningKeyFilePair,
        tls: Option<(PathBuf, PathBuf)>,
    ) -> ControllerRuntimeMetadata {
        ControllerRuntimeMetadata {
            external_url: None,
            tls_enabled: tls.is_some(),
            controller_signing_public_key_path: Some(active.public_key_path.clone()),
            controller_signing_private_key_path: Some(active.private_key_path.clone()),
            tls_cert_path: tls.as_ref().map(|(cert, _)| cert.clone()),
            tls_key_path: tls.map(|(_, key)| key),
        }
    }

    fn artifact_test_root(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("sponzey-{name}-{}-{unique}", std::process::id()))
    }

    fn controller_test_artifact_checksum(bytes: &[u8]) -> ArtifactChecksum {
        let checksum = Sha256::digest(bytes);
        ArtifactChecksum::sha256(format!("{checksum:x}")).unwrap()
    }

    fn free_loopback_port() -> Option<u16> {
        std::net::TcpListener::bind("127.0.0.1:0")
            .ok()
            .and_then(|listener| listener.local_addr().ok().map(|addr| addr.port()))
    }

    fn poll_http_get(port: u16, path: &str) -> String {
        let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        for _ in 0..100 {
            match std::net::TcpStream::connect(("127.0.0.1", port)) {
                Ok(mut stream) => {
                    stream.write_all(request.as_bytes()).unwrap();
                    let mut buffer = [0_u8; 4096];
                    let read = stream.read(&mut buffer).unwrap();
                    return String::from_utf8_lossy(&buffer[..read]).to_string();
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        panic!("controller did not accept HTTP requests");
    }

    fn poll_https_get(port: u16, path: &str, ca_cert_path: &Path) -> String {
        let certificate =
            reqwest::Certificate::from_pem(&std::fs::read(ca_cert_path).unwrap()).unwrap();
        let client = reqwest::blocking::Client::builder()
            .add_root_certificate(certificate)
            .build()
            .unwrap();
        let url = format!("https://localhost:{port}{path}");
        for _ in 0..100 {
            match client.get(&url).send() {
                Ok(response) => {
                    let status = response.status();
                    let body = response.text().unwrap();
                    return format!("HTTP/1.1 {}\r\n\r\n{}", status.as_u16(), body);
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        panic!("controller did not accept HTTPS requests");
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "sponzey-fleet-controller-{name}-{}-{}",
            std::process::id(),
            epoch_millis()
        ))
    }
}
