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
    CreateRunbookJobError, CreateRunbookJobInput, DriftRepository, EnrollmentTokenRepository,
    EnrollmentTokenUseCaseError, EnsureAdminToken, FactsRepository, GetInventoryAgent,
    GetLatestDrift, GetLatestFacts, GetLatestMetrics, JobOutputChunk, JobOutputRepository,
    JobOutputStream, JobQueryRepository, JobRepository, ListAuditEvents, ListDriftReports,
    ListEnrollmentTokens, ListFactsSnapshots, ListInventoryAgents, ListJobOutputForJob,
    ListJobSummaries, ListMetricsSnapshots, MetricsRepository, RevokeAgentKey, RevokeAgentKeyError,
    RevokeAgentKeyInput, RevokeEnrollmentToken, RevokeEnrollmentTokenInput, RunbookJobRepository,
    SnapshotPageCursor, TaskAssignmentRepository, TaskEnvelopeSigner, UpdateAgentLabels,
    UpdateAgentLabelsError, UpdateAgentLabelsInput, VerifyAdminToken, select_dispatch_targets,
};
use fleet_domain::{
    Agent, AgentFingerprint, AgentId, AgentIdentity, AgentLabel, AgentName, AgentPublicKey,
    AgentStatus, AuditActor, AuditCategory, AuditEvent, AuditTarget, AuditValue,
    ControllerPublicKey, DriftReport, DriftStatus, Job, JobStatus, Selector, TaskEnvelope,
};
use fleet_store::SqliteStore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::io::BufReader;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
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
}

#[derive(Debug, Clone, Default)]
struct ControllerRuntimeMetadata {
    external_url: Option<String>,
    tls_enabled: bool,
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
    pub risk: String,
    pub command_program: Option<String>,
    pub command_args: Vec<String>,
    pub target_count: usize,
    pub created_at_ms: u64,
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
            route_request_with_identity(&request, &store, &state.identity, &state.metadata)
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

    let heartbeat = read_axum_wire_message(&mut socket).await?;
    if let fleet_protocol::WirePayload::Heartbeat {
        agent_id: heartbeat_agent_id,
        ..
    } = heartbeat.payload
        && heartbeat_agent_id == agent_id
    {
        let assignment = {
            let store = lock_store(&state)?;
            store.mark_agent_online(&agent_id, SystemTime::now())?;
            pending_task_assignment_message(&store, &agent_id)?
        };
        let dispatched = assignment.is_some();
        if let Some(message) = assignment {
            send_axum_wire_message(&mut socket, &message).await?;
        }
        read_task_data_until_close_axum(&mut socket, &state, &agent_id, !dispatched).await?;
        let _ = socket.send(AxumWsMessage::Close(None)).await;
    } else {
        let store = lock_store(&state)?;
        audit_security(&store, "websocket_invalid_heartbeat", &agent_id)?;
    }

    Ok(())
}

async fn read_task_data_until_close_axum(
    socket: &mut WebSocket,
    state: &ControllerAppState,
    agent_id: &str,
    stop_after_idle: bool,
) -> Result<(), ControllerError> {
    loop {
        let read_result = if stop_after_idle {
            match tokio::time::timeout(Duration::from_millis(75), read_axum_wire_message(socket))
                .await
            {
                Ok(result) => result,
                Err(_) => return Ok(()),
            }
        } else {
            read_axum_wire_message(socket).await
        };
        let message = match read_result {
            Ok(message) => message,
            Err(ControllerError::Json(error)) if error == "websocket closed" => return Ok(()),
            Err(error) => return Err(error),
        };
        let done = {
            let store = lock_store(state)?;
            handle_agent_task_data_message(&store, agent_id, message)?
        };
        if done {
            return Ok(());
        }
    }
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

fn pending_task_assignment_message(
    store: &SqliteStore,
    agent_id: &str,
) -> Result<Option<fleet_protocol::WireMessage>, ControllerError> {
    if let Some(assignment) = store
        .list_pending_command_assignments_for_agent(agent_id)?
        .into_iter()
        .next()
    {
        store.update_job_status(assignment.envelope.job_id.as_str(), JobStatus::Running)?;
        audit_job(
            store,
            "job_started",
            assignment.envelope.job_id.as_str(),
            AuditValue::Plain(format!("agent_id={agent_id}")),
        )?;
        return Ok(Some(fleet_protocol::WireMessage::new(
            fleet_core::generate_prefixed_ulid("msg")
                .map_err(|error| ControllerError::Json(error.to_string()))?,
            assignment.envelope.task_id.as_str().to_owned(),
            Some(agent_id.to_owned()),
            epoch_millis() as u64,
            fleet_protocol::WirePayload::TaskAssignment {
                envelope: task_envelope_to_wire(&assignment.envelope),
                task: command_task_to_wire(&assignment.command),
            },
        )));
    }

    if let Some(assignment) = store
        .list_pending_runbook_assignments_for_agent(agent_id)?
        .into_iter()
        .next()
    {
        store.update_job_status(assignment.envelope.job_id.as_str(), JobStatus::Running)?;
        audit_job(
            store,
            "runbook_job_started",
            assignment.envelope.job_id.as_str(),
            AuditValue::Plain(format!("agent_id={agent_id}")),
        )?;
        return Ok(Some(fleet_protocol::WireMessage::new(
            fleet_core::generate_prefixed_ulid("msg")
                .map_err(|error| ControllerError::Json(error.to_string()))?,
            assignment.envelope.task_id.as_str().to_owned(),
            Some(agent_id.to_owned()),
            epoch_millis() as u64,
            fleet_protocol::WirePayload::TaskAssignment {
                envelope: task_envelope_to_wire(&assignment.envelope),
                task: fleet_protocol::TaskWire::RunbookExecution(
                    fleet_protocol::RunbookExecutionTaskWire {
                        runbook_document: assignment.runbook.runbook_document().to_owned(),
                        timeout_ms: assignment.runbook.timeout().as_millis() as u64,
                        confirmed_high_risk: true,
                    },
                ),
            },
        )));
    }

    let Some(assignment) = store
        .list_pending_drift_check_assignments_for_agent(agent_id)?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    store.update_job_status(assignment.envelope.job_id.as_str(), JobStatus::Running)?;
    audit_job(
        store,
        "drift_check_job_started",
        assignment.envelope.job_id.as_str(),
        AuditValue::Plain(format!("agent_id={agent_id}")),
    )?;
    Ok(Some(fleet_protocol::WireMessage::new(
        fleet_core::generate_prefixed_ulid("msg")
            .map_err(|error| ControllerError::Json(error.to_string()))?,
        assignment.envelope.task_id.as_str().to_owned(),
        Some(agent_id.to_owned()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::TaskAssignment {
            envelope: task_envelope_to_wire(&assignment.envelope),
            task: fleet_protocol::TaskWire::DriftCheck(fleet_protocol::DriftCheckTaskWire {
                policy_document: assignment.drift_check.policy_document().to_owned(),
            }),
        },
    )))
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
        } => store.append_job_output_chunk_record(&JobOutputChunk {
            job_id,
            agent_id: agent_id.to_owned(),
            stream: output_stream_from_wire(stream),
            sequence,
            body: data,
        })?,
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
            return Ok(true);
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
            return Ok(true);
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

fn route_request_with_identity(
    request: &str,
    store: &SqliteStore,
    identity: &ControllerIdentity,
    metadata: &ControllerRuntimeMetadata,
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
                Ok(body) => Ok(response(201, "application/json", &format!("{body}\n"))),
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
                Ok(body) => Ok(response(201, "application/json", &format!("{body}\n"))),
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
                Ok(body) => Ok(response(201, "application/json", &format!("{body}\n"))),
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
            let body = list_jobs(store)?;
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
        ("GET", "/api/agents") => {
            let body = list_agents(store)?;
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
            match revoke_agent_key(agent_id, store) {
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
            match get_agent(agent_id, store)? {
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
            match update_agent_labels(agent_id, request_body(request), store) {
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
) -> Result<String, CreateCommandJobHttpError> {
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
    serde_json::to_string(&CreateCommandJobResponse {
        job_id,
        target_count: output.targets.len(),
        assignment_count: output.envelopes.len(),
    })
    .map_err(|error| CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string())))
}

fn create_drift_check_job(
    body: &str,
    store: &SqliteStore,
    identity: &ControllerIdentity,
) -> Result<String, CreateCommandJobHttpError> {
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
    serde_json::to_string(&CreateDriftCheckJobResponse {
        job_id,
        target_count: output.targets.len(),
        assignment_count: output.envelopes.len(),
    })
    .map_err(|error| CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string())))
}

fn create_runbook_job(
    body: &str,
    store: &SqliteStore,
    identity: &ControllerIdentity,
) -> Result<String, CreateCommandJobHttpError> {
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
    serde_json::to_string(&CreateRunbookJobResponse {
        job_id,
        target_count: output.targets.len(),
        assignment_count: output.envelopes.len(),
    })
    .map_err(|error| CreateCommandJobHttpError::Internal(ControllerError::Json(error.to_string())))
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

fn list_agents(store: &SqliteStore) -> Result<String, ControllerError> {
    mark_stale_agents_offline_for_inventory(store)?;
    let repo = ControllerAgentInventoryRepository { store };
    let agents = ListInventoryAgents::execute(&repo)?
        .iter()
        .map(|agent| agent_to_response_with_latest_facts(agent, store))
        .collect::<Result<Vec<_>, _>>()?;
    serde_json::to_string(&agents).map_err(|error| ControllerError::Json(error.to_string()))
}

fn get_agent(agent_id: &str, store: &SqliteStore) -> Result<Option<String>, ControllerError> {
    mark_stale_agents_offline_for_inventory(store)?;
    let agent_id = AgentId::new(agent_id).map_err(|error| ControllerError::Store(error.into()))?;
    let repo = ControllerAgentInventoryRepository { store };
    let Some(agent) = GetInventoryAgent::execute(&repo, agent_id)? else {
        return Ok(None);
    };
    let response = agent_to_response_with_latest_facts(&agent, store)?;
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
    let response = agent_to_response_with_latest_facts(&agent, store)?;
    serde_json::to_string(&response)
        .map(Some)
        .map_err(|error| ControllerError::Json(error.to_string()))
}

fn revoke_agent_key(
    agent_id: &str,
    store: &SqliteStore,
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
    let response = agent_to_response_with_latest_facts(&agent, store)?;
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

fn list_jobs(store: &SqliteStore) -> Result<String, ControllerError> {
    let repo = ControllerJobQueryRepository { store };
    let jobs = ListJobSummaries::execute(&repo, 50)?;
    let response = jobs
        .into_iter()
        .map(|job| JobSummaryResponse {
            id: job.id,
            status: job.status,
            risk: job.risk,
            command_program: job.command_program,
            command_args: job.command_args,
            target_count: job.target_count,
            created_at_ms: system_time_to_millis(job.created_at),
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&response).map_err(|error| ControllerError::Json(error.to_string()))
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
) -> Result<AgentResponse, ControllerError> {
    let summary = store
        .latest_facts_snapshot(agent.id().as_str())?
        .and_then(|record| agent_facts_summary(&record.body));
    Ok(agent_to_response(agent, summary.as_ref()))
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

fn agent_to_response(agent: &Agent, facts: Option<&AgentFactsSummary>) -> AgentResponse {
    let last_seen_at = agent.last_seen_at();
    let revoked = agent.status() == AgentStatus::Disabled;
    AgentResponse {
        id: agent.id().as_str().to_owned(),
        name: agent.name().as_str().to_owned(),
        status: agent_status_for_inventory(agent.status()).to_owned(),
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

fn agent_status_for_inventory(status: AgentStatus) -> &'static str {
    if status == AgentStatus::Disabled {
        "offline"
    } else {
        agent_status_to_str(status)
    }
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
                created_at: record.created_at,
            })
            .collect())
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

        let response = route_request(
            "GET /api/agents HTTP/1.1\r\nAuthorization: Bearer admin-token\r\n\r\n",
            &store,
        )
        .unwrap();

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"id\":\"agent-1\""));
        assert!(response.contains("\"name\":\"agent-1\""));
        assert!(response.contains("\"status\":\"online\""));
        assert!(response.contains("\"revoked\":false"));
        assert!(response.contains("\"fingerprint\""));
        assert!(response.contains("\"hostname\":\"web-01\""));
        assert!(response.contains("\"os\":\"linux\""));
        assert!(response.contains("\"arch\":\"x86_64\""));
        assert!(response.contains("\"last_seen_age_seconds\""));
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

        assert!(finished);
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

        assert!(finished);
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

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("\"id\":\"job-history-1\""));
        assert!(response.contains("\"command_program\":\"uptime\""));
        assert!(response.contains("\"target_count\":1"));
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
