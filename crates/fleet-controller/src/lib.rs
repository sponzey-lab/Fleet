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
    AdminTokenRepository, AgentInventoryRepository, CommandJobRepository, CreateCommandJob,
    CreateCommandJobError, CreateCommandJobInput, CreateDriftCheckJob, CreateDriftCheckJobError,
    CreateDriftCheckJobInput, CreateEnrollmentToken, CreateEnrollmentTokenInput, CreateRunbookJob,
    CreateRunbookJobError, CreateRunbookJobInput, DispatchAssignmentRepository,
    DispatchPendingAssignments, DispatchPendingAssignmentsInput, DispatchPendingAssignmentsOutput,
    DriftRepository, EnrollmentTokenRepository, EnrollmentTokenUseCaseError, EnsureAdminToken,
    FactsRepository, GetInventoryAgent, GetJobSummary, GetLatestDrift, GetLatestFacts,
    GetLatestMetrics, JobOutputChunk, JobOutputRepository, JobOutputStream, JobQueryRepository,
    JobRepository, ListAuditEvents, ListDriftReports, ListEnrollmentTokens, ListFactsSnapshots,
    ListInventoryAgents, ListJobOutputForJob, ListJobSummaries, ListMetricsSnapshots,
    MetricsRepository, PendingAssignmentDispatcher, PendingTaskAssignment, RevokeAgentKey,
    RevokeAgentKeyError, RevokeAgentKeyInput, RevokeEnrollmentToken, RevokeEnrollmentTokenInput,
    RunbookJobRepository, SnapshotPageCursor, TaskAssignmentRepository, TaskEnvelopeSigner,
    UpdateAgentLabels, UpdateAgentLabelsError, UpdateAgentLabelsInput, VerifyAdminToken,
    select_dispatch_targets,
};
use fleet_domain::{
    Agent, AgentFingerprint, AgentId, AgentIdentity, AgentLabel, AgentName, AgentPublicKey,
    AgentStatus, AuditActor, AuditCategory, AuditEvent, AuditTarget, AuditValue,
    ControllerPublicKey, DriftReport, DriftStatus, Job, JobId, JobStatus, Selector, TaskEnvelope,
};
use fleet_store::SqliteStore;
use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
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

#[derive(Debug, Clone)]
pub struct ControllerServerConfig {
    pub host: String,
    pub port: u16,
    pub external_url: Option<String>,
    pub tls_cert_path: Option<PathBuf>,
    pub tls_key_path: Option<PathBuf>,
    pub data_dir: PathBuf,
    pub database_path: Option<PathBuf>,
}

#[derive(Clone)]
struct ControllerAppState {
    store: Arc<Mutex<SqliteStore>>,
    identity: Arc<ControllerIdentity>,
    metadata: Arc<ControllerRuntimeMetadata>,
    sessions: Arc<Mutex<AgentSessionRegistry>>,
}

#[derive(Debug, Clone, Default)]
struct ControllerRuntimeMetadata {
    external_url: Option<String>,
    tls_enabled: bool,
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

#[derive(Clone)]
pub struct AgentSessionHandle {
    agent_id: String,
    connection_id: String,
    connected_at: SystemTime,
    last_seen_at: SystemTime,
    capabilities: Vec<String>,
    outbound_sender: AgentSessionOutboundSender,
    queue_capacity: Option<usize>,
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
        Self {
            agent_id: handle.agent_id.clone(),
            connected: true,
            connection_id: handle.connection_id.clone(),
            connected_at_ms: system_time_to_millis(handle.connected_at),
            last_session_seen_at_ms: system_time_to_millis(handle.last_seen_at),
            capabilities: handle.capabilities.clone(),
            queue_depth,
            queue_capacity: Some(queue_capacity),
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDriftCheckJobRequest {
    pub job_id: String,
    pub target_agent_ids: Vec<String>,
    #[serde(default)]
    pub selector: Option<String>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRunbookJobRequest {
    pub job_id: String,
    pub target_agent_ids: Vec<String>,
    #[serde(default)]
    pub selector: Option<String>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSummaryResponse {
    pub id: String,
    pub status: String,
    pub dispatch_state: String,
    pub risk: String,
    pub command_program: Option<String>,
    pub command_args: Vec<String>,
    pub target_count: usize,
    pub target_agent_ids: Vec<String>,
    pub target_agents: Vec<JobTargetSummaryResponse>,
    pub target_connected: bool,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub expires_at_ms: Option<u64>,
    pub last_error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobTargetSummaryResponse {
    pub agent_id: String,
    pub status: String,
    pub connected: bool,
    pub revoked: bool,
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
pub struct LatestDriftReportResponse {
    pub agent_id: String,
    pub checked_at_ms: u64,
    pub agent_system_time_ms: u64,
    pub policy_name: String,
    pub status: String,
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

#[derive(Debug)]
pub enum ControllerError {
    Io(std::io::Error),
    Store(fleet_store::StoreError),
    Protocol(fleet_protocol::ProtocolError),
    Json(String),
    Tls(String),
}

impl Display for ControllerError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Store(error) => write!(formatter, "store error: {error:?}"),
            Self::Protocol(error) => write!(formatter, "{error}"),
            Self::Json(error) => write!(formatter, "json error: {error}"),
            Self::Tls(error) => write!(formatter, "tls error: {error}"),
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

pub fn start_controller_server_until<F>(
    config: ControllerServerConfig,
    should_shutdown: F,
) -> Result<(), ControllerError>
where
    F: Fn() -> bool + Send + Sync + 'static,
{
    validate_transport(&config)?;
    let db_path = config
        .database_path
        .clone()
        .unwrap_or_else(|| config.data_dir.join("controller").join("fleet.db"));
    let store = SqliteStore::open(db_path)?;
    let identity = load_controller_identity(&config.data_dir)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run_axum_controller_server(
        config,
        store,
        identity,
        should_shutdown,
    ))
}

async fn run_axum_controller_server<F>(
    config: ControllerServerConfig,
    store: SqliteStore,
    identity: ControllerIdentity,
    should_shutdown: F,
) -> Result<(), ControllerError>
where
    F: Fn() -> bool + Send + Sync + 'static,
{
    let bind_addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .map_err(|error| controller_bind_error(&bind_addr, error))?;
    let tls_acceptor = match (&config.tls_cert_path, &config.tls_key_path) {
        (Some(cert_path), Some(key_path)) => Some(build_tls_acceptor(cert_path, key_path)?),
        _ => None,
    };
    let insecure_http_target = insecure_http_transport_target(&config);
    if let Some(target) = &insecure_http_target {
        audit_insecure_http_transport_enabled(&store, target)?;
    }
    announce_controller_started(&config, &identity, insecure_http_target.as_deref());

    let state = ControllerAppState {
        store: Arc::new(Mutex::new(store)),
        identity: Arc::new(identity),
        metadata: Arc::new(ControllerRuntimeMetadata {
            external_url: config.external_url.clone(),
            tls_enabled: config.tls_cert_path.is_some(),
        }),
        sessions: Arc::new(Mutex::new(AgentSessionRegistry::default())),
    };
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
            sessions.mark_seen(agent_id, connection_id, SystemTime::now());
        }
        let done = {
            let store = lock_store(state)?;
            handle_agent_task_data_message(&store, agent_id, message)?
        };
        if done {
            return Ok(AgentSessionCloseReason::NormalShutdown);
        }
    }
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
        Err(ControllerError::Io(_)) | Err(ControllerError::Tls(_)) => {
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

fn dispatch_pending_assignments_for_created_job(
    store: &SqliteStore,
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
    dispatch_pending_assignments(&store, &state.sessions, Some(agent_id), None, limit)
}

fn dispatch_pending_assignments(
    store: &SqliteStore,
    sessions: &Arc<Mutex<AgentSessionRegistry>>,
    agent_id: Option<AgentId>,
    job_id: Option<JobId>,
    limit: usize,
) -> Result<DispatchPendingAssignmentsOutput, ControllerError> {
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
) -> Result<std::sync::MutexGuard<'_, SqliteStore>, ControllerError> {
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

fn audit_agent_session_replaced(
    store: &SqliteStore,
    replacement: &AgentSessionReplacement,
) -> Result<(), ControllerError> {
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

fn audit_agent_session_started(
    store: &SqliteStore,
    agent_id: &str,
    connection_id: &str,
) -> Result<(), ControllerError> {
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

fn audit_agent_session_ended(
    store: &SqliteStore,
    ended: &AgentSessionEnded,
) -> Result<(), ControllerError> {
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

fn audit_agent_session_revoked_closed(
    store: &SqliteStore,
    ended: &AgentSessionEnded,
) -> Result<(), ControllerError> {
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
    let mut repo = ControllerAdminTokenRepository { store };
    let token = generate_token("admin")?;
    let created = EnsureAdminToken::execute(&mut repo, &hash_token(&token))?;
    if !created {
        return Ok(None);
    }
    Ok(Some(token))
}

fn load_controller_identity(data_dir: &Path) -> Result<ControllerIdentity, ControllerError> {
    let public_key_path = data_dir.join("controller").join("controller_public.key");
    let private_key_path = data_dir.join("controller").join("controller_private.key");
    let public_key = std::fs::read_to_string(public_key_path)?.trim().to_owned();
    let private_key = std::fs::read_to_string(private_key_path)?.trim().to_owned();
    let fingerprint = fleet_core::fingerprint_public_key(&public_key)
        .map_err(|error| ControllerError::Json(error.to_string()))?;
    Ok(ControllerIdentity {
        public_key,
        fingerprint,
        private_key,
    })
}

fn validate_agent_ws_hello(
    store: &SqliteStore,
    agent_id: &str,
    fingerprint: &str,
) -> Result<Option<String>, ControllerError> {
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

fn handle_agent_task_data_message(
    store: &SqliteStore,
    agent_id: &str,
    message: fleet_protocol::WireMessage,
) -> Result<bool, ControllerError> {
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
        fleet_protocol::WirePayload::TaskResult {
            job_id,
            task_id: _,
            exit_code,
        } => {
            let status = if exit_code == 0 {
                JobStatus::Success
            } else {
                JobStatus::Failed
            };
            store.update_job_status(&job_id, status)?;
            audit_job(
                store,
                if exit_code == 0 {
                    "job_completed"
                } else {
                    "job_failed"
                },
                &job_id,
                AuditValue::Plain(format!("agent_id={agent_id},exit_code={exit_code}")),
            )?;
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

fn append_agent_output_chunk(
    store: &SqliteStore,
    agent_id: &str,
    chunk: JobOutputChunk,
) -> Result<(), ControllerError> {
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

fn audit_security(store: &SqliteStore, action: &str, target: &str) -> Result<(), ControllerError> {
    store.write_audit_event(AuditEvent::security(action, target))?;
    Ok(())
}

fn audit_security_with_value(
    store: &SqliteStore,
    action: &str,
    target: &str,
    value: AuditValue,
) -> Result<(), ControllerError> {
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

fn audit_insecure_http_transport_enabled(
    store: &SqliteStore,
    target: &str,
) -> Result<(), ControllerError> {
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

fn audit_job(
    store: &SqliteStore,
    action: &str,
    target: &str,
    value: AuditValue,
) -> Result<(), ControllerError> {
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

fn audit_drift(
    store: &SqliteStore,
    action: &str,
    target: &str,
    value: AuditValue,
) -> Result<(), ControllerError> {
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
    route_request_with_identity_and_sessions(request, store, identity, metadata, None)
}

fn route_request_with_identity_and_sessions(
    request: &str,
    store: &SqliteStore,
    identity: &ControllerIdentity,
    metadata: &ControllerRuntimeMetadata,
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
) -> Result<String, ControllerError> {
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

    if route_path.starts_with("/api/")
        && route_path != "/api/agents/enroll"
        && !authorized(request, store)?
    {
        return Ok(response(
            401,
            "application/json",
            "{\"error\":\"unauthorized\"}\n",
        ));
    }

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
        ("POST", "/api/enrollment-tokens") => {
            match create_enrollment_token(request_body(request), store) {
                Ok(body) => Ok(response(201, "application/json", &format!("{body}\n"))),
                Err(ControllerError::Json(message)) => Ok(response(
                    400,
                    "application/json",
                    &format!("{{\"error\":\"{}\"}}\n", json_escape(&message)),
                )),
                Err(error) => Err(error),
            }
        }
        ("POST", "/api/jobs/command") => {
            match create_command_job(request_body(request), store, identity) {
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
            match create_drift_check_job(request_body(request), store, identity) {
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
            match create_runbook_job(request_body(request), store, identity) {
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
        ("GET", path) if path.starts_with("/api/jobs/") && path.ends_with("/output") => {
            let job_id = path
                .trim_start_matches("/api/jobs/")
                .trim_end_matches("/output")
                .trim_end_matches('/');
            let body = list_job_output(job_id, store)?;
            Ok(response(200, "application/json", &format!("{body}\n")))
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
        ("GET", "/api/audit") => {
            let body = list_audit_events(store)?;
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
            match revoke_agent_key(agent_id, store, sessions) {
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
            match update_agent_labels(agent_id, request_body(request), store, sessions) {
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
            if revoke_enrollment_token(id, store)? {
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CreateJobHttpOutput {
    job_id: String,
    body: String,
}

fn create_enrollment_token(body: &str, store: &SqliteStore) -> Result<String, ControllerError> {
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
    let mut repo = ControllerEnrollmentTokenRepository { store };
    let mut audit = ControllerAuditWriter { store };
    let output = CreateEnrollmentToken::execute(
        &mut repo,
        &mut audit,
        CreateEnrollmentTokenInput {
            id,
            token_hash: hash_token(&token),
            default_labels: request.default_labels,
            expires_at: now + Duration::from_secs(request.expires_in_seconds),
            max_uses: request.max_uses,
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

fn list_enrollment_tokens(store: &SqliteStore) -> Result<String, ControllerError> {
    let repo = ControllerEnrollmentTokenRepository { store };
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

fn revoke_enrollment_token(id: &str, store: &SqliteStore) -> Result<bool, ControllerError> {
    let mut repo = ControllerEnrollmentTokenRepository { store };
    let mut audit = ControllerAuditWriter { store };
    let output = RevokeEnrollmentToken::execute(
        &mut repo,
        &mut audit,
        RevokeEnrollmentTokenInput {
            id: id.to_owned(),
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

fn create_command_job(
    body: &str,
    store: &SqliteStore,
    identity: &ControllerIdentity,
) -> Result<CreateJobHttpOutput, CreateCommandJobHttpError> {
    let request: CreateCommandJobRequest = serde_json::from_str(body)
        .map_err(|error| CreateCommandJobHttpError::BadRequest(error.to_string()))?;
    let issued_at = SystemTime::now();
    let expires_at = issued_at + Duration::from_secs(request.expires_in_seconds);
    let job_id = request.job_id.clone();
    let target_agent_ids = resolve_command_targets(store, &request)?;
    let nonce_prefix = match request.nonce_prefix {
        Some(prefix) => prefix,
        None => fleet_core::generate_prefixed_ulid("nonce").map_err(|error| {
            CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
        })?,
    };
    let input = CreateCommandJobInput {
        job_id: request.job_id,
        target_agent_ids,
        program: request.program,
        args: request.args,
        timeout: Duration::from_secs(request.timeout_seconds),
        confirmed_high_risk: request.confirmed_high_risk,
        confirmed_by: request.confirmed_by,
        issued_at,
        expires_at,
        nonce_prefix,
    };
    let mut job_repo = ControllerJobRepository { store };
    let mut audit_writer = ControllerAuditWriter { store };
    let mut signer = ControllerTaskSigner {
        private_key: &identity.private_key,
    };

    let output = CreateCommandJob::execute(&mut job_repo, &mut audit_writer, &mut signer, input)
        .map_err(map_create_command_job_error)?;
    let body = serde_json::to_string(&CreateCommandJobResponse {
        job_id: job_id.clone(),
        target_count: output.targets.len(),
        assignment_count: output.envelopes.len(),
    })
    .map_err(|error| {
        CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
    })?;
    Ok(CreateJobHttpOutput { job_id, body })
}

fn create_drift_check_job(
    body: &str,
    store: &SqliteStore,
    identity: &ControllerIdentity,
) -> Result<CreateJobHttpOutput, CreateCommandJobHttpError> {
    let request: CreateDriftCheckJobRequest = serde_json::from_str(body)
        .map_err(|error| CreateCommandJobHttpError::BadRequest(error.to_string()))?;
    fleet_domain::parse_policy_document(&request.policy_document)
        .map_err(|error| CreateCommandJobHttpError::BadRequest(error.to_string()))?;
    let issued_at = SystemTime::now();
    let expires_at = issued_at + Duration::from_secs(request.expires_in_seconds);
    let job_id = request.job_id.clone();
    let target_agent_ids = resolve_drift_check_targets(store, &request)?;
    let nonce_prefix = match request.nonce_prefix {
        Some(prefix) => prefix,
        None => fleet_core::generate_prefixed_ulid("nonce").map_err(|error| {
            CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
        })?,
    };
    let input = CreateDriftCheckJobInput {
        job_id: request.job_id,
        target_agent_ids,
        policy_document: request.policy_document,
        timeout: Duration::from_secs(request.timeout_seconds),
        created_by: request.created_by,
        issued_at,
        expires_at,
        nonce_prefix,
    };
    let mut job_repo = ControllerJobRepository { store };
    let mut audit_writer = ControllerAuditWriter { store };
    let mut signer = ControllerTaskSigner {
        private_key: &identity.private_key,
    };

    let output = CreateDriftCheckJob::execute(&mut job_repo, &mut audit_writer, &mut signer, input)
        .map_err(map_create_drift_check_job_error)?;
    let body = serde_json::to_string(&CreateDriftCheckJobResponse {
        job_id: job_id.clone(),
        target_count: output.targets.len(),
        assignment_count: output.envelopes.len(),
    })
    .map_err(|error| {
        CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
    })?;
    Ok(CreateJobHttpOutput { job_id, body })
}

fn create_runbook_job(
    body: &str,
    store: &SqliteStore,
    identity: &ControllerIdentity,
) -> Result<CreateJobHttpOutput, CreateCommandJobHttpError> {
    let request: CreateRunbookJobRequest = serde_json::from_str(body)
        .map_err(|error| CreateCommandJobHttpError::BadRequest(error.to_string()))?;
    fleet_domain::parse_runbook_document(&request.runbook_document)
        .map_err(|error| CreateCommandJobHttpError::BadRequest(error.to_string()))?;
    let issued_at = SystemTime::now();
    let expires_at = issued_at + Duration::from_secs(request.expires_in_seconds);
    let job_id = request.job_id.clone();
    let target_agent_ids = resolve_runbook_targets(store, &request)?;
    let nonce_prefix = match request.nonce_prefix {
        Some(prefix) => prefix,
        None => fleet_core::generate_prefixed_ulid("nonce").map_err(|error| {
            CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
        })?,
    };
    let input = CreateRunbookJobInput {
        job_id: request.job_id,
        target_agent_ids,
        runbook_document: request.runbook_document,
        timeout: Duration::from_secs(request.timeout_seconds),
        confirmed_high_risk: request.confirmed_high_risk,
        confirmed_by: request.confirmed_by,
        issued_at,
        expires_at,
        nonce_prefix,
    };
    let mut job_repo = ControllerJobRepository { store };
    let mut audit_writer = ControllerAuditWriter { store };
    let mut signer = ControllerTaskSigner {
        private_key: &identity.private_key,
    };

    let output = CreateRunbookJob::execute(&mut job_repo, &mut audit_writer, &mut signer, input)
        .map_err(map_create_runbook_job_error)?;
    let body = serde_json::to_string(&CreateRunbookJobResponse {
        job_id: job_id.clone(),
        target_count: output.targets.len(),
        assignment_count: output.envelopes.len(),
    })
    .map_err(|error| {
        CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string()))
    })?;
    Ok(CreateJobHttpOutput { job_id, body })
}

fn resolve_command_targets(
    store: &SqliteStore,
    request: &CreateCommandJobRequest,
) -> Result<Vec<String>, CreateCommandJobHttpError> {
    if !request.target_agent_ids.is_empty() {
        return Ok(request.target_agent_ids.clone());
    }
    let Some(selector) = request.selector.as_deref() else {
        return Ok(Vec::new());
    };
    let selector = Selector::parse(selector)
        .map_err(|error| CreateCommandJobHttpError::BadRequest(error.to_string()))?;
    let repo = ControllerAgentInventoryRepository { store };
    let agents = ListInventoryAgents::execute(&repo)
        .map_err(|error| CreateCommandJobHttpError::Internal(ControllerError::Store(error)))?;
    let selection = select_dispatch_targets(&agents, &selector);
    tracing::debug!(
        matched_count = selection.matched_count,
        selected_count = selection.targets.len(),
        disabled_count = selection.disabled_count,
        offline_count = selection.offline_count,
        "job_selector_resolved"
    );
    Ok(selection
        .targets
        .into_iter()
        .map(|agent| agent.id().as_str().to_owned())
        .collect())
}

fn resolve_drift_check_targets(
    store: &SqliteStore,
    request: &CreateDriftCheckJobRequest,
) -> Result<Vec<String>, CreateCommandJobHttpError> {
    if !request.target_agent_ids.is_empty() {
        return Ok(request.target_agent_ids.clone());
    }
    let Some(selector) = request.selector.as_deref() else {
        return Ok(Vec::new());
    };
    let selector = Selector::parse(selector)
        .map_err(|error| CreateCommandJobHttpError::BadRequest(error.to_string()))?;
    let repo = ControllerAgentInventoryRepository { store };
    let agents = ListInventoryAgents::execute(&repo)
        .map_err(|error| CreateCommandJobHttpError::Internal(ControllerError::Store(error)))?;
    let selection = select_dispatch_targets(&agents, &selector);
    tracing::debug!(
        matched_count = selection.matched_count,
        selected_count = selection.targets.len(),
        disabled_count = selection.disabled_count,
        offline_count = selection.offline_count,
        "drift_check_selector_resolved"
    );
    Ok(selection
        .targets
        .into_iter()
        .map(|agent| agent.id().as_str().to_owned())
        .collect())
}

fn resolve_runbook_targets(
    store: &SqliteStore,
    request: &CreateRunbookJobRequest,
) -> Result<Vec<String>, CreateCommandJobHttpError> {
    if !request.target_agent_ids.is_empty() {
        return Ok(request.target_agent_ids.clone());
    }
    let Some(selector) = request.selector.as_deref() else {
        return Ok(Vec::new());
    };
    let selector = Selector::parse(selector)
        .map_err(|error| CreateCommandJobHttpError::BadRequest(error.to_string()))?;
    let repo = ControllerAgentInventoryRepository { store };
    let agents = ListInventoryAgents::execute(&repo)
        .map_err(|error| CreateCommandJobHttpError::Internal(ControllerError::Store(error)))?;
    let selection = select_dispatch_targets(&agents, &selector);
    tracing::debug!(
        matched_count = selection.matched_count,
        selected_count = selection.targets.len(),
        disabled_count = selection.disabled_count,
        offline_count = selection.offline_count,
        "runbook_selector_resolved"
    );
    Ok(selection
        .targets
        .into_iter()
        .map(|agent| agent.id().as_str().to_owned())
        .collect())
}

fn list_job_output(job_id: &str, store: &SqliteStore) -> Result<String, ControllerError> {
    let repo = ControllerJobOutputRepository { store };
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

fn job_output_stream_to_str(stream: JobOutputStream) -> &'static str {
    match stream {
        JobOutputStream::Stdout => "stdout",
        JobOutputStream::Stderr => "stderr",
    }
}

fn list_agents(
    store: &SqliteStore,
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
) -> Result<String, ControllerError> {
    mark_stale_agents_offline_for_inventory(store)?;
    let repo = ControllerAgentInventoryRepository { store };
    let connected_agent_ids = connected_agent_ids(sessions);
    let agents = ListInventoryAgents::execute(&repo)?
        .iter()
        .map(|agent| agent_to_response_with_latest_facts(agent, store, &connected_agent_ids))
        .collect::<Result<Vec<_>, _>>()?;
    serde_json::to_string(&agents).map_err(|error| ControllerError::Json(error.to_string()))
}

fn get_agent(
    agent_id: &str,
    store: &SqliteStore,
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
) -> Result<Option<String>, ControllerError> {
    mark_stale_agents_offline_for_inventory(store)?;
    let agent_id = AgentId::new(agent_id).map_err(|error| ControllerError::Store(error.into()))?;
    let repo = ControllerAgentInventoryRepository { store };
    let Some(agent) = GetInventoryAgent::execute(&repo, agent_id)? else {
        return Ok(None);
    };
    let connected_agent_ids = connected_agent_ids(sessions);
    let response = agent_to_response_with_latest_facts(&agent, store, &connected_agent_ids)?;
    serde_json::to_string(&response)
        .map(Some)
        .map_err(|error| ControllerError::Json(error.to_string()))
}

fn mark_stale_agents_offline_for_inventory(store: &SqliteStore) -> Result<(), ControllerError> {
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

fn update_agent_labels(
    agent_id: &str,
    body: &str,
    store: &SqliteStore,
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
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
    let mut repo = ControllerAgentInventoryRepository { store };
    let mut audit = ControllerAuditWriter { store };
    let Some(agent) = UpdateAgentLabels::execute(
        &mut repo,
        &mut audit,
        UpdateAgentLabelsInput {
            agent_id: agent_id.to_owned(),
            labels,
            actor: "admin".to_owned(),
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

fn revoke_agent_key(
    agent_id: &str,
    store: &SqliteStore,
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
) -> Result<Option<String>, ControllerError> {
    let mut repo = ControllerAgentInventoryRepository { store };
    let mut audit = ControllerAuditWriter { store };
    let Some(agent) = RevokeAgentKey::execute(
        &mut repo,
        &mut audit,
        RevokeAgentKeyInput {
            agent_id: agent_id.to_owned(),
            actor: "admin".to_owned(),
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

fn latest_facts(agent_id: &str, store: &SqliteStore) -> Result<Option<String>, ControllerError> {
    let repo = ControllerFactsRepository { store };
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

fn list_facts_snapshots(
    agent_id: &str,
    raw_path: &str,
    store: &SqliteStore,
) -> Result<String, ControllerError> {
    let page = parse_snapshot_page_request(raw_path)?;
    let repo = ControllerFactsRepository { store };
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

fn list_jobs(
    store: &SqliteStore,
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
) -> Result<String, ControllerError> {
    let repo = ControllerJobQueryRepository { store };
    let jobs = ListJobSummaries::execute(&repo, 50)?;
    let connected_agent_ids = connected_agent_ids(sessions);
    let response = jobs
        .into_iter()
        .map(|job| job_summary_response(job, &connected_agent_ids))
        .collect::<Vec<_>>();
    serde_json::to_string(&response).map_err(|error| ControllerError::Json(error.to_string()))
}

fn get_job(
    job_id: &str,
    store: &SqliteStore,
    sessions: Option<&Arc<Mutex<AgentSessionRegistry>>>,
) -> Result<Option<String>, ControllerError> {
    let repo = ControllerJobQueryRepository { store };
    let Some(job) = GetJobSummary::execute(&repo, job_id)? else {
        return Ok(None);
    };
    let connected_agent_ids = connected_agent_ids(sessions);
    serde_json::to_string(&job_summary_response(job, &connected_agent_ids))
        .map(Some)
        .map_err(|error| ControllerError::Json(error.to_string()))
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
            status: agent_status_for_job_target(
                &target.status,
                connected_agent_ids.contains(&target.agent_id),
            ),
            revoked: target.status == "disabled",
        })
        .collect::<Vec<_>>();
    let target_agent_ids = target_agents
        .iter()
        .map(|target| target.agent_id.clone())
        .collect::<Vec<_>>();
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
        target_count: job.target_count,
        target_agent_ids,
        target_agents,
        target_connected,
        created_at_ms,
        updated_at_ms: created_at_ms,
        expires_at_ms: job.expires_at.map(system_time_to_millis),
        last_error: String::new(),
    }
}

fn job_dispatch_state(status: &str, target_connected: bool) -> String {
    match status {
        "queued" if target_connected => "created",
        "queued" => "queued",
        "running" => "delivered",
        "success" => "completed",
        "failed" => "failed",
        "expired" => "expired",
        "canceled" => "rejected",
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

fn latest_metrics(agent_id: &str, store: &SqliteStore) -> Result<Option<String>, ControllerError> {
    let repo = ControllerMetricsRepository { store };
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

fn list_metrics_snapshots(
    agent_id: &str,
    raw_path: &str,
    store: &SqliteStore,
) -> Result<String, ControllerError> {
    let page = parse_snapshot_page_request(raw_path)?;
    let repo = ControllerMetricsRepository { store };
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

fn latest_drift_report(
    agent_id: &str,
    store: &SqliteStore,
) -> Result<Option<String>, ControllerError> {
    let repo = ControllerDriftRepository { store };
    let Some(record) = GetLatestDrift::execute(&repo, agent_id)? else {
        return Ok(None);
    };
    serde_json::to_string(&LatestDriftReportResponse {
        agent_id: record.agent_id,
        checked_at_ms: system_time_to_millis(record.checked_at),
        agent_system_time_ms: system_time_to_millis(record.checked_at),
        policy_name: record.report.policy_name,
        status: drift_status_to_str(&record.report.status).to_owned(),
        expected: record.report.expected,
        actual: record.report.actual,
    })
    .map(Some)
    .map_err(|error| ControllerError::Json(error.to_string()))
}

fn list_drift_reports(
    agent_id: &str,
    raw_path: &str,
    store: &SqliteStore,
) -> Result<String, ControllerError> {
    let page = parse_snapshot_page_request(raw_path)?;
    let repo = ControllerDriftRepository { store };
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
            expected: record.report.expected,
            actual: record.report.actual,
            cursor: encode_snapshot_page_cursor(record.cursor),
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&DriftReportPageResponse { items, next_cursor })
        .map_err(|error| ControllerError::Json(error.to_string()))
}

fn list_audit_events(store: &SqliteStore) -> Result<String, ControllerError> {
    let repo = ControllerAuditRepository { store };
    let events = ListAuditEvents::execute(&repo, 50)?;
    let response = events
        .iter()
        .map(audit_event_to_response)
        .collect::<Vec<_>>();
    serde_json::to_string(&response).map_err(|error| ControllerError::Json(error.to_string()))
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

fn agent_to_response_with_latest_facts(
    agent: &Agent,
    store: &SqliteStore,
    connected_agent_ids: &std::collections::BTreeSet<String>,
) -> Result<AgentResponse, ControllerError> {
    let summary = store
        .latest_facts_snapshot(agent.id().as_str())?
        .and_then(|record| agent_facts_summary(&record.body));
    Ok(agent_to_response(
        agent,
        summary.as_ref(),
        connected_agent_ids,
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

struct ControllerAdminTokenRepository<'a> {
    store: &'a SqliteStore,
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
}

struct ControllerAgentInventoryRepository<'a> {
    store: &'a SqliteStore,
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
    store: &'a SqliteStore,
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
    store: &'a SqliteStore,
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

struct ControllerDriftRepository<'a> {
    store: &'a SqliteStore,
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

struct ControllerAuditRepository<'a> {
    store: &'a SqliteStore,
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
}

struct ControllerEnrollmentTokenRepository<'a> {
    store: &'a SqliteStore,
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
    store: &'a SqliteStore,
}

struct ControllerJobOutputRepository<'a> {
    store: &'a SqliteStore,
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
    store: &'a SqliteStore,
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
                target_count: record.target_count,
                target_agents: record
                    .target_agents
                    .into_iter()
                    .map(|target| fleet_application::JobTargetSummaryRecord {
                        agent_id: target.agent_id,
                        status: target.status,
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
                target_count: record.target_count,
                target_agents: record
                    .target_agents
                    .into_iter()
                    .map(|target| fleet_application::JobTargetSummaryRecord {
                        agent_id: target.agent_id,
                        status: target.status,
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

impl TaskAssignmentRepository for ControllerJobRepository<'_> {
    type Error = fleet_store::StoreError;

    fn save_assignment(&mut self, envelope: TaskEnvelope) -> Result<(), Self::Error> {
        self.store.save_task_assignment_record(&envelope)
    }
}

impl CommandJobRepository for ControllerJobRepository<'_> {
    fn save_command_job(
        &mut self,
        job: Job,
        task: &fleet_domain::CommandTask,
    ) -> Result<(), Self::Error> {
        self.store.save_command_job_record(&job, task)
    }
}

impl fleet_application::DriftCheckJobRepository for ControllerJobRepository<'_> {
    fn save_drift_check_job(
        &mut self,
        job: Job,
        task: &fleet_domain::DriftCheckTask,
    ) -> Result<(), Self::Error> {
        self.store.save_drift_check_job_record(&job, task)
    }
}

impl RunbookJobRepository for ControllerJobRepository<'_> {
    fn save_runbook_job(
        &mut self,
        job: Job,
        task: &fleet_domain::RunbookExecutionTask,
    ) -> Result<(), Self::Error> {
        self.store.save_runbook_job_record(&job, task)
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
    store: &'a SqliteStore,
}

impl fleet_application::AuditWriter for ControllerAuditWriter<'_> {
    type Error = fleet_store::StoreError;

    fn write(&mut self, event: AuditEvent) -> Result<(), Self::Error> {
        self.store.write_audit_event(event)
    }
}

struct ControllerTaskSigner<'a> {
    private_key: &'a str,
}

impl TaskEnvelopeSigner for ControllerTaskSigner<'_> {
    type Error = fleet_core::IdentityError;

    fn sign(&mut self, payload: &str) -> Result<String, Self::Error> {
        fleet_core::sign_challenge(self.private_key, payload)
    }
}

fn enroll_agent(
    body: &str,
    store: &SqliteStore,
    identity: &ControllerIdentity,
) -> Result<String, ControllerError> {
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

fn authorized(request: &str, store: &SqliteStore) -> Result<bool, ControllerError> {
    let Some(token) = bearer_token(request) else {
        return Ok(false);
    };
    let repo = ControllerAdminTokenRepository { store };
    VerifyAdminToken::execute(&repo, &hash_token(token)).map_err(ControllerError::from)
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
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
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
    fn protected_api_requires_admin_token() {
        let store = SqliteStore::in_memory().unwrap();
        let response =
            route_request("POST /api/enrollment-tokens HTTP/1.1\r\n\r\n", &store).unwrap();
        assert!(response.starts_with("HTTP/1.1 401"));
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
        assert_eq!(audits[0].actor.as_str(), "operator-1");
        assert_eq!(
            audits[0].value,
            AuditValue::Plain(
                "confirmed_high_risk=true,confirmed_by=operator-1,target_count=1".to_owned()
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
        assert!(audits.iter().any(|event| event.action == "job_created"));
        assert!(audits.iter().any(|event| event.action == "task_dispatched"));
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
    fn dispatch_does_not_bypass_high_risk_confirmation() {
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

        let response = route_request_with_sessions(
            &command_job_request("job-needs-confirmation", "agent-1", false),
            &store,
            &sessions,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 400"));
        assert!(response.contains("high-risk task requires approval"));
        assert!(receiver.try_recv().is_err());
        assert!(
            store
                .find_job_status_value("job-needs-confirmation")
                .unwrap()
                .is_none()
        );
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
        let audits = store
            .list_audit_events_by_category(AuditCategory::Job, 10)
            .unwrap();

        assert!(response.starts_with("HTTP/1.1 201"));
        assert!(response.contains("\"target_count\":1"));
        assert_eq!(assignments.len(), 1);
        assert!(
            assignments[0]
                .runbook
                .runbook_document()
                .contains("kind: Runbook")
        );
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].action, "runbook_job_created");
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
        assert_eq!(audits[0].actor.as_str(), "operator-1");
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
    fn command_job_requires_high_risk_confirmation() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_admin_token_hash(&hash_token("admin-token"))
            .unwrap();
        save_test_agent(&store, "agent-1");
        let body = serde_json::to_string(&CreateCommandJobRequest {
            job_id: "job-1".to_owned(),
            target_agent_ids: vec!["agent-1".to_owned()],
            selector: None,
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

        assert!(response.starts_with("HTTP/1.1 400"));
        assert!(response.contains("high-risk task requires approval"));
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
        let message = fleet_protocol::WireMessage::new(
            "msg-result",
            "corr-result",
            Some("agent-1".to_owned()),
            1,
            fleet_protocol::WirePayload::TaskResult {
                job_id: "job-1".to_owned(),
                task_id: "task-1".to_owned(),
                exit_code: 0,
            },
        );

        let finished = handle_agent_task_data_message(&store, "agent-1", message).unwrap();
        let status = store.find_job_status_value("job-1").unwrap().unwrap();
        let audits = store
            .list_audit_events_by_category(AuditCategory::Job, 10)
            .unwrap();

        assert!(!finished);
        assert_eq!(status, "success");
        assert_eq!(audits[0].action, "job_completed");
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
        assert!(response.contains("\"actual\":\"stopped\""));
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
    fn admin_can_page_facts_metrics_and_drift_reports() {
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
        assert_eq!(job["target_count"], 1);
        assert_eq!(job["target_agent_ids"], serde_json::json!(["agent-1"]));
        assert_eq!(job["target_agents"][0]["agent_id"], "agent-1");
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
    fn controller_allows_remote_bind_with_https_external_url() {
        let config = ControllerServerConfig {
            host: "0.0.0.0".to_owned(),
            port: 7700,
            external_url: Some("https://fleet.example.com".to_owned()),
            tls_cert_path: None,
            tls_key_path: None,
            data_dir: PathBuf::from(".sponzey"),
            database_path: None,
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
            data_dir: PathBuf::from(".sponzey"),
            database_path: None,
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
            data_dir: PathBuf::from(".sponzey"),
            database_path: None,
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
            data_dir: PathBuf::from(".sponzey"),
            database_path: None,
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
            data_dir: PathBuf::from(".sponzey"),
            database_path: None,
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
            data_dir: PathBuf::from(".sponzey"),
            database_path: None,
        };

        assert!(validate_transport(&config).is_ok());
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
        std::fs::create_dir_all(data_dir.join("controller")).unwrap();
        let key_pair = fleet_core::generate_agent_key_pair().unwrap();
        std::fs::write(
            data_dir.join("controller").join("controller_public.key"),
            format!("{}\n", key_pair.public_key_hex),
        )
        .unwrap();
        std::fs::write(
            data_dir.join("controller").join("controller_private.key"),
            format!("{}\n", key_pair.private_key_hex),
        )
        .unwrap();

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
                    data_dir,
                    database_path: None,
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
        std::fs::create_dir_all(data_dir.join("controller")).unwrap();
        let key_pair = fleet_core::generate_agent_key_pair().unwrap();
        std::fs::write(
            data_dir.join("controller").join("controller_public.key"),
            format!("{}\n", key_pair.public_key_hex),
        )
        .unwrap();
        std::fs::write(
            data_dir.join("controller").join("controller_private.key"),
            format!("{}\n", key_pair.private_key_hex),
        )
        .unwrap();
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
                    data_dir,
                    database_path: None,
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
            &ControllerIdentity::dev_insecure(),
            &ControllerRuntimeMetadata::default(),
            Some(sessions),
        )
    }

    fn command_job_request(job_id: &str, agent_id: &str, confirmed_high_risk: bool) -> String {
        let body = serde_json::to_string(&CreateCommandJobRequest {
            job_id: job_id.to_owned(),
            target_agent_ids: vec![agent_id.to_owned()],
            selector: None,
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

    fn runbook_job_request(job_id: &str, agent_id: &str) -> String {
        let body = serde_json::to_string(&CreateRunbookJobRequest {
            job_id: job_id.to_owned(),
            target_agent_ids: vec![agent_id.to_owned()],
            selector: None,
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
