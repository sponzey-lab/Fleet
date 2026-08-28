use clap::{Args, Parser, Subcommand};
use fleet_application::{
    DisabledSecretProvider, ResolvedSecret, RetentionPolicy, RunRetentionCleanup,
    RunRetentionCleanupInput, SecretProvider, SecretProviderError,
};
use fleet_core::{
    DatabaseSettings, LogProfile, format_error_message, format_warning_message, init_logging,
    redact_secret,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::error::Error as StdError;
use std::fmt::{Display, Formatter};
use std::fs::{self, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio_rustls::rustls::{ClientConfig as RustlsClientConfig, RootCertStore};
use tungstenite::client::IntoClientRequest;
use tungstenite::{Connector, Message};

#[cfg(test)]
use std::sync::mpsc;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

const LOG_TAIL_MAX_LINES: usize = 50;
const LOG_TAIL_MAX_LINE_BYTES: usize = 4096;
const LOG_TAIL_POLL_INTERVAL: Duration = Duration::from_millis(100);
const REMOTE_RUN_EXPIRES_IN_SECONDS: u64 = 300;
const AGENT_SESSION_OUTBOUND_QUEUE_CAPACITY: usize = 64;
const AGENT_SESSION_READ_POLL_INTERVAL: Duration = Duration::from_millis(250);
const DEFAULT_CLI_PROFILE_PATH: &str = ".fleet/cli-profile.json";
const CONTROLLER_SIGNING_STAGED_TRUST_BUNDLE_PATH: &str =
    "/api/controller/signing-rotation/rollout-trust-bundle/staged";
const AGENT_CERTIFICATE_LIFECYCLE_RUNTIME_NOT_IMPLEMENTED: &str =
    "certificate_lifecycle_runtime_not_implemented";

#[derive(Debug, Parser)]
#[command(name = "fleet")]
#[command(about = "Sponzey Fleet command line interface")]
#[command(version = env!("CARGO_PKG_VERSION"))]
pub struct Cli {
    #[arg(long, default_value = "product")]
    pub log_profile: LogProfileArg,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum LogProfileArg {
    Product,
    FieldDebug,
    Development,
}

impl From<LogProfileArg> for LogProfile {
    fn from(value: LogProfileArg) -> Self {
        match value {
            LogProfileArg::Product => Self::Product,
            LogProfileArg::FieldDebug => Self::FieldDebug,
            LogProfileArg::Development => Self::Development,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Login(LoginCommand),
    Controller(ControllerCommand),
    Agent(AgentCommand),
    Agents(AgentsCommand),
    Jobs(JobsCommand),
    Approvals(ApprovalsCommand),
    Remediations(RemediationsCommand),
    Selectors(SelectorsCommand),
    Audit(AuditCommand),
    EnrollToken(EnrollTokenCommand),
    Run(RunCommand),
    Facts(FactsCommand),
    Metrics(MetricsCommand),
    Logs(LogsCommand),
    Drift(DriftCommand),
    Apply(ApplyCommand),
    Retention(RetentionCommand),
    Upgrade(UpgradeCommand),
    Demo(DemoCommand),
}

#[derive(Debug, Args)]
pub struct DemoCommand {
    #[arg(long)]
    keep_temp: bool,
    #[arg(long)]
    port: Option<u16>,
}

#[derive(Debug, Args)]
pub struct LoginCommand {
    #[arg(long)]
    controller_url: String,
    #[arg(long)]
    admin_token: String,
    #[arg(long, default_value = DEFAULT_CLI_PROFILE_PATH)]
    profile_path: PathBuf,
}

#[derive(Debug, Args, Clone)]
pub struct ProtectedApiArgs {
    #[arg(long)]
    pub controller_url: Option<String>,
    #[arg(long)]
    pub admin_token: Option<String>,
    #[arg(long, default_value = DEFAULT_CLI_PROFILE_PATH)]
    pub profile_path: PathBuf,
}

#[derive(Debug, Args)]
pub struct ControllerCommand {
    #[command(subcommand)]
    pub command: ControllerSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum ControllerSubcommand {
    Init {
        #[arg(long, default_value = ".fleet")]
        data_dir: PathBuf,
    },
    #[command(
        about = "Start the Sponzey Fleet controller",
        long_about = "Start the Sponzey Fleet controller API, Web Admin UI, and agent gateway.\n\nThe bind host controls where the process listens. The external URL is the public URL agents and operators should use. Do not use 0.0.0.0 as an agent URL. HTTP URLs are allowed for tests only, but every HTTP use prints a warning because traffic is not encrypted. Product and production environments must use HTTPS.",
        after_help = "Examples:\n  Local loopback demo:\n    fleet controller start --host 127.0.0.1 --port 7700 --data-dir .fleet --external-url http://127.0.0.1:7700\n\n  Test-only HTTP remote controller with warning:\n    fleet controller start --host 0.0.0.0 --port 7700 --data-dir /var/lib/fleet --external-url http://192.168.0.10:7700\n\n  HTTPS behind DNS/reverse proxy:\n    fleet controller start --host 127.0.0.1 --port 7700 --data-dir /var/lib/fleet --external-url https://fleet.example.com\n\n  Built-in HTTPS listener:\n    fleet controller start --host 0.0.0.0 --port 7700 --data-dir /var/lib/fleet --external-url https://fleet.example.com --tls-cert /etc/fleet/tls/fullchain.pem --tls-key /etc/fleet/tls/privkey.pem"
    )]
    Start {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 7700)]
        port: u16,
        #[arg(
            long,
            help = "Public controller URL agents use; http:// is test-only and prints warnings"
        )]
        external_url: Option<String>,
        #[arg(
            long,
            help = "Controller database URL; sqlite:// works today, postgresql:// is recognized but not implemented yet"
        )]
        db: Option<String>,
        #[arg(long, help = "PEM certificate chain for built-in HTTPS listener")]
        tls_cert: Option<PathBuf>,
        #[arg(long, help = "PEM private key for built-in HTTPS listener")]
        tls_key: Option<PathBuf>,
        #[arg(
            long,
            help = "PEM CA certificate for future agent client-certificate mTLS; currently rejected until enforcement is implemented"
        )]
        agent_client_ca_cert: Option<PathBuf>,
        #[arg(long, default_value = ".fleet")]
        data_dir: PathBuf,
    },
    InstallService {
        #[arg(long)]
        binary: Option<PathBuf>,
        #[arg(long, default_value = "/var/lib/fleet")]
        data_dir: PathBuf,
        #[arg(long)]
        user: Option<String>,
        #[arg(long)]
        group: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    StartService {
        #[arg(long)]
        dry_run: bool,
    },
    RestartService {
        #[arg(long)]
        dry_run: bool,
    },
    StatusService {
        #[arg(long)]
        dry_run: bool,
    },
    #[command(about = "Show controller signing rotation readiness status")]
    SigningRotationStatus {
        #[command(flatten)]
        api: ProtectedApiArgs,
        #[arg(long, help = "Print the secret-free JSON response")]
        json: bool,
    },
    #[command(about = "Mutate controller signing rotation state")]
    SigningRotation {
        #[command(subcommand)]
        command: ControllerSigningRotationSubcommand,
    },
    LogsService {
        #[arg(long, default_value_t = 50)]
        lines: usize,
        #[arg(long)]
        dry_run: bool,
    },
    UninstallService {
        #[arg(long)]
        dry_run: bool,
    },
    #[command(
        about = "Create a controller data backup archive",
        long_about = "Create a JSON backup archive for the controller data under <data-dir>/controller.\n\nThe archive contains controller keys, SQLite data, metadata, and checksums. Treat it as sensitive because it contains enough controller state to restore the fleet."
    )]
    Backup {
        #[arg(long, default_value = ".fleet")]
        data_dir: PathBuf,
        #[arg(long, help = "Backup archive path to create")]
        output: PathBuf,
    },
    #[command(
        about = "Restore a controller data backup archive",
        long_about = "Restore a JSON backup archive into <data-dir>/controller.\n\nDry-run validates format, checksums, schema compatibility, and SQLite integrity without writing. Actual restore refuses to overwrite existing controller data unless --force is provided."
    )]
    Restore {
        #[arg(long, default_value = ".fleet")]
        data_dir: PathBuf,
        #[arg(long, help = "Backup archive path to restore")]
        input: PathBuf,
        #[arg(
            long,
            help = "Validate the archive and print the restore plan without writing"
        )]
        dry_run: bool,
        #[arg(
            long,
            help = "Allow replacing an existing non-empty controller data directory"
        )]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ControllerSigningRotationSubcommand {
    RestartPlan {
        #[command(flatten)]
        api: ProtectedApiArgs,
        #[arg(long, help = "Print the secret-free JSON response")]
        json: bool,
    },
    RestartAction {
        #[command(flatten)]
        api: ProtectedApiArgs,
        #[arg(long)]
        confirm_external_restart: bool,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long, help = "Print the secret-free JSON response")]
        json: bool,
    },
    Request {
        #[command(flatten)]
        api: ProtectedApiArgs,
        #[arg(long)]
        new_fingerprint: String,
        #[arg(long)]
        old_key_verifies_for_seconds: Option<u64>,
        #[arg(long)]
        old_key_verifies_until_ms: Option<u64>,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long, help = "Print the secret-free JSON response")]
        json: bool,
    },
    Validate {
        #[command(flatten)]
        api: ProtectedApiArgs,
        #[arg(long)]
        candidate_public_key_path: PathBuf,
        #[arg(long)]
        candidate_private_key_path: PathBuf,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long, help = "Print the secret-free JSON response")]
        json: bool,
    },
    Activate {
        #[command(flatten)]
        api: ProtectedApiArgs,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long, help = "Print the secret-free JSON response")]
        json: bool,
    },
    Retire {
        #[command(flatten)]
        api: ProtectedApiArgs,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long, help = "Print the secret-free JSON response")]
        json: bool,
    },
    Fail {
        #[command(flatten)]
        api: ProtectedApiArgs,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long, help = "Print the secret-free JSON response")]
        json: bool,
    },
    RolloutTrustBundle {
        #[command(flatten)]
        api: ProtectedApiArgs,
        #[arg(long)]
        previous_public_key_path: Option<PathBuf>,
        #[arg(long = "agent-id")]
        agent_ids: Vec<String>,
        #[arg(long, help = "Print the secret-free JSON response")]
        json: bool,
    },
    RetryTrustBundle {
        #[command(flatten)]
        api: ProtectedApiArgs,
        #[arg(long)]
        previous_public_key_path: Option<PathBuf>,
        #[arg(long = "agent-id")]
        agent_ids: Vec<String>,
        #[arg(long)]
        max_agent_count: Option<usize>,
        #[arg(long, help = "Print the secret-free JSON response")]
        json: bool,
    },
    StagedTrustBundle {
        #[command(flatten)]
        api: ProtectedApiArgs,
        #[arg(long)]
        previous_public_key_path: Option<PathBuf>,
        #[arg(long = "agent-id")]
        agent_ids: Vec<String>,
        #[arg(long)]
        batch_size: usize,
        #[arg(long)]
        max_failures: usize,
        #[arg(long)]
        ack_timeout_seconds: u64,
        #[arg(long, help = "Print the secret-free JSON response")]
        json: bool,
    },
}

#[derive(Debug, Args)]
#[command(about = "Manage the local Sponzey Fleet agent")]
pub struct AgentCommand {
    #[command(subcommand)]
    pub command: AgentSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum AgentSubcommand {
    #[command(
        about = "Enroll this host as an agent",
        long_about = "Enroll this host with a controller using a one-time enrollment token.\n\nEnrollment writes the local agent identity, private key, labels, and pinned controller fingerprint under the selected data directory. Run this once before starting the agent.",
        visible_alias = "enroll",
        after_help = "Examples:\n  Local loopback development:\n    fleet agent init --url http://127.0.0.1:7700 --token <token> --name web-01 --labels role=web,env=dev\n\n  Test-only remote HTTP with warning:\n    fleet agent init --data-dir /var/lib/fleet --url http://192.168.0.10:7700 --token <token> --name test-web-01 --labels role=web,env=test\n\n  HTTPS:\n    fleet agent init --data-dir /var/lib/fleet --url https://fleet.example.com --token <token> --name prod-web-01 --labels role=web,env=prod\n\n  HTTPS with a private CA:\n    fleet agent init --data-dir /var/lib/fleet --url https://fleet.example.com --tls-ca-cert /etc/fleet/tls/ca.pem --token <token> --name prod-web-01"
    )]
    Init {
        #[arg(long, help = "Controller URL to enroll against")]
        url: String,
        #[arg(long, help = "One-time enrollment token created by the controller")]
        token: String,
        #[arg(long, help = "Human-readable agent name shown in inventory")]
        name: String,
        #[arg(
            long,
            default_value = "",
            help = "Comma-separated labels used for targeting, for example role=web,env=prod"
        )]
        labels: String,
        #[arg(
            long,
            help = "Additional PEM CA certificate used to trust a private/self-signed controller TLS endpoint"
        )]
        tls_ca_cert: Option<PathBuf>,
        #[arg(long, default_value = ".fleet", help = "Agent data directory")]
        data_dir: PathBuf,
    },
    #[command(
        about = "Start the enrolled local agent",
        long_about = "Start the enrolled local agent persistent session loop.\n\nThe agent reads its local identity from <data-dir>/agent/agent.conf, verifies the pinned controller fingerprint before opening the session, sends heartbeat liveness ticks, static facts inventory, metrics snapshots, and product-safe agent operational logs through one outbound writer queue, and receives controller-signed tasks on the same session. Heartbeat is only a liveness signal; facts, metrics, and logs each have their own bootstrap CLI interval. Connection failures are retried indefinitely by default. The agent must be enrolled before this command can run.",
        after_help = "Examples:\n  fleet agent start --data-dir .fleet\n  fleet agent start --data-dir /var/lib/fleet\n  fleet agent start --data-dir .fleet --once\n  fleet agent start --data-dir .fleet --facts-interval-seconds 300 --metrics-interval-seconds 30 --log-upload-interval-seconds 30\n\nLocal development flow:\n  fleet controller init --data-dir .fleet\n  fleet enroll-token create --data-dir .fleet --labels role=web,env=dev\n  fleet agent init --data-dir .fleet --url http://127.0.0.1:7700 --token <token> --name web-01 --labels role=web,env=dev\n  fleet agent start --data-dir .fleet"
    )]
    Start {
        #[arg(
            long,
            default_value = ".fleet",
            help = "Directory containing agent/agent.conf and agent/agent_private.key"
        )]
        data_dir: PathBuf,
        #[arg(
            long,
            help = "Send one heartbeat, process pending signed tasks once, then exit"
        )]
        once: bool,
        #[arg(
            long,
            default_value_t = 30,
            help = "Seconds between heartbeat liveness ticks; this does not control facts, metrics, log upload, or task dispatch"
        )]
        heartbeat_interval_seconds: u64,
        #[arg(
            long,
            default_value_t = 300,
            help = "Seconds between static inventory facts snapshots after the initial session snapshot"
        )]
        facts_interval_seconds: u64,
        #[arg(
            long,
            default_value_t = 30,
            help = "Seconds between usage metrics snapshots after the initial session snapshot"
        )]
        metrics_interval_seconds: u64,
        #[arg(long, help = "Disable periodic product-safe agent log upload")]
        disable_log_upload: bool,
        #[arg(
            long,
            default_value_t = 30,
            help = "Seconds between product-safe agent log uploads"
        )]
        log_upload_interval_seconds: u64,
        #[arg(
            long,
            default_value_t = 0,
            help = "Maximum reconnect attempts before exit; 0 means retry indefinitely"
        )]
        max_reconnect_attempts: u32,
    },
    #[command(
        about = "Install the agent as a systemd service",
        long_about = "Render or install the Linux systemd unit for running 'fleet agent start'.\n\nDry-run is safe on every platform. Writing service files requires Linux and root privileges."
    )]
    InstallService {
        #[arg(long, help = "Absolute fleet binary path to pin in the service unit")]
        binary: Option<PathBuf>,
        #[arg(
            long,
            default_value = "/var/lib/fleet",
            help = "Persistent agent data directory used by the service"
        )]
        data_dir: PathBuf,
        #[arg(long, help = "Linux service user")]
        user: Option<String>,
        #[arg(long, help = "Linux service group")]
        group: Option<String>,
        #[arg(long, help = "Print the unit file without writing system files")]
        dry_run: bool,
    },
    #[command(about = "Start the installed agent systemd service")]
    StartService {
        #[arg(long, help = "Print the systemctl command without executing it")]
        dry_run: bool,
    },
    #[command(about = "Show the installed agent systemd service status")]
    StatusService {
        #[arg(long, help = "Print the systemctl command without executing it")]
        dry_run: bool,
    },
    #[command(about = "Show recent installed agent service logs from journald")]
    LogsService {
        #[arg(long, default_value_t = 50, help = "Number of recent journald lines")]
        lines: usize,
        #[arg(long, help = "Print the journalctl command without executing it")]
        dry_run: bool,
    },
    #[command(about = "Disable and remove the installed agent systemd service")]
    UninstallService {
        #[arg(long, help = "Print the uninstall commands without executing them")]
        dry_run: bool,
    },
}

#[derive(Debug, Args)]
pub struct AgentsCommand {
    #[command(subcommand)]
    pub command: AgentsSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum AgentsSubcommand {
    List {
        #[arg(long, default_value = ".fleet")]
        data_dir: PathBuf,
    },
    RemoteList {
        #[command(flatten)]
        api: ProtectedApiArgs,
    },
    RemoteGet {
        agent_id: String,
        #[command(flatten)]
        api: ProtectedApiArgs,
    },
    RequestCertificateIssuance {
        agent_id: String,
        #[command(flatten)]
        api: ProtectedApiArgs,
        #[arg(long)]
        json: bool,
    },
    CertificateStatus {
        agent_id: String,
        #[command(flatten)]
        api: ProtectedApiArgs,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub struct JobsCommand {
    #[command(subcommand)]
    pub command: JobsSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum JobsSubcommand {
    List {
        #[command(flatten)]
        api: ProtectedApiArgs,
    },
    Get {
        job_id: String,
        #[command(flatten)]
        api: ProtectedApiArgs,
    },
    Output {
        job_id: String,
        #[command(flatten)]
        api: ProtectedApiArgs,
    },
    Cancel {
        job_id: String,
        #[command(flatten)]
        api: ProtectedApiArgs,
    },
}

#[derive(Debug, Args)]
pub struct ApprovalsCommand {
    #[command(subcommand)]
    pub command: ApprovalsSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum ApprovalsSubcommand {
    List {
        #[command(flatten)]
        api: ProtectedApiArgs,
    },
    Approve {
        approval_id: String,
        #[arg(long, default_value = "approved from CLI")]
        reason: String,
        #[command(flatten)]
        api: ProtectedApiArgs,
    },
    Reject {
        approval_id: String,
        #[arg(long, default_value = "rejected from CLI")]
        reason: String,
        #[command(flatten)]
        api: ProtectedApiArgs,
    },
}

#[derive(Debug, Args)]
pub struct RemediationsCommand {
    #[command(subcommand)]
    pub command: RemediationsSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum RemediationsSubcommand {
    List {
        #[arg(long)]
        agent_id: Option<String>,
        #[arg(long)]
        policy_id: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[command(flatten)]
        api: ProtectedApiArgs,
    },
    Get {
        remediation_id: String,
        #[command(flatten)]
        api: ProtectedApiArgs,
    },
    RequestApproval {
        remediation_id: String,
        #[arg(long)]
        approval_id: Option<String>,
        #[arg(long)]
        job_id: Option<String>,
        #[arg(long, default_value = "remediation requires approval")]
        reason: String,
        #[arg(long, default_value_t = REMOTE_RUN_EXPIRES_IN_SECONDS)]
        expires_in_seconds: u64,
        #[command(flatten)]
        api: ProtectedApiArgs,
    },
    Approve {
        remediation_id: String,
        #[arg(long)]
        approval_id: String,
        #[arg(long)]
        job_id: String,
        #[arg(long)]
        runbook: PathBuf,
        #[arg(long, default_value_t = 30)]
        timeout_seconds: u64,
        #[arg(long, default_value_t = REMOTE_RUN_EXPIRES_IN_SECONDS)]
        expires_in_seconds: u64,
        #[arg(long)]
        nonce_prefix: Option<String>,
        #[arg(long, default_value = "approved remediation")]
        reason: String,
        #[command(flatten)]
        api: ProtectedApiArgs,
    },
    Running {
        remediation_id: String,
        #[arg(long)]
        job_id: String,
        #[command(flatten)]
        api: ProtectedApiArgs,
    },
    Result {
        remediation_id: String,
        #[arg(long)]
        job_id: String,
        #[arg(long)]
        status: String,
        #[command(flatten)]
        api: ProtectedApiArgs,
    },
    Verify {
        remediation_id: String,
        #[arg(long)]
        agent_id: String,
        #[arg(long)]
        policy_id: String,
        #[arg(long)]
        policy_name: String,
        #[arg(long)]
        job_id: String,
        #[command(flatten)]
        api: ProtectedApiArgs,
    },
}

#[derive(Debug, Args)]
pub struct SelectorsCommand {
    #[command(subcommand)]
    pub command: SelectorsSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum SelectorsSubcommand {
    Preview {
        #[arg(long)]
        selector: String,
        #[command(flatten)]
        api: ProtectedApiArgs,
    },
}

#[derive(Debug, Args)]
pub struct AuditCommand {
    #[command(subcommand)]
    pub command: AuditSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum AuditSubcommand {
    Export {
        #[arg(long)]
        category: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        before: Option<String>,
        #[command(flatten)]
        api: ProtectedApiArgs,
    },
}

#[derive(Debug, Args)]
pub struct EnrollTokenCommand {
    #[command(subcommand)]
    pub command: EnrollTokenSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum EnrollTokenSubcommand {
    Create {
        #[arg(long, default_value = "")]
        labels: String,
        #[arg(long, default_value_t = 1)]
        max_uses: u32,
        #[arg(long, default_value_t = 3600)]
        expires_in_seconds: u64,
        #[arg(long)]
        controller_url: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        print_init_command: bool,
        #[arg(long, default_value = ".fleet")]
        data_dir: PathBuf,
    },
    List {
        #[arg(long, default_value = ".fleet")]
        data_dir: PathBuf,
    },
    Revoke {
        id: String,
        #[arg(long, default_value = ".fleet")]
        data_dir: PathBuf,
    },
}

#[derive(Debug, Args)]
pub struct RunCommand {
    #[arg(long)]
    pub selector: Option<String>,

    #[arg(long)]
    pub confirm_risk: bool,

    #[arg(long)]
    pub controller_url: Option<String>,

    #[arg(long)]
    pub admin_token: Option<String>,

    #[arg(long, default_value = DEFAULT_CLI_PROFILE_PATH)]
    pub profile_path: PathBuf,

    #[arg(long)]
    pub remote: bool,

    #[arg(long)]
    pub job_id: Option<String>,

    #[arg(long, default_value_t = 30)]
    pub timeout_seconds: u64,

    pub command: Vec<String>,
}

#[derive(Debug, Args)]
pub struct FactsCommand {
    pub agent: Option<String>,
}

#[derive(Debug, Args)]
pub struct MetricsCommand {
    pub agent: Option<String>,
}

#[derive(Debug, Args)]
pub struct ApplyCommand {
    pub file: PathBuf,
}

#[derive(Debug, Args)]
pub struct RetentionCommand {
    #[command(subcommand)]
    pub command: RetentionSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum RetentionSubcommand {
    Cleanup {
        #[arg(long, default_value = ".fleet")]
        data_dir: PathBuf,
        #[arg(long, default_value_t = 30)]
        older_than_days: u64,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Args)]
pub struct LogsCommand {
    pub target: Option<String>,

    #[arg(long)]
    pub file: Option<String>,

    #[arg(long)]
    pub follow: bool,

    #[arg(long)]
    pub max_duration_seconds: Option<u64>,
}

#[derive(Debug, Args)]
pub struct DriftCommand {
    #[command(subcommand)]
    pub command: DriftSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum DriftSubcommand {
    Check {
        #[arg(long)]
        policy: Option<String>,
    },
}

#[derive(Debug, Args)]
#[command(
    about = "Inspect the Sponzey Fleet upgrade plan",
    long_about = "Inspect the current upgrade policy without replacing the running binary.\n\nAutomatic self-upgrade is intentionally not implemented yet. Use --dry-run to see the required backup, artifact integrity, channel, and recovery steps before upgrading with an external package manager or release artifact."
)]
pub struct UpgradeCommand {
    #[arg(long, default_value = "stable")]
    channel: UpgradeChannelArg,
    #[arg(
        long,
        help = "Target version to inspect; defaults to latest in the channel"
    )]
    version: Option<String>,
    #[arg(long, help = "Print the upgrade plan without changing files")]
    dry_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum UpgradeChannelArg {
    Stable,
    Beta,
}

impl UpgradeChannelArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
        }
    }
}

#[derive(Debug)]
pub enum CliError {
    Io(std::io::Error),
    Controller(Box<fleet_controller::ControllerError>),
    Identity(fleet_core::IdentityError),
    Store(fleet_store::StoreError),
    Http(String),
    ControllerNotInitialized { data_dir: PathBuf },
    HighRiskConfirmationRequired,
    EmptyCommand,
    MissingPolicy,
    ServiceInstallRequiresDryRun,
    ServiceOperationRequiresLinux,
    ServiceOperationRequiresRoot,
    ServiceBinaryMustBeAbsolute(PathBuf),
    InvalidServiceAccount(String),
    UpgradeRequiresDryRun,
}

impl Display for CliError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Controller(error) => write!(formatter, "{error}"),
            Self::Identity(error) => write!(formatter, "identity error: {error}"),
            Self::Store(error) => write!(formatter, "store error: {error:?}"),
            Self::Http(error) => write!(formatter, "http error: {error}"),
            Self::ControllerNotInitialized { data_dir } => {
                write!(
                    formatter,
                    "controller is not initialized for data dir: {}\n\nInitialize it once before starting the controller:\n\n  fleet controller init --data-dir \"{}\"\n  fleet controller start --host 127.0.0.1 --port 7700 --data-dir \"{}\" --external-url http://127.0.0.1:7700\n\nIf you use local scripts:\n\n  ./scripts/run_controller.sh --host 127.0.0.1 --port 7700 --data-dir \"{}\" --external-url http://127.0.0.1:7700",
                    data_dir.display(),
                    data_dir.display(),
                    data_dir.display(),
                    data_dir.display()
                )
            }
            Self::HighRiskConfirmationRequired => {
                write!(formatter, "high-risk command requires --confirm-risk")
            }
            Self::EmptyCommand => write!(formatter, "command cannot be empty"),
            Self::MissingPolicy => write!(formatter, "drift check requires --policy"),
            Self::ServiceInstallRequiresDryRun => {
                write!(
                    formatter,
                    "service install writes system files and requires Linux root; use --dry-run to inspect the unit first"
                )
            }
            Self::ServiceOperationRequiresLinux => {
                write!(formatter, "systemd service operations require Linux")
            }
            Self::ServiceOperationRequiresRoot => {
                write!(
                    formatter,
                    "systemd service operations require root; rerun with sudo"
                )
            }
            Self::ServiceBinaryMustBeAbsolute(path) => {
                write!(
                    formatter,
                    "service binary path must be absolute: {}",
                    path.display()
                )
            }
            Self::InvalidServiceAccount(value) => {
                write!(formatter, "invalid service user/group value: {value}")
            }
            Self::UpgradeRequiresDryRun => {
                write!(
                    formatter,
                    "automatic self-upgrade is not implemented; rerun with --dry-run to inspect the supported external upgrade plan"
                )
            }
        }
    }
}

impl std::error::Error for CliError {}

impl From<std::io::Error> for CliError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<fleet_store::StoreError> for CliError {
    fn from(value: fleet_store::StoreError) -> Self {
        Self::Store(value)
    }
}

impl From<fleet_controller::ControllerError> for CliError {
    fn from(value: fleet_controller::ControllerError) -> Self {
        Self::Controller(Box::new(value))
    }
}

impl From<fleet_core::IdentityError> for CliError {
    fn from(value: fleet_core::IdentityError) -> Self {
        Self::Identity(value)
    }
}

pub fn main_entry() -> ExitCode {
    let cli = Cli::parse();
    init_logging(cli.log_profile.into());

    match execute(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}", format_error_message(error.to_string()));
            ExitCode::from(2)
        }
    }
}

pub fn execute(cli: Cli) -> Result<(), CliError> {
    match cli.command {
        Command::Login(command) => execute_login(command),
        Command::Controller(command) => execute_controller(command),
        Command::Agent(command) => execute_agent(command),
        Command::Agents(command) => execute_agents(command),
        Command::Jobs(command) => execute_jobs(command),
        Command::Approvals(command) => execute_approvals(command),
        Command::Remediations(command) => execute_remediations(command),
        Command::Selectors(command) => execute_selectors(command),
        Command::Audit(command) => execute_audit(command),
        Command::EnrollToken(command) => execute_enroll_token(command),
        Command::Run(command) => execute_run(command),
        Command::Facts(command) => execute_facts(command),
        Command::Metrics(command) => execute_metrics(command),
        Command::Logs(command) => execute_logs(command),
        Command::Drift(command) => execute_drift(command),
        Command::Apply(command) => execute_apply(command),
        Command::Retention(command) => execute_retention(command),
        Command::Upgrade(command) => execute_upgrade(command),
        Command::Demo(command) => execute_demo(command),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CliProfile {
    controller_url: String,
    admin_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProtectedApiClient {
    controller_url: String,
    admin_token: String,
}

fn execute_login(command: LoginCommand) -> Result<(), CliError> {
    let profile = CliProfile {
        controller_url: command.controller_url,
        admin_token: command.admin_token,
    };
    save_cli_profile(&command.profile_path, &profile)?;
    println!("profile_path={}", command.profile_path.display());
    println!("controller_url={}", profile.controller_url);
    println!("status=created");
    Ok(())
}

fn save_cli_profile(path: &Path, profile: &CliProfile) -> Result<(), CliError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let body =
        serde_json::to_string_pretty(profile).map_err(|error| CliError::Http(error.to_string()))?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(body.as_bytes())?;
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn read_cli_profile(path: &Path) -> Result<CliProfile, CliError> {
    ensure_secure_profile_permissions(path)?;
    let body = fs::read_to_string(path)?;
    serde_json::from_str(&body).map_err(|error| CliError::Http(error.to_string()))
}

fn ensure_secure_profile_permissions(path: &Path) -> Result<(), CliError> {
    let metadata = fs::metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CliError::Http(format!(
                "CLI profile not found: {}; run `fleet login --controller-url <url> --admin-token <token>` or pass --controller-url and --admin-token",
                path.display()
            ))
        } else {
            CliError::Io(error)
        }
    })?;
    #[cfg(unix)]
    {
        let mode = metadata.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(CliError::Http(format!(
                "CLI profile has insecure permissions: {}; expected mode 600",
                path.display()
            )));
        }
    }
    Ok(())
}

fn resolve_protected_api(args: &ProtectedApiArgs) -> Result<ProtectedApiClient, CliError> {
    let profile_needed = args.controller_url.is_none() || args.admin_token.is_none();
    let profile = if profile_needed {
        Some(read_cli_profile(&args.profile_path)?)
    } else {
        None
    };
    let controller_url = args
        .controller_url
        .clone()
        .or_else(|| {
            profile
                .as_ref()
                .map(|profile| profile.controller_url.clone())
        })
        .ok_or_else(|| CliError::Http("--controller-url is required".to_owned()))?;
    let admin_token = args
        .admin_token
        .clone()
        .or_else(|| profile.as_ref().map(|profile| profile.admin_token.clone()))
        .ok_or_else(|| CliError::Http("--admin-token is required".to_owned()))?;
    Ok(ProtectedApiClient {
        controller_url,
        admin_token,
    })
}

fn run_command_api_args(command: &RunCommand) -> ProtectedApiArgs {
    ProtectedApiArgs {
        controller_url: command.controller_url.clone(),
        admin_token: command.admin_token.clone(),
        profile_path: command.profile_path.clone(),
    }
}

impl ProtectedApiClient {
    fn request(&self, method: &str, path: &str, body: Option<&str>) -> Result<String, CliError> {
        let response = http_request_url(
            &self.controller_url,
            method,
            path,
            Some(&self.admin_token),
            body,
        )?;
        protected_response_body(&response, &[200, 201])
    }

    fn get(&self, path: &str) -> Result<String, CliError> {
        self.request("GET", path, None)
    }

    fn post(&self, path: &str, body: Option<&str>) -> Result<String, CliError> {
        self.request("POST", path, body)
    }
}

fn protected_response_body(response: &str, success_codes: &[u16]) -> Result<String, CliError> {
    let status = http_response_status_code(response).unwrap_or(0);
    if success_codes.contains(&status) {
        return Ok(http_response_body(response).to_owned());
    }
    Err(CliError::Http(render_http_status_error(status)))
}

fn http_response_status_code(response: &str) -> Option<u16> {
    response
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn http_response_body(response: &str) -> &str {
    response.split("\r\n\r\n").nth(1).unwrap_or_default()
}

fn render_http_status_error(status: u16) -> String {
    match status {
        401 => "unauthorized: admin token is missing or invalid".to_owned(),
        403 => "forbidden: admin token lacks required permission".to_owned(),
        404 => "not found: requested resource does not exist".to_owned(),
        409 => "conflict: request conflicts with current state".to_owned(),
        0 => "request failed: malformed HTTP response".to_owned(),
        _ => format!("request failed: HTTP {status}"),
    }
}

fn print_json_response(body: &str) -> Result<(), CliError> {
    let json: serde_json::Value =
        serde_json::from_str(body).map_err(|error| CliError::Http(error.to_string()))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json).map_err(|error| CliError::Http(error.to_string()))?
    );
    Ok(())
}

fn render_agent_certificate_issuance_request_for_cli(body: &str) -> Result<Vec<String>, CliError> {
    let json: serde_json::Value =
        serde_json::from_str(body).map_err(|error| CliError::Http(error.to_string()))?;
    Ok(vec![
        format!(
            "agent_id={}\taction={}\tstate={}",
            json_field(&json, "agent_id"),
            json_field(&json, "action"),
            json_field(&json, "lifecycle_state")
        ),
        format!(
            "dispatch_status={}\taudit_event_action={}",
            json_field(&json, "dispatch_status"),
            json_field(&json, "audit_event_action")
        ),
        format!(
            "current_fingerprint_prefix={}\tnext_fingerprint_prefix={}",
            json_optional_field(&json, "current_fingerprint_prefix")
                .unwrap_or_else(|| "none".to_owned()),
            json_optional_field(&json, "next_fingerprint_prefix")
                .unwrap_or_else(|| "none".to_owned())
        ),
    ])
}

fn render_agent_certificate_lifecycle_status_for_cli(body: &str) -> Result<Vec<String>, CliError> {
    let json: serde_json::Value =
        serde_json::from_str(body).map_err(|error| CliError::Http(error.to_string()))?;
    Ok(vec![
        format!(
            "agent_id={}\tstate={}\trecord_present={}",
            json_field(&json, "agent_id"),
            json_field(&json, "lifecycle_state"),
            json_field(&json, "record_present")
        ),
        format!(
            "current_fingerprint_prefix={}\tnext_fingerprint_prefix={}",
            json_optional_field(&json, "current_fingerprint_prefix")
                .unwrap_or_else(|| "none".to_owned()),
            json_optional_field(&json, "next_fingerprint_prefix")
                .unwrap_or_else(|| "none".to_owned())
        ),
        format!(
            "grace_until_ms={}\trevocation_reason={}\tupdated_at_ms={}",
            json_optional_number(&json, "grace_until_ms")
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_owned()),
            json_optional_field(&json, "revocation_reason").unwrap_or_else(|| "none".to_owned()),
            json_optional_number(&json, "updated_at_ms")
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_owned())
        ),
    ])
}

fn render_controller_signing_rotation_status_for_cli(body: &str) -> Result<Vec<String>, CliError> {
    let json: serde_json::Value =
        serde_json::from_str(body).map_err(|error| CliError::Http(error.to_string()))?;
    let mut lines = vec![
        format!(
            "controller_id={}\tstate={}\treadiness={}",
            json_field(&json, "controller_id"),
            json_field(&json, "persisted_state"),
            json_field(&json, "readiness")
        ),
        format!(
            "active_signing_fingerprint_prefix={}\tselected_signing_fingerprint_prefix={}",
            json_field(&json, "active_signing_fingerprint_prefix"),
            json_field(&json, "selected_signing_fingerprint_prefix")
        ),
        format!(
            "old_fingerprint_prefix={}\tnew_fingerprint_prefix={}",
            json_field(&json, "old_fingerprint_prefix"),
            json_optional_field(&json, "new_fingerprint_prefix")
                .unwrap_or_else(|| "none".to_owned())
        ),
        format!(
            "bootstrap_guard={}\tagent_trust_rollout={}",
            json_field(&json, "bootstrap_guard"),
            json_field(&json, "agent_trust_rollout")
        ),
    ];
    if let Some(window) = json_optional_number(&json, "old_key_verifies_until_ms") {
        lines.push(format!("old_key_verifies_until_ms={window}"));
    }
    Ok(lines)
}

fn render_controller_signing_rotation_restart_plan_for_cli(
    body: &str,
) -> Result<Vec<String>, CliError> {
    let json: serde_json::Value =
        serde_json::from_str(body).map_err(|error| CliError::Http(error.to_string()))?;
    let mut lines = vec![
        format!(
            "controller_id={}\trestart_required={}\treload_supported={}",
            json_field(&json, "controller_id"),
            json_field(&json, "restart_required"),
            json_field(&json, "reload_supported")
        ),
        format!(
            "recommended_action={}\treadiness={}",
            json_field(&json, "recommended_action"),
            json_field(&json, "readiness")
        ),
        format!(
            "bootstrap_guard={}\tagent_trust_rollout={}",
            json_field(&json, "bootstrap_guard"),
            json_field(&json, "agent_trust_rollout")
        ),
        format!(
            "active_signing_fingerprint_prefix={}\tselected_signing_fingerprint_prefix={}",
            json_field(&json, "active_signing_fingerprint_prefix"),
            json_field(&json, "selected_signing_fingerprint_prefix")
        ),
    ];
    if let Some(reason) = json_optional_field(&json, "blocked_reason") {
        lines.push(format!("blocked_reason={reason}"));
    }
    append_json_string_array_lines(
        &mut lines,
        &json,
        "verification_commands",
        "verification_command",
    );
    append_json_string_array_lines(&mut lines, &json, "safety_notes", "safety_note");
    Ok(lines)
}

fn render_controller_signing_rotation_restart_action_for_cli(
    body: &str,
) -> Result<Vec<String>, CliError> {
    let json: serde_json::Value =
        serde_json::from_str(body).map_err(|error| CliError::Http(error.to_string()))?;
    let mut lines = vec![
        format!(
            "controller_id={}\taction={}",
            json_field(&json, "controller_id"),
            json_field(&json, "action")
        ),
        format!(
            "action_status={}\trestart_required={}\treload_supported={}",
            json_field(&json, "action_status"),
            json_field(&json, "restart_required"),
            json_field(&json, "reload_supported")
        ),
        format!(
            "readiness={}\tbootstrap_guard={}",
            json_field(&json, "readiness"),
            json_field(&json, "bootstrap_guard")
        ),
        format!(
            "active_signing_fingerprint_prefix={}\tselected_signing_fingerprint_prefix={}",
            json_field(&json, "active_signing_fingerprint_prefix"),
            json_field(&json, "selected_signing_fingerprint_prefix")
        ),
        format!("service_command={}", json_field(&json, "service_command")),
    ];
    append_json_string_array_lines(
        &mut lines,
        &json,
        "verification_commands",
        "verification_command",
    );
    append_json_string_array_lines(&mut lines, &json, "safety_notes", "safety_note");
    Ok(lines)
}

fn render_controller_signing_trust_bundle_rollout_for_cli(
    body: &str,
) -> Result<Vec<String>, CliError> {
    let json: serde_json::Value =
        serde_json::from_str(body).map_err(|error| CliError::Http(error.to_string()))?;
    let mut lines = vec![
        format!(
            "controller_id={}\tstate={}",
            json_field(&json, "controller_id"),
            json_field(&json, "persisted_state")
        ),
        format!(
            "attempted_count={}\tupdated_count={}\tskipped_count={}\tfailed_count={}",
            json_field(&json, "attempted_count"),
            json_field(&json, "updated_count"),
            json_field(&json, "skipped_count"),
            json_field(&json, "failed_count")
        ),
        format!(
            "entries_count={}\tcurrent_fingerprint_prefix={}\tprevious_fingerprint_prefix={}",
            json_field(&json, "entries_count"),
            json_field(&json, "current_fingerprint_prefix"),
            json_optional_field(&json, "previous_fingerprint_prefix")
                .unwrap_or_else(|| "none".to_owned())
        ),
    ];
    if let Some(results) = json.get("agent_results").and_then(|value| value.as_array()) {
        for result in results {
            lines.push(format!(
                "agent_id={}\tstatus={}",
                json_field(result, "agent_id"),
                json_field(result, "status")
            ));
        }
    }
    Ok(lines)
}

fn render_controller_signing_trust_bundle_staged_rollout_for_cli(
    body: &str,
) -> Result<Vec<String>, CliError> {
    let json: serde_json::Value =
        serde_json::from_str(body).map_err(|error| CliError::Http(error.to_string()))?;
    let mut lines = vec![
        format!(
            "controller_id={}\tstate={}\trollout_state={}",
            json_field(&json, "controller_id"),
            json_field(&json, "persisted_state"),
            json_field(&json, "rollout_state")
        ),
        format!(
            "target_count={}\tplanned_count={}\tattempted_count={}\tupdated_count={}",
            json_field(&json, "target_count"),
            json_field(&json, "planned_count"),
            json_field(&json, "attempted_count"),
            json_field(&json, "updated_count")
        ),
        format!(
            "skipped_count={}\tfailed_count={}\talready_current_count={}\tunavailable_count={}\tpending_count={}",
            json_field(&json, "skipped_count"),
            json_field(&json, "failed_count"),
            json_field(&json, "already_current_count"),
            json_field(&json, "unavailable_count"),
            json_field(&json, "pending_count")
        ),
        format!(
            "entries_count={}\tcurrent_fingerprint_prefix={}\tprevious_fingerprint_prefix={}",
            json_field(&json, "entries_count"),
            json_field(&json, "current_fingerprint_prefix"),
            json_optional_field(&json, "previous_fingerprint_prefix")
                .unwrap_or_else(|| "none".to_owned())
        ),
    ];
    if let Some(results) = json.get("agent_results").and_then(|value| value.as_array()) {
        for result in results {
            lines.push(format!(
                "agent_id={}\tstatus={}",
                json_field(result, "agent_id"),
                json_field(result, "status")
            ));
        }
    }
    Ok(lines)
}

fn append_json_string_array_lines(
    lines: &mut Vec<String>,
    json: &serde_json::Value,
    field: &str,
    label: &str,
) {
    let Some(values) = json.get(field).and_then(|value| value.as_array()) else {
        return;
    };
    for value in values.iter().filter_map(|value| value.as_str()) {
        lines.push(format!("{label}={value}"));
    }
}

fn json_optional_number(json: &serde_json::Value, key: &str) -> Option<String> {
    json.get(key).and_then(|value| match value {
        serde_json::Value::Number(number) => Some(number.to_string()),
        _ => None,
    })
}

fn execute_controller(command: ControllerCommand) -> Result<(), CliError> {
    match command.command {
        ControllerSubcommand::Init { data_dir } => {
            fs::create_dir_all(controller_dir(&data_dir))?;
            fs::create_dir_all(agent_dir(&data_dir))?;
            let store = fleet_store::SqliteStore::open(controller_db_path(&data_dir))?;
            let controller_fingerprint = ensure_controller_identity(&data_dir)?;
            println!("controller initialized at {}", data_dir.display());
            println!("controller fingerprint: {controller_fingerprint}");
            if let Some(admin_token) = fleet_controller::create_admin_token(&store)? {
                println!("admin token: {admin_token}");
            } else {
                println!("admin token: already initialized");
            }
            Ok(())
        }
        ControllerSubcommand::Start {
            host,
            port,
            external_url,
            db,
            tls_cert,
            tls_key,
            agent_client_ca_cert,
            data_dir,
        } => {
            let database = parse_controller_database_settings(db.as_deref(), &data_dir)?;
            ensure_controller_initialized_for_start(&data_dir)?;
            fleet_controller::start_controller_server(fleet_controller::ControllerServerConfig {
                host,
                port,
                external_url,
                tls_cert_path: tls_cert,
                tls_key_path: tls_key,
                agent_client_ca_cert_path: agent_client_ca_cert,
                data_dir,
                database: Some(database),
                secret_provider: None,
            })?;
            Ok(())
        }
        ControllerSubcommand::InstallService {
            binary,
            data_dir,
            user,
            group,
            dry_run,
        } => {
            let unit = render_service_unit(
                ServiceRole::Controller,
                &resolve_service_binary(binary)?,
                &data_dir,
                user.as_deref(),
                group.as_deref(),
            )?;
            if !dry_run {
                install_systemd_service(ServiceRole::Controller, &unit)?;
                return Ok(());
            }
            print!("{unit}");
            Ok(())
        }
        ControllerSubcommand::StartService { dry_run } => {
            start_systemd_service(ServiceRole::Controller, dry_run)
        }
        ControllerSubcommand::RestartService { dry_run } => {
            restart_systemd_service(ServiceRole::Controller, dry_run)
        }
        ControllerSubcommand::StatusService { dry_run } => {
            status_systemd_service(ServiceRole::Controller, dry_run)
        }
        ControllerSubcommand::SigningRotationStatus { api, json } => {
            let client = resolve_protected_api(&api)?;
            let body = client.get("/api/controller/signing-rotation/status")?;
            if json {
                return print_json_response(&body);
            }
            for line in render_controller_signing_rotation_status_for_cli(&body)? {
                println!("{line}");
            }
            Ok(())
        }
        ControllerSubcommand::SigningRotation { command } => {
            execute_controller_signing_rotation(command)
        }
        ControllerSubcommand::LogsService { lines, dry_run } => {
            logs_systemd_service(ServiceRole::Controller, lines, dry_run)
        }
        ControllerSubcommand::UninstallService { dry_run } => {
            uninstall_systemd_service(ServiceRole::Controller, dry_run)
        }
        ControllerSubcommand::Backup { data_dir, output } => {
            execute_controller_backup(&data_dir, &output)
        }
        ControllerSubcommand::Restore {
            data_dir,
            input,
            dry_run,
            force,
        } => execute_controller_restore(&data_dir, &input, dry_run, force),
    }
}

fn execute_controller_signing_rotation(
    command: ControllerSigningRotationSubcommand,
) -> Result<(), CliError> {
    let (api, path, body, json) = match command {
        ControllerSigningRotationSubcommand::RestartPlan { api, json } => {
            let client = resolve_protected_api(&api)?;
            let response = client.get("/api/controller/signing-rotation/restart-plan")?;
            if json {
                return print_json_response(&response);
            }
            for line in render_controller_signing_rotation_restart_plan_for_cli(&response)? {
                println!("{line}");
            }
            return Ok(());
        }
        ControllerSigningRotationSubcommand::RestartAction {
            api,
            confirm_external_restart,
            reason,
            json,
        } => (
            api,
            "/api/controller/signing-rotation/restart-action",
            serde_json::json!({
                "confirm_external_restart": confirm_external_restart,
                "reason": reason
            })
            .to_string(),
            json,
        ),
        ControllerSigningRotationSubcommand::Request {
            api,
            new_fingerprint,
            old_key_verifies_for_seconds,
            old_key_verifies_until_ms,
            reason,
            json,
        } => (
            api,
            "/api/controller/signing-rotation/request",
            serde_json::json!({
                "new_fingerprint": new_fingerprint,
                "old_key_verifies_for_seconds": old_key_verifies_for_seconds,
                "old_key_verifies_until_ms": old_key_verifies_until_ms,
                "reason": reason,
            })
            .to_string(),
            json,
        ),
        ControllerSigningRotationSubcommand::Validate {
            api,
            candidate_public_key_path,
            candidate_private_key_path,
            reason,
            json,
        } => (
            api,
            "/api/controller/signing-rotation/validate",
            serde_json::json!({
                "candidate_public_key_path": candidate_public_key_path,
                "candidate_private_key_path": candidate_private_key_path,
                "reason": reason,
            })
            .to_string(),
            json,
        ),
        ControllerSigningRotationSubcommand::Activate { api, reason, json } => (
            api,
            "/api/controller/signing-rotation/activate",
            serde_json::json!({ "reason": reason }).to_string(),
            json,
        ),
        ControllerSigningRotationSubcommand::Retire { api, reason, json } => (
            api,
            "/api/controller/signing-rotation/retire",
            serde_json::json!({ "reason": reason }).to_string(),
            json,
        ),
        ControllerSigningRotationSubcommand::Fail { api, reason, json } => (
            api,
            "/api/controller/signing-rotation/fail",
            serde_json::json!({ "reason": reason }).to_string(),
            json,
        ),
        ControllerSigningRotationSubcommand::RolloutTrustBundle {
            api,
            previous_public_key_path,
            agent_ids,
            json,
        } => (
            api,
            "/api/controller/signing-rotation/rollout-trust-bundle",
            serde_json::json!({
                "previous_public_key_path": previous_public_key_path,
                "agent_ids": agent_ids
            })
            .to_string(),
            json,
        ),
        ControllerSigningRotationSubcommand::RetryTrustBundle {
            api,
            previous_public_key_path,
            agent_ids,
            max_agent_count,
            json,
        } => (
            api,
            "/api/controller/signing-rotation/rollout-trust-bundle/retry",
            serde_json::json!({
                "previous_public_key_path": previous_public_key_path,
                "agent_ids": agent_ids,
                "max_agent_count": max_agent_count
            })
            .to_string(),
            json,
        ),
        ControllerSigningRotationSubcommand::StagedTrustBundle {
            api,
            previous_public_key_path,
            agent_ids,
            batch_size,
            max_failures,
            ack_timeout_seconds,
            json,
        } => (
            api,
            CONTROLLER_SIGNING_STAGED_TRUST_BUNDLE_PATH,
            controller_signing_staged_trust_bundle_request_body(
                previous_public_key_path,
                agent_ids,
                batch_size,
                max_failures,
                ack_timeout_seconds,
            ),
            json,
        ),
    };
    let client = resolve_protected_api(&api)?;
    let response = client.post(path, Some(&body))?;
    if json {
        return print_json_response(&response);
    }
    if path == "/api/controller/signing-rotation/restart-action" {
        for line in render_controller_signing_rotation_restart_action_for_cli(&response)? {
            println!("{line}");
        }
        return Ok(());
    }
    if path == "/api/controller/signing-rotation/rollout-trust-bundle"
        || path == "/api/controller/signing-rotation/rollout-trust-bundle/retry"
    {
        for line in render_controller_signing_trust_bundle_rollout_for_cli(&response)? {
            println!("{line}");
        }
        return Ok(());
    }
    if path == CONTROLLER_SIGNING_STAGED_TRUST_BUNDLE_PATH {
        for line in render_controller_signing_trust_bundle_staged_rollout_for_cli(&response)? {
            println!("{line}");
        }
        return Ok(());
    }
    for line in render_controller_signing_rotation_status_for_cli(&response)? {
        println!("{line}");
    }
    Ok(())
}

fn controller_signing_staged_trust_bundle_request_body(
    previous_public_key_path: Option<PathBuf>,
    agent_ids: Vec<String>,
    batch_size: usize,
    max_failures: usize,
    ack_timeout_seconds: u64,
) -> String {
    serde_json::json!({
        "previous_public_key_path": previous_public_key_path,
        "agent_ids": agent_ids,
        "batch_size": batch_size,
        "max_failures": max_failures,
        "ack_timeout_seconds": ack_timeout_seconds
    })
    .to_string()
}

const CONTROLLER_BACKUP_FORMAT: &str = "fleet-controller-backup";
const CONTROLLER_BACKUP_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ControllerBackupArchive {
    format: String,
    format_version: u32,
    package_version: String,
    created_at_ms: u64,
    source_data_dir: String,
    schema_version: i64,
    sqlite_integrity_check: String,
    files: Vec<ControllerBackupFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ControllerBackupFile {
    path: String,
    size_bytes: u64,
    sha256: String,
    content_hex: String,
}

fn execute_controller_backup(data_dir: &Path, output: &Path) -> Result<(), CliError> {
    ensure_controller_initialized_for_start(data_dir)?;
    let db_path = controller_db_path(data_dir);
    if !db_path.is_file() {
        return Err(CliError::Http(format!(
            "controller database was not found: {}",
            db_path.display()
        )));
    }
    ensure_no_sqlite_transient_files(&db_path)?;
    let store = fleet_store::SqliteStore::open(&db_path)?;
    let integrity = store.integrity_check()?;
    if integrity != "ok" {
        return Err(CliError::Http(format!(
            "sqlite integrity check failed: {integrity}"
        )));
    }
    let schema_version = store.current_schema_version()?.unwrap_or(0);
    let archive = build_controller_backup_archive(data_dir, schema_version, integrity)?;
    write_backup_archive(output, &archive)?;
    println!("backup_archive={}", output.display());
    println!("format={}", archive.format);
    println!("format_version={}", archive.format_version);
    println!("schema_version={}", archive.schema_version);
    println!("file_count={}", archive.files.len());
    println!(
        "{}",
        format_warning_message(
            "backup contains sensitive controller state; store it securely and do not share it",
        )
    );
    Ok(())
}

fn execute_controller_restore(
    data_dir: &Path,
    input: &Path,
    dry_run: bool,
    force: bool,
) -> Result<(), CliError> {
    let archive = read_backup_archive(input)?;
    validate_backup_archive(&archive)?;
    if archive.schema_version > fleet_store::CURRENT_SCHEMA_VERSION {
        return Err(CliError::Http(format!(
            "backup schema version {} is newer than this binary supports ({})",
            archive.schema_version,
            fleet_store::CURRENT_SCHEMA_VERSION
        )));
    }
    let target_controller_dir = controller_dir(data_dir);
    let existing_non_empty = directory_exists_and_is_non_empty(&target_controller_dir)?;
    if dry_run {
        println!("restore_dry_run=true");
        println!("input={}", input.display());
        println!("target_data_dir={}", data_dir.display());
        println!("schema_version={}", archive.schema_version);
        println!("file_count={}", archive.files.len());
        println!("would_overwrite={existing_non_empty}");
        return Ok(());
    }
    if existing_non_empty && !force {
        return Err(CliError::Http(format!(
            "refusing to overwrite existing controller data dir {}; rerun with --force after taking a backup",
            target_controller_dir.display()
        )));
    }

    let temp_root = restore_temp_dir(data_dir);
    if temp_root.exists() {
        fs::remove_dir_all(&temp_root)?;
    }
    fs::create_dir_all(&temp_root)?;
    let restore_result = restore_archive_into_temp_dir(&archive, &temp_root)
        .and_then(|()| verify_restored_controller_dir(&temp_root))
        .and_then(|()| replace_controller_dir(data_dir, &temp_root, force));
    if restore_result.is_err() {
        let _ = fs::remove_dir_all(&temp_root);
    }
    restore_result?;
    println!("restore_dry_run=false");
    println!("restored_data_dir={}", data_dir.display());
    println!("schema_version={}", archive.schema_version);
    println!("file_count={}", archive.files.len());
    println!(
        "{}",
        format_warning_message(
            "restored backup contains controller secrets; verify filesystem permissions before production use",
        )
    );
    Ok(())
}

fn build_controller_backup_archive(
    data_dir: &Path,
    schema_version: i64,
    sqlite_integrity_check: String,
) -> Result<ControllerBackupArchive, CliError> {
    let files = collect_controller_backup_files(data_dir)?;
    Ok(ControllerBackupArchive {
        format: CONTROLLER_BACKUP_FORMAT.to_owned(),
        format_version: CONTROLLER_BACKUP_FORMAT_VERSION,
        package_version: env!("CARGO_PKG_VERSION").to_owned(),
        created_at_ms: epoch_millis() as u64,
        source_data_dir: data_dir.display().to_string(),
        schema_version,
        sqlite_integrity_check,
        files,
    })
}

fn collect_controller_backup_files(data_dir: &Path) -> Result<Vec<ControllerBackupFile>, CliError> {
    let mut files = Vec::new();
    collect_controller_backup_files_from_dir(data_dir, &controller_dir(data_dir), &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn collect_controller_backup_files_from_dir(
    data_dir: &Path,
    directory: &Path,
    files: &mut Vec<ControllerBackupFile>,
) -> Result<(), CliError> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            collect_controller_backup_files_from_dir(data_dir, &path, files)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(CliError::Http(format!(
                "controller backup only supports regular files: {}",
                path.display()
            )));
        }
        let relative_path = backup_relative_path(data_dir, &path)?;
        let content = fs::read(&path)?;
        let sha256 = sha256_hex(&content);
        files.push(ControllerBackupFile {
            path: relative_path,
            size_bytes: content.len() as u64,
            sha256,
            content_hex: hex_encode(&content),
        });
    }
    Ok(())
}

fn backup_relative_path(data_dir: &Path, path: &Path) -> Result<String, CliError> {
    let relative = path.strip_prefix(data_dir).map_err(|_| {
        CliError::Http(format!(
            "backup path is outside data dir: {}",
            path.display()
        ))
    })?;
    let parts = relative
        .components()
        .map(|component| match component {
            std::path::Component::Normal(value) => Ok(value.to_string_lossy().to_string()),
            _ => Err(CliError::Http(format!(
                "invalid backup path component: {}",
                path.display()
            ))),
        })
        .collect::<Result<Vec<_>, CliError>>()?;
    Ok(parts.join("/"))
}

fn write_backup_archive(output: &Path, archive: &ControllerBackupArchive) -> Result<(), CliError> {
    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let temp_output = output.with_extension(format!(
        "{}.tmp",
        output
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("backup")
    ));
    let body =
        serde_json::to_vec_pretty(archive).map_err(|error| CliError::Http(error.to_string()))?;
    fs::write(&temp_output, body)?;
    fs::rename(&temp_output, output)?;
    Ok(())
}

fn read_backup_archive(input: &Path) -> Result<ControllerBackupArchive, CliError> {
    let body = fs::read(input)?;
    serde_json::from_slice(&body).map_err(|error| CliError::Http(error.to_string()))
}

fn validate_backup_archive(archive: &ControllerBackupArchive) -> Result<(), CliError> {
    if archive.format != CONTROLLER_BACKUP_FORMAT {
        return Err(CliError::Http(format!(
            "unsupported backup format: {}",
            archive.format
        )));
    }
    if archive.format_version != CONTROLLER_BACKUP_FORMAT_VERSION {
        return Err(CliError::Http(format!(
            "unsupported backup format version: {}",
            archive.format_version
        )));
    }
    if archive.files.is_empty() {
        return Err(CliError::Http(
            "backup archive contains no files".to_owned(),
        ));
    }
    for file in &archive.files {
        validate_backup_file_path(&file.path)?;
        let content = hex_decode(&file.content_hex)?;
        if content.len() as u64 != file.size_bytes {
            return Err(CliError::Http(format!(
                "backup file size mismatch: {}",
                file.path
            )));
        }
        let actual = sha256_hex(&content);
        if actual != file.sha256 {
            return Err(CliError::Http(format!(
                "backup checksum mismatch for {}",
                file.path
            )));
        }
    }
    if !archive
        .files
        .iter()
        .any(|file| file.path == "controller/fleet.db")
    {
        return Err(CliError::Http(
            "backup archive does not contain controller/fleet.db".to_owned(),
        ));
    }
    Ok(())
}

fn validate_backup_file_path(path: &str) -> Result<(), CliError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || !path.starts_with("controller/")
    {
        return Err(CliError::Http(format!("invalid backup file path: {path}")));
    }
    Ok(())
}

fn restore_archive_into_temp_dir(
    archive: &ControllerBackupArchive,
    temp_root: &Path,
) -> Result<(), CliError> {
    for file in &archive.files {
        let path = temp_root.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, hex_decode(&file.content_hex)?)?;
        set_restored_file_permissions(&path)?;
    }
    Ok(())
}

fn set_restored_file_permissions(path: &Path) -> Result<(), CliError> {
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn verify_restored_controller_dir(temp_root: &Path) -> Result<(), CliError> {
    let db_path = temp_root.join("controller").join("fleet.db");
    let store = fleet_store::SqliteStore::open(&db_path)?;
    let integrity = store.integrity_check()?;
    if integrity != "ok" {
        return Err(CliError::Http(format!(
            "restored sqlite integrity check failed: {integrity}"
        )));
    }
    Ok(())
}

fn replace_controller_dir(data_dir: &Path, temp_root: &Path, force: bool) -> Result<(), CliError> {
    let target_controller_dir = controller_dir(data_dir);
    let temp_controller_dir = temp_root.join("controller");
    fs::create_dir_all(data_dir)?;
    if target_controller_dir.exists() {
        if directory_exists_and_is_non_empty(&target_controller_dir)? && !force {
            return Err(CliError::Http(format!(
                "refusing to overwrite existing controller data dir {}",
                target_controller_dir.display()
            )));
        }
        let old_controller_dir =
            data_dir.join(format!(".controller-restore-old-{}", epoch_millis()));
        if old_controller_dir.exists() {
            fs::remove_dir_all(&old_controller_dir)?;
        }
        fs::rename(&target_controller_dir, &old_controller_dir)?;
        match fs::rename(&temp_controller_dir, &target_controller_dir) {
            Ok(()) => {
                fs::remove_dir_all(old_controller_dir)?;
            }
            Err(error) => {
                let _ = fs::rename(&old_controller_dir, &target_controller_dir);
                return Err(CliError::Io(error));
            }
        }
    } else {
        fs::rename(&temp_controller_dir, &target_controller_dir)?;
    }
    fs::remove_dir_all(temp_root)?;
    Ok(())
}

fn directory_exists_and_is_non_empty(path: &Path) -> Result<bool, CliError> {
    if !path.exists() {
        return Ok(false);
    }
    Ok(fs::read_dir(path)?.next().transpose()?.is_some())
}

fn restore_temp_dir(data_dir: &Path) -> PathBuf {
    let parent = data_dir.parent().unwrap_or_else(|| Path::new("."));
    let name = data_dir
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("fleet");
    parent.join(format!(".{name}-restore-tmp-{}", epoch_millis()))
}

fn ensure_no_sqlite_transient_files(db_path: &Path) -> Result<(), CliError> {
    for suffix in ["-wal", "-shm"] {
        let path = PathBuf::from(format!("{}{suffix}", db_path.display()));
        if path.exists() {
            return Err(CliError::Http(format!(
                "refusing to backup SQLite database with transient file present: {}; stop the controller and retry",
                path.display()
            )));
        }
    }
    Ok(())
}

fn sha256_hex(content: &[u8]) -> String {
    hex_encode(&Sha256::digest(content))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn hex_decode(value: &str) -> Result<Vec<u8>, CliError> {
    if !value.len().is_multiple_of(2) {
        return Err(CliError::Http(
            "hex content must have even length".to_owned(),
        ));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text =
                std::str::from_utf8(pair).map_err(|error| CliError::Http(error.to_string()))?;
            u8::from_str_radix(text, 16)
                .map_err(|_| CliError::Http(format!("invalid hex byte in backup content: {text}")))
        })
        .collect()
}

fn execute_enroll_token(command: EnrollTokenCommand) -> Result<(), CliError> {
    match command.command {
        EnrollTokenSubcommand::Create {
            labels,
            max_uses,
            expires_in_seconds,
            controller_url,
            name,
            print_init_command,
            data_dir,
        } => {
            fs::create_dir_all(controller_dir(&data_dir))?;
            let store = fleet_store::SqliteStore::open(controller_db_path(&data_dir))?;
            let token = prefixed_ulid("enroll")?;
            let token_id = prefixed_ulid("et")?;
            store.insert_enrollment_token_hash(
                &token_id,
                &fleet_controller::hash_token(&token),
                &labels,
                SystemTime::now() + Duration::from_secs(expires_in_seconds),
                max_uses,
            )?;
            store.write_audit_event(fleet_domain::AuditEvent {
                category: fleet_domain::AuditCategory::Security,
                action: "enrollment_token_created".to_owned(),
                actor: fleet_domain::AuditActor::new("cli"),
                target: fleet_domain::AuditTarget::new(&token_id),
                value: fleet_domain::AuditValue::SecretRef(format!(
                    "labels={},max_uses={},expires_in_seconds={}",
                    labels, max_uses, expires_in_seconds
                )),
                occurred_at: SystemTime::now(),
            })?;
            if print_init_command {
                let controller_url = controller_url.ok_or_else(|| {
                    CliError::Http(
                        "--controller-url is required with --print-init-command".to_owned(),
                    )
                })?;
                let name = name.unwrap_or_else(|| "<agent-name>".to_owned());
                println!(
                    "fleet agent init --url {} --token {} --name {} --labels {}",
                    shell_arg(&controller_url),
                    shell_arg(&token),
                    shell_arg(&name),
                    shell_arg(&labels)
                );
            } else {
                println!("{token}");
            }
            Ok(())
        }
        EnrollTokenSubcommand::List { data_dir } => {
            let store = fleet_store::SqliteStore::open(controller_db_path(&data_dir))?;
            let records = store.list_enrollment_tokens()?;
            if records.is_empty() {
                println!("no enrollment tokens");
                return Ok(());
            }
            println!("id\tlabels\tmax_uses\tused_count\tremaining_uses\trevoked\texpires_at_epoch");
            for record in records {
                let remaining_uses = record.max_uses.saturating_sub(record.used_count);
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    record.id,
                    record.default_labels,
                    record.max_uses,
                    record.used_count,
                    remaining_uses,
                    record.revoked,
                    epoch_seconds(record.expires_at)
                );
            }
            Ok(())
        }
        EnrollTokenSubcommand::Revoke { id, data_dir } => {
            let store = fleet_store::SqliteStore::open(controller_db_path(&data_dir))?;
            if store.revoke_enrollment_token(&id)? {
                store.write_audit_event(fleet_domain::AuditEvent {
                    category: fleet_domain::AuditCategory::Security,
                    action: "enrollment_token_revoked".to_owned(),
                    actor: fleet_domain::AuditActor::new("cli"),
                    target: fleet_domain::AuditTarget::new(&id),
                    value: fleet_domain::AuditValue::SecretRef("revoked".to_owned()),
                    occurred_at: SystemTime::now(),
                })?;
                println!("revoked enrollment token: {id}");
            } else {
                println!("enrollment token not found: {id}");
            }
            Ok(())
        }
    }
}

fn execute_agent(command: AgentCommand) -> Result<(), CliError> {
    match command.command {
        AgentSubcommand::Init {
            url,
            token,
            name,
            labels,
            tls_ca_cert,
            data_dir,
        } => {
            warn_if_insecure_http_url(&url);
            fs::create_dir_all(agent_dir(&data_dir))?;
            let agent_id = format!("agent-{name}");
            let key_pair = fleet_core::generate_agent_key_pair()?;
            let tls_ca_cert = tls_ca_cert
                .as_deref()
                .map(canonicalize_tls_ca_cert)
                .transpose()?;
            let response = enroll_agent_via_controller(
                &url,
                tls_ca_cert.as_deref(),
                &fleet_controller::EnrollAgentRequest {
                    token: token.clone(),
                    agent_id,
                    name: name.clone(),
                    public_key: key_pair.public_key_hex.clone(),
                    fingerprint: key_pair.fingerprint.clone(),
                    labels: parse_labels(&labels)?,
                },
            )?;
            let tls_ca_cert_line = tls_ca_cert
                .as_ref()
                .map(|path| format!("tls_ca_cert={}\n", path.display()))
                .unwrap_or_default();
            let config = format!(
                "url={}\n{}agent_id={}\nname={}\nlabels={}\nfingerprint={}\ncontroller_fingerprint={}\n",
                url,
                tls_ca_cert_line,
                response.agent_id,
                name,
                labels,
                key_pair.fingerprint,
                response.controller_fingerprint
            );
            write_secure_file(&agent_dir(&data_dir).join("agent.conf"), &config)?;
            write_secure_file(
                &agent_dir(&data_dir).join("agent_private.key"),
                &format!("{}\n", key_pair.private_key_hex),
            )?;
            append_line(
                &agent_dir(&data_dir).join("agents.tsv"),
                &format!("{name}\t{labels}\tPending\n"),
            )?;
            println!(
                "agent enrolled: {}",
                redact_secret(&format!("name={name} token={token}"))
            );
            println!(
                "controller fingerprint: {}",
                response.controller_fingerprint
            );
            Ok(())
        }
        AgentSubcommand::Start {
            data_dir,
            once,
            heartbeat_interval_seconds,
            facts_interval_seconds,
            metrics_interval_seconds,
            disable_log_upload,
            log_upload_interval_seconds,
            max_reconnect_attempts,
        } => {
            let path = agent_dir(&data_dir).join("agent.conf");
            if !path.exists() {
                return Err(CliError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "agent is not enrolled",
                )));
            }
            if log_upload_interval_seconds == 0 {
                return Err(CliError::Http(
                    "log upload interval must be at least 1 second; use --disable-log-upload to turn it off"
                        .to_owned(),
                ));
            }
            if heartbeat_interval_seconds == 0 {
                return Err(CliError::Http(
                    "heartbeat interval must be at least 1 second".to_owned(),
                ));
            }
            if facts_interval_seconds == 0 {
                return Err(CliError::Http(
                    "facts interval must be at least 1 second".to_owned(),
                ));
            }
            if metrics_interval_seconds == 0 {
                return Err(CliError::Http(
                    "metrics interval must be at least 1 second".to_owned(),
                ));
            }
            let config = read_agent_config(&path)?;
            warn_if_insecure_http_url(&config.url);
            run_agent_session_loop(
                &config,
                AgentHeartbeatOptions {
                    once,
                    heartbeat_interval: Duration::from_secs(heartbeat_interval_seconds),
                    facts_interval: Duration::from_secs(facts_interval_seconds),
                    metrics_interval: Duration::from_secs(metrics_interval_seconds),
                    log_upload: AgentLogUploadOptions {
                        enabled: !disable_log_upload,
                        interval: Duration::from_secs(log_upload_interval_seconds),
                    },
                    max_reconnect_attempts,
                },
            )?;
            Ok(())
        }
        AgentSubcommand::InstallService {
            binary,
            data_dir,
            user,
            group,
            dry_run,
        } => {
            let unit = render_service_unit(
                ServiceRole::Agent,
                &resolve_service_binary(binary)?,
                &data_dir,
                user.as_deref(),
                group.as_deref(),
            )?;
            if !dry_run {
                install_systemd_service(ServiceRole::Agent, &unit)?;
                return Ok(());
            }
            print!("{unit}");
            Ok(())
        }
        AgentSubcommand::StartService { dry_run } => {
            start_systemd_service(ServiceRole::Agent, dry_run)
        }
        AgentSubcommand::StatusService { dry_run } => {
            status_systemd_service(ServiceRole::Agent, dry_run)
        }
        AgentSubcommand::LogsService { lines, dry_run } => {
            logs_systemd_service(ServiceRole::Agent, lines, dry_run)
        }
        AgentSubcommand::UninstallService { dry_run } => {
            uninstall_systemd_service(ServiceRole::Agent, dry_run)
        }
    }
}

fn execute_agents(command: AgentsCommand) -> Result<(), CliError> {
    match command.command {
        AgentsSubcommand::List { data_dir } => {
            let db_path = controller_db_path(&data_dir);
            if db_path.exists() {
                let store = fleet_store::SqliteStore::open(db_path)?;
                let agents = store.list_agents()?;
                if agents.is_empty() {
                    println!("no agents");
                    return Ok(());
                }
                for agent in agents {
                    let labels = agent
                        .labels()
                        .iter()
                        .map(|label| format!("{}={}", label.key(), label.value()))
                        .collect::<Vec<_>>()
                        .join(",");
                    println!(
                        "{}\t{}\t{:?}",
                        agent.name().as_str(),
                        labels,
                        agent.status()
                    );
                }
                return Ok(());
            }

            let path = agent_dir(&data_dir).join("agents.tsv");
            if !path.exists() {
                println!("no agents");
                return Ok(());
            }
            let mut body = String::new();
            fs::File::open(path)?.read_to_string(&mut body)?;
            print!("{body}");
            Ok(())
        }
        AgentsSubcommand::RemoteList { api } => {
            let client = resolve_protected_api(&api)?;
            let body = client.get("/api/agents")?;
            print_json_response(&body)
        }
        AgentsSubcommand::RemoteGet { agent_id, api } => {
            let client = resolve_protected_api(&api)?;
            let body = client.get(&format!("/api/agents/{agent_id}"))?;
            print_json_response(&body)
        }
        AgentsSubcommand::RequestCertificateIssuance {
            agent_id,
            api,
            json,
        } => {
            let client = resolve_protected_api(&api)?;
            let response = client.post(
                &format!("/api/agents/{agent_id}/certificate-lifecycle/request-issuance"),
                Some("{}"),
            )?;
            if json {
                return print_json_response(&response);
            }
            for line in render_agent_certificate_issuance_request_for_cli(&response)? {
                println!("{line}");
            }
            Ok(())
        }
        AgentsSubcommand::CertificateStatus {
            agent_id,
            api,
            json,
        } => {
            let client = resolve_protected_api(&api)?;
            let response = client.get(&format!(
                "/api/agents/{agent_id}/certificate-lifecycle/status"
            ))?;
            if json {
                return print_json_response(&response);
            }
            for line in render_agent_certificate_lifecycle_status_for_cli(&response)? {
                println!("{line}");
            }
            Ok(())
        }
    }
}

fn execute_jobs(command: JobsCommand) -> Result<(), CliError> {
    match command.command {
        JobsSubcommand::List { api } => {
            let client = resolve_protected_api(&api)?;
            let body = client.get("/api/jobs")?;
            print_json_response(&body)
        }
        JobsSubcommand::Get { job_id, api } => {
            let client = resolve_protected_api(&api)?;
            let body = client.get(&format!("/api/jobs/{job_id}"))?;
            print_json_response(&body)
        }
        JobsSubcommand::Output { job_id, api } => {
            let client = resolve_protected_api(&api)?;
            let body = client.get(&format!("/api/jobs/{job_id}/output"))?;
            for line in render_job_output_api_for_cli(&body)? {
                println!("{line}");
            }
            Ok(())
        }
        JobsSubcommand::Cancel { job_id, api } => {
            let client = resolve_protected_api(&api)?;
            let body = serde_json::json!({ "reason": "canceled from CLI" }).to_string();
            let response = client.post(&format!("/api/jobs/{job_id}/cancel"), Some(&body))?;
            print_json_response(&response)
        }
    }
}

fn execute_approvals(command: ApprovalsCommand) -> Result<(), CliError> {
    match command.command {
        ApprovalsSubcommand::List { api } => {
            let client = resolve_protected_api(&api)?;
            let body = client.get("/api/approvals?status=pending")?;
            print_json_response(&body)
        }
        ApprovalsSubcommand::Approve {
            approval_id,
            reason,
            api,
        } => {
            let client = resolve_protected_api(&api)?;
            let body = serde_json::json!({ "reason": reason }).to_string();
            let response = client.post(
                &format!("/api/approvals/{approval_id}/approve"),
                Some(&body),
            )?;
            print_json_response(&response)
        }
        ApprovalsSubcommand::Reject {
            approval_id,
            reason,
            api,
        } => {
            let client = resolve_protected_api(&api)?;
            let body = serde_json::json!({ "reason": reason }).to_string();
            let response =
                client.post(&format!("/api/approvals/{approval_id}/reject"), Some(&body))?;
            print_json_response(&response)
        }
    }
}

fn execute_remediations(command: RemediationsCommand) -> Result<(), CliError> {
    match command.command {
        RemediationsSubcommand::List {
            agent_id,
            policy_id,
            limit,
            api,
        } => {
            let client = resolve_protected_api(&api)?;
            let body = client.get(&remediations_list_path(
                agent_id.as_deref(),
                policy_id.as_deref(),
                limit,
            ))?;
            print_remediation_response(&body)
        }
        RemediationsSubcommand::Get {
            remediation_id,
            api,
        } => {
            let client = resolve_protected_api(&api)?;
            let body = client.get(&format!(
                "/api/remediations/{}",
                query_encode(&remediation_id)
            ))?;
            print_remediation_response(&body)
        }
        RemediationsSubcommand::RequestApproval {
            remediation_id,
            approval_id,
            job_id,
            reason,
            expires_in_seconds,
            api,
        } => {
            let client = resolve_protected_api(&api)?;
            let body = serde_json::json!({
                "approval_id": approval_id,
                "job_id": job_id,
                "reason": reason,
                "expires_in_seconds": expires_in_seconds
            })
            .to_string();
            let response = client.post(
                &format!(
                    "/api/remediations/{}/approval-request",
                    query_encode(&remediation_id)
                ),
                Some(&body),
            )?;
            print_remediation_response(&response)
        }
        RemediationsSubcommand::Approve {
            remediation_id,
            approval_id,
            job_id,
            runbook,
            timeout_seconds,
            expires_in_seconds,
            nonce_prefix,
            reason,
            api,
        } => {
            let client = resolve_protected_api(&api)?;
            let runbook_document = fs::read_to_string(&runbook)?;
            let body = build_remediation_approve_body(
                &approval_id,
                &job_id,
                &runbook_document,
                timeout_seconds,
                expires_in_seconds,
                nonce_prefix.as_deref(),
                &reason,
            );
            let response = client.post(
                &format!(
                    "/api/remediations/{}/approve",
                    query_encode(&remediation_id)
                ),
                Some(&body),
            )?;
            print_remediation_response(&response)
        }
        RemediationsSubcommand::Running {
            remediation_id,
            job_id,
            api,
        } => {
            warn_deprecated_manual_remediation_lifecycle("running");
            let client = resolve_protected_api(&api)?;
            let body = serde_json::json!({ "job_id": job_id }).to_string();
            let response = client.post(
                &format!(
                    "/api/remediations/{}/running",
                    query_encode(&remediation_id)
                ),
                Some(&body),
            )?;
            print_remediation_response(&response)
        }
        RemediationsSubcommand::Result {
            remediation_id,
            job_id,
            status,
            api,
        } => {
            warn_deprecated_manual_remediation_lifecycle("result");
            let client = resolve_protected_api(&api)?;
            let body = serde_json::json!({ "job_id": job_id, "status": status }).to_string();
            let response = client.post(
                &format!("/api/remediations/{}/result", query_encode(&remediation_id)),
                Some(&body),
            )?;
            print_remediation_response(&response)
        }
        RemediationsSubcommand::Verify {
            remediation_id,
            agent_id,
            policy_id,
            policy_name,
            job_id,
            api,
        } => {
            warn_deprecated_manual_remediation_lifecycle("verify");
            let client = resolve_protected_api(&api)?;
            let body = serde_json::json!({
                "agent_id": agent_id,
                "policy_id": policy_id,
                "policy_name": policy_name,
                "job_id": job_id
            })
            .to_string();
            let response = client.post(
                &format!("/api/remediations/{}/verify", query_encode(&remediation_id)),
                Some(&body),
            )?;
            print_remediation_response(&response)
        }
    }
}

fn execute_selectors(command: SelectorsCommand) -> Result<(), CliError> {
    match command.command {
        SelectorsSubcommand::Preview { selector, api } => {
            let client = resolve_protected_api(&api)?;
            let body = serde_json::json!({ "selector": selector }).to_string();
            let response = client.post("/api/selectors/preview", Some(&body))?;
            for line in render_selector_preview_for_cli(&response)? {
                println!("{line}");
            }
            Ok(())
        }
    }
}

fn execute_audit(command: AuditCommand) -> Result<(), CliError> {
    match command.command {
        AuditSubcommand::Export {
            category,
            limit,
            before,
            api,
        } => {
            if limit == 0 {
                return Err(CliError::Http(
                    "audit export limit must be greater than zero".to_owned(),
                ));
            }
            let client = resolve_protected_api(&api)?;
            let path = audit_export_path(category.as_deref(), limit, before.as_deref());
            let body = client.get(&path)?;
            for line in render_audit_export_jsonl(&body)? {
                println!("{line}");
            }
            Ok(())
        }
    }
}

fn execute_run(command: RunCommand) -> Result<(), CliError> {
    if !command.confirm_risk {
        return Err(CliError::HighRiskConfirmationRequired);
    }
    let Some((program, args)) = command.command.split_first() else {
        return Err(CliError::EmptyCommand);
    };
    if command.remote || command.controller_url.is_some() || command.admin_token.is_some() {
        return execute_remote_run(&command, program, args);
    }
    let output = fleet_runner::run_command(program, args)?;
    let context = run_context_label(command.selector.as_deref());
    if let Some(selector) = context.strip_prefix("selector:") {
        println!("selector: {selector}");
    }
    let (stdout, stderr) = render_command_output(&output);
    print!("{stdout}");
    eprint!("{stderr}");
    println!("exit_code={}", output.exit_code);
    Ok(())
}

fn execute_remote_run(
    command: &RunCommand,
    program: &str,
    args: &[String],
) -> Result<(), CliError> {
    let client = resolve_protected_api(&run_command_api_args(command))?;
    if command.selector.is_none() {
        return Err(CliError::Http(
            "remote run requires --selector in MVP".to_owned(),
        ));
    }
    let job_id = command.job_id.clone().unwrap_or(prefixed_ulid("job-cli")?);
    let body = remote_run_request_body(command, &job_id, program, args)?;
    let response_body = client.post("/api/jobs/command", Some(&body))?;
    let response_json: serde_json::Value =
        serde_json::from_str(&response_body).map_err(|error| CliError::Http(error.to_string()))?;
    println!(
        "job_id={}",
        response_json
            .get("job_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&job_id)
    );
    println!(
        "target_count={}",
        response_json
            .get("target_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    );
    let status = remote_run_response_status(&response_json);
    println!("status={status}");
    if remote_run_response_needs_approval(&response_json) {
        if let Some(approval_request_id) = remote_run_response_approval_request_id(&response_json) {
            println!("approval_request_id={approval_request_id}");
        }
        println!("output_state=waiting_for_approval");
        return Ok(());
    }

    let output_path = format!("/api/jobs/{job_id}/output");
    let output = client.get(&output_path)?;
    for line in render_job_output_api_for_cli(&output)? {
        println!("{line}");
    }
    Ok(())
}

fn remote_run_response_status(response_json: &serde_json::Value) -> &str {
    response_json
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("queued")
}

fn remote_run_response_approval_request_id(response_json: &serde_json::Value) -> Option<&str> {
    response_json
        .get("approval_request_id")
        .and_then(serde_json::Value::as_str)
}

fn remote_run_response_needs_approval(response_json: &serde_json::Value) -> bool {
    remote_run_response_status(response_json) == "pending_approval"
        || remote_run_response_approval_request_id(response_json).is_some()
}

fn remote_run_request_body(
    command: &RunCommand,
    job_id: &str,
    program: &str,
    args: &[String],
) -> Result<String, CliError> {
    serde_json::to_string(&serde_json::json!({
        "job_id": job_id,
        "target_agent_ids": [],
        "selector": command.selector,
        "program": program,
        "args": args,
        "timeout_seconds": command.timeout_seconds,
        "confirmed_high_risk": true,
        "confirmed_by": "cli-admin-token",
        "expires_in_seconds": REMOTE_RUN_EXPIRES_IN_SECONDS,
        "nonce_prefix": job_id
    }))
    .map_err(|error| CliError::Http(error.to_string()))
}

fn render_command_output(output: &fleet_runner::CommandOutput) -> (String, String) {
    (redact_secret(&output.stdout), redact_secret(&output.stderr))
}

fn run_context_label(selector: Option<&str>) -> String {
    selector
        .map(|selector| format!("selector:{selector}"))
        .unwrap_or_else(|| "local".to_owned())
}

fn render_job_output_api_for_cli(body: &str) -> Result<Vec<String>, CliError> {
    let chunks: serde_json::Value =
        serde_json::from_str(body).map_err(|error| CliError::Http(error.to_string()))?;
    let Some(chunks) = chunks.as_array() else {
        return Err(CliError::Http(
            "job output response must be an array".to_owned(),
        ));
    };
    Ok(chunks
        .iter()
        .map(|chunk| {
            let agent_id = chunk
                .get("agent_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown-agent");
            let stream = chunk
                .get("stream")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("stdout");
            let sequence = chunk
                .get("sequence")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let data = chunk
                .get("data")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            format!("[{agent_id} {stream} #{sequence}] {}", redact_secret(data))
        })
        .collect())
}

fn render_selector_preview_for_cli(body: &str) -> Result<Vec<String>, CliError> {
    let preview: serde_json::Value =
        serde_json::from_str(body).map_err(|error| CliError::Http(error.to_string()))?;
    let mut lines = vec![
        format!(
            "matched_count={}",
            preview
                .get("matched_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0)
        ),
        format!(
            "selected_count={}",
            preview
                .get("selected_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0)
        ),
        format!(
            "disabled_count={}",
            preview
                .get("disabled_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0)
        ),
        format!(
            "offline_count={}",
            preview
                .get("offline_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0)
        ),
    ];
    if let Some(warnings) = preview
        .get("warnings")
        .and_then(serde_json::Value::as_array)
    {
        for warning in warnings {
            let code = warning
                .get("code")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("warning");
            let message = warning
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            lines.push(format!("warning={code}:{message}"));
        }
    }
    if let Some(agents) = preview.get("agents").and_then(serde_json::Value::as_array) {
        for agent in agents {
            let agent_id = agent
                .get("agent_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let name = agent
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let status = agent
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let selected = agent
                .get("selected_for_dispatch")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            lines.push(format!(
                "agent={agent_id}\tname={name}\tstatus={status}\tselected={selected}"
            ));
        }
    }
    Ok(lines)
}

fn audit_export_path(category: Option<&str>, limit: usize, before: Option<&str>) -> String {
    let mut query = Vec::new();
    if let Some(category) = category.filter(|value| !value.is_empty()) {
        query.push(format!("category={}", query_encode(category)));
    }
    query.push(format!("limit={limit}"));
    if let Some(before) = before.filter(|value| !value.is_empty()) {
        query.push(format!("before={}", query_encode(before)));
    }
    format!("/api/audit/export?{}", query.join("&"))
}

fn remediations_list_path(agent_id: Option<&str>, policy_id: Option<&str>, limit: usize) -> String {
    let mut query = Vec::new();
    if let Some(agent_id) = agent_id.filter(|value| !value.is_empty()) {
        query.push(format!("agent_id={}", query_encode(agent_id)));
    }
    if let Some(policy_id) = policy_id.filter(|value| !value.is_empty()) {
        query.push(format!("policy_id={}", query_encode(policy_id)));
    }
    query.push(format!("limit={}", limit.max(1)));
    format!("/api/remediations?{}", query.join("&"))
}

fn build_remediation_approve_body(
    approval_id: &str,
    job_id: &str,
    runbook_document: &str,
    timeout_seconds: u64,
    expires_in_seconds: u64,
    nonce_prefix: Option<&str>,
    reason: &str,
) -> String {
    serde_json::json!({
        "approval_id": approval_id,
        "job_id": job_id,
        "runbook_document": runbook_document,
        "timeout_seconds": timeout_seconds,
        "expires_in_seconds": expires_in_seconds,
        "nonce_prefix": nonce_prefix,
        "reason": reason
    })
    .to_string()
}

fn render_audit_export_jsonl(body: &str) -> Result<Vec<String>, CliError> {
    let page: serde_json::Value =
        serde_json::from_str(body).map_err(|error| CliError::Http(error.to_string()))?;
    let Some(items) = page.get("items").and_then(serde_json::Value::as_array) else {
        return Err(CliError::Http(
            "audit export response must contain an items array".to_owned(),
        ));
    };
    items
        .iter()
        .map(|item| serde_json::to_string(item).map_err(|error| CliError::Http(error.to_string())))
        .collect()
}

fn print_remediation_response(body: &str) -> Result<(), CliError> {
    for line in render_remediation_api_for_cli(body)? {
        println!("{line}");
    }
    Ok(())
}

/// Warns before retaining a compatibility call that the Controller will reject.
fn warn_deprecated_manual_remediation_lifecycle(command: &str) {
    eprintln!(
        "{}",
        format_warning_message(format!(
            "remediations {command} is deprecated and will return 409; wait for authenticated agent task events and persisted verification evidence"
        ))
    );
}

fn render_remediation_api_for_cli(body: &str) -> Result<Vec<String>, CliError> {
    let json: serde_json::Value =
        serde_json::from_str(body).map_err(|error| CliError::Http(error.to_string()))?;
    if let Some(items) = json.as_array() {
        return Ok(items.iter().map(remediation_summary_line).collect());
    }
    if let Some(remediation) = json.get("remediation") {
        let mut lines = vec![remediation_summary_line(remediation)];
        if let Some(approval) = json.get("approval") {
            lines.push(format!(
                "approval_id={}\tapproval_status={}\tapproval_job_id={}",
                json_field(approval, "id"),
                json_field(approval, "status"),
                json_field(approval, "job_id")
            ));
        }
        if json.get("assignment_count").is_some() || json.get("status").is_some() {
            lines.push(format!(
                "job_id={}\tassignment_count={}\tstatus={}",
                json_field(&json, "job_id"),
                json_field(&json, "assignment_count"),
                json_field(&json, "status")
            ));
        }
        return Ok(lines);
    }
    Ok(vec![remediation_summary_line(&json)])
}

fn remediation_summary_line(value: &serde_json::Value) -> String {
    format!(
        "remediation_id={}\tpolicy_id={}\tagent_id={}\tstatus={}\trunbook_ref={}\tjob_id={}\tlifecycle_source={}\tverification_job_id={}\tverification_assignment_status={}\tverification_evidence_status={}\tlegacy_state={}",
        json_field(value, "id"),
        json_field(value, "policy_id"),
        json_field(value, "agent_id"),
        json_field(value, "status"),
        json_field(value, "runbook_ref"),
        json_field(value, "job_id"),
        json_field(value, "lifecycle_source"),
        json_field(value, "verification_job_id"),
        json_field(value, "verification_assignment_status"),
        json_field(value, "verification_evidence_status"),
        json_field(value, "legacy_state")
    )
}

fn json_field(value: &serde_json::Value, key: &str) -> String {
    match value.get(key) {
        Some(serde_json::Value::String(value)) => value.clone(),
        Some(serde_json::Value::Number(value)) => value.to_string(),
        Some(serde_json::Value::Bool(value)) => value.to_string(),
        Some(serde_json::Value::Null) | None => "".to_owned(),
        Some(other) => other.to_string(),
    }
}

fn json_optional_field(value: &serde_json::Value, key: &str) -> Option<String> {
    match value.get(key)? {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        serde_json::Value::Bool(value) => Some(value.to_string()),
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    }
}

fn query_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn execute_facts(command: FactsCommand) -> Result<(), CliError> {
    let mut facts = collect_local_facts();
    if let Some(object) = facts.as_object_mut() {
        object.insert(
            "agent".to_owned(),
            serde_json::Value::String(command.agent.unwrap_or_else(|| "local".to_owned())),
        );
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&facts).map_err(|error| CliError::Http(error.to_string()))?
    );
    Ok(())
}

fn execute_metrics(command: MetricsCommand) -> Result<(), CliError> {
    let mut metrics = collect_local_metrics();
    if let Some(object) = metrics.as_object_mut() {
        object.insert(
            "agent".to_owned(),
            serde_json::Value::String(command.agent.unwrap_or_else(|| "local".to_owned())),
        );
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&metrics)
            .map_err(|error| CliError::Http(error.to_string()))?
    );
    Ok(())
}

fn execute_apply(command: ApplyCommand) -> Result<(), CliError> {
    let body = fs::read_to_string(command.file)?;
    let runbook = fleet_domain::parse_runbook_document(&body)
        .map_err(|error| CliError::Http(format!("invalid runbook: {error}")))?;
    let plan = fleet_runner::build_runbook_execution_plan(
        &runbook,
        fleet_runner::LinuxPackageManager::Apt,
    )
    .map_err(|error| CliError::Http(format!("invalid runbook primitive: {error}")))?;
    println!("runbook valid: {}", runbook.name);
    println!("task_count={}", runbook.tasks.len());
    println!("execution_plan_steps={}", plan.steps.len());
    println!("execution=not_started");
    println!("note=runbook validation only; execution must use signed task dispatch");
    Ok(())
}

fn execute_retention(command: RetentionCommand) -> Result<(), CliError> {
    match command.command {
        RetentionSubcommand::Cleanup {
            data_dir,
            older_than_days,
            dry_run,
        } => {
            let store = fleet_store::SqliteStore::open(controller_db_path(&data_dir))?;
            let now = SystemTime::now();
            let policy = RetentionPolicy::uniform(Duration::from_secs(
                older_than_days.saturating_mul(86_400),
            ));
            let mut repo = &store;
            let mut audit = &store;
            let output = RunRetentionCleanup::execute(
                &mut repo,
                &mut audit,
                RunRetentionCleanupInput {
                    now,
                    policy,
                    dry_run,
                    actor: "cli".to_owned(),
                    target: "controller-store".to_owned(),
                },
            )
            .map_err(map_cli_retention_cleanup_error)?;
            let summary = output.summary;
            println!("job_output_chunks={}", summary.job_output_chunks);
            println!("facts_snapshots={}", summary.facts_snapshots);
            println!("metrics_snapshots={}", summary.metrics_snapshots);
            println!("agent_log_chunks={}", summary.agent_log_chunks);
            println!("total={}", summary.total());
            println!("dry_run={dry_run}");
            Ok(())
        }
    }
}

fn map_cli_retention_cleanup_error(
    error: fleet_application::RunRetentionCleanupError<
        fleet_store::StoreError,
        fleet_store::StoreError,
    >,
) -> CliError {
    match error {
        fleet_application::RunRetentionCleanupError::Domain(error) => CliError::Http(error),
        fleet_application::RunRetentionCleanupError::Repository(error)
        | fleet_application::RunRetentionCleanupError::Audit(error) => CliError::Store(error),
    }
}

fn execute_logs(command: LogsCommand) -> Result<(), CliError> {
    if let Some(file) = command.file {
        stream_log_file(
            Path::new(&file),
            LogStreamOptions {
                follow: command.follow,
                max_duration: command.max_duration_seconds.map(Duration::from_secs),
                poll_interval: LOG_TAIL_POLL_INTERVAL,
            },
            |line| println!("{line}"),
            || false,
        )?;
        return Ok(());
    }
    if let Some(target) = command.target.as_deref()
        && let Some(command) = journald_command_for_service(target)
    {
        println!("log target={target}");
        println!(
            "journald command: {} {}",
            command.program,
            command.args.join(" ")
        );
        return Ok(());
    }
    println!(
        "log target={}",
        command.target.as_deref().unwrap_or("local")
    );
    println!("no log file provided");
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct LogStreamOptions {
    follow: bool,
    max_duration: Option<Duration>,
    poll_interval: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JournaldCommand {
    program: String,
    args: Vec<String>,
}

fn journald_command_for_service(service: &str) -> Option<JournaldCommand> {
    if !is_safe_journald_service_name(service) {
        return None;
    }
    Some(JournaldCommand {
        program: "journalctl".to_owned(),
        args: vec![
            "-u".to_owned(),
            service.to_owned(),
            "--no-pager".to_owned(),
            "-n".to_owned(),
            LOG_TAIL_MAX_LINES.to_string(),
        ],
    })
}

fn is_safe_journald_service_name(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '@' | '-')
        })
}

fn stream_log_file(
    path: &Path,
    options: LogStreamOptions,
    mut emit: impl FnMut(String),
    mut should_cancel: impl FnMut() -> bool,
) -> Result<(), CliError> {
    let body = fs::read_to_string(path)?;
    for line in render_log_tail(&body) {
        emit(line);
    }
    if !options.follow {
        return Ok(());
    }

    let mut offset = body.len();
    let started_at = Instant::now();
    loop {
        if should_cancel() {
            return Ok(());
        }
        if options
            .max_duration
            .is_some_and(|duration| started_at.elapsed() >= duration)
        {
            return Ok(());
        }

        let next_body = fs::read_to_string(path)?;
        if next_body.len() < offset {
            offset = 0;
        }
        if next_body.len() > offset {
            for line in render_appended_log_lines(&next_body[offset..]) {
                emit(line);
            }
            offset = next_body.len();
        }
        std::thread::sleep(options.poll_interval);
    }
}

fn render_log_tail(body: &str) -> Vec<String> {
    body.lines()
        .rev()
        .take(LOG_TAIL_MAX_LINES)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(redact_and_truncate_log_line)
        .collect()
}

fn render_appended_log_lines(body: &str) -> Vec<String> {
    body.lines().map(redact_and_truncate_log_line).collect()
}

fn redact_and_truncate_log_line(line: &str) -> String {
    let redacted = redact_secret(line);
    if redacted.len() <= LOG_TAIL_MAX_LINE_BYTES {
        return redacted;
    }

    let mut end = LOG_TAIL_MAX_LINE_BYTES;
    while !redacted.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...[truncated]", &redacted[..end])
}

fn execute_drift(command: DriftCommand) -> Result<(), CliError> {
    match command.command {
        DriftSubcommand::Check { policy } => {
            let policy = policy.ok_or(CliError::MissingPolicy)?;
            let body = fs::read_to_string(policy)?;
            let parsed = fleet_domain::parse_policy_document(&body)
                .map_err(|error| CliError::Http(format!("invalid policy: {error}")))?;
            let report =
                fleet_runner::evaluate_policy_drift(&parsed, &fleet_runner::LocalDriftProbe);
            println!("status={}", drift_status_to_cli(&report.status));
            println!("policy={}", report.policy_name);
            println!("expected={}", report.expected);
            println!("actual={}", report.actual);
            Ok(())
        }
    }
}

fn execute_upgrade(command: UpgradeCommand) -> Result<(), CliError> {
    if !command.dry_run {
        return Err(CliError::UpgradeRequiresDryRun);
    }
    for line in upgrade_dry_run_lines(&command) {
        println!("{line}");
    }
    Ok(())
}

fn upgrade_dry_run_lines(command: &UpgradeCommand) -> Vec<String> {
    let target_version = command.version.as_deref().unwrap_or("latest");
    vec![
        "upgrade_dry_run=true".to_owned(),
        format!("current_version={}", env!("CARGO_PKG_VERSION")),
        format!("channel={}", command.channel.as_str()),
        format!("target_version={target_version}"),
        "backup_required=true".to_owned(),
        "recommended_backup_command=fleet controller backup --data-dir <controller-data-dir> --output ./fleet-controller-before-upgrade.backup.json".to_owned(),
        "artifact_integrity_required=true".to_owned(),
        "artifact_integrity_command=./scripts/verify_standalone_artifacts.sh dist/release".to_owned(),
        "artifact_signature_command=./scripts/verify_release_signature.sh dist/release <release-public-key.pem>".to_owned(),
        "recovery_policy=restore the previous fleet binary; if controller data was migrated or changed, restore the controller backup before restarting services".to_owned(),
        "service_policy=stop services before binary replacement, then restart and verify with status-service".to_owned(),
    ]
}

fn drift_status_to_cli(status: &fleet_domain::DriftStatus) -> &'static str {
    match status {
        fleet_domain::DriftStatus::Compliant => "compliant",
        fleet_domain::DriftStatus::Drifted => "drifted",
        fleet_domain::DriftStatus::Unknown => "unknown",
    }
}

fn execute_demo(command: DemoCommand) -> Result<(), CliError> {
    let port = match command.port {
        Some(port) => {
            ensure_loopback_port_available(port)?;
            port
        }
        None => free_loopback_port()?,
    };
    let data_dir = unique_demo_dir();
    fs::create_dir_all(controller_dir(&data_dir))?;
    fs::create_dir_all(agent_dir(&data_dir))?;

    let store = fleet_store::SqliteStore::open(controller_db_path(&data_dir))?;
    let controller_fingerprint = ensure_controller_identity(&data_dir)?;
    let admin_token = fleet_controller::create_admin_token(&store)?
        .ok_or_else(|| CliError::Http("demo data dir unexpectedly reused".to_owned()))?;
    let enroll_token = prefixed_ulid("enroll-demo")?;
    store.insert_enrollment_token_hash(
        "et-demo",
        &fleet_controller::hash_token(&enroll_token),
        "role=web,env=demo",
        SystemTime::now() + Duration::from_secs(300),
        1,
    )?;
    drop(store);

    let shutdown = Arc::new(AtomicBool::new(false));
    let thread_shutdown = shutdown.clone();
    let server_data_dir = data_dir.clone();
    let handle = std::thread::spawn(move || {
        fleet_controller::start_controller_server_until(
            fleet_controller::ControllerServerConfig {
                host: "127.0.0.1".to_owned(),
                port,
                external_url: Some(format!("http://127.0.0.1:{port}")),
                tls_cert_path: None,
                tls_key_path: None,
                agent_client_ca_cert_path: None,
                data_dir: server_data_dir,
                database: None,
                secret_provider: None,
            },
            move || thread_shutdown.load(Ordering::SeqCst),
        )
    });
    let _guard = DemoGuard {
        data_dir: data_dir.clone(),
        keep_temp: command.keep_temp,
        shutdown,
        handle: Some(handle),
    };

    wait_for_controller_health(port)?;
    let controller_url = format!("http://127.0.0.1:{port}");
    let agent_config = enroll_demo_agent(&data_dir, &controller_url, &enroll_token)?;
    create_demo_command_job(port, &admin_token)?;
    run_agent_heartbeat_once(&agent_config, true)?;
    let output = http_get(port, "/api/jobs/job-demo-1/output", Some(&admin_token))?;
    if !output.contains("demo-ok") {
        return Err(CliError::Http(format!(
            "demo command output was not observed: {output}"
        )));
    }
    let rendered_output = render_job_output_api_for_cli(&output)?;

    println!("demo controller: {controller_url}");
    println!("demo admin: {controller_url}/admin");
    println!("demo controller fingerprint: {controller_fingerprint}");
    eprintln!(
        "{}",
        format_warning_message(format!(
            "demo uses insecure HTTP controller URL: {controller_url}; HTTP is test-only and not encrypted; use HTTPS for product or production environments"
        ))
    );
    if command.keep_temp {
        println!("demo data dir: {}", data_dir.display());
    }
    println!("demo command output: demo-ok");
    for line in rendered_output {
        print!("{line}");
    }
    Ok(())
}

fn enroll_demo_agent(
    data_dir: &Path,
    controller_url: &str,
    token: &str,
) -> Result<LocalAgentConfig, CliError> {
    let key_pair = fleet_core::generate_agent_key_pair()?;
    let response = enroll_agent_via_controller(
        controller_url,
        None,
        &fleet_controller::EnrollAgentRequest {
            token: token.to_owned(),
            agent_id: "agent-web-01".to_owned(),
            name: "web-01".to_owned(),
            public_key: key_pair.public_key_hex.clone(),
            fingerprint: key_pair.fingerprint.clone(),
            labels: parse_labels("role=web,env=demo")?,
        },
    )?;
    let config_body = format!(
        "url={controller_url}\nagent_id={}\nname=web-01\nlabels=role=web,env=demo\nfingerprint={}\ncontroller_fingerprint={}\n",
        response.agent_id, key_pair.fingerprint, response.controller_fingerprint
    );
    write_secure_file(&agent_dir(data_dir).join("agent.conf"), &config_body)?;
    write_secure_file(
        &agent_dir(data_dir).join("agent_private.key"),
        &format!("{}\n", key_pair.private_key_hex),
    )?;
    read_agent_config(&agent_dir(data_dir).join("agent.conf"))
}

fn create_demo_command_job(port: u16, admin_token: &str) -> Result<(), CliError> {
    let body = serde_json::json!({
        "job_id": "job-demo-1",
        "target_agent_ids": ["agent-web-01"],
        "program": "echo",
        "args": ["demo-ok"],
        "timeout_seconds": 30,
        "confirmed_high_risk": false,
        "confirmed_by": "demo-admin",
        "expires_in_seconds": 60,
        "nonce_prefix": "demo"
    })
    .to_string();
    let response = http_request(
        port,
        "POST",
        "/api/jobs/command",
        Some(admin_token),
        Some(&body),
    )?;
    if !response.starts_with("HTTP/1.1 201") {
        return Err(CliError::Http(
            response.lines().next().unwrap_or("").to_owned(),
        ));
    }
    Ok(())
}

fn wait_for_controller_health(port: u16) -> Result<(), CliError> {
    for _ in 0..100 {
        if http_get(port, "/healthz", None)
            .map(|body| body.contains("\"status\":\"ok\""))
            .unwrap_or(false)
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(CliError::Http(
        "demo controller did not become healthy".to_owned(),
    ))
}

fn http_get(port: u16, path: &str, bearer_token: Option<&str>) -> Result<String, CliError> {
    let response = http_request(port, "GET", path, bearer_token, None)?;
    if !response.starts_with("HTTP/1.1 200") {
        return Err(CliError::Http(
            response.lines().next().unwrap_or("").to_owned(),
        ));
    }
    Ok(response
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or_default()
        .to_owned())
}

fn http_request(
    port: u16,
    method: &str,
    path: &str,
    bearer_token: Option<&str>,
    body: Option<&str>,
) -> Result<String, CliError> {
    let body = body.unwrap_or_default();
    let auth_header = bearer_token
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let content_headers = if body.is_empty() {
        String::new()
    } else {
        format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        )
    };
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth_header}{content_headers}Connection: close\r\n\r\n{body}"
    );
    stream.write_all(request.as_bytes())?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

fn http_request_url(
    url: &str,
    method: &str,
    path: &str,
    bearer_token: Option<&str>,
    body: Option<&str>,
) -> Result<String, CliError> {
    let endpoint = parse_controller_url(url)?;
    warn_if_insecure_http_endpoint(&endpoint);
    let body = body.unwrap_or_default();
    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|error| CliError::Http(error.to_string()))?;
    let client = reqwest::blocking::Client::new();
    let mut request = client.request(method, endpoint.api_url(path));
    if let Some(token) = bearer_token {
        request = request.bearer_auth(token);
    }
    if !body.is_empty() {
        request = request
            .header("content-type", "application/json")
            .body(body.to_owned());
    }
    let response = request
        .send()
        .map_err(|error| CliError::Http(error.to_string()))?;
    let status = response.status();
    let body = response
        .text()
        .map_err(|error| CliError::Http(error.to_string()))?;
    Ok(format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status.as_u16(),
        status.canonical_reason().unwrap_or("Unknown"),
        body.len(),
        body
    ))
}

fn free_loopback_port() -> Result<u16, CliError> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

fn ensure_loopback_port_available(port: u16) -> Result<(), CliError> {
    TcpListener::bind(("127.0.0.1", port))
        .map(|_| ())
        .map_err(|error| CliError::Http(format!("demo port {port} is unavailable: {error}")))
}

fn unique_demo_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "fleet-demo-{}-{}",
        std::process::id(),
        epoch_millis()
    ))
}

struct DemoGuard {
    data_dir: PathBuf,
    keep_temp: bool,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<Result<(), fleet_controller::ControllerError>>>,
}

impl Drop for DemoGuard {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        if !self.keep_temp {
            let _ = fs::remove_dir_all(&self.data_dir);
        }
    }
}

fn controller_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("controller")
}

fn agent_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("agent")
}

fn controller_db_path(data_dir: &Path) -> PathBuf {
    controller_dir(data_dir).join("fleet.db")
}

fn ensure_controller_initialized_for_start(data_dir: &Path) -> Result<(), CliError> {
    let controller_path = controller_dir(data_dir);
    let public_key_path = controller_path.join("controller_public.key");
    let private_key_path = controller_path.join("controller_private.key");
    if !controller_path.is_dir() || !public_key_path.is_file() || !private_key_path.is_file() {
        return Err(CliError::ControllerNotInitialized {
            data_dir: data_dir.to_path_buf(),
        });
    }
    Ok(())
}

fn parse_controller_database_settings(
    value: Option<&str>,
    data_dir: &Path,
) -> Result<DatabaseSettings, CliError> {
    DatabaseSettings::parse_optional(value, controller_db_path(data_dir))
        .map_err(|error| CliError::Http(error.to_string()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceRole {
    Controller,
    Agent,
}

fn resolve_service_binary(binary: Option<PathBuf>) -> Result<PathBuf, CliError> {
    let path = match binary {
        Some(path) => path,
        None => std::env::current_exe()?,
    };
    if !path.is_absolute() {
        return Err(CliError::ServiceBinaryMustBeAbsolute(path));
    }
    Ok(path)
}

fn render_service_unit(
    role: ServiceRole,
    binary: &Path,
    data_dir: &Path,
    user: Option<&str>,
    group: Option<&str>,
) -> Result<String, CliError> {
    if !binary.is_absolute() {
        return Err(CliError::ServiceBinaryMustBeAbsolute(binary.to_path_buf()));
    }
    validate_service_account(user)?;
    validate_service_account(group)?;

    let (description, role_args) = match role {
        ServiceRole::Controller => (
            "Sponzey Fleet Controller",
            format!("controller start --data-dir {}", systemd_arg(data_dir)),
        ),
        ServiceRole::Agent => (
            "Sponzey Fleet Agent",
            format!("agent start --data-dir {}", systemd_arg(data_dir)),
        ),
    };
    let mut unit = format!(
        "[Unit]\nDescription={description}\nAfter=network-online.target\n\n[Service]\nType=simple\nExecStart={} {role_args}\nRestart=on-failure\n",
        systemd_arg(binary)
    );
    if let Some(user) = user {
        unit.push_str(&format!("User={user}\n"));
    }
    if let Some(group) = group {
        unit.push_str(&format!("Group={group}\n"));
    }
    unit.push_str("\n[Install]\nWantedBy=multi-user.target\n");
    Ok(unit)
}

fn service_unit_name(role: ServiceRole) -> &'static str {
    match role {
        ServiceRole::Controller => "fleet-controller.service",
        ServiceRole::Agent => "fleet-agent.service",
    }
}

fn systemd_unit_path(role: ServiceRole) -> PathBuf {
    Path::new("/etc/systemd/system").join(service_unit_name(role))
}

fn render_systemctl_command(action: &str, role: ServiceRole) -> String {
    format!("systemctl {action} {}", service_unit_name(role))
}

fn render_service_status_command(role: ServiceRole) -> String {
    format!("systemctl status {} --no-pager", service_unit_name(role))
}

fn render_service_logs_command(role: ServiceRole, lines: usize) -> String {
    format!(
        "journalctl -u {} --no-pager -n {lines}",
        service_unit_name(role)
    )
}

fn render_uninstall_service_commands(role: ServiceRole) -> Vec<String> {
    vec![
        render_systemctl_command("disable --now", role),
        format!("rm {}", systemd_arg(&systemd_unit_path(role))),
        "systemctl daemon-reload".to_owned(),
    ]
}

fn start_systemd_service(role: ServiceRole, dry_run: bool) -> Result<(), CliError> {
    if dry_run {
        println!("{}", render_systemctl_command("start", role));
        return Ok(());
    }
    ensure_systemd_operation_allowed()?;
    run_systemctl(&["start", service_unit_name(role)])
}

fn restart_systemd_service(role: ServiceRole, dry_run: bool) -> Result<(), CliError> {
    if dry_run {
        println!("{}", render_systemctl_command("restart", role));
        return Ok(());
    }
    ensure_systemd_operation_allowed()?;
    run_systemctl(&["restart", service_unit_name(role)])
}

fn status_systemd_service(role: ServiceRole, dry_run: bool) -> Result<(), CliError> {
    if dry_run {
        println!("{}", render_service_status_command(role));
        return Ok(());
    }
    ensure_systemd_query_allowed()?;
    let status = ProcessCommand::new("systemctl")
        .arg("status")
        .arg(service_unit_name(role))
        .arg("--no-pager")
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::Io(std::io::Error::other(format!(
            "systemctl status {} failed with status {status}",
            service_unit_name(role)
        ))))
    }
}

fn logs_systemd_service(role: ServiceRole, lines: usize, dry_run: bool) -> Result<(), CliError> {
    if dry_run {
        println!("{}", render_service_logs_command(role, lines));
        return Ok(());
    }
    ensure_systemd_query_allowed()?;
    let status = ProcessCommand::new("journalctl")
        .arg("-u")
        .arg(service_unit_name(role))
        .arg("--no-pager")
        .arg("-n")
        .arg(lines.to_string())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::Io(std::io::Error::other(format!(
            "journalctl -u {} failed with status {status}",
            service_unit_name(role)
        ))))
    }
}

fn install_systemd_service(role: ServiceRole, unit: &str) -> Result<(), CliError> {
    ensure_systemd_operation_allowed()?;
    let path = systemd_unit_path(role);
    fs::write(&path, unit)?;
    run_systemctl(&["daemon-reload"])?;
    run_systemctl(&["enable", service_unit_name(role)])
}

fn uninstall_systemd_service(role: ServiceRole, dry_run: bool) -> Result<(), CliError> {
    if dry_run {
        for command in render_uninstall_service_commands(role) {
            println!("{command}");
        }
        return Ok(());
    }
    ensure_systemd_operation_allowed()?;
    run_systemctl(&["disable", "--now", service_unit_name(role)])?;
    let path = systemd_unit_path(role);
    if path.exists() {
        fs::remove_file(path)?;
    }
    run_systemctl(&["daemon-reload"])
}

fn ensure_systemd_operation_allowed() -> Result<(), CliError> {
    ensure_systemd_query_allowed()?;
    if effective_user_id()? != 0 {
        return Err(CliError::ServiceOperationRequiresRoot);
    }
    Ok(())
}

fn ensure_systemd_query_allowed() -> Result<(), CliError> {
    if std::env::consts::OS != "linux" {
        return Err(CliError::ServiceOperationRequiresLinux);
    }
    Ok(())
}

fn effective_user_id() -> Result<u32, CliError> {
    let output = ProcessCommand::new("id").arg("-u").output()?;
    if !output.status.success() {
        return Err(CliError::Io(std::io::Error::other(
            "failed to determine effective user id",
        )));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim()
        .parse::<u32>()
        .map_err(|error| CliError::Io(std::io::Error::other(error.to_string())))
}

fn run_systemctl(args: &[&str]) -> Result<(), CliError> {
    let status = ProcessCommand::new("systemctl").args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::Io(std::io::Error::other(format!(
            "systemctl {} failed with status {status}",
            args.join(" ")
        ))))
    }
}

fn validate_service_account(value: Option<&str>) -> Result<(), CliError> {
    let Some(value) = value else {
        return Ok(());
    };
    let valid = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if valid {
        Ok(())
    } else {
        Err(CliError::InvalidServiceAccount(value.to_owned()))
    }
}

fn systemd_arg(path: &Path) -> String {
    let value = path.display().to_string();
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.' | ':'))
    {
        return value;
    }
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn shell_arg(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.' | ':' | '=' | ','))
    {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn epoch_seconds(value: SystemTime) -> u64 {
    value
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn ensure_controller_identity(data_dir: &Path) -> Result<String, CliError> {
    let public_key_path = controller_dir(data_dir).join("controller_public.key");
    let private_key_path = controller_dir(data_dir).join("controller_private.key");

    match (public_key_path.exists(), private_key_path.exists()) {
        (false, false) => {
            let key_pair = fleet_core::generate_agent_key_pair()?;
            write_once(&public_key_path, &format!("{}\n", key_pair.public_key_hex))?;
            write_once_secure(
                &private_key_path,
                &format!("{}\n", key_pair.private_key_hex),
            )?;
            Ok(key_pair.fingerprint)
        }
        (true, true) => {
            validate_secure_file_permissions(&private_key_path)?;
            let public_key = fs::read_to_string(public_key_path)?.trim().to_owned();
            Ok(fleet_core::fingerprint_public_key(&public_key)?)
        }
        _ => Err(CliError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "controller identity is incomplete; public and private key files must exist together",
        ))),
    }
}

fn append_line(path: &Path, line: &str) -> Result<(), std::io::Error> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())
}

fn write_once(path: &Path, body: &str) -> Result<(), std::io::Error> {
    if path.exists() {
        return Ok(());
    }
    fs::write(path, body)
}

fn write_once_secure(path: &Path, body: &str) -> Result<(), std::io::Error> {
    if path.exists() {
        return Ok(());
    }
    write_secure_file(path, body)
}

fn write_secure_file(path: &Path, body: &str) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(body.as_bytes())?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
    }

    #[cfg(not(unix))]
    {
        fs::write(path, body)
    }
}

fn write_secure_file_atomic(path: &Path, body: &str) -> Result<(), std::io::Error> {
    let tmp_path = path.with_extension("tmp");
    write_secure_file(&tmp_path, body)?;
    fs::rename(tmp_path, path)
}

fn validate_secure_file_permissions(path: &Path) -> Result<(), CliError> {
    #[cfg(unix)]
    {
        let mode = fs::metadata(path)?.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(CliError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "{} must not be readable, writable, or executable by group/other",
                    path.display()
                ),
            )));
        }
    }

    Ok(())
}

fn canonicalize_tls_ca_cert(path: &Path) -> Result<PathBuf, CliError> {
    let path = fs::canonicalize(path)?;
    let certificates = load_pem_certificates(&path)?;
    if certificates.is_empty() {
        return Err(CliError::Http(format!(
            "TLS CA certificate file has no certificates: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn load_pem_certificates(
    path: &Path,
) -> Result<Vec<tokio_rustls::rustls::pki_types::CertificateDer<'static>>, CliError> {
    let file = fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            CliError::Http(format!(
                "failed to parse TLS CA certificate file {}: {error}",
                path.display()
            ))
        })
}

fn parse_labels(labels: &str) -> Result<Vec<fleet_controller::EnrollAgentLabel>, CliError> {
    labels
        .split(',')
        .filter(|part| !part.trim().is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').ok_or_else(|| {
                CliError::Http(format!("invalid label, expected key=value: {part}"))
            })?;
            Ok(fleet_controller::EnrollAgentLabel {
                key: key.to_owned(),
                value: value.to_owned(),
            })
        })
        .collect()
}

fn enroll_agent_via_controller(
    url: &str,
    tls_ca_cert: Option<&Path>,
    request: &fleet_controller::EnrollAgentRequest,
) -> Result<fleet_controller::EnrollAgentResponse, CliError> {
    let body = serde_json::to_string(request).map_err(|error| CliError::Http(error.to_string()))?;
    let endpoint = parse_controller_url(url)?;
    let enroll_url = endpoint.api_url("/api/agents/enroll");
    let response = reqwest_client(tls_ca_cert)?
        .post(&enroll_url)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .map_err(|error| {
            CliError::Http(http_request_error_message(
                "agent enrollment",
                &enroll_url,
                &error,
            ))
        })?;
    let status = response.status();
    if status.as_u16() != 201 {
        return Err(CliError::Http(format!(
            "HTTP/1.1 {} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("request failed")
        )));
    }
    response
        .json()
        .map_err(|error| CliError::Http(error.to_string()))
}

fn controller_identity_via_controller(
    url: &str,
    tls_ca_cert: Option<&Path>,
) -> Result<fleet_controller::ControllerIdentityResponse, CliError> {
    let endpoint = parse_controller_url(url)?;
    let identity_url = endpoint.api_url("/api/controller/identity");
    let response = reqwest_client(tls_ca_cert)?
        .get(&identity_url)
        .send()
        .map_err(|error| {
            CliError::Http(http_request_error_message(
                "controller identity request",
                &identity_url,
                &error,
            ))
        })?;
    let status = response.status();
    if status.as_u16() != 200 {
        return Err(CliError::Http(format!(
            "HTTP/1.1 {} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("request failed")
        )));
    }
    response
        .json()
        .map_err(|error| CliError::Http(error.to_string()))
}

fn http_request_error_message(action: &str, url: &str, error: &dyn StdError) -> String {
    let mut message = format!("{action} failed for {url}: {error}");
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_message = cause.to_string();
        if !cause_message.is_empty() && !message.contains(&cause_message) {
            message.push_str(": ");
            message.push_str(&cause_message);
        }
        source = cause.source();
    }
    message.push_str(
        ". Check that the controller is running, the URL host/port are reachable from this agent, the controller is bound to a reachable host such as 0.0.0.0 for remote tests, and any firewall allows the port.",
    );
    message
}

fn reqwest_client(tls_ca_cert: Option<&Path>) -> Result<reqwest::blocking::Client, CliError> {
    let mut builder = reqwest::blocking::Client::builder();
    if let Some(path) = tls_ca_cert {
        let certificate = reqwest::Certificate::from_pem(&fs::read(path)?)
            .map_err(|error| CliError::Http(format!("invalid TLS CA certificate: {error}")))?;
        builder = builder.add_root_certificate(certificate);
    }
    builder
        .build()
        .map_err(|error| CliError::Http(error.to_string()))
}

fn connect_agent_websocket(
    ws_url: &str,
    endpoint: &ControllerEndpoint,
    tls_ca_cert: Option<&Path>,
) -> Result<
    (
        tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
        tungstenite::handshake::client::Response,
    ),
    CliError,
> {
    if tls_ca_cert.is_none() {
        return tungstenite::connect(ws_url).map_err(|error| CliError::Http(error.to_string()));
    }

    let request = ws_url
        .into_client_request()
        .map_err(|error| CliError::Http(error.to_string()))?;
    let stream = TcpStream::connect(format!(
        "{}:{}",
        display_socket_host(&endpoint.host),
        endpoint.port
    ))?;
    stream.set_nodelay(true)?;
    let connector = match endpoint.scheme {
        ControllerUrlScheme::Http => None,
        ControllerUrlScheme::Https => Some(Connector::Rustls(build_websocket_tls_config(
            tls_ca_cert.expect("checked above"),
        )?)),
    };

    tungstenite::client_tls_with_config(request, stream, None, connector)
        .map_err(|error| CliError::Http(error.to_string()))
}

fn build_websocket_tls_config(tls_ca_cert: &Path) -> Result<Arc<RustlsClientConfig>, CliError> {
    ensure_rustls_crypto_provider();
    let mut root_store = RootCertStore::empty();
    let mut added = 0_usize;
    for certificate in load_pem_certificates(tls_ca_cert)? {
        root_store
            .add(certificate)
            .map_err(|error| CliError::Http(format!("invalid TLS CA certificate: {error}")))?;
        added += 1;
    }
    if added == 0 {
        return Err(CliError::Http(format!(
            "TLS CA certificate file has no certificates: {}",
            tls_ca_cert.display()
        )));
    }
    Ok(Arc::new(
        RustlsClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth(),
    ))
}

fn ensure_rustls_crypto_provider() {
    let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControllerUrlScheme {
    Http,
    Https,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ControllerEndpoint {
    scheme: ControllerUrlScheme,
    host: String,
    port: u16,
}

impl ControllerEndpoint {
    fn api_url(&self, path: &str) -> String {
        let scheme = match self.scheme {
            ControllerUrlScheme::Http => "http",
            ControllerUrlScheme::Https => "https",
        };
        let host = display_url_host(&self.host);
        format!("{scheme}://{host}:{}{}", self.port, normalized_path(path))
    }

    fn websocket_url(&self, path: &str) -> String {
        let scheme = match self.scheme {
            ControllerUrlScheme::Http => "ws",
            ControllerUrlScheme::Https => "wss",
        };
        let host = display_url_host(&self.host);
        format!("{scheme}://{host}:{}{}", self.port, normalized_path(path))
    }
}

fn normalized_path(path: &str) -> String {
    if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    }
}

fn display_url_host(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    }
}

fn display_socket_host(host: &str) -> String {
    display_url_host(host)
}

fn parse_controller_url(url: &str) -> Result<ControllerEndpoint, CliError> {
    let (scheme, rest) = if let Some(rest) = url.strip_prefix("http://") {
        (ControllerUrlScheme::Http, rest)
    } else if let Some(rest) = url.strip_prefix("https://") {
        (ControllerUrlScheme::Https, rest)
    } else {
        return Err(CliError::Http(
            "controller URL must start with http:// or https://".to_owned(),
        ));
    };

    let authority = rest.split('/').next().unwrap_or(rest);
    if authority.is_empty() {
        return Err(CliError::Http(
            "controller URL host cannot be empty".to_owned(),
        ));
    }
    let (host, port) = parse_controller_authority(authority, scheme)?;
    if is_wildcard_host(&host) {
        return Err(CliError::Http(
            "controller URL must not use a wildcard host such as 0.0.0.0".to_owned(),
        ));
    }
    Ok(ControllerEndpoint { scheme, host, port })
}

fn parse_controller_authority(
    authority: &str,
    scheme: ControllerUrlScheme,
) -> Result<(String, u16), CliError> {
    if let Some(stripped) = authority.strip_prefix('[') {
        let (host, rest) = stripped
            .split_once(']')
            .ok_or_else(|| CliError::Http("invalid bracketed IPv6 host".to_owned()))?;
        let port = if let Some(port) = rest.strip_prefix(':') {
            parse_controller_port(port)?
        } else {
            default_controller_port(scheme)
        };
        return Ok((host.to_owned(), port));
    }

    let colon_count = authority
        .chars()
        .filter(|character| *character == ':')
        .count();
    if colon_count > 1 {
        return Ok((authority.to_owned(), default_controller_port(scheme)));
    }

    if let Some((host, port)) = authority.split_once(':') {
        if host.is_empty() {
            return Err(CliError::Http(
                "controller URL host cannot be empty".to_owned(),
            ));
        }
        return Ok((host.to_owned(), parse_controller_port(port)?));
    }

    Ok((authority.to_owned(), default_controller_port(scheme)))
}

fn parse_controller_port(port: &str) -> Result<u16, CliError> {
    if port.is_empty() {
        return Err(CliError::Http("controller port cannot be empty".to_owned()));
    }
    port.parse::<u16>()
        .map_err(|_| CliError::Http("invalid controller port".to_owned()))
}

fn default_controller_port(scheme: ControllerUrlScheme) -> u16 {
    match scheme {
        ControllerUrlScheme::Http => 80,
        ControllerUrlScheme::Https => 443,
    }
}

fn is_wildcard_host(host: &str) -> bool {
    matches!(host, "0.0.0.0" | "::")
}

fn warn_if_insecure_http_url(url: &str) {
    if let Ok(endpoint) = parse_controller_url(url) {
        warn_if_insecure_http_endpoint(&endpoint);
    }
}

fn warn_if_insecure_http_endpoint(endpoint: &ControllerEndpoint) {
    if endpoint.scheme == ControllerUrlScheme::Http {
        eprintln!(
            "{}",
            format_warning_message(format!(
                "insecure HTTP controller URL enabled: {}; HTTP is test-only and not encrypted; use HTTPS for product or production environments",
                endpoint.api_url("").trim_end_matches('/')
            ))
        );
    }
}

#[derive(Debug, Clone)]
struct LocalAgentConfig {
    url: String,
    tls_ca_cert: Option<PathBuf>,
    agent_id: String,
    fingerprint: String,
    private_key: String,
    controller_fingerprint: String,
    replay_store_path: PathBuf,
    controller_trust_bundle_path: PathBuf,
    controller_trust_bundle: Option<fleet_domain::ControllerSigningTrustBundle>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AgentControllerTrustBundleSidecar {
    entries: Vec<fleet_protocol::ControllerSigningTrustEntryWire>,
}

#[derive(Debug, Clone, Copy)]
struct AgentHeartbeatOptions {
    once: bool,
    heartbeat_interval: Duration,
    facts_interval: Duration,
    metrics_interval: Duration,
    log_upload: AgentLogUploadOptions,
    max_reconnect_attempts: u32,
}

#[derive(Debug, Clone, Copy)]
struct AgentLogUploadOptions {
    enabled: bool,
    interval: Duration,
}

fn read_agent_config(path: &Path) -> Result<LocalAgentConfig, CliError> {
    validate_secure_file_permissions(path)?;
    let body = fs::read_to_string(path)?;
    let value = |key: &str| {
        body.lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .map(str::to_owned)
            .ok_or_else(|| CliError::Http(format!("missing agent config key: {key}")))
    };
    let optional_value = |key: &str| {
        body.lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .map(PathBuf::from)
    };
    let private_key_path = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("agent_private.key");
    validate_secure_file_permissions(&private_key_path)?;
    let private_key = fs::read_to_string(private_key_path)?.trim().to_owned();
    let controller_trust_bundle_path = agent_controller_trust_bundle_path(path);
    let controller_trust_bundle =
        read_agent_controller_trust_bundle_sidecar(&controller_trust_bundle_path)?;
    Ok(LocalAgentConfig {
        url: value("url")?,
        tls_ca_cert: optional_value("tls_ca_cert"),
        agent_id: value("agent_id")?,
        fingerprint: value("fingerprint")?,
        private_key,
        controller_fingerprint: value("controller_fingerprint")?,
        replay_store_path: agent_replay_store_path(path),
        controller_trust_bundle_path,
        controller_trust_bundle,
    })
}

fn agent_replay_store_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("task_nonces.log")
}

fn agent_controller_trust_bundle_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("controller_trust_bundle.json")
}

fn agent_nonce_replay_guard(
    config: &LocalAgentConfig,
) -> Result<fleet_runner::NonceReplayGuard, CliError> {
    fleet_runner::NonceReplayGuard::file_backed(&config.replay_store_path)
        .map_err(|error| CliError::Http(error.to_string()))
}

fn legacy_controller_signing_trust_bundle(
    config: &LocalAgentConfig,
    controller_public_key: &str,
) -> Result<fleet_domain::ControllerSigningTrustBundle, CliError> {
    let fingerprint =
        fleet_domain::SigningKeyFingerprint::new(config.controller_fingerprint.clone())
            .map_err(|error| CliError::Http(error.to_string()))?;
    let public_key =
        fleet_domain::ControllerSigningPublicKey::new(controller_public_key.to_owned())
            .map_err(|error| CliError::Http(error.to_string()))?;
    fleet_domain::ControllerSigningTrustBundle::from_legacy_pinned(fingerprint, public_key)
        .map_err(|error| CliError::Http(error.to_string()))
}

fn controller_signing_trust_bundle_from_wire(
    entries: &[fleet_protocol::ControllerSigningTrustEntryWire],
) -> Result<fleet_domain::ControllerSigningTrustBundle, CliError> {
    let mut domain_entries = Vec::with_capacity(entries.len());
    for entry in entries {
        let role = match entry.role {
            fleet_protocol::ControllerSigningTrustRoleWire::Current => {
                fleet_domain::ControllerSigningTrustRole::Current
            }
            fleet_protocol::ControllerSigningTrustRoleWire::Previous => {
                fleet_domain::ControllerSigningTrustRole::Previous
            }
        };
        domain_entries.push(
            fleet_domain::ControllerSigningTrustEntry::new(
                role,
                fleet_domain::SigningKeyFingerprint::new(entry.fingerprint.clone())
                    .map_err(|error| CliError::Http(error.to_string()))?,
                fleet_domain::ControllerSigningPublicKey::new(entry.public_key.clone())
                    .map_err(|error| CliError::Http(error.to_string()))?,
                millis_to_system_time(entry.valid_from_ms),
                entry.valid_until_ms.map(millis_to_system_time),
            )
            .map_err(|error| CliError::Http(error.to_string()))?,
        );
    }
    fleet_domain::ControllerSigningTrustBundle::new(domain_entries)
        .map_err(|error| CliError::Http(error.to_string()))
}

fn controller_signing_trust_bundle_to_wire(
    bundle: &fleet_domain::ControllerSigningTrustBundle,
) -> Vec<fleet_protocol::ControllerSigningTrustEntryWire> {
    bundle
        .entries()
        .iter()
        .map(|entry| fleet_protocol::ControllerSigningTrustEntryWire {
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
        })
        .collect()
}

fn read_agent_controller_trust_bundle_sidecar(
    path: &Path,
) -> Result<Option<fleet_domain::ControllerSigningTrustBundle>, CliError> {
    if !path.exists() {
        return Ok(None);
    }
    validate_secure_file_permissions(path)?;
    let body = fs::read_to_string(path)?;
    let sidecar: AgentControllerTrustBundleSidecar =
        serde_json::from_str(&body).map_err(|error| {
            CliError::Http(format!("invalid controller trust bundle sidecar: {error}"))
        })?;
    controller_signing_trust_bundle_from_wire(&sidecar.entries).map(Some)
}

fn write_agent_controller_trust_bundle_sidecar(
    path: &Path,
    bundle: &fleet_domain::ControllerSigningTrustBundle,
) -> Result<(), CliError> {
    let sidecar = AgentControllerTrustBundleSidecar {
        entries: controller_signing_trust_bundle_to_wire(bundle),
    };
    let body = serde_json::to_string_pretty(&sidecar).map_err(|error| {
        CliError::Http(format!(
            "serialize controller trust bundle sidecar: {error}"
        ))
    })?;
    write_secure_file_atomic(path, &format!("{body}\n"))?;
    Ok(())
}

fn validate_pinned_controller_identity(
    config: &LocalAgentConfig,
    identity: &fleet_controller::ControllerIdentityResponse,
) -> Result<(), CliError> {
    let observed_fingerprint = controller_signing_fingerprint(identity);
    if observed_fingerprint != config.controller_fingerprint {
        return Err(CliError::Http(format!(
            "controller signing fingerprint changed from {} to {}; explicit re-enroll is required because this may indicate controller key rotation or a security issue",
            config.controller_fingerprint, observed_fingerprint
        )));
    }
    Ok(())
}

fn controller_signing_fingerprint(identity: &fleet_controller::ControllerIdentityResponse) -> &str {
    if identity.controller_signing_fingerprint.is_empty() {
        &identity.controller_fingerprint
    } else {
        &identity.controller_signing_fingerprint
    }
}

fn controller_signing_public_key(identity: &fleet_controller::ControllerIdentityResponse) -> &str {
    if identity.controller_signing_public_key.is_empty() {
        &identity.controller_public_key
    } else {
        &identity.controller_signing_public_key
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentSessionEnd {
    ControllerClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct AgentSessionTickActions {
    heartbeat: bool,
    facts: bool,
    metrics: bool,
    log: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AgentSessionInbound {
    Message(Box<fleet_protocol::WireMessage>),
    Idle,
    Closed,
}

fn run_agent_session_loop(
    config: &LocalAgentConfig,
    options: AgentHeartbeatOptions,
) -> Result<(), CliError> {
    let mut session_runtime = agent_session_runtime(config)?;
    run_agent_session_loop_with_state(
        &mut session_runtime,
        options,
        |runtime| run_agent_session_once(config, options, runtime),
        std::thread::sleep,
    )
}

fn run_agent_session_loop_with<F, S>(
    options: AgentHeartbeatOptions,
    mut session_once: F,
    mut sleep: S,
) -> Result<(), CliError>
where
    F: FnMut() -> Result<AgentSessionEnd, CliError>,
    S: FnMut(Duration),
{
    let mut reconnect_attempts = 0;
    loop {
        match session_once() {
            Ok(AgentSessionEnd::ControllerClosed) => {
                reconnect_attempts = 0;
                println!("agent session closed by controller");
                if options.once {
                    return Ok(());
                }
                sleep(options.heartbeat_interval);
            }
            Err(error) => {
                if is_fatal_agent_session_error(&error) {
                    return Err(error);
                }
                reconnect_attempts += 1;
                if options.once
                    || (options.max_reconnect_attempts != 0
                        && reconnect_attempts > options.max_reconnect_attempts)
                {
                    return Err(error);
                }
                eprintln!(
                    "{}",
                    format_warning_message(format!("agent session failed: {error}"))
                );
                sleep(reconnect_backoff(reconnect_attempts));
            }
        }
    }
}

fn run_agent_session_loop_with_state<T, F, S>(
    state: &mut T,
    options: AgentHeartbeatOptions,
    mut session_once: F,
    sleep: S,
) -> Result<(), CliError>
where
    F: FnMut(&mut T) -> Result<AgentSessionEnd, CliError>,
    S: FnMut(Duration),
{
    run_agent_session_loop_with(options, || session_once(state), sleep)
}

#[cfg(test)]
fn run_agent_session_loop_with_state_for_test<T, F, S>(
    state: &mut T,
    options: AgentHeartbeatOptions,
    session_once: F,
    sleep: S,
) -> Result<(), CliError>
where
    F: FnMut(&mut T) -> Result<AgentSessionEnd, CliError>,
    S: FnMut(Duration),
{
    run_agent_session_loop_with_state(state, options, session_once, sleep)
}

fn is_fatal_agent_session_error(error: &CliError) -> bool {
    matches!(
        error,
        CliError::Http(message) if message.contains("controller signing fingerprint changed")
    )
}

fn agent_session_tick_actions(
    heartbeat_elapsed: Duration,
    facts_elapsed: Duration,
    metrics_elapsed: Duration,
    log_elapsed: Duration,
    options: AgentHeartbeatOptions,
) -> AgentSessionTickActions {
    AgentSessionTickActions {
        heartbeat: heartbeat_elapsed >= options.heartbeat_interval,
        facts: facts_elapsed >= options.facts_interval,
        metrics: metrics_elapsed >= options.metrics_interval,
        log: should_upload_agent_log(options.log_upload, log_elapsed),
    }
}

#[cfg(test)]
fn run_agent_heartbeat_loop_with<F, S>(
    options: AgentHeartbeatOptions,
    mut heartbeat_once: F,
    mut sleep: S,
) -> Result<(), CliError>
where
    F: FnMut(bool) -> Result<(), CliError>,
    S: FnMut(Duration),
{
    let mut reconnect_attempts = 0;
    let mut elapsed_since_log_upload = options.log_upload.interval;
    loop {
        let upload_log = should_upload_agent_log(options.log_upload, elapsed_since_log_upload);
        match heartbeat_once(upload_log) {
            Ok(()) => {
                reconnect_attempts = 0;
                println!("agent heartbeat sent");
                if options.once {
                    return Ok(());
                }
                elapsed_since_log_upload = if upload_log {
                    Duration::ZERO
                } else {
                    elapsed_since_log_upload.saturating_add(options.heartbeat_interval)
                };
                sleep(options.heartbeat_interval);
            }
            Err(error) => {
                reconnect_attempts += 1;
                if options.once
                    || (options.max_reconnect_attempts != 0
                        && reconnect_attempts > options.max_reconnect_attempts)
                {
                    return Err(error);
                }
                eprintln!(
                    "{}",
                    format_warning_message(format!("agent heartbeat failed: {error}"))
                );
                sleep(reconnect_backoff(reconnect_attempts));
            }
        }
    }
}

fn should_upload_agent_log(options: AgentLogUploadOptions, elapsed: Duration) -> bool {
    options.enabled && elapsed >= options.interval
}

fn reconnect_backoff(attempt: u32) -> Duration {
    Duration::from_secs(2_u64.saturating_pow(attempt.min(5)))
}

fn run_agent_heartbeat_once(
    config: &LocalAgentConfig,
    upload_agent_log: bool,
) -> Result<(), CliError> {
    let identity = controller_identity_via_controller(&config.url, config.tls_ca_cert.as_deref())?;
    validate_pinned_controller_identity(config, &identity)?;
    let endpoint = parse_controller_url(&config.url)?;
    let ws_url = endpoint.websocket_url("/api/agents/ws");
    let (mut socket, _) =
        connect_agent_websocket(&ws_url, &endpoint, config.tls_ca_cert.as_deref())?;
    let correlation_id = prefixed_ulid("corr")?;

    let hello = fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.clone(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::AgentHello {
            agent_id: config.agent_id.clone(),
            fingerprint: config.fingerprint.clone(),
        },
    );
    socket
        .send(Message::Text(
            fleet_protocol::encode_message(&hello)
                .map_err(|error| CliError::Http(error.to_string()))?,
        ))
        .map_err(|error| CliError::Http(error.to_string()))?;

    let challenge = read_ws_message(&mut socket)?;
    let fleet_protocol::WirePayload::AuthChallenge { nonce } = challenge.payload else {
        return Err(CliError::Http("expected auth challenge".to_owned()));
    };

    let auth = fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.clone(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::AuthResponse {
            nonce: nonce.clone(),
            signature: fleet_core::sign_challenge(&config.private_key, &nonce)?,
        },
    );
    socket
        .send(Message::Text(
            fleet_protocol::encode_message(&auth)
                .map_err(|error| CliError::Http(error.to_string()))?,
        ))
        .map_err(|error| CliError::Http(error.to_string()))?;

    let accepted = read_ws_message(&mut socket)?;
    if !matches!(accepted.payload, fleet_protocol::WirePayload::AuthAccepted) {
        return Err(CliError::Http("expected auth accepted".to_owned()));
    }
    set_agent_socket_read_timeout(&mut socket, Some(Duration::from_secs(10)))?;

    let heartbeat = fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.clone(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::Heartbeat {
            agent_id: config.agent_id.clone(),
            status: "online".to_owned(),
        },
    );
    socket
        .send(Message::Text(
            fleet_protocol::encode_message(&heartbeat)
                .map_err(|error| CliError::Http(error.to_string()))?,
        ))
        .map_err(|error| CliError::Http(error.to_string()))?;

    send_wire_message_to_socket(
        &mut socket,
        &agent_capability_snapshot_message(config, &correlation_id)?,
    )?;
    send_facts_snapshot(&mut socket, config, &correlation_id)?;
    send_metrics_snapshot(&mut socket, config, &correlation_id)?;
    if upload_agent_log {
        send_agent_log_chunk(
            &mut socket,
            config,
            &correlation_id,
            &agent_operational_log_line(config),
        )?;
    }
    read_and_handle_task_assignment(
        &mut socket,
        config,
        controller_signing_public_key(&identity),
        &correlation_id,
    )?;

    Ok(())
}

type AgentWebSocket = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>;
type AgentOutboundQueue =
    Arc<Mutex<fleet_agent::AgentSessionSupervisor<fleet_protocol::WireMessage>>>;

#[derive(Default)]
struct AgentTaskRuntimeState {
    current_task_id: Mutex<Option<String>>,
    cancel_requested: AtomicBool,
}

#[derive(Clone)]
struct AgentTaskSessionState {
    busy: Arc<AtomicBool>,
    runtime: Arc<AgentTaskRuntimeState>,
    replay_guard: Arc<Mutex<fleet_runner::NonceReplayGuard>>,
    controller_trust_bundle: Arc<Mutex<Option<fleet_domain::ControllerSigningTrustBundle>>>,
}

struct AgentSessionRuntime {
    task_state: AgentTaskSessionState,
    connection: fleet_agent::AgentSessionSupervisor<()>,
    outbound_queue: AgentOutboundQueue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentControllerSigningTrustBundleUpdateOutcome {
    current_fingerprint: String,
    entries_count: usize,
}

impl AgentTaskRuntimeState {
    fn start_task(&self, task_id: &str) -> Result<(), CliError> {
        let mut current = self
            .current_task_id
            .lock()
            .map_err(|_| CliError::Http("agent task runtime lock poisoned".to_owned()))?;
        *current = Some(task_id.to_owned());
        self.cancel_requested.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn request_cancel(&self, task_id: &str) -> Result<bool, CliError> {
        let current = self
            .current_task_id
            .lock()
            .map_err(|_| CliError::Http("agent task runtime lock poisoned".to_owned()))?;
        if current.as_deref() == Some(task_id) {
            self.cancel_requested.store(true, Ordering::SeqCst);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn should_cancel(&self, task_id: &str) -> bool {
        if !self.cancel_requested.load(Ordering::SeqCst) {
            return false;
        }
        self.current_task_id
            .lock()
            .map(|current| current.as_deref() == Some(task_id))
            .unwrap_or(false)
    }

    fn finish_task(&self, task_id: &str) {
        if let Ok(mut current) = self.current_task_id.lock()
            && current.as_deref() == Some(task_id)
        {
            *current = None;
            self.cancel_requested.store(false, Ordering::SeqCst);
        }
    }
}

fn apply_agent_controller_signing_trust_bundle_update(
    task_state: &AgentTaskSessionState,
    entries: &[fleet_protocol::ControllerSigningTrustEntryWire],
    persistence_path: Option<&Path>,
) -> Result<AgentControllerSigningTrustBundleUpdateOutcome, CliError> {
    let bundle = controller_signing_trust_bundle_from_wire(entries)?;
    let current_fingerprint = bundle
        .entries()
        .iter()
        .find(|entry| entry.role() == fleet_domain::ControllerSigningTrustRole::Current)
        .map(|entry| entry.fingerprint().as_str().to_owned())
        .ok_or_else(|| {
            CliError::Http("controller signing trust bundle is missing current entry".to_owned())
        })?;
    let entries_count = bundle.entries().len();
    if let Some(path) = persistence_path {
        write_agent_controller_trust_bundle_sidecar(path, &bundle)?;
    }
    let mut runtime_bundle = task_state
        .controller_trust_bundle
        .lock()
        .map_err(|_| CliError::Http("agent controller trust bundle lock poisoned".to_owned()))?;
    *runtime_bundle = Some(bundle);
    Ok(AgentControllerSigningTrustBundleUpdateOutcome {
        current_fingerprint,
        entries_count,
    })
}

fn agent_task_session_state(config: &LocalAgentConfig) -> Result<AgentTaskSessionState, CliError> {
    Ok(AgentTaskSessionState {
        busy: Arc::new(AtomicBool::new(false)),
        runtime: Arc::new(AgentTaskRuntimeState::default()),
        replay_guard: Arc::new(Mutex::new(agent_nonce_replay_guard(config)?)),
        controller_trust_bundle: Arc::new(Mutex::new(config.controller_trust_bundle.clone())),
    })
}

fn agent_session_runtime(config: &LocalAgentConfig) -> Result<AgentSessionRuntime, CliError> {
    Ok(AgentSessionRuntime {
        task_state: agent_task_session_state(config)?,
        connection: fleet_agent::AgentSessionSupervisor::new(0),
        outbound_queue: Arc::new(Mutex::new(fleet_agent::AgentSessionSupervisor::new(
            AGENT_SESSION_OUTBOUND_QUEUE_CAPACITY,
        ))),
    })
}

fn agent_controller_signing_trust_bundle(
    task_state: &AgentTaskSessionState,
    config: &LocalAgentConfig,
    controller_public_key: &str,
) -> Result<fleet_domain::ControllerSigningTrustBundle, CliError> {
    let runtime_bundle = task_state
        .controller_trust_bundle
        .lock()
        .map_err(|_| CliError::Http("agent controller trust bundle lock poisoned".to_owned()))?
        .clone();
    runtime_bundle
        .map(Ok)
        .unwrap_or_else(|| legacy_controller_signing_trust_bundle(config, controller_public_key))
}

fn verify_agent_task_envelope_once_with_session_trust(
    envelope: &fleet_domain::TaskEnvelope,
    config: &LocalAgentConfig,
    controller_public_key: &str,
    task_state: &AgentTaskSessionState,
    now: SystemTime,
) -> Result<
    Result<fleet_domain::ControllerSigningTrustVerification, fleet_runner::RunnerError>,
    CliError,
> {
    let trust_bundle =
        agent_controller_signing_trust_bundle(task_state, config, controller_public_key)?;
    let verifier = ControllerSignatureVerifier;
    let agent_id = fleet_domain::AgentId::new(config.agent_id.clone())
        .map_err(|error| CliError::Http(error.to_string()))?;
    let mut replay_guard = task_state
        .replay_guard
        .lock()
        .map_err(|_| CliError::Http("agent nonce replay guard lock poisoned".to_owned()))?;
    Ok(
        fleet_runner::verify_signed_envelope_once_with_controller_trust(
            envelope,
            &agent_id,
            now,
            &trust_bundle,
            None,
            &verifier,
            &mut replay_guard,
        ),
    )
}

fn run_agent_session_once(
    config: &LocalAgentConfig,
    options: AgentHeartbeatOptions,
    runtime: &mut AgentSessionRuntime,
) -> Result<AgentSessionEnd, CliError> {
    runtime
        .connection
        .begin_connect()
        .map_err(|_| CliError::Http("invalid agent session connection transition".to_owned()))?;
    let result = run_agent_session_connected_once(config, options, runtime);
    runtime.connection.connection_lost();
    result
}

fn run_agent_session_connected_once(
    config: &LocalAgentConfig,
    options: AgentHeartbeatOptions,
    runtime: &mut AgentSessionRuntime,
) -> Result<AgentSessionEnd, CliError> {
    let identity = controller_identity_via_controller(&config.url, config.tls_ca_cert.as_deref())?;
    validate_pinned_controller_identity(config, &identity)?;
    let endpoint = parse_controller_url(&config.url)?;
    let ws_url = endpoint.websocket_url("/api/agents/ws");
    let (mut socket, _) =
        connect_agent_websocket(&ws_url, &endpoint, config.tls_ca_cert.as_deref())?;
    set_agent_socket_read_timeout(&mut socket, Some(AGENT_SESSION_READ_POLL_INTERVAL))?;
    let correlation_id = prefixed_ulid("corr")?;

    perform_agent_session_handshake(&mut socket, config, &correlation_id)?;
    runtime.connection.mark_authenticated().map_err(|_| {
        CliError::Http("invalid agent session authentication transition".to_owned())
    })?;

    enqueue_initial_agent_session_messages(
        &runtime.outbound_queue,
        config,
        &correlation_id,
        options,
    )?;

    let mut last_heartbeat = Instant::now();
    let mut last_facts = Instant::now();
    let mut last_metrics = Instant::now();
    let mut last_log = if options.log_upload.enabled {
        Instant::now()
    } else {
        Instant::now() - options.log_upload.interval
    };

    loop {
        flush_agent_outbound_queue(&mut socket, &runtime.outbound_queue)?;

        match read_agent_session_message(&mut socket)? {
            AgentSessionInbound::Message(message) => handle_agent_session_message(
                *message,
                config,
                controller_signing_public_key(&identity),
                &correlation_id,
                &runtime.outbound_queue,
                &runtime.task_state,
            )?,
            AgentSessionInbound::Idle => {}
            AgentSessionInbound::Closed => return Ok(AgentSessionEnd::ControllerClosed),
        }

        let now = Instant::now();
        let actions = agent_session_tick_actions(
            now.saturating_duration_since(last_heartbeat),
            now.saturating_duration_since(last_facts),
            now.saturating_duration_since(last_metrics),
            now.saturating_duration_since(last_log),
            options,
        );
        enqueue_agent_session_tick_messages(
            &runtime.outbound_queue,
            config,
            &correlation_id,
            actions,
        )?;
        if actions.heartbeat {
            last_heartbeat = now;
        }
        if actions.facts {
            last_facts = now;
        }
        if actions.metrics {
            last_metrics = now;
        }
        if actions.log {
            last_log = now;
        }
        flush_agent_outbound_queue(&mut socket, &runtime.outbound_queue)?;
    }
}

fn enqueue_agent_session_tick_messages(
    outbound_sender: &AgentOutboundQueue,
    config: &LocalAgentConfig,
    correlation_id: &str,
    actions: AgentSessionTickActions,
) -> Result<(), CliError> {
    if actions.heartbeat {
        enqueue_wire_message(
            outbound_sender,
            agent_heartbeat_message(config, correlation_id)?,
        )?;
    }
    if actions.facts {
        enqueue_wire_message(
            outbound_sender,
            agent_facts_snapshot_message(config, correlation_id)?,
        )?;
    }
    if actions.metrics {
        enqueue_wire_message(
            outbound_sender,
            agent_metrics_snapshot_message(config, correlation_id)?,
        )?;
    }
    if actions.log {
        enqueue_wire_message(
            outbound_sender,
            agent_log_chunk_message(config, correlation_id, &agent_operational_log_line(config))?,
        )?;
    }
    Ok(())
}

fn perform_agent_session_handshake(
    socket: &mut AgentWebSocket,
    config: &LocalAgentConfig,
    correlation_id: &str,
) -> Result<(), CliError> {
    send_wire_message_to_socket(
        socket,
        &fleet_protocol::WireMessage::new(
            prefixed_ulid("msg")?,
            correlation_id.to_owned(),
            Some(config.agent_id.clone()),
            epoch_millis() as u64,
            fleet_protocol::WirePayload::AgentHello {
                agent_id: config.agent_id.clone(),
                fingerprint: config.fingerprint.clone(),
            },
        ),
    )?;

    let challenge = read_ws_message(socket)?;
    let fleet_protocol::WirePayload::AuthChallenge { nonce } = challenge.payload else {
        return Err(CliError::Http("expected auth challenge".to_owned()));
    };

    send_wire_message_to_socket(
        socket,
        &fleet_protocol::WireMessage::new(
            prefixed_ulid("msg")?,
            correlation_id.to_owned(),
            Some(config.agent_id.clone()),
            epoch_millis() as u64,
            fleet_protocol::WirePayload::AuthResponse {
                nonce: nonce.clone(),
                signature: fleet_core::sign_challenge(&config.private_key, &nonce)?,
            },
        ),
    )?;

    let accepted = read_ws_message(socket)?;
    if !matches!(accepted.payload, fleet_protocol::WirePayload::AuthAccepted) {
        return Err(CliError::Http("expected auth accepted".to_owned()));
    }
    Ok(())
}

fn enqueue_initial_agent_session_messages(
    outbound_sender: &AgentOutboundQueue,
    config: &LocalAgentConfig,
    correlation_id: &str,
    options: AgentHeartbeatOptions,
) -> Result<(), CliError> {
    enqueue_wire_message(
        outbound_sender,
        agent_heartbeat_message(config, correlation_id)?,
    )?;
    enqueue_wire_message(
        outbound_sender,
        agent_capability_snapshot_message(config, correlation_id)?,
    )?;
    enqueue_wire_message(
        outbound_sender,
        agent_facts_snapshot_message(config, correlation_id)?,
    )?;
    enqueue_wire_message(
        outbound_sender,
        agent_metrics_snapshot_message(config, correlation_id)?,
    )?;
    if options.log_upload.enabled {
        enqueue_wire_message(
            outbound_sender,
            agent_log_chunk_message(config, correlation_id, &agent_operational_log_line(config))?,
        )?;
    }
    Ok(())
}

fn flush_agent_outbound_queue(
    socket: &mut AgentWebSocket,
    outbound_queue: &AgentOutboundQueue,
) -> Result<(), CliError> {
    flush_agent_outbound_queue_with(outbound_queue, |message| {
        send_wire_message_to_socket(socket, message)
    })
}

/// Gives the sole socket writer each queued report and removes it only after a successful write.
fn flush_agent_outbound_queue_with<F>(
    outbound_queue: &AgentOutboundQueue,
    mut write_message: F,
) -> Result<(), CliError>
where
    F: FnMut(&fleet_protocol::WireMessage) -> Result<(), CliError>,
{
    loop {
        let message = outbound_queue
            .lock()
            .map_err(|_| CliError::Http("agent outbound queue lock poisoned".to_owned()))?
            .pending_report()
            .cloned();
        let Some(message) = message else {
            return Ok(());
        };
        write_message(&message)?;
        let removed = outbound_queue
            .lock()
            .map_err(|_| CliError::Http("agent outbound queue lock poisoned".to_owned()))?
            .remove_pending_report();
        debug_assert!(removed.is_some());
    }
}

fn read_agent_session_message(
    socket: &mut AgentWebSocket,
) -> Result<AgentSessionInbound, CliError> {
    match socket.read() {
        Ok(message) if message.is_close() => Ok(AgentSessionInbound::Closed),
        Ok(message) => {
            let body = message
                .to_text()
                .map_err(|error| CliError::Http(error.to_string()))?;
            let message = fleet_protocol::decode_message(body)
                .map_err(|error| CliError::Http(error.to_string()))?;
            Ok(AgentSessionInbound::Message(Box::new(message)))
        }
        Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
            Ok(AgentSessionInbound::Closed)
        }
        Err(error) if agent_session_read_error_is_idle(&error) => Ok(AgentSessionInbound::Idle),
        Err(error) => Err(CliError::Http(error.to_string())),
    }
}

/// Classifies recoverable socket read interruptions without ending the Agent session.
fn agent_session_read_error_is_idle(error: &tungstenite::Error) -> bool {
    matches!(
        error,
        tungstenite::Error::Io(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::Interrupted
            )
    )
}

fn handle_agent_session_message(
    message: fleet_protocol::WireMessage,
    config: &LocalAgentConfig,
    controller_public_key: &str,
    correlation_id: &str,
    outbound_sender: &AgentOutboundQueue,
    task_state: &AgentTaskSessionState,
) -> Result<(), CliError> {
    let (envelope, task) = match message.payload {
        fleet_protocol::WirePayload::TaskAssignment { envelope, task } => (envelope, task),
        fleet_protocol::WirePayload::TaskCancel {
            job_id,
            task_id,
            reason,
        } => {
            let _ = job_id;
            let _ = reason;
            let _ = task_state.runtime.request_cancel(&task_id)?;
            return Ok(());
        }
        fleet_protocol::WirePayload::ControllerSigningTrustBundleUpdate { entries } => {
            match apply_agent_controller_signing_trust_bundle_update(
                task_state,
                &entries,
                Some(&config.controller_trust_bundle_path),
            ) {
                Ok(outcome) => {
                    enqueue_wire_message(
                        outbound_sender,
                        agent_controller_signing_trust_bundle_ack_message(
                            config,
                            correlation_id,
                            true,
                            Some(&outcome.current_fingerprint),
                            outcome.entries_count,
                            None,
                        )?,
                    )?;
                    send_agent_security_event_queue(
                        outbound_sender,
                        config,
                        correlation_id,
                        "controller_signing_trust_bundle_update_accepted",
                        "controller signing trust bundle update accepted",
                    )?;
                }
                Err(error) => {
                    enqueue_wire_message(
                        outbound_sender,
                        agent_controller_signing_trust_bundle_ack_message(
                            config,
                            correlation_id,
                            false,
                            None,
                            entries.len(),
                            Some("invalid_trust_bundle"),
                        )?,
                    )?;
                    send_agent_security_event_queue(
                        outbound_sender,
                        config,
                        correlation_id,
                        "controller_signing_trust_bundle_update_rejected",
                        &error.to_string(),
                    )?;
                }
            }
            return Ok(());
        }
        fleet_protocol::WirePayload::AgentCertificateLifecycleUpdate { state, .. } => {
            enqueue_wire_message(
                outbound_sender,
                agent_certificate_lifecycle_ack_message(
                    config,
                    correlation_id,
                    false,
                    state,
                    None,
                    Some(AGENT_CERTIFICATE_LIFECYCLE_RUNTIME_NOT_IMPLEMENTED),
                )?,
            )?;
            send_agent_security_event_queue(
                outbound_sender,
                config,
                correlation_id,
                "agent_certificate_lifecycle_update_rejected",
                "agent certificate lifecycle runtime handling is not implemented",
            )?;
            return Ok(());
        }
        _ => return Ok(()),
    };
    if task_state.busy.swap(true, Ordering::SeqCst) {
        enqueue_wire_message(
            outbound_sender,
            agent_task_rejected_message(
                config,
                correlation_id,
                &envelope.job_id,
                &envelope.task_id,
                fleet_protocol::TaskRejectionReasonCode::AgentBusy,
                "agent is busy",
            )?,
        )?;
        return Ok(());
    }
    task_state.runtime.start_task(&envelope.task_id)?;

    let worker_config = config.clone();
    let worker_public_key = controller_public_key.to_owned();
    let worker_correlation_id = correlation_id.to_owned();
    let worker_sender = outbound_sender.clone();
    let worker_task_state = task_state.clone();
    let worker_task_id = envelope.task_id.clone();
    thread::spawn(move || {
        if let Err(error) = handle_task_assignment_with_queue(
            &worker_sender,
            &worker_config,
            &worker_public_key,
            &worker_correlation_id,
            envelope,
            task,
            &worker_task_state,
        ) {
            let _ = enqueue_wire_message(
                &worker_sender,
                agent_security_event_message(
                    &worker_config,
                    &worker_correlation_id,
                    "task_worker_failed",
                    &error.to_string(),
                )
                .unwrap_or_else(|_| {
                    fleet_protocol::WireMessage::new(
                        "msg-task-worker-failed",
                        worker_correlation_id.clone(),
                        Some(worker_config.agent_id.clone()),
                        epoch_millis() as u64,
                        fleet_protocol::WirePayload::SecurityEvent {
                            agent_id: worker_config.agent_id.clone(),
                            action: "task_worker_failed".to_owned(),
                            detail: "task worker failed".to_owned(),
                        },
                    )
                }),
            );
        }
        worker_task_state.runtime.finish_task(&worker_task_id);
        worker_task_state.busy.store(false, Ordering::SeqCst);
    });
    Ok(())
}

fn set_agent_socket_read_timeout(
    socket: &mut AgentWebSocket,
    timeout: Option<Duration>,
) -> Result<(), CliError> {
    match socket.get_mut() {
        tungstenite::stream::MaybeTlsStream::Plain(stream) => stream.set_read_timeout(timeout)?,
        tungstenite::stream::MaybeTlsStream::Rustls(stream) => {
            stream.get_mut().set_read_timeout(timeout)?
        }
        #[allow(unreachable_patterns)]
        _ => {}
    }
    Ok(())
}

fn send_wire_message_to_socket(
    socket: &mut AgentWebSocket,
    message: &fleet_protocol::WireMessage,
) -> Result<(), CliError> {
    socket
        .send(Message::Text(
            fleet_protocol::encode_message(message)
                .map_err(|error| CliError::Http(error.to_string()))?,
        ))
        .map_err(|error| CliError::Http(error.to_string()))
}

fn enqueue_wire_message(
    outbound_sender: &AgentOutboundQueue,
    message: fleet_protocol::WireMessage,
) -> Result<(), CliError> {
    outbound_sender
        .lock()
        .map_err(|_| CliError::Http("agent outbound queue lock poisoned".to_owned()))?
        .enqueue_report(message)
        .map_err(|error| match error {
            fleet_agent::AgentSessionSupervisorError::PendingReportsFull => {
                CliError::Http("agent outbound queue is full".to_owned())
            }
            _ => CliError::Http("invalid agent outbound queue transition".to_owned()),
        })
}

fn agent_heartbeat_message(
    config: &LocalAgentConfig,
    correlation_id: &str,
) -> Result<fleet_protocol::WireMessage, CliError> {
    Ok(fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::Heartbeat {
            agent_id: config.agent_id.clone(),
            status: "online".to_owned(),
        },
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AgentCapabilityProbe {
    privilege_level: fleet_protocol::CapabilityPrivilegeLevelWire,
    package_manager: Option<fleet_protocol::PackageManagerWire>,
    service_manager: Option<fleet_protocol::ServiceManagerWire>,
}

fn agent_capability_snapshot_message(
    config: &LocalAgentConfig,
    correlation_id: &str,
) -> Result<fleet_protocol::WireMessage, CliError> {
    let probe = collect_agent_capability_probe();
    Ok(fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::CapabilitySnapshot {
            agent_id: config.agent_id.clone(),
            privilege_level: probe.privilege_level,
            package_manager: probe.package_manager,
            service_manager: probe.service_manager,
            capabilities: agent_capability_names(&probe),
            reported_at_ms: epoch_millis() as u64,
        },
    ))
}

fn collect_agent_capability_probe() -> AgentCapabilityProbe {
    AgentCapabilityProbe {
        privilege_level: fleet_protocol::CapabilityPrivilegeLevelWire::Unprivileged,
        package_manager: fleet_runner::detect_local_linux_package_manager()
            .map(package_manager_to_wire),
        service_manager: detect_local_service_manager(),
    }
}

fn agent_capability_names(probe: &AgentCapabilityProbe) -> Vec<String> {
    let mut capabilities = vec![
        "persistent_session".to_owned(),
        "command_execution".to_owned(),
        "drift_check".to_owned(),
        "runbook_execution".to_owned(),
    ];
    if probe.package_manager.is_some() {
        capabilities.push("package_install".to_owned());
    }
    if probe.service_manager.is_some() {
        capabilities.push("service_control".to_owned());
    }
    capabilities
}

fn package_manager_to_wire(
    manager: fleet_runner::LinuxPackageManager,
) -> fleet_protocol::PackageManagerWire {
    match manager {
        fleet_runner::LinuxPackageManager::Apt => fleet_protocol::PackageManagerWire::Apt,
        fleet_runner::LinuxPackageManager::Dnf => fleet_protocol::PackageManagerWire::Dnf,
        fleet_runner::LinuxPackageManager::Yum => fleet_protocol::PackageManagerWire::Yum,
        fleet_runner::LinuxPackageManager::Apk => fleet_protocol::PackageManagerWire::Apk,
    }
}

fn detect_local_service_manager() -> Option<fleet_protocol::ServiceManagerWire> {
    if Path::new("/run/systemd/system").exists()
        || Path::new("/usr/bin/systemctl").exists()
        || Path::new("/bin/systemctl").exists()
    {
        Some(fleet_protocol::ServiceManagerWire::Systemd)
    } else {
        None
    }
}

fn agent_facts_snapshot_message(
    config: &LocalAgentConfig,
    correlation_id: &str,
) -> Result<fleet_protocol::WireMessage, CliError> {
    Ok(fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::FactsSnapshot {
            agent_id: config.agent_id.clone(),
            body: collect_local_facts().to_string(),
        },
    ))
}

fn agent_metrics_snapshot_message(
    config: &LocalAgentConfig,
    correlation_id: &str,
) -> Result<fleet_protocol::WireMessage, CliError> {
    Ok(fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::MetricsSnapshot {
            agent_id: config.agent_id.clone(),
            body: collect_local_metrics().to_string(),
        },
    ))
}

fn agent_log_chunk_message(
    config: &LocalAgentConfig,
    correlation_id: &str,
    line: &str,
) -> Result<fleet_protocol::WireMessage, CliError> {
    Ok(fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::LogChunk {
            agent_id: config.agent_id.clone(),
            line: redact_and_truncate_log_line(line),
        },
    ))
}

fn agent_controller_signing_trust_bundle_ack_message(
    config: &LocalAgentConfig,
    correlation_id: &str,
    accepted: bool,
    current_fingerprint: Option<&str>,
    entries_count: usize,
    reason_code: Option<&str>,
) -> Result<fleet_protocol::WireMessage, CliError> {
    Ok(fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::ControllerSigningTrustBundleAck {
            agent_id: config.agent_id.clone(),
            accepted,
            current_fingerprint: current_fingerprint.map(str::to_owned),
            entries_count,
            reason_code: reason_code.map(str::to_owned),
        },
    ))
}

fn agent_certificate_lifecycle_ack_message(
    config: &LocalAgentConfig,
    correlation_id: &str,
    accepted: bool,
    state: fleet_protocol::AgentCertificateLifecycleStateWire,
    current_fingerprint: Option<&str>,
    reason_code: Option<&str>,
) -> Result<fleet_protocol::WireMessage, CliError> {
    Ok(fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::AgentCertificateLifecycleAck {
            agent_id: config.agent_id.clone(),
            accepted,
            state,
            current_fingerprint: current_fingerprint.map(str::to_owned),
            reason_code: reason_code.map(str::to_owned),
        },
    ))
}

fn agent_security_event_message(
    config: &LocalAgentConfig,
    correlation_id: &str,
    action: &str,
    detail: &str,
) -> Result<fleet_protocol::WireMessage, CliError> {
    Ok(fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::SecurityEvent {
            agent_id: config.agent_id.clone(),
            action: action.to_owned(),
            detail: detail.to_owned(),
        },
    ))
}

fn send_agent_log_chunk(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    config: &LocalAgentConfig,
    correlation_id: &str,
    line: &str,
) -> Result<(), CliError> {
    let message = fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::LogChunk {
            agent_id: config.agent_id.clone(),
            line: redact_and_truncate_log_line(line),
        },
    );
    socket
        .send(Message::Text(
            fleet_protocol::encode_message(&message)
                .map_err(|error| CliError::Http(error.to_string()))?,
        ))
        .map_err(|error| CliError::Http(error.to_string()))?;
    Ok(())
}

fn agent_operational_log_line(config: &LocalAgentConfig) -> String {
    format!(
        "level=info event=agent_heartbeat_completed agent_id={} status=online",
        config.agent_id
    )
}

fn send_facts_snapshot(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    config: &LocalAgentConfig,
    correlation_id: &str,
) -> Result<(), CliError> {
    let body = collect_local_facts().to_string();
    let message = fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::FactsSnapshot {
            agent_id: config.agent_id.clone(),
            body,
        },
    );
    socket
        .send(Message::Text(
            fleet_protocol::encode_message(&message)
                .map_err(|error| CliError::Http(error.to_string()))?,
        ))
        .map_err(|error| CliError::Http(error.to_string()))?;
    Ok(())
}

fn send_metrics_snapshot(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    config: &LocalAgentConfig,
    correlation_id: &str,
) -> Result<(), CliError> {
    let body = collect_local_metrics().to_string();
    let message = fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::MetricsSnapshot {
            agent_id: config.agent_id.clone(),
            body,
        },
    );
    socket
        .send(Message::Text(
            fleet_protocol::encode_message(&message)
                .map_err(|error| CliError::Http(error.to_string()))?,
        ))
        .map_err(|error| CliError::Http(error.to_string()))?;
    Ok(())
}

fn collect_local_facts() -> serde_json::Value {
    let system_time_ms = epoch_millis() as u64;
    let meminfo = read_optional_trimmed("/proc/meminfo");
    let network_body = read_optional_trimmed("/proc/net/dev");
    let network_interfaces = network_body
        .as_deref()
        .map(linux_network_interfaces)
        .unwrap_or_default();
    let memory_total_kb = meminfo
        .as_deref()
        .and_then(|body| linux_meminfo_kb(body, "MemTotal"));
    let memory_modules = collect_memory_module_inventory();
    let root_disk = collect_root_disk_usage();
    let mount_body = read_optional_trimmed("/proc/mounts");
    let mounts = mount_body.as_deref().map(parse_linux_mounts);
    let root_mount = mounts
        .as_ref()
        .and_then(|mounts| mounts.iter().find(|mount| mount.mount_point == "/"));
    let block_devices = collect_linux_block_devices(Path::new("/sys/block"));
    let mut degraded_signals = Vec::new();
    if memory_total_kb.is_none() {
        degraded_signals.push("memory_facts_unavailable");
    }
    if network_body.is_none() {
        degraded_signals.push("network_facts_unavailable");
    }
    if root_disk.is_none() && mounts.is_none() && block_devices.is_none() {
        degraded_signals.push("disk_inventory_unavailable");
    }

    serde_json::json!({
        "system_time_ms": system_time_ms,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "family": std::env::consts::FAMILY,
        "hostname": read_optional_trimmed("/proc/sys/kernel/hostname")
            .or_else(|| read_optional_trimmed("/etc/hostname")),
        "runtime": {
            "pid": std::process::id(),
            "executable": "fleet",
        },
        "cpu": {
            "logical_count": std::thread::available_parallelism()
                .map(|value| value.get())
                .unwrap_or(0),
        },
        "memory": {
            "total_kb": memory_total_kb,
            "module_count_known": memory_modules.is_some(),
            "module_count": memory_modules.as_ref().map(|inventory| inventory.count),
            "module_count_source": memory_modules.as_ref().map(|inventory| inventory.source),
        },
        "disk": {
            "device_inventory_known": block_devices.is_some(),
            "device_count": block_devices.as_ref().map(Vec::len),
            "devices": block_devices.unwrap_or_default(),
            "mount_inventory_known": mounts.is_some(),
            "mount_count": mounts.as_ref().map(Vec::len),
            "mounts": mounts.clone().unwrap_or_default(),
            "root_mount_known": root_mount.is_some(),
            "root_source": root_mount.map(|mount| mount.source.as_str()),
            "root_fs_type": root_mount.map(|mount| mount.fs_type.as_str()),
            "root_capacity_known": root_disk.is_some(),
            "root_filesystem": root_disk.as_ref().map(|usage| usage.filesystem.as_str()),
            "root_total_kb": root_disk.as_ref().map(|usage| usage.total_kb),
        },
        "network": {
            "interfaces": network_interfaces,
        },
        "degraded": {
            "status": !degraded_signals.is_empty(),
            "signals": degraded_signals,
        },
    })
}

fn collect_local_metrics() -> serde_json::Value {
    let system_time_ms = epoch_millis() as u64;
    let meminfo = read_optional_trimmed("/proc/meminfo");
    let memory_usage = meminfo.as_deref().and_then(memory_usage_from_meminfo);
    let cpu_usage_percent = collect_cpu_usage_percent();
    let disk_usage = collect_root_disk_usage();
    let service_summary = collect_systemd_service_summary();

    serde_json::json!({
        "system_time_ms": system_time_ms,
        "cpu": {
            "logical_count": std::thread::available_parallelism()
                .map(|value| value.get())
                .unwrap_or(0),
            "usage_percent": cpu_usage_percent,
        },
        "memory": {
            "usage_available": memory_usage.is_some(),
            "total_kb": memory_usage.as_ref().map(|usage| usage.total_kb),
            "used_kb": memory_usage.as_ref().map(|usage| usage.used_kb),
            "available_kb": memory_usage.as_ref().map(|usage| usage.available_kb),
            "used_percent": memory_usage.as_ref().map(|usage| usage.used_percent),
        },
        "disk": {
            "usage_available": disk_usage.is_some(),
            "filesystem": disk_usage.as_ref().map(|usage| usage.filesystem.as_str()),
            "total_kb": disk_usage.as_ref().map(|usage| usage.total_kb),
            "used_kb": disk_usage.as_ref().map(|usage| usage.used_kb),
            "available_kb": disk_usage.as_ref().map(|usage| usage.available_kb),
            "used_percent": disk_usage.as_ref().map(|usage| usage.used_percent),
        },
        "process": {
            "pid": std::process::id(),
            "count": linux_proc_process_count("/proc"),
        },
        "service": {
            "status_available": service_summary.status_available,
            "failed_units_count": service_summary.failed_units_count,
            "failed_units": service_summary.failed_units,
        },
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiskUsage {
    filesystem: String,
    total_kb: u64,
    used_kb: u64,
    available_kb: u64,
    used_percent: u8,
}

fn collect_root_disk_usage() -> Option<DiskUsage> {
    let output = ProcessCommand::new("df").arg("-k").arg("/").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let body = String::from_utf8_lossy(&output.stdout);
    parse_df_root_usage(&body)
}

fn parse_df_root_usage(body: &str) -> Option<DiskUsage> {
    let line = body.lines().find(|line| {
        line.split_whitespace()
            .last()
            .is_some_and(|mount| mount == "/")
    })?;
    let parts = line.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 6 {
        return None;
    }
    Some(DiskUsage {
        filesystem: parts.first()?.to_string(),
        total_kb: parts.get(1)?.parse().ok()?,
        used_kb: parts.get(2)?.parse().ok()?,
        available_kb: parts.get(3)?.parse().ok()?,
        used_percent: parts.get(4)?.trim_end_matches('%').parse().ok()?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct BlockDeviceFact {
    name: String,
    kind: String,
    size_kb: Option<u64>,
    removable: Option<bool>,
    rotational: Option<bool>,
    partitions: Vec<BlockPartitionFact>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct BlockPartitionFact {
    name: String,
    size_kb: Option<u64>,
}

fn collect_linux_block_devices(path: &Path) -> Option<Vec<BlockDeviceFact>> {
    let entries = fs::read_dir(path).ok()?;
    let mut devices = entries
        .filter_map(Result::ok)
        .filter_map(|entry| linux_block_device_from_sysfs_entry(&entry.path()))
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| left.name.cmp(&right.name));
    Some(devices)
}

fn linux_block_device_from_sysfs_entry(path: &Path) -> Option<BlockDeviceFact> {
    let name = path.file_name()?.to_string_lossy().to_string();
    if is_ignored_linux_block_device(&name) {
        return None;
    }
    Some(BlockDeviceFact {
        kind: linux_block_device_kind(&name).to_owned(),
        size_kb: read_optional_trimmed_path(&path.join("size"))
            .and_then(|value| value.parse::<u64>().ok())
            .map(|sectors| sectors / 2),
        removable: read_linux_bool_file(&path.join("removable")),
        rotational: read_linux_bool_file(&path.join("queue").join("rotational")),
        partitions: linux_block_device_partitions(path, &name),
        name,
    })
}

fn linux_block_device_partitions(path: &Path, device_name: &str) -> Vec<BlockPartitionFact> {
    let mut partitions = fs::read_dir(path)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .filter_map(|entry| {
            let partition_path = entry.path();
            let name = partition_path.file_name()?.to_string_lossy().to_string();
            if !linux_partition_name_matches(device_name, &name) {
                return None;
            }
            Some(BlockPartitionFact {
                size_kb: read_optional_trimmed_path(&partition_path.join("size"))
                    .and_then(|value| value.parse::<u64>().ok())
                    .map(|sectors| sectors / 2),
                name,
            })
        })
        .collect::<Vec<_>>();
    partitions.sort_by(|left, right| left.name.cmp(&right.name));
    partitions
}

fn linux_partition_name_matches(device_name: &str, partition_name: &str) -> bool {
    if partition_name == device_name {
        return false;
    }
    let Some(suffix) = partition_name.strip_prefix(device_name) else {
        return false;
    };
    let suffix = if device_name.starts_with("nvme") || device_name.starts_with("mmcblk") {
        suffix.strip_prefix('p').unwrap_or("")
    } else {
        suffix
    };
    !suffix.is_empty() && suffix.chars().all(|character| character.is_ascii_digit())
}

fn is_ignored_linux_block_device(name: &str) -> bool {
    name.starts_with("loop") || name.starts_with("ram") || name.starts_with("zram")
}

fn linux_block_device_kind(name: &str) -> &'static str {
    if name.starts_with("nvme")
        || name.starts_with("sd")
        || name.starts_with("vd")
        || name.starts_with("xvd")
        || name.starts_with("hd")
        || name.starts_with("mmcblk")
    {
        "disk"
    } else {
        "block"
    }
}

fn read_linux_bool_file(path: &Path) -> Option<bool> {
    read_optional_trimmed_path(path).and_then(|value| match value.as_str() {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct MountFact {
    source: String,
    mount_point: String,
    fs_type: String,
    read_only: bool,
}

fn parse_linux_mounts(body: &str) -> Vec<MountFact> {
    body.lines()
        .filter_map(parse_linux_mount_line)
        .collect::<Vec<_>>()
}

fn parse_linux_mount_line(line: &str) -> Option<MountFact> {
    let mut parts = line.split_whitespace();
    let source = decode_linux_mount_field(parts.next()?);
    let mount_point = decode_linux_mount_field(parts.next()?);
    let fs_type = decode_linux_mount_field(parts.next()?);
    let options = parts.next().unwrap_or("");
    Some(MountFact {
        source,
        mount_point,
        fs_type,
        read_only: options.split(',').any(|option| option == "ro"),
    })
}

fn decode_linux_mount_field(value: &str) -> String {
    value
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MemoryUsage {
    total_kb: u64,
    used_kb: u64,
    available_kb: u64,
    used_percent: u8,
}

fn memory_usage_from_meminfo(body: &str) -> Option<MemoryUsage> {
    let total_kb = linux_meminfo_kb(body, "MemTotal")?;
    let available_kb = linux_meminfo_kb(body, "MemAvailable")?;
    let used_kb = total_kb.saturating_sub(available_kb);
    let used_percent = used_kb
        .saturating_mul(100)
        .checked_div(total_kb)
        .unwrap_or(0)
        .min(100) as u8;
    Some(MemoryUsage {
        total_kb,
        used_kb,
        available_kb,
        used_percent,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MemoryModuleInventory {
    count: usize,
    source: &'static str,
}

fn collect_memory_module_inventory() -> Option<MemoryModuleInventory> {
    linux_edac_memory_module_count(Path::new("/sys/devices/system/edac/mc"))
        .map(|count| MemoryModuleInventory {
            count,
            source: "linux_edac",
        })
        .or_else(|| {
            linux_dmi_memory_device_count(Path::new("/sys/firmware/dmi/entries")).map(|count| {
                MemoryModuleInventory {
                    count,
                    source: "linux_dmi_type17",
                }
            })
        })
}

fn linux_edac_memory_module_count(path: &Path) -> Option<usize> {
    let mut count = 0usize;
    let controllers = fs::read_dir(path).ok()?;
    for controller in controllers.filter_map(Result::ok) {
        let controller_name = controller.file_name();
        if !controller_name.to_string_lossy().starts_with("mc") {
            continue;
        }
        let Ok(entries) = fs::read_dir(controller.path()) else {
            continue;
        };
        count += entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("dimm"))
            .count();
    }
    (count > 0).then_some(count)
}

fn linux_dmi_memory_device_count(path: &Path) -> Option<usize> {
    let count = fs::read_dir(path)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("17-"))
        .count();
    (count > 0).then_some(count)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CpuSample {
    idle: u64,
    total: u64,
}

fn collect_cpu_usage_percent() -> Option<f64> {
    let first = read_linux_cpu_sample("/proc/stat")?;
    thread::sleep(Duration::from_millis(100));
    let second = read_linux_cpu_sample("/proc/stat")?;
    cpu_usage_percent_between(first, second)
}

fn read_linux_cpu_sample(path: &str) -> Option<CpuSample> {
    let body = fs::read_to_string(path).ok()?;
    parse_linux_cpu_sample(&body)
}

fn parse_linux_cpu_sample(body: &str) -> Option<CpuSample> {
    let line = body.lines().find(|line| line.starts_with("cpu "))?;
    let values = line
        .split_whitespace()
        .skip(1)
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if values.len() < 4 {
        return None;
    }
    let idle = values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0);
    let total = values.iter().copied().sum();
    Some(CpuSample { idle, total })
}

fn cpu_usage_percent_between(first: CpuSample, second: CpuSample) -> Option<f64> {
    let total_delta = second.total.checked_sub(first.total)?;
    if total_delta == 0 {
        return None;
    }
    let idle_delta = second.idle.saturating_sub(first.idle).min(total_delta);
    let busy_delta = total_delta - idle_delta;
    Some(((busy_delta as f64 / total_delta as f64) * 1000.0).round() / 10.0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServiceSummary {
    status_available: bool,
    failed_units_count: Option<usize>,
    failed_units: Vec<String>,
}

fn collect_systemd_service_summary() -> ServiceSummary {
    let output = ProcessCommand::new("systemctl")
        .args([
            "--no-pager",
            "--plain",
            "--state=failed",
            "--type=service",
            "list-units",
        ])
        .output();
    let Ok(output) = output else {
        return systemd_service_summary_unavailable();
    };
    if !output.status.success() {
        return systemd_service_summary_unavailable();
    }
    let body = String::from_utf8_lossy(&output.stdout);
    let failed_units = parse_systemd_failed_services(&body);
    ServiceSummary {
        status_available: true,
        failed_units_count: Some(failed_units.len()),
        failed_units,
    }
}

fn systemd_service_summary_unavailable() -> ServiceSummary {
    ServiceSummary {
        status_available: false,
        failed_units_count: None,
        failed_units: Vec::new(),
    }
}

fn parse_systemd_failed_services(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|unit| unit.ends_with(".service"))
        .map(ToOwned::to_owned)
        .collect()
}

fn read_optional_trimmed(path: &str) -> Option<String> {
    read_optional_trimmed_path(Path::new(path))
}

fn read_optional_trimmed_path(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn linux_meminfo_kb(body: &str, key: &str) -> Option<u64> {
    body.lines().find_map(|line| {
        let (name, rest) = line.split_once(':')?;
        if name != key {
            return None;
        }
        rest.split_whitespace().next()?.parse().ok()
    })
}

fn linux_network_interfaces(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| {
            let (name, _stats) = line.split_once(':')?;
            let name = name.trim();
            if name.is_empty() {
                None
            } else {
                Some(name.to_owned())
            }
        })
        .collect()
}

fn linux_proc_process_count(path: &str) -> Option<usize> {
    let entries = fs::read_dir(path).ok()?;
    Some(
        entries
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .chars()
                    .all(|character| character.is_ascii_digit())
            })
            .count(),
    )
}

fn read_and_handle_task_assignment(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    config: &LocalAgentConfig,
    controller_public_key: &str,
    correlation_id: &str,
) -> Result<(), CliError> {
    let message = match socket.read() {
        Ok(message) => message,
        Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
            return Ok(());
        }
        Err(error) => return Err(CliError::Http(error.to_string())),
    };
    if message.is_close() {
        return Ok(());
    }
    let body = message
        .to_text()
        .map_err(|error| CliError::Http(error.to_string()))?;
    let message =
        fleet_protocol::decode_message(body).map_err(|error| CliError::Http(error.to_string()))?;
    let fleet_protocol::WirePayload::TaskAssignment { envelope, task } = message.payload else {
        return Ok(());
    };
    let envelope = task_envelope_from_wire(envelope)?;
    let trust_bundle = legacy_controller_signing_trust_bundle(config, controller_public_key)?;
    let verifier = ControllerSignatureVerifier;
    let agent_id = fleet_domain::AgentId::new(config.agent_id.clone())
        .map_err(|error| CliError::Http(error.to_string()))?;
    let mut replay_guard = match agent_nonce_replay_guard(config) {
        Ok(replay_guard) => replay_guard,
        Err(error) => {
            send_agent_task_rejected(
                socket,
                config,
                correlation_id,
                &envelope,
                fleet_protocol::TaskRejectionReasonCode::InternalError,
                &error.to_string(),
            )?;
            return Ok(());
        }
    };
    if let Err(error) = fleet_runner::verify_signed_envelope_once_with_controller_trust(
        &envelope,
        &agent_id,
        SystemTime::now(),
        &trust_bundle,
        None,
        &verifier,
        &mut replay_guard,
    ) {
        send_agent_task_rejected(
            socket,
            config,
            correlation_id,
            &envelope,
            task_rejection_reason_from_runner_error(&error),
            &error.to_string(),
        )?;
        return Ok(());
    }
    send_agent_task_ack(socket, config, correlation_id, &envelope)?;

    match task {
        fleet_protocol::TaskWire::Command(command) => {
            run_signed_command_task(socket, config, correlation_id, &envelope, command)?;
        }
        fleet_protocol::TaskWire::DriftCheck(task) => {
            run_signed_drift_check_task(socket, config, correlation_id, &envelope, task)?;
        }
        fleet_protocol::TaskWire::RunbookExecution(task) => {
            run_signed_runbook_task(socket, config, correlation_id, &envelope, task)?;
        }
    }
    Ok(())
}

fn handle_task_assignment_with_queue(
    outbound_sender: &AgentOutboundQueue,
    config: &LocalAgentConfig,
    controller_public_key: &str,
    correlation_id: &str,
    envelope: fleet_protocol::SignedTaskEnvelopeWire,
    task: fleet_protocol::TaskWire,
    task_state: &AgentTaskSessionState,
) -> Result<(), CliError> {
    let envelope = task_envelope_from_wire(envelope)?;
    let verification = verify_agent_task_envelope_once_with_session_trust(
        &envelope,
        config,
        controller_public_key,
        task_state,
        SystemTime::now(),
    )?;
    if let Err(error) = verification {
        send_agent_task_rejected_queue(
            outbound_sender,
            config,
            correlation_id,
            &envelope,
            task_rejection_reason_from_runner_error(&error),
            &error.to_string(),
        )?;
        return Ok(());
    }
    if task_state.runtime.should_cancel(envelope.task_id.as_str()) {
        send_agent_task_result_queue(
            outbound_sender,
            config,
            correlation_id,
            &envelope,
            -1,
            fleet_protocol::TaskResultStatus::Canceled,
            "operator requested cancel",
        )?;
        return Ok(());
    }
    send_agent_task_ack_queue(outbound_sender, config, correlation_id, &envelope)?;

    match task {
        fleet_protocol::TaskWire::Command(command) => {
            run_signed_command_task_queue(
                outbound_sender,
                config,
                correlation_id,
                &envelope,
                command,
                task_state.runtime.as_ref(),
            )?;
        }
        fleet_protocol::TaskWire::DriftCheck(task) => {
            run_signed_drift_check_task_queue(
                outbound_sender,
                config,
                correlation_id,
                &envelope,
                task,
            )?;
        }
        fleet_protocol::TaskWire::RunbookExecution(task) => {
            run_signed_runbook_task_queue(
                outbound_sender,
                config,
                correlation_id,
                &envelope,
                task,
            )?;
        }
    }
    Ok(())
}

fn run_signed_command_task_queue(
    outbound_sender: &AgentOutboundQueue,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
    command: fleet_protocol::CommandTaskWire,
    task_runtime: &AgentTaskRuntimeState,
) -> Result<(), CliError> {
    let mut spec = fleet_runner::CommandSpec::new(
        command.program,
        command.args,
        Duration::from_millis(command.timeout_ms),
    );
    spec.max_output_bytes = command.max_output_bytes;
    send_agent_task_started_queue(outbound_sender, config, correlation_id, envelope)?;
    let mut streamed_any = false;
    let task_id = envelope.task_id.as_str().to_owned();
    let result = fleet_runner::run_command_streaming_with_cancel(
        spec,
        || task_runtime.should_cancel(&task_id),
        |chunk| {
            streamed_any = true;
            send_agent_output_chunk_queue(outbound_sender, config, correlation_id, envelope, chunk)
                .map_err(|error| fleet_runner::RunnerError::Stream(error.to_string()))
        },
    );
    let (output, result_status, reason) = command_execution_result(result);
    if !streamed_any && output.exit_code != 0 && !output.stderr.is_empty() {
        send_agent_output_chunk_queue(
            outbound_sender,
            config,
            correlation_id,
            envelope,
            fleet_runner::CommandOutputChunk {
                stream: fleet_runner::CommandOutputStream::Stderr,
                sequence: 0,
                data: output.stderr.clone(),
            },
        )?;
    }
    send_agent_task_result_queue(
        outbound_sender,
        config,
        correlation_id,
        envelope,
        output.exit_code,
        result_status,
        &reason,
    )?;
    Ok(())
}

fn command_execution_result(
    result: Result<fleet_runner::CommandOutput, fleet_runner::RunnerError>,
) -> (
    fleet_runner::CommandOutput,
    fleet_protocol::TaskResultStatus,
    String,
) {
    match result {
        Ok(output) if output.exit_code == 0 => (
            output,
            fleet_protocol::TaskResultStatus::Succeeded,
            String::new(),
        ),
        Ok(output) => {
            let reason = if output.stderr.is_empty() {
                format!("exit_code={}", output.exit_code)
            } else {
                output.stderr.clone()
            };
            (output, fleet_protocol::TaskResultStatus::Failed, reason)
        }
        Err(error @ fleet_runner::RunnerError::Canceled) => (
            fleet_runner::CommandOutput {
                stdout: String::new(),
                stderr: error.to_string(),
                exit_code: -1,
                truncated: false,
            },
            fleet_protocol::TaskResultStatus::Canceled,
            error.to_string(),
        ),
        Err(error @ fleet_runner::RunnerError::Timeout) => (
            fleet_runner::CommandOutput {
                stdout: String::new(),
                stderr: error.to_string(),
                exit_code: -1,
                truncated: false,
            },
            fleet_protocol::TaskResultStatus::TimedOut,
            error.to_string(),
        ),
        Err(error) => (
            fleet_runner::CommandOutput {
                stdout: String::new(),
                stderr: error.to_string(),
                exit_code: -1,
                truncated: false,
            },
            fleet_protocol::TaskResultStatus::Failed,
            error.to_string(),
        ),
    }
}

fn run_signed_drift_check_task_queue(
    outbound_sender: &AgentOutboundQueue,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
    task: fleet_protocol::DriftCheckTaskWire,
) -> Result<(), CliError> {
    let policy = match fleet_domain::parse_policy_document(&task.policy_document) {
        Ok(policy) => policy,
        Err(error) => {
            send_agent_output_chunk_queue(
                outbound_sender,
                config,
                correlation_id,
                envelope,
                fleet_runner::CommandOutputChunk {
                    stream: fleet_runner::CommandOutputStream::Stderr,
                    sequence: 0,
                    data: format!("invalid drift policy: {error}"),
                },
            )?;
            send_agent_task_result_queue(
                outbound_sender,
                config,
                correlation_id,
                envelope,
                -1,
                fleet_protocol::TaskResultStatus::Failed,
                &error.to_string(),
            )?;
            return Ok(());
        }
    };
    send_agent_task_started_queue(outbound_sender, config, correlation_id, envelope)?;
    let report = fleet_runner::evaluate_policy_drift(&policy, &fleet_runner::LocalDriftProbe);
    enqueue_wire_message(
        outbound_sender,
        fleet_protocol::WireMessage::new(
            prefixed_ulid("msg")?,
            correlation_id.to_owned(),
            Some(config.agent_id.clone()),
            epoch_millis() as u64,
            drift_report_payload_for_envelope(
                &config.agent_id,
                envelope,
                drift_status_to_cli(&report.status).to_owned(),
                report.expected,
                report.actual,
            ),
        ),
    )?;
    send_agent_task_result_queue(
        outbound_sender,
        config,
        correlation_id,
        envelope,
        0,
        fleet_protocol::TaskResultStatus::Succeeded,
        "",
    )?;
    Ok(())
}

fn drift_report_payload_for_envelope(
    agent_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
    status: String,
    expected: String,
    actual: String,
) -> fleet_protocol::WirePayload {
    fleet_protocol::WirePayload::DriftReport {
        agent_id: agent_id.to_owned(),
        job_id: Some(envelope.job_id.as_str().to_owned()),
        task_id: Some(envelope.task_id.as_str().to_owned()),
        status,
        expected,
        actual,
    }
}

fn run_signed_runbook_task_queue(
    outbound_sender: &AgentOutboundQueue,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
    task: fleet_protocol::RunbookExecutionTaskWire,
) -> Result<(), CliError> {
    let runbook = match fleet_domain::parse_runbook_document(&task.runbook_document) {
        Ok(runbook) => runbook,
        Err(error) => {
            send_agent_output_chunk_queue(
                outbound_sender,
                config,
                correlation_id,
                envelope,
                fleet_runner::CommandOutputChunk {
                    stream: fleet_runner::CommandOutputStream::Stderr,
                    sequence: 0,
                    data: format!("invalid runbook: {error}"),
                },
            )?;
            send_agent_task_result_queue(
                outbound_sender,
                config,
                correlation_id,
                envelope,
                -1,
                fleet_protocol::TaskResultStatus::Failed,
                &error.to_string(),
            )?;
            return Ok(());
        }
    };
    let package_manager = match local_runbook_package_manager(&runbook) {
        Ok(package_manager) => package_manager,
        Err(error) => {
            send_agent_output_chunk_queue(
                outbound_sender,
                config,
                correlation_id,
                envelope,
                fleet_runner::CommandOutputChunk {
                    stream: fleet_runner::CommandOutputStream::Stderr,
                    sequence: 0,
                    data: error.to_owned(),
                },
            )?;
            send_agent_task_result_queue(
                outbound_sender,
                config,
                correlation_id,
                envelope,
                -1,
                fleet_protocol::TaskResultStatus::Failed,
                error,
            )?;
            return Ok(());
        }
    };
    let provider = DisabledSecretProvider;
    let plan = match build_agent_runbook_execution_plan_with_provider(
        &runbook,
        package_manager,
        &provider,
    ) {
        Ok(plan) => plan,
        Err(error) => {
            send_agent_output_chunk_queue(
                outbound_sender,
                config,
                correlation_id,
                envelope,
                fleet_runner::CommandOutputChunk {
                    stream: fleet_runner::CommandOutputStream::Stderr,
                    sequence: 0,
                    data: format!("invalid runbook primitive: {error}"),
                },
            )?;
            send_agent_task_result_queue(
                outbound_sender,
                config,
                correlation_id,
                envelope,
                -1,
                fleet_protocol::TaskResultStatus::Failed,
                &error.to_string(),
            )?;
            return Ok(());
        }
    };
    send_agent_task_started_queue(outbound_sender, config, correlation_id, envelope)?;
    let report = match fleet_runner::execute_runbook_execution_plan_with_hooks(
        &plan,
        fleet_runner::RunbookExecutionOptions {
            confirmed_high_risk: task.confirmed_high_risk,
            check_mode: runbook.check_mode,
            dry_run: runbook.dry_run,
            command_timeout: Duration::from_millis(task.timeout_ms),
            ..fleet_runner::RunbookExecutionOptions::default()
        },
        runbook_command_runner,
        fleet_runner::copy_file_atomic,
        fleet_runner::check_tcp_port,
        fleet_runner::check_local_process,
        collect_runbook_snapshot,
    ) {
        Ok(report) => report,
        Err(error) => {
            send_agent_output_chunk_queue(
                outbound_sender,
                config,
                correlation_id,
                envelope,
                fleet_runner::CommandOutputChunk {
                    stream: fleet_runner::CommandOutputStream::Stderr,
                    sequence: 0,
                    data: error.to_string(),
                },
            )?;
            send_agent_task_result_queue(
                outbound_sender,
                config,
                correlation_id,
                envelope,
                -1,
                fleet_protocol::TaskResultStatus::Failed,
                &error.to_string(),
            )?;
            return Ok(());
        }
    };
    let mut sequence = 0;
    let mut artifacts = Vec::new();
    for outcome in report.outcomes {
        if let Some(artifact) = &outcome.artifact {
            artifacts.push(fleet_protocol::TaskResultArtifactWire {
                artifact_id: prefixed_ulid("artifact")?,
                step_id: artifact.step_id.clone(),
                destination: artifact.destination.clone(),
                checksum_sha256: artifact.checksum_sha256.clone(),
                size_bytes: artifact.size_bytes,
                retention_class: artifact.retention_class.clone(),
                content_bytes: artifact.content_bytes.clone(),
            });
        }
        send_agent_output_chunk_queue(
            outbound_sender,
            config,
            correlation_id,
            envelope,
            fleet_runner::CommandOutputChunk {
                stream: fleet_runner::CommandOutputStream::Stdout,
                sequence,
                data: format!(
                    "runbook_step={} changed={:?} exit_code={:?} {}",
                    outcome.id, outcome.changed, outcome.exit_code, outcome.audit_metadata
                ),
            },
        )?;
        sequence += 1;
        if !outcome.stdout.is_empty() {
            send_agent_output_chunk_queue(
                outbound_sender,
                config,
                correlation_id,
                envelope,
                fleet_runner::CommandOutputChunk {
                    stream: fleet_runner::CommandOutputStream::Stdout,
                    sequence,
                    data: outcome.stdout,
                },
            )?;
            sequence += 1;
        }
        if !outcome.stderr.is_empty() {
            send_agent_output_chunk_queue(
                outbound_sender,
                config,
                correlation_id,
                envelope,
                fleet_runner::CommandOutputChunk {
                    stream: fleet_runner::CommandOutputStream::Stderr,
                    sequence,
                    data: outcome.stderr,
                },
            )?;
            sequence += 1;
        }
    }
    send_agent_task_result_queue_report(
        outbound_sender,
        config,
        correlation_id,
        envelope,
        AgentTaskResultReport::with_artifacts(
            0,
            fleet_protocol::TaskResultStatus::Succeeded,
            "",
            artifacts,
        ),
    )?;
    Ok(())
}

fn run_signed_command_task(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
    command: fleet_protocol::CommandTaskWire,
) -> Result<(), CliError> {
    let mut spec = fleet_runner::CommandSpec::new(
        command.program,
        command.args,
        Duration::from_millis(command.timeout_ms),
    );
    spec.max_output_bytes = command.max_output_bytes;
    send_agent_task_started(socket, config, correlation_id, envelope)?;
    let mut streamed_any = false;
    let result = fleet_runner::run_command_streaming_with_cancel(
        spec,
        || false,
        |chunk| {
            streamed_any = true;
            send_agent_output_chunk(socket, config, correlation_id, envelope, chunk)
                .map_err(|error| fleet_runner::RunnerError::Stream(error.to_string()))
        },
    );
    let (output, result_status, reason) = command_execution_result(result);
    if !streamed_any && output.exit_code != 0 && !output.stderr.is_empty() {
        send_agent_output_chunk(
            socket,
            config,
            correlation_id,
            envelope,
            fleet_runner::CommandOutputChunk {
                stream: fleet_runner::CommandOutputStream::Stderr,
                sequence: 0,
                data: output.stderr.clone(),
            },
        )?;
    }
    send_agent_task_result(
        socket,
        config,
        correlation_id,
        envelope,
        output.exit_code,
        result_status,
        &reason,
    )?;
    Ok(())
}

fn run_signed_drift_check_task(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
    task: fleet_protocol::DriftCheckTaskWire,
) -> Result<(), CliError> {
    let policy = match fleet_domain::parse_policy_document(&task.policy_document) {
        Ok(policy) => policy,
        Err(error) => {
            send_agent_output_chunk(
                socket,
                config,
                correlation_id,
                envelope,
                fleet_runner::CommandOutputChunk {
                    stream: fleet_runner::CommandOutputStream::Stderr,
                    sequence: 0,
                    data: format!("invalid drift policy: {error}"),
                },
            )?;
            send_agent_task_result(
                socket,
                config,
                correlation_id,
                envelope,
                -1,
                fleet_protocol::TaskResultStatus::Failed,
                &error.to_string(),
            )?;
            return Ok(());
        }
    };
    send_agent_task_started(socket, config, correlation_id, envelope)?;
    let report = fleet_runner::evaluate_policy_drift(&policy, &fleet_runner::LocalDriftProbe);
    let message = fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        drift_report_payload_for_envelope(
            &config.agent_id,
            envelope,
            drift_status_to_cli(&report.status).to_owned(),
            report.expected,
            report.actual,
        ),
    );
    socket
        .send(Message::Text(
            fleet_protocol::encode_message(&message)
                .map_err(|error| CliError::Http(error.to_string()))?,
        ))
        .map_err(|error| CliError::Http(error.to_string()))?;
    send_agent_task_result(
        socket,
        config,
        correlation_id,
        envelope,
        0,
        fleet_protocol::TaskResultStatus::Succeeded,
        "",
    )?;
    Ok(())
}

fn local_runbook_package_manager(
    runbook: &fleet_domain::Runbook,
) -> Result<fleet_runner::LinuxPackageManager, &'static str> {
    if runbook
        .tasks
        .iter()
        .any(|task| matches!(task, fleet_domain::RunbookTask::Package(_)))
    {
        fleet_runner::detect_local_linux_package_manager()
            .ok_or("no supported Linux package manager detected")
    } else {
        Ok(fleet_runner::LinuxPackageManager::Apt)
    }
}

fn runbook_command_runner(
    command: &fleet_runner::PrimitiveCommand,
    spec: &fleet_runner::CommandSpec,
) -> Result<fleet_runner::CommandOutput, fleet_runner::RunnerError> {
    let mut spec = spec.clone();
    spec.program = command.program.clone();
    spec.args = command.args.clone();
    fleet_runner::run_command_with_spec(spec)
}

fn collect_runbook_snapshot(
    spec: &fleet_runner::SnapshotSpec,
) -> Result<fleet_runner::SnapshotResult, fleet_runner::PrimitiveError> {
    let (message, mut body) = match spec.kind {
        fleet_runner::SnapshotKind::Facts => ("facts snapshot collected", collect_local_facts()),
        fleet_runner::SnapshotKind::Metrics => {
            ("metrics snapshot collected", collect_local_metrics())
        }
    };
    if let Some(object) = body.as_object_mut() {
        object.insert(
            "source".to_owned(),
            serde_json::Value::String("runbook".to_owned()),
        );
    }
    Ok(fleet_runner::SnapshotResult {
        message: message.to_owned(),
        body: body.to_string(),
    })
}

fn build_agent_runbook_execution_plan_with_provider<P>(
    runbook: &fleet_domain::Runbook,
    package_manager: fleet_runner::LinuxPackageManager,
    provider: &P,
) -> Result<fleet_runner::RunbookExecutionPlan, fleet_runner::PrimitiveError>
where
    P: SecretProvider,
{
    fleet_runner::build_runbook_execution_plan_with_secret_resolver(
        runbook,
        package_manager,
        |reference| resolve_secret_for_runner(provider, reference),
    )
}

fn resolve_secret_for_runner<P>(
    provider: &P,
    reference: &fleet_domain::SecretRef,
) -> Result<String, fleet_domain::TemplateSecretResolutionFailure>
where
    P: SecretProvider,
{
    provider
        .resolve_secret(reference)
        .map(|secret: ResolvedSecret| secret.expose_secret_for_rendering().to_owned())
        .map_err(|error| match error {
            SecretProviderError::NotFound { .. } => {
                fleet_domain::TemplateSecretResolutionFailure::NotFound
            }
            SecretProviderError::Denied { .. } => {
                fleet_domain::TemplateSecretResolutionFailure::Denied
            }
            SecretProviderError::Provider { .. } => {
                fleet_domain::TemplateSecretResolutionFailure::Provider
            }
        })
}

fn run_signed_runbook_task(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
    task: fleet_protocol::RunbookExecutionTaskWire,
) -> Result<(), CliError> {
    let runbook = match fleet_domain::parse_runbook_document(&task.runbook_document) {
        Ok(runbook) => runbook,
        Err(error) => {
            send_agent_output_chunk(
                socket,
                config,
                correlation_id,
                envelope,
                fleet_runner::CommandOutputChunk {
                    stream: fleet_runner::CommandOutputStream::Stderr,
                    sequence: 0,
                    data: format!("invalid runbook: {error}"),
                },
            )?;
            send_agent_task_result(
                socket,
                config,
                correlation_id,
                envelope,
                -1,
                fleet_protocol::TaskResultStatus::Failed,
                &error.to_string(),
            )?;
            return Ok(());
        }
    };
    let package_manager = match local_runbook_package_manager(&runbook) {
        Ok(package_manager) => package_manager,
        Err(error) => {
            send_agent_output_chunk(
                socket,
                config,
                correlation_id,
                envelope,
                fleet_runner::CommandOutputChunk {
                    stream: fleet_runner::CommandOutputStream::Stderr,
                    sequence: 0,
                    data: error.to_owned(),
                },
            )?;
            send_agent_task_result(
                socket,
                config,
                correlation_id,
                envelope,
                -1,
                fleet_protocol::TaskResultStatus::Failed,
                error,
            )?;
            return Ok(());
        }
    };
    let provider = DisabledSecretProvider;
    let plan = match build_agent_runbook_execution_plan_with_provider(
        &runbook,
        package_manager,
        &provider,
    ) {
        Ok(plan) => plan,
        Err(error) => {
            send_agent_output_chunk(
                socket,
                config,
                correlation_id,
                envelope,
                fleet_runner::CommandOutputChunk {
                    stream: fleet_runner::CommandOutputStream::Stderr,
                    sequence: 0,
                    data: format!("invalid runbook primitive: {error}"),
                },
            )?;
            send_agent_task_result(
                socket,
                config,
                correlation_id,
                envelope,
                -1,
                fleet_protocol::TaskResultStatus::Failed,
                &error.to_string(),
            )?;
            return Ok(());
        }
    };
    send_agent_task_started(socket, config, correlation_id, envelope)?;
    let report = match fleet_runner::execute_runbook_execution_plan_with_hooks(
        &plan,
        fleet_runner::RunbookExecutionOptions {
            confirmed_high_risk: task.confirmed_high_risk,
            check_mode: runbook.check_mode,
            dry_run: runbook.dry_run,
            command_timeout: Duration::from_millis(task.timeout_ms),
            ..fleet_runner::RunbookExecutionOptions::default()
        },
        runbook_command_runner,
        fleet_runner::copy_file_atomic,
        fleet_runner::check_tcp_port,
        fleet_runner::check_local_process,
        collect_runbook_snapshot,
    ) {
        Ok(report) => report,
        Err(error) => {
            send_agent_output_chunk(
                socket,
                config,
                correlation_id,
                envelope,
                fleet_runner::CommandOutputChunk {
                    stream: fleet_runner::CommandOutputStream::Stderr,
                    sequence: 0,
                    data: error.to_string(),
                },
            )?;
            send_agent_task_result(
                socket,
                config,
                correlation_id,
                envelope,
                -1,
                fleet_protocol::TaskResultStatus::Failed,
                &error.to_string(),
            )?;
            return Ok(());
        }
    };
    let mut sequence = 0;
    let mut artifacts = Vec::new();
    for outcome in report.outcomes {
        if let Some(artifact) = &outcome.artifact {
            artifacts.push(fleet_protocol::TaskResultArtifactWire {
                artifact_id: prefixed_ulid("artifact")?,
                step_id: artifact.step_id.clone(),
                destination: artifact.destination.clone(),
                checksum_sha256: artifact.checksum_sha256.clone(),
                size_bytes: artifact.size_bytes,
                retention_class: artifact.retention_class.clone(),
                content_bytes: artifact.content_bytes.clone(),
            });
        }
        let summary = format!(
            "runbook_step={} changed={:?} exit_code={:?} {}",
            outcome.id, outcome.changed, outcome.exit_code, outcome.audit_metadata
        );
        send_agent_output_chunk(
            socket,
            config,
            correlation_id,
            envelope,
            fleet_runner::CommandOutputChunk {
                stream: fleet_runner::CommandOutputStream::Stdout,
                sequence,
                data: summary,
            },
        )?;
        sequence += 1;
        if !outcome.stdout.is_empty() {
            send_agent_output_chunk(
                socket,
                config,
                correlation_id,
                envelope,
                fleet_runner::CommandOutputChunk {
                    stream: fleet_runner::CommandOutputStream::Stdout,
                    sequence,
                    data: outcome.stdout,
                },
            )?;
            sequence += 1;
        }
        if !outcome.stderr.is_empty() {
            send_agent_output_chunk(
                socket,
                config,
                correlation_id,
                envelope,
                fleet_runner::CommandOutputChunk {
                    stream: fleet_runner::CommandOutputStream::Stderr,
                    sequence,
                    data: outcome.stderr,
                },
            )?;
            sequence += 1;
        }
    }
    send_agent_task_result_report(
        socket,
        config,
        correlation_id,
        envelope,
        AgentTaskResultReport::with_artifacts(
            0,
            fleet_protocol::TaskResultStatus::Succeeded,
            "",
            artifacts,
        ),
    )?;
    Ok(())
}

fn send_agent_task_ack(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
) -> Result<(), CliError> {
    send_wire_message_to_socket(
        socket,
        &agent_task_ack_message(config, correlation_id, envelope)?,
    )
}

fn send_agent_task_ack_queue(
    outbound_sender: &AgentOutboundQueue,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
) -> Result<(), CliError> {
    enqueue_wire_message(
        outbound_sender,
        agent_task_ack_message(config, correlation_id, envelope)?,
    )
}

fn send_agent_task_started(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
) -> Result<(), CliError> {
    send_wire_message_to_socket(
        socket,
        &agent_task_started_message(config, correlation_id, envelope)?,
    )
}

fn send_agent_task_started_queue(
    outbound_sender: &AgentOutboundQueue,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
) -> Result<(), CliError> {
    enqueue_wire_message(
        outbound_sender,
        agent_task_started_message(config, correlation_id, envelope)?,
    )
}

fn send_agent_task_rejected(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
    reason_code: fleet_protocol::TaskRejectionReasonCode,
    reason: &str,
) -> Result<(), CliError> {
    send_wire_message_to_socket(
        socket,
        &agent_task_rejected_message(
            config,
            correlation_id,
            envelope.job_id.as_str(),
            envelope.task_id.as_str(),
            reason_code,
            reason,
        )?,
    )
}

fn send_agent_task_rejected_queue(
    outbound_sender: &AgentOutboundQueue,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
    reason_code: fleet_protocol::TaskRejectionReasonCode,
    reason: &str,
) -> Result<(), CliError> {
    enqueue_wire_message(
        outbound_sender,
        agent_task_rejected_message(
            config,
            correlation_id,
            envelope.job_id.as_str(),
            envelope.task_id.as_str(),
            reason_code,
            reason,
        )?,
    )
}

fn agent_task_ack_message(
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
) -> Result<fleet_protocol::WireMessage, CliError> {
    Ok(fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::TaskAck {
            job_id: envelope.job_id.as_str().to_owned(),
            task_id: envelope.task_id.as_str().to_owned(),
        },
    ))
}

fn agent_task_started_message(
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
) -> Result<fleet_protocol::WireMessage, CliError> {
    Ok(fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::TaskStarted {
            job_id: envelope.job_id.as_str().to_owned(),
            task_id: envelope.task_id.as_str().to_owned(),
        },
    ))
}

fn agent_task_rejected_message(
    config: &LocalAgentConfig,
    correlation_id: &str,
    job_id: &str,
    task_id: &str,
    reason_code: fleet_protocol::TaskRejectionReasonCode,
    reason: &str,
) -> Result<fleet_protocol::WireMessage, CliError> {
    Ok(fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::TaskRejected {
            job_id: job_id.to_owned(),
            task_id: task_id.to_owned(),
            reason_code,
            reason: fleet_core::redact_secret(reason),
        },
    ))
}

fn task_rejection_reason_from_runner_error(
    error: &fleet_runner::RunnerError,
) -> fleet_protocol::TaskRejectionReasonCode {
    match error {
        fleet_runner::RunnerError::InvalidSignature
        | fleet_runner::RunnerError::UnknownControllerSigningKey
        | fleet_runner::RunnerError::ExpiredControllerSigningTrust
        | fleet_runner::RunnerError::Job(fleet_domain::JobError::UnsignedTask) => {
            fleet_protocol::TaskRejectionReasonCode::InvalidSignature
        }
        fleet_runner::RunnerError::ReplayedNonce => fleet_protocol::TaskRejectionReasonCode::Replay,
        fleet_runner::RunnerError::Job(fleet_domain::JobError::ExpiredTask) => {
            fleet_protocol::TaskRejectionReasonCode::Expired
        }
        fleet_runner::RunnerError::Job(fleet_domain::JobError::TargetAgentMismatch) => {
            fleet_protocol::TaskRejectionReasonCode::TargetMismatch
        }
        fleet_runner::RunnerError::HighRiskConfirmationRequired(_) => {
            fleet_protocol::TaskRejectionReasonCode::LocalPolicy
        }
        fleet_runner::RunnerError::Primitive(_) => {
            fleet_protocol::TaskRejectionReasonCode::CapabilityUnsupported
        }
        fleet_runner::RunnerError::Job(_) => fleet_protocol::TaskRejectionReasonCode::InvalidTask,
        fleet_runner::RunnerError::Io(_)
        | fleet_runner::RunnerError::ReplayStoreUnavailable(_)
        | fleet_runner::RunnerError::Timeout
        | fleet_runner::RunnerError::Canceled
        | fleet_runner::RunnerError::OutputLimitExceeded
        | fleet_runner::RunnerError::Stream(_) => {
            fleet_protocol::TaskRejectionReasonCode::InternalError
        }
    }
}

struct AgentTaskResultReport {
    exit_code: i32,
    status: fleet_protocol::TaskResultStatus,
    reason: String,
    artifacts: Vec<fleet_protocol::TaskResultArtifactWire>,
}

impl AgentTaskResultReport {
    fn new(exit_code: i32, status: fleet_protocol::TaskResultStatus, reason: &str) -> Self {
        Self::with_artifacts(exit_code, status, reason, Vec::new())
    }

    fn with_artifacts(
        exit_code: i32,
        status: fleet_protocol::TaskResultStatus,
        reason: &str,
        artifacts: Vec<fleet_protocol::TaskResultArtifactWire>,
    ) -> Self {
        Self {
            exit_code,
            status,
            reason: reason.to_owned(),
            artifacts,
        }
    }
}

fn send_agent_task_result(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
    exit_code: i32,
    status: fleet_protocol::TaskResultStatus,
    reason: &str,
) -> Result<(), CliError> {
    send_agent_task_result_report(
        socket,
        config,
        correlation_id,
        envelope,
        AgentTaskResultReport::new(exit_code, status, reason),
    )
}

fn send_agent_task_result_report(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
    report: AgentTaskResultReport,
) -> Result<(), CliError> {
    let result = fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::TaskResult {
            job_id: envelope.job_id.as_str().to_owned(),
            task_id: envelope.task_id.as_str().to_owned(),
            exit_code: report.exit_code,
            status: Some(report.status),
            reason: report.reason,
            artifacts: report.artifacts,
        },
    );
    socket
        .send(Message::Text(
            fleet_protocol::encode_message(&result)
                .map_err(|error| CliError::Http(error.to_string()))?,
        ))
        .map_err(|error| CliError::Http(error.to_string()))?;
    Ok(())
}

fn send_agent_task_result_queue(
    outbound_sender: &AgentOutboundQueue,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
    exit_code: i32,
    status: fleet_protocol::TaskResultStatus,
    reason: &str,
) -> Result<(), CliError> {
    send_agent_task_result_queue_report(
        outbound_sender,
        config,
        correlation_id,
        envelope,
        AgentTaskResultReport::new(exit_code, status, reason),
    )
}

fn send_agent_task_result_queue_report(
    outbound_sender: &AgentOutboundQueue,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
    report: AgentTaskResultReport,
) -> Result<(), CliError> {
    enqueue_wire_message(
        outbound_sender,
        fleet_protocol::WireMessage::new(
            prefixed_ulid("msg")?,
            correlation_id.to_owned(),
            Some(config.agent_id.clone()),
            epoch_millis() as u64,
            fleet_protocol::WirePayload::TaskResult {
                job_id: envelope.job_id.as_str().to_owned(),
                task_id: envelope.task_id.as_str().to_owned(),
                exit_code: report.exit_code,
                status: Some(report.status),
                reason: report.reason,
                artifacts: report.artifacts,
            },
        ),
    )
}

fn send_agent_output_chunk(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
    chunk: fleet_runner::CommandOutputChunk,
) -> Result<(), CliError> {
    let message = fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::OutputChunk {
            job_id: envelope.job_id.as_str().to_owned(),
            task_id: envelope.task_id.as_str().to_owned(),
            stream: output_stream_to_wire(chunk.stream),
            sequence: chunk.sequence,
            data: chunk.data,
        },
    );
    socket
        .send(Message::Text(
            fleet_protocol::encode_message(&message)
                .map_err(|error| CliError::Http(error.to_string()))?,
        ))
        .map_err(|error| CliError::Http(error.to_string()))
}

fn send_agent_output_chunk_queue(
    outbound_sender: &AgentOutboundQueue,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
    chunk: fleet_runner::CommandOutputChunk,
) -> Result<(), CliError> {
    enqueue_wire_message(
        outbound_sender,
        fleet_protocol::WireMessage::new(
            prefixed_ulid("msg")?,
            correlation_id.to_owned(),
            Some(config.agent_id.clone()),
            epoch_millis() as u64,
            fleet_protocol::WirePayload::OutputChunk {
                job_id: envelope.job_id.as_str().to_owned(),
                task_id: envelope.task_id.as_str().to_owned(),
                stream: output_stream_to_wire(chunk.stream),
                sequence: chunk.sequence,
                data: chunk.data,
            },
        ),
    )
}

fn send_agent_security_event_queue(
    outbound_sender: &AgentOutboundQueue,
    config: &LocalAgentConfig,
    correlation_id: &str,
    action: &str,
    detail: &str,
) -> Result<(), CliError> {
    enqueue_wire_message(
        outbound_sender,
        agent_security_event_message(config, correlation_id, action, detail)?,
    )
}

struct ControllerSignatureVerifier;

impl fleet_runner::ControllerSigningMaterialVerifier for ControllerSignatureVerifier {
    fn verify(&self, public_key: &str, payload_hash: &str, signature: &str) -> bool {
        fleet_core::verify_challenge_signature(public_key, payload_hash, signature).unwrap_or(false)
    }
}

fn task_envelope_from_wire(
    envelope: fleet_protocol::SignedTaskEnvelopeWire,
) -> Result<fleet_domain::TaskEnvelope, CliError> {
    Ok(fleet_domain::TaskEnvelope {
        job_id: fleet_domain::JobId::new(envelope.job_id)
            .map_err(|error| CliError::Http(error.to_string()))?,
        task_id: fleet_domain::TaskId::new(envelope.task_id)
            .map_err(|error| CliError::Http(error.to_string()))?,
        target_agent_id: fleet_domain::AgentId::new(envelope.target_agent_id)
            .map_err(|error| CliError::Http(error.to_string()))?,
        issued_at: millis_to_system_time(envelope.issued_at_ms),
        expires_at: fleet_domain::TaskExpiry::new(millis_to_system_time(envelope.expires_at_ms)),
        nonce: fleet_domain::TaskNonce::new(envelope.nonce)
            .map_err(|error| CliError::Http(error.to_string()))?,
        payload_hash: envelope.payload_hash,
        signature: Some(
            fleet_domain::TaskSignature::new(envelope.signature)
                .map_err(|error| CliError::Http(error.to_string()))?,
        ),
    })
}

fn output_stream_to_wire(
    stream: fleet_runner::CommandOutputStream,
) -> fleet_protocol::OutputStream {
    match stream {
        fleet_runner::CommandOutputStream::Stdout => fleet_protocol::OutputStream::Stdout,
        fleet_runner::CommandOutputStream::Stderr => fleet_protocol::OutputStream::Stderr,
    }
}

fn read_ws_message(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
) -> Result<fleet_protocol::WireMessage, CliError> {
    loop {
        match socket.read() {
            Ok(message) => {
                let body = message
                    .to_text()
                    .map_err(|error| CliError::Http(error.to_string()))?;
                return fleet_protocol::decode_message(body)
                    .map_err(|error| CliError::Http(error.to_string()));
            }
            Err(error) if handshake_read_error_is_retryable(&error) => continue,
            Err(error) => return Err(CliError::Http(error.to_string())),
        }
    }
}

/// Retries only an OS-interrupted handshake read; timeouts and protocol failures stay fatal.
fn handshake_read_error_is_retryable(error: &tungstenite::Error) -> bool {
    matches!(
        error,
        tungstenite::Error::Io(error) if error.kind() == std::io::ErrorKind::Interrupted
    )
}

fn prefixed_ulid(prefix: &str) -> Result<String, CliError> {
    fleet_core::generate_prefixed_ulid(prefix).map_err(CliError::from)
}

fn epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn millis_to_system_time(value: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(value)
}

fn system_time_to_millis(value: SystemTime) -> u64 {
    value
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn test_outbound_queue(capacity: usize) -> AgentOutboundQueue {
        Arc::new(Mutex::new(fleet_agent::AgentSessionSupervisor::new(
            capacity,
        )))
    }

    fn take_outbound_message(queue: &AgentOutboundQueue) -> Option<fleet_protocol::WireMessage> {
        queue.lock().unwrap().remove_pending_report()
    }

    struct TestOutboundReceiver(AgentOutboundQueue);

    impl TestOutboundReceiver {
        fn try_recv(&self) -> Result<fleet_protocol::WireMessage, mpsc::TryRecvError> {
            take_outbound_message(&self.0).ok_or(mpsc::TryRecvError::Empty)
        }
    }

    #[derive(Debug)]
    struct OuterHttpTestError {
        source: InnerHttpTestError,
    }

    impl Display for OuterHttpTestError {
        fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "outer request failed")
        }
    }

    impl StdError for OuterHttpTestError {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            Some(&self.source)
        }
    }

    #[derive(Debug)]
    struct InnerHttpTestError;

    impl Display for InnerHttpTestError {
        fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "connection refused")
        }
    }

    impl StdError for InnerHttpTestError {}

    #[test]
    fn parses_controller_init() {
        let cli = Cli::try_parse_from(["fleet", "controller", "init"]).expect("valid command");
        assert!(matches!(
            cli.command,
            Command::Controller(ControllerCommand {
                command: ControllerSubcommand::Init { .. }
            })
        ));
    }

    #[test]
    fn http_request_error_message_includes_source_chain_and_connect_hint() {
        let error = OuterHttpTestError {
            source: InnerHttpTestError,
        };

        let message = http_request_error_message(
            "agent enrollment",
            "http://192.168.10.11:7700/api/agents/enroll",
            &error,
        );

        assert!(message.contains("agent enrollment failed"));
        assert!(message.contains("outer request failed"));
        assert!(message.contains("connection refused"));
        assert!(message.contains("controller is running"));
        assert!(message.contains("0.0.0.0"));
        assert!(message.contains("firewall"));
    }

    #[test]
    fn parses_controller_start_sqlite_db_url() {
        let cli = Cli::try_parse_from([
            "fleet",
            "controller",
            "start",
            "--db",
            "sqlite:///tmp/fleet.db",
        ])
        .expect("valid command");

        let Command::Controller(ControllerCommand {
            command: ControllerSubcommand::Start { db, .. },
        }) = cli.command
        else {
            panic!("expected controller start command");
        };

        assert_eq!(db.as_deref(), Some("sqlite:///tmp/fleet.db"));
        let settings =
            parse_controller_database_settings(db.as_deref(), Path::new("/ignored/data-dir"))
                .unwrap();
        assert_eq!(settings.backend_name(), "sqlite");
        assert_eq!(settings.sqlite_path(), Some(Path::new("/tmp/fleet.db")));
    }

    #[test]
    fn agent_runbook_secret_handoff_disabled_provider_rejects_without_ref_leak() {
        let runbook = fleet_domain::parse_runbook_document(
            r#"
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Runbook
name: secret-template
selector: role=web
steps:
  - id: write-token
    file.template:
      dest: /tmp/fleet-secret.conf
      content: "token={{ api_token }}"
      secretRefs: api_token=secret://app/disabled-token
"#,
        )
        .unwrap();

        let error = build_agent_runbook_execution_plan_with_provider(
            &runbook,
            fleet_runner::LinuxPackageManager::Apt,
            &DisabledSecretProvider,
        )
        .expect_err("disabled provider should reject secret-backed template planning");

        let message = error.to_string();
        assert!(message.contains("template render error"));
        assert!(!message.contains("disabled-token"));
        assert!(!message.contains("secret://app"));
    }

    #[test]
    fn agent_runbook_secret_handoff_static_provider_renders_without_artifact_body() {
        let destination = unique_test_dir("agent-secret-handoff").join("secret.conf");
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        let runbook = fleet_domain::parse_runbook_document(&format!(
            r#"
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Runbook
name: secret-template
selector: role=web
steps:
  - id: write-token
    file.template:
      dest: {}
      content: "token={{{{ api_token }}}}"
      secretRefs: api_token=secret://app/api-token
"#,
            destination.display()
        ))
        .unwrap();
        let reference = fleet_domain::SecretRef::parse("secret://app/api-token").unwrap();
        let raw_secret = "agent-static-provider-secret";
        let provider =
            fleet_application::StaticSecretProvider::new().with_secret(reference, raw_secret);

        let plan = build_agent_runbook_execution_plan_with_provider(
            &runbook,
            fleet_runner::LinuxPackageManager::Apt,
            &provider,
        )
        .expect("static provider should plan secret-backed template");
        let report = fleet_runner::execute_runbook_execution_plan_with_hooks(
            &plan,
            fleet_runner::RunbookExecutionOptions {
                confirmed_high_risk: true,
                ..fleet_runner::RunbookExecutionOptions::default()
            },
            runbook_command_runner,
            fleet_runner::copy_file_atomic,
            fleet_runner::check_tcp_port,
            fleet_runner::check_local_process,
            collect_runbook_snapshot,
        )
        .expect("secret-backed template should execute");

        let artifact = report.outcomes[0]
            .artifact
            .as_ref()
            .expect("template should report artifact metadata");
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            format!("token={raw_secret}")
        );
        assert_eq!(artifact.content_bytes, None);
        assert!(!format!("{report:?}").contains(raw_secret));
    }

    #[test]
    fn apply_validation_does_not_resolve_secret_backed_templates() {
        let dir = unique_test_dir("apply-secret-validation");
        std::fs::create_dir_all(&dir).unwrap();
        let runbook_path = dir.join("secret-runbook.yml");
        std::fs::write(
            &runbook_path,
            r#"
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Runbook
name: secret-template
selector: role=web
steps:
  - id: write-token
    file.template:
      dest: /tmp/fleet-secret.conf
      content: "token={{ api_token }}"
      secretRefs: api_token=secret://app/apply-token
"#,
        )
        .unwrap();

        let error = execute_apply(ApplyCommand { file: runbook_path })
            .expect_err("apply validation must not resolve secret-backed templates");

        let message = error.to_string();
        assert!(message.contains("invalid runbook primitive"));
        assert!(!message.contains("apply-token"));
        assert!(!message.contains("secret://app"));
    }

    #[test]
    fn parses_controller_start_postgres_db_url_as_typed_backend() {
        let cli = Cli::try_parse_from([
            "fleet",
            "controller",
            "start",
            "--db",
            "postgresql://fleet:secret@db.example.com/fleet",
        ])
        .expect("valid command");

        let Command::Controller(ControllerCommand {
            command: ControllerSubcommand::Start { db, .. },
        }) = cli.command
        else {
            panic!("expected controller start command");
        };

        let settings =
            parse_controller_database_settings(db.as_deref(), Path::new("/ignored/data-dir"))
                .unwrap();
        assert_eq!(settings.backend_name(), "postgres");
        assert_eq!(settings.sqlite_path(), None);
    }

    #[test]
    fn rejects_unsupported_controller_db_url_scheme_without_leaking_secret() {
        let error = parse_controller_database_settings(
            Some("mysql://fleet:secret@db.example.com/fleet"),
            Path::new("/ignored/data-dir"),
        )
        .expect_err("unsupported database scheme should fail");

        let message = error.to_string();
        assert!(message.contains("mysql"));
        assert!(!message.contains("secret"));
        assert!(!message.contains("db.example.com"));
    }

    #[test]
    fn parses_controller_backup_and_restore_commands() {
        let backup = Cli::try_parse_from([
            "fleet",
            "controller",
            "backup",
            "--data-dir",
            "/tmp/fleet",
            "--output",
            "/tmp/fleet.backup.json",
        ])
        .expect("valid backup command");
        let Command::Controller(ControllerCommand {
            command: ControllerSubcommand::Backup { data_dir, output },
        }) = backup.command
        else {
            panic!("expected controller backup command");
        };
        assert_eq!(data_dir, PathBuf::from("/tmp/fleet"));
        assert_eq!(output, PathBuf::from("/tmp/fleet.backup.json"));

        let restore = Cli::try_parse_from([
            "fleet",
            "controller",
            "restore",
            "--data-dir",
            "/tmp/fleet-restored",
            "--input",
            "/tmp/fleet.backup.json",
            "--dry-run",
            "--force",
        ])
        .expect("valid restore command");
        let Command::Controller(ControllerCommand {
            command:
                ControllerSubcommand::Restore {
                    data_dir,
                    input,
                    dry_run,
                    force,
                },
        }) = restore.command
        else {
            panic!("expected controller restore command");
        };
        assert_eq!(data_dir, PathBuf::from("/tmp/fleet-restored"));
        assert_eq!(input, PathBuf::from("/tmp/fleet.backup.json"));
        assert!(dry_run);
        assert!(force);
    }

    #[test]
    fn parses_controller_start_external_https_url() {
        let cli = Cli::try_parse_from([
            "fleet",
            "controller",
            "start",
            "--host",
            "0.0.0.0",
            "--external-url",
            "https://fleet.example.com",
        ])
        .expect("valid command");

        let Command::Controller(ControllerCommand {
            command:
                ControllerSubcommand::Start {
                    host, external_url, ..
                },
        }) = cli.command
        else {
            panic!("expected controller start command");
        };

        assert_eq!(host, "0.0.0.0");
        assert_eq!(external_url.as_deref(), Some("https://fleet.example.com"));
    }

    #[test]
    fn parses_controller_start_builtin_tls_paths() {
        let cli = Cli::try_parse_from([
            "fleet",
            "controller",
            "start",
            "--external-url",
            "https://fleet.example.com",
            "--tls-cert",
            "/etc/fleet/tls/fullchain.pem",
            "--tls-key",
            "/etc/fleet/tls/privkey.pem",
        ])
        .expect("valid command");

        let Command::Controller(ControllerCommand {
            command:
                ControllerSubcommand::Start {
                    tls_cert, tls_key, ..
                },
        }) = cli.command
        else {
            panic!("expected controller start command");
        };

        assert_eq!(
            tls_cert.as_deref(),
            Some(Path::new("/etc/fleet/tls/fullchain.pem"))
        );
        assert_eq!(
            tls_key.as_deref(),
            Some(Path::new("/etc/fleet/tls/privkey.pem"))
        );
    }

    #[test]
    fn parses_controller_start_agent_client_ca_cert() {
        let cli = Cli::try_parse_from([
            "fleet",
            "controller",
            "start",
            "--external-url",
            "https://fleet.example.com",
            "--agent-client-ca-cert",
            "/etc/fleet/agent-client-ca.pem",
        ])
        .expect("valid command");

        let Command::Controller(ControllerCommand {
            command:
                ControllerSubcommand::Start {
                    agent_client_ca_cert,
                    ..
                },
        }) = cli.command
        else {
            panic!("expected controller start command");
        };

        assert_eq!(
            agent_client_ca_cert.as_deref(),
            Some(Path::new("/etc/fleet/agent-client-ca.pem"))
        );
    }

    #[test]
    fn controller_start_preflight_explains_missing_init() {
        let data_dir = unique_demo_dir();
        let error = ensure_controller_initialized_for_start(&data_dir)
            .expect_err("missing controller init should be explained");
        let message = error.to_string();

        assert!(matches!(
            error,
            CliError::ControllerNotInitialized { data_dir: _ }
        ));
        assert!(message.contains("controller is not initialized"));
        assert!(message.contains("fleet controller init --data-dir"));
        assert!(message.contains("./scripts/run_controller.sh"));
    }

    #[test]
    fn enroll_token_create_persists_scope_and_audit() {
        let data_dir = unique_test_dir("enroll-token-create");
        let cli = Cli::try_parse_from([
            "fleet",
            "enroll-token",
            "create",
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--labels",
            "role=web,env=prod",
            "--max-uses",
            "2",
            "--expires-in-seconds",
            "120",
            "--controller-url",
            "https://fleet.example.com",
            "--name",
            "web-01",
            "--print-init-command",
        ])
        .expect("valid command");

        execute(cli).expect("token create should succeed");

        let store = fleet_store::SqliteStore::open(controller_db_path(&data_dir)).unwrap();
        let records = store.list_enrollment_tokens().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].default_labels, "role=web,env=prod");
        assert_eq!(records[0].max_uses, 2);
        assert_eq!(records[0].used_count, 0);
        assert!(!records[0].revoked);
        let audits = store
            .list_audit_events_by_category(fleet_domain::AuditCategory::Security, 10)
            .unwrap();
        assert!(
            audits
                .iter()
                .any(|event| event.action == "enrollment_token_created")
        );
        assert!(
            audits
                .iter()
                .all(|event| !format!("{:?}", event.value).contains("enroll-"))
        );
    }

    #[test]
    fn enroll_token_revoke_updates_state_and_audit() {
        let data_dir = unique_test_dir("enroll-token-revoke");
        fs::create_dir_all(controller_dir(&data_dir)).unwrap();
        let store = fleet_store::SqliteStore::open(controller_db_path(&data_dir)).unwrap();
        store
            .insert_enrollment_token_hash(
                "et-1",
                &fleet_controller::hash_token("raw-token"),
                "role=web",
                SystemTime::now() + Duration::from_secs(60),
                1,
            )
            .unwrap();
        drop(store);

        let cli = Cli::try_parse_from([
            "fleet",
            "enroll-token",
            "revoke",
            "et-1",
            "--data-dir",
            data_dir.to_str().unwrap(),
        ])
        .expect("valid command");
        execute(cli).expect("token revoke should succeed");

        let store = fleet_store::SqliteStore::open(controller_db_path(&data_dir)).unwrap();
        let records = store.list_enrollment_tokens().unwrap();
        assert!(records[0].revoked);
        let audits = store
            .list_audit_events_by_category(fleet_domain::AuditCategory::Security, 10)
            .unwrap();
        assert!(
            audits
                .iter()
                .any(|event| event.action == "enrollment_token_revoked")
        );
    }

    #[test]
    fn rejects_unsupported_controller_db_url_scheme() {
        assert!(matches!(
            parse_controller_database_settings(
                Some("mysql://localhost/fleet"),
                Path::new("/ignored/data-dir")
            ),
            Err(CliError::Http(_))
        ));
    }

    #[test]
    fn renders_controller_service_unit_with_absolute_binary() {
        let unit = render_service_unit(
            ServiceRole::Controller,
            Path::new("/usr/local/bin/fleet"),
            Path::new("/var/lib/fleet"),
            Some("fleet"),
            Some("fleet"),
        )
        .unwrap();

        assert!(unit.contains("Description=Sponzey Fleet Controller"));
        assert!(
            unit.contains(
                "ExecStart=/usr/local/bin/fleet controller start --data-dir /var/lib/fleet"
            )
        );
        assert!(unit.contains("User=fleet"));
        assert!(unit.contains("Group=fleet"));
    }

    #[test]
    fn renders_agent_service_unit_with_quoted_paths() {
        let unit = render_service_unit(
            ServiceRole::Agent,
            Path::new("/opt/Fleet/bin/fleet"),
            Path::new("/var/lib/fleet path"),
            None,
            None,
        )
        .unwrap();

        assert!(unit.contains("Description=Sponzey Fleet Agent"));
        assert!(unit.contains(
            "ExecStart=/opt/Fleet/bin/fleet agent start --data-dir \"/var/lib/fleet path\""
        ));
    }

    #[test]
    fn service_unit_rejects_relative_binary_path() {
        assert!(matches!(
            render_service_unit(
                ServiceRole::Agent,
                Path::new("target/debug/fleet"),
                Path::new("/var/lib/fleet"),
                None,
                None,
            ),
            Err(CliError::ServiceBinaryMustBeAbsolute(_))
        ));
    }

    #[test]
    fn service_unit_rejects_invalid_user() {
        assert!(matches!(
            render_service_unit(
                ServiceRole::Controller,
                Path::new("/usr/local/bin/fleet"),
                Path::new("/var/lib/fleet"),
                Some("bad user"),
                None,
            ),
            Err(CliError::InvalidServiceAccount(_))
        ));
    }

    #[test]
    fn service_install_guard_message_mentions_systemd_requirements() {
        assert_eq!(
            CliError::ServiceInstallRequiresDryRun.to_string(),
            "service install writes system files and requires Linux root; use --dry-run to inspect the unit first"
        );
    }

    #[test]
    fn agent_start_service_dry_run_renders_systemctl_command() {
        let cli = Cli::try_parse_from(["fleet", "agent", "start-service", "--dry-run"]).unwrap();

        assert!(execute(cli).is_ok());
        assert_eq!(
            render_systemctl_command("start", ServiceRole::Agent),
            "systemctl start fleet-agent.service"
        );
    }

    #[test]
    fn controller_restart_service_dry_run_renders_systemctl_command() {
        let cli =
            Cli::try_parse_from(["fleet", "controller", "restart-service", "--dry-run"]).unwrap();

        assert!(execute(cli).is_ok());
        assert_eq!(
            render_systemctl_command("restart", ServiceRole::Controller),
            "systemctl restart fleet-controller.service"
        );
    }

    #[test]
    fn service_status_and_logs_dry_run_render_commands() {
        let status =
            Cli::try_parse_from(["fleet", "controller", "status-service", "--dry-run"]).unwrap();
        let logs = Cli::try_parse_from([
            "fleet",
            "agent",
            "logs-service",
            "--lines",
            "25",
            "--dry-run",
        ])
        .unwrap();

        assert!(execute(status).is_ok());
        assert!(execute(logs).is_ok());
        assert_eq!(
            render_service_status_command(ServiceRole::Controller),
            "systemctl status fleet-controller.service --no-pager"
        );
        assert_eq!(
            render_service_logs_command(ServiceRole::Agent, 25),
            "journalctl -u fleet-agent.service --no-pager -n 25"
        );
    }

    #[test]
    fn agent_uninstall_service_dry_run_renders_safe_commands() {
        let cli =
            Cli::try_parse_from(["fleet", "agent", "uninstall-service", "--dry-run"]).unwrap();

        assert!(execute(cli).is_ok());
        assert_eq!(
            render_uninstall_service_commands(ServiceRole::Agent),
            vec![
                "systemctl disable --now fleet-agent.service".to_owned(),
                "rm /etc/systemd/system/fleet-agent.service".to_owned(),
                "systemctl daemon-reload".to_owned(),
            ]
        );
    }

    #[test]
    fn controller_service_unit_path_is_systemd_path() {
        assert_eq!(
            systemd_unit_path(ServiceRole::Controller),
            PathBuf::from("/etc/systemd/system/fleet-controller.service")
        );
    }

    #[test]
    fn service_operation_error_mentions_linux_or_root() {
        assert_eq!(
            CliError::ServiceOperationRequiresLinux.to_string(),
            "systemd service operations require Linux"
        );
        assert_eq!(
            CliError::ServiceOperationRequiresRoot.to_string(),
            "systemd service operations require root; rerun with sudo"
        );
    }

    #[test]
    fn parses_upgrade_dry_run_and_rejects_automatic_upgrade() {
        let cli = Cli::try_parse_from([
            "fleet",
            "upgrade",
            "--channel",
            "beta",
            "--version",
            "0.2.0-beta.1",
            "--dry-run",
        ])
        .expect("valid upgrade dry-run command");
        let Command::Upgrade(UpgradeCommand {
            channel,
            version,
            dry_run,
        }) = cli.command
        else {
            panic!("expected upgrade command");
        };
        assert_eq!(channel, UpgradeChannelArg::Beta);
        assert_eq!(version.as_deref(), Some("0.2.0-beta.1"));
        assert!(dry_run);

        let cli = Cli::try_parse_from(["fleet", "upgrade"]).expect("valid upgrade command");
        assert!(matches!(execute(cli), Err(CliError::UpgradeRequiresDryRun)));

        let lines = upgrade_dry_run_lines(&UpgradeCommand {
            channel: UpgradeChannelArg::Beta,
            version: Some("0.2.0-beta.1".to_owned()),
            dry_run: true,
        });
        assert!(
            lines.contains(
                &"artifact_integrity_command=./scripts/verify_standalone_artifacts.sh dist/release"
                    .to_owned()
            )
        );
        assert!(lines.contains(&"artifact_signature_command=./scripts/verify_release_signature.sh dist/release <release-public-key.pem>".to_owned()));
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("recommended_backup_command="))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("recovery_policy="))
        );
        assert!(lines.iter().any(|line| line.starts_with("service_policy=")));
    }

    #[test]
    fn parses_run_command() {
        let cli = Cli::try_parse_from([
            "fleet",
            "run",
            "--selector",
            "role=web",
            "--confirm-risk",
            "uptime",
        ])
        .expect("valid command");

        let Command::Run(command) = cli.command else {
            panic!("expected run command");
        };

        assert_eq!(command.selector.as_deref(), Some("role=web"));
        assert!(command.confirm_risk);
        assert_eq!(command.command, ["uptime"]);
    }

    #[test]
    fn parses_remote_run_command_with_explicit_admin_token() {
        let cli = Cli::try_parse_from([
            "fleet",
            "run",
            "--controller-url",
            "http://127.0.0.1:7700",
            "--admin-token",
            "admin-secret",
            "--selector",
            "role=web",
            "--job-id",
            "job-cli-1",
            "--timeout-seconds",
            "45",
            "--confirm-risk",
            "uptime",
            "--",
            "-a",
        ])
        .expect("valid remote run command");

        let Command::Run(command) = cli.command else {
            panic!("expected run command");
        };

        assert_eq!(
            command.controller_url.as_deref(),
            Some("http://127.0.0.1:7700")
        );
        assert_eq!(command.admin_token.as_deref(), Some("admin-secret"));
        assert_eq!(command.selector.as_deref(), Some("role=web"));
        assert_eq!(command.job_id.as_deref(), Some("job-cli-1"));
        assert_eq!(command.timeout_seconds, 45);
        assert_eq!(command.command, ["uptime", "-a"]);
    }

    #[test]
    fn parses_login_and_remote_operator_commands() {
        let login = Cli::try_parse_from([
            "fleet",
            "login",
            "--controller-url",
            "https://fleet.example.com",
            "--admin-token",
            "admin-secret",
        ])
        .expect("valid login command");
        let Command::Login(command) = login.command else {
            panic!("expected login command");
        };
        assert_eq!(command.controller_url, "https://fleet.example.com");
        assert_eq!(command.admin_token, "admin-secret");
        assert_eq!(
            command.profile_path,
            PathBuf::from(DEFAULT_CLI_PROFILE_PATH)
        );

        let preview =
            Cli::try_parse_from(["fleet", "selectors", "preview", "--selector", "role=web"])
                .expect("valid selector preview command");
        let Command::Selectors(command) = preview.command else {
            panic!("expected selectors command");
        };
        let SelectorsSubcommand::Preview { selector, api } = command.command;
        assert_eq!(selector, "role=web");
        assert_eq!(api.profile_path, PathBuf::from(DEFAULT_CLI_PROFILE_PATH));

        let jobs =
            Cli::try_parse_from(["fleet", "jobs", "output", "job-1"]).expect("valid jobs command");
        assert!(matches!(
            jobs.command,
            Command::Jobs(JobsCommand {
                command: JobsSubcommand::Output { .. }
            })
        ));

        let remediations = Cli::try_parse_from([
            "fleet",
            "remediations",
            "list",
            "--agent-id",
            "agent-1",
            "--policy-id",
            "nginx-running",
            "--limit",
            "10",
        ])
        .expect("valid remediations list command");
        assert!(matches!(
            remediations.command,
            Command::Remediations(RemediationsCommand {
                command: RemediationsSubcommand::List { .. }
            })
        ));

        let remediation_approve = Cli::try_parse_from([
            "fleet",
            "remediations",
            "approve",
            "rem-1",
            "--approval-id",
            "approval-1",
            "--job-id",
            "job-1",
            "--runbook",
            "runbooks/remediate.yml",
        ])
        .expect("valid remediations approve command");
        assert!(matches!(
            remediation_approve.command,
            Command::Remediations(RemediationsCommand {
                command: RemediationsSubcommand::Approve { .. }
            })
        ));

        let audit = Cli::try_parse_from([
            "fleet",
            "audit",
            "export",
            "--category",
            "security",
            "--limit",
            "10",
            "--before",
            "3:1",
        ])
        .expect("valid audit export command");
        let Command::Audit(command) = audit.command else {
            panic!("expected audit command");
        };
        let AuditSubcommand::Export {
            category,
            limit,
            before,
            ..
        } = command.command;
        assert_eq!(category.as_deref(), Some("security"));
        assert_eq!(limit, 10);
        assert_eq!(before.as_deref(), Some("3:1"));
    }

    #[test]
    fn parses_agent_certificate_issuance_request_command() {
        let cli = Cli::try_parse_from([
            "fleet",
            "agents",
            "request-certificate-issuance",
            "agent-1",
            "--controller-url",
            "https://fleet.example.com",
            "--admin-token",
            "admin-secret",
            "--json",
        ])
        .expect("valid agent certificate issuance request command");

        let Command::Agents(command) = cli.command else {
            panic!("expected agents command");
        };
        let AgentsSubcommand::RequestCertificateIssuance {
            agent_id,
            api,
            json,
        } = command.command
        else {
            panic!("expected request certificate issuance command");
        };

        assert_eq!(agent_id, "agent-1");
        assert_eq!(
            api.controller_url.as_deref(),
            Some("https://fleet.example.com")
        );
        assert_eq!(api.admin_token.as_deref(), Some("admin-secret"));
        assert!(json);
    }

    #[test]
    fn renders_agent_certificate_issuance_request_without_material() {
        let body = serde_json::json!({
            "agent_id": "agent-1",
            "action": "request_issuance",
            "lifecycle_state": "issuance_requested",
            "dispatch_status": "sent",
            "current_fingerprint_prefix": null,
            "next_fingerprint_prefix": null,
            "audit_event_action": "agent_certificate_issuance_requested",
            "updated_at_ms": 1000
        })
        .to_string();

        let rendered = render_agent_certificate_issuance_request_for_cli(&body)
            .expect("issuance response should render")
            .join("\n");

        assert!(rendered.contains("agent_id=agent-1"));
        assert!(rendered.contains("action=request_issuance"));
        assert!(rendered.contains("state=issuance_requested"));
        assert!(rendered.contains("dispatch_status=sent"));
        assert!(rendered.contains("current_fingerprint_prefix=none"));
        assert!(!rendered.contains("certificate_body"));
        assert!(!rendered.contains("private_key"));
        assert!(!rendered.contains("ca_path"));
        assert!(!rendered.contains("admin-secret"));
        assert!(!rendered.contains("runtime_env"));
    }

    #[test]
    fn parses_agent_certificate_lifecycle_status_command() {
        let cli = Cli::try_parse_from([
            "fleet",
            "agents",
            "certificate-status",
            "agent-1",
            "--controller-url",
            "https://fleet.example.com",
            "--admin-token",
            "admin-secret",
            "--json",
        ])
        .expect("valid agent certificate lifecycle status command");

        let Command::Agents(command) = cli.command else {
            panic!("expected agents command");
        };
        let AgentsSubcommand::CertificateStatus {
            agent_id,
            api,
            json,
        } = command.command
        else {
            panic!("expected certificate status command");
        };

        assert_eq!(agent_id, "agent-1");
        assert_eq!(
            api.controller_url.as_deref(),
            Some("https://fleet.example.com")
        );
        assert_eq!(api.admin_token.as_deref(), Some("admin-secret"));
        assert!(json);
    }

    #[test]
    fn renders_agent_certificate_lifecycle_status_without_material() {
        let body = serde_json::json!({
            "agent_id": "agent-1",
            "record_present": true,
            "lifecycle_state": "dual_certificate_active",
            "current_fingerprint_prefix": "0123456789ab",
            "next_fingerprint_prefix": "fedcba987654",
            "grace_until_ms": 2000,
            "revocation_reason": null,
            "updated_at_ms": 1000
        })
        .to_string();

        let rendered = render_agent_certificate_lifecycle_status_for_cli(&body)
            .expect("certificate lifecycle status should render")
            .join("\n");

        assert!(rendered.contains("agent_id=agent-1"));
        assert!(rendered.contains("state=dual_certificate_active"));
        assert!(rendered.contains("record_present=true"));
        assert!(rendered.contains("current_fingerprint_prefix=0123456789ab"));
        assert!(rendered.contains("next_fingerprint_prefix=fedcba987654"));
        assert!(rendered.contains("grace_until_ms=2000"));
        assert!(!rendered.contains("certificate_body"));
        assert!(!rendered.contains("private_key"));
        assert!(!rendered.contains("ca_path"));
        assert!(!rendered.contains("admin-secret"));
        assert!(!rendered.contains("runtime_env"));
    }

    #[test]
    fn parses_controller_signing_rotation_status_command() {
        let cli = Cli::try_parse_from([
            "fleet",
            "controller",
            "signing-rotation-status",
            "--controller-url",
            "https://fleet.example.com",
            "--admin-token",
            "admin-secret",
            "--json",
        ])
        .expect("valid controller signing rotation status command");

        let Command::Controller(command) = cli.command else {
            panic!("expected controller command");
        };
        let ControllerSubcommand::SigningRotationStatus { api, json } = command.command else {
            panic!("expected signing rotation status command");
        };

        assert_eq!(
            api.controller_url.as_deref(),
            Some("https://fleet.example.com")
        );
        assert_eq!(api.admin_token.as_deref(), Some("admin-secret"));
        assert!(json);
    }

    #[test]
    fn parses_controller_signing_rotation_mutation_commands() {
        let request = Cli::try_parse_from([
            "fleet",
            "controller",
            "signing-rotation",
            "request",
            "--controller-url",
            "https://fleet.example.com",
            "--admin-token",
            "admin-secret",
            "--new-fingerprint",
            "new-fingerprint",
            "--old-key-verifies-for-seconds",
            "3600",
            "--json",
        ])
        .expect("valid signing rotation request command");
        let Command::Controller(ControllerCommand {
            command: ControllerSubcommand::SigningRotation { command },
        }) = request.command
        else {
            panic!("expected signing rotation command");
        };
        let ControllerSigningRotationSubcommand::Request {
            api,
            new_fingerprint,
            old_key_verifies_for_seconds,
            json,
            ..
        } = command
        else {
            panic!("expected request command");
        };
        assert_eq!(
            api.controller_url.as_deref(),
            Some("https://fleet.example.com")
        );
        assert_eq!(api.admin_token.as_deref(), Some("admin-secret"));
        assert_eq!(new_fingerprint, "new-fingerprint");
        assert_eq!(old_key_verifies_for_seconds, Some(3600));
        assert!(json);

        let validate = Cli::try_parse_from([
            "fleet",
            "controller",
            "signing-rotation",
            "validate",
            "--controller-url",
            "https://fleet.example.com",
            "--admin-token",
            "admin-secret",
            "--candidate-public-key-path",
            "/var/lib/fleet/controller/candidate_public.key",
            "--candidate-private-key-path",
            "/var/lib/fleet/controller/candidate_private.key",
        ])
        .expect("valid signing rotation validate command");
        assert!(matches!(
            validate.command,
            Command::Controller(ControllerCommand {
                command: ControllerSubcommand::SigningRotation {
                    command: ControllerSigningRotationSubcommand::Validate { .. }
                }
            })
        ));

        for action in ["activate", "retire", "fail"] {
            let parsed = Cli::try_parse_from([
                "fleet",
                "controller",
                "signing-rotation",
                action,
                "--controller-url",
                "https://fleet.example.com",
                "--admin-token",
                "admin-secret",
                "--reason",
                "operator requested state change",
            ])
            .expect("valid signing rotation mutation command");
            assert!(matches!(
                parsed.command,
                Command::Controller(ControllerCommand {
                    command: ControllerSubcommand::SigningRotation { .. }
                })
            ));
        }

        let rollout = Cli::try_parse_from([
            "fleet",
            "controller",
            "signing-rotation",
            "rollout-trust-bundle",
            "--controller-url",
            "https://fleet.example.com",
            "--admin-token",
            "admin-secret",
            "--previous-public-key-path",
            "/var/lib/fleet/controller/controller_public.key.bak",
            "--agent-id",
            "agent-1",
            "--json",
        ])
        .expect("valid signing rotation rollout command");
        assert!(matches!(
            rollout.command,
            Command::Controller(ControllerCommand {
                command: ControllerSubcommand::SigningRotation {
                    command: ControllerSigningRotationSubcommand::RolloutTrustBundle { .. }
                }
            })
        ));

        let retry = Cli::try_parse_from([
            "fleet",
            "controller",
            "signing-rotation",
            "retry-trust-bundle",
            "--controller-url",
            "https://fleet.example.com",
            "--admin-token",
            "admin-secret",
            "--previous-public-key-path",
            "/var/lib/fleet/controller/controller_public.key.bak",
            "--agent-id",
            "agent-1",
            "--max-agent-count",
            "25",
            "--json",
        ])
        .expect("valid signing rotation trust bundle retry command");
        assert!(matches!(
            retry.command,
            Command::Controller(ControllerCommand {
                command: ControllerSubcommand::SigningRotation {
                    command: ControllerSigningRotationSubcommand::RetryTrustBundle { .. }
                }
            })
        ));

        let staged = Cli::try_parse_from([
            "fleet",
            "controller",
            "signing-rotation",
            "staged-trust-bundle",
            "--controller-url",
            "https://fleet.example.com",
            "--admin-token",
            "admin-secret",
            "--previous-public-key-path",
            "/var/lib/fleet/controller/controller_public.key.bak",
            "--agent-id",
            "agent-1",
            "--batch-size",
            "10",
            "--max-failures",
            "1",
            "--ack-timeout-seconds",
            "30",
            "--json",
        ])
        .expect("valid signing rotation staged trust bundle command");
        let Command::Controller(ControllerCommand {
            command: ControllerSubcommand::SigningRotation { command },
        }) = staged.command
        else {
            panic!("expected signing rotation command");
        };
        let ControllerSigningRotationSubcommand::StagedTrustBundle {
            api,
            previous_public_key_path,
            agent_ids,
            batch_size,
            max_failures,
            ack_timeout_seconds,
            json,
        } = command
        else {
            panic!("expected staged trust bundle command");
        };
        assert_eq!(
            api.controller_url.as_deref(),
            Some("https://fleet.example.com")
        );
        assert_eq!(api.admin_token.as_deref(), Some("admin-secret"));
        assert_eq!(
            previous_public_key_path.as_deref(),
            Some(Path::new(
                "/var/lib/fleet/controller/controller_public.key.bak"
            ))
        );
        assert_eq!(agent_ids, ["agent-1"]);
        assert_eq!(batch_size, 10);
        assert_eq!(max_failures, 1);
        assert_eq!(ack_timeout_seconds, 30);
        assert!(json);

        let restart_action = Cli::try_parse_from([
            "fleet",
            "controller",
            "signing-rotation",
            "restart-action",
            "--controller-url",
            "https://fleet.example.com",
            "--admin-token",
            "admin-secret",
            "--confirm-external-restart",
            "--reason",
            "operator approved service restart",
            "--json",
        ])
        .expect("valid signing rotation restart action command");
        assert!(matches!(
            restart_action.command,
            Command::Controller(ControllerCommand {
                command: ControllerSubcommand::SigningRotation {
                    command: ControllerSigningRotationSubcommand::RestartAction { .. }
                }
            })
        ));
    }

    #[test]
    fn parses_controller_signing_rotation_restart_plan_command() {
        let parsed = Cli::try_parse_from([
            "fleet",
            "controller",
            "signing-rotation",
            "restart-plan",
            "--controller-url",
            "https://fleet.example.com",
            "--admin-token",
            "admin-secret",
            "--json",
        ])
        .expect("valid signing rotation restart-plan command");

        let Command::Controller(ControllerCommand {
            command: ControllerSubcommand::SigningRotation { command },
        }) = parsed.command
        else {
            panic!("expected signing rotation command");
        };
        let ControllerSigningRotationSubcommand::RestartPlan { api, json } = command else {
            panic!("expected restart-plan command");
        };

        assert_eq!(
            api.controller_url.as_deref(),
            Some("https://fleet.example.com")
        );
        assert_eq!(api.admin_token.as_deref(), Some("admin-secret"));
        assert!(json);
    }

    #[test]
    fn controller_signing_rotation_status_renderer_omits_key_material() {
        let body = serde_json::json!({
            "controller_id": "default-controller",
            "persisted_record_present": true,
            "persisted_state": "dual_trust_active",
            "readiness": "dual_trust_active_agents_migrating",
            "active_signing_fingerprint_prefix": "new-fp-12345678",
            "selected_signing_fingerprint_prefix": "new-fp-12345678",
            "old_fingerprint_prefix": "old-fp-12345678",
            "new_fingerprint_prefix": "new-fp-12345678",
            "requested_at_ms": 1710000000000_u64,
            "validated_at_ms": 1710000001000_u64,
            "activated_at_ms": 1710000002000_u64,
            "old_key_verifies_until_ms": 1710003600000_u64,
            "retired_at_ms": null,
            "failed_at_ms": null,
            "bootstrap_guard": "active_matches_selected",
            "agent_trust_rollout": "agents_migrating",
            "controller_private_key_path": "controller_private.key",
            "controller_signing_public_key": "raw-public-key-body",
            "candidate_private_key": "private-key-secret",
            "tls_certificate_path": "tls/fullchain.pem",
            "task_payload_body": "{\"program\":\"uptime\"}"
        })
        .to_string();

        let lines = render_controller_signing_rotation_status_for_cli(&body)
            .expect("status response should render");
        let rendered = lines.join("\n");

        assert!(rendered.contains("controller_id=default-controller"));
        assert!(rendered.contains("state=dual_trust_active"));
        assert!(rendered.contains("readiness=dual_trust_active_agents_migrating"));
        assert!(rendered.contains("active_signing_fingerprint_prefix=new-fp-12345678"));
        assert!(rendered.contains("old_key_verifies_until_ms=1710003600000"));
        for forbidden in [
            "controller_private.key",
            "raw-public-key-body",
            "private-key-secret",
            "tls/fullchain.pem",
            "task_payload_body",
            "uptime",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "renderer must not expose {forbidden}"
            );
        }
    }

    #[test]
    fn controller_signing_rotation_restart_plan_renderer_omits_key_material() {
        let body = serde_json::json!({
            "controller_id": "default-controller",
            "restart_required": true,
            "reload_supported": false,
            "recommended_action": "restart_controller_process",
            "readiness": "dual_trust_active_agents_migrating",
            "bootstrap_guard": "active_mismatch_selected",
            "agent_trust_rollout": "agents_migrating",
            "active_signing_fingerprint_prefix": "old-fp-12345678",
            "selected_signing_fingerprint_prefix": "new-fp-12345678",
            "blocked_reason": "active signer does not match selected signer",
            "verification_commands": [
                "fleet controller signing-rotation-status --controller-url <controller-url>",
                "fleet controller signing-rotation restart-plan --controller-url <controller-url>"
            ],
            "safety_notes": [
                "controller signing reload is not supported by this version",
                "restart the controller process through the service manager and verify status afterwards"
            ],
            "controller_private_key_path": "controller_private.key",
            "candidate_private_key": "private-key-secret",
            "tls_certificate_path": "tls/fullchain.pem",
            "admin_token": "admin-secret",
            "task_payload_body": "{\"program\":\"uptime\"}"
        })
        .to_string();

        let lines = render_controller_signing_rotation_restart_plan_for_cli(&body)
            .expect("restart plan response should render");
        let rendered = lines.join("\n");

        assert!(rendered.contains("controller_id=default-controller"));
        assert!(rendered.contains("restart_required=true"));
        assert!(rendered.contains("reload_supported=false"));
        assert!(rendered.contains("recommended_action=restart_controller_process"));
        assert!(rendered.contains("verification_command=fleet controller signing-rotation-status"));
        for forbidden in [
            "controller_private.key",
            "private-key-secret",
            "tls/fullchain.pem",
            "admin-secret",
            "task_payload_body",
            "uptime",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "renderer must not expose {forbidden}"
            );
        }
    }

    #[test]
    fn controller_signing_rotation_restart_action_renderer_omits_key_material() {
        let body = serde_json::json!({
            "controller_id": "default-controller",
            "action": "external_service_manager_restart",
            "action_status": "audit_recorded_external_restart_required",
            "restart_required": true,
            "reload_supported": false,
            "readiness": "dual_trust_active_agents_migrating",
            "bootstrap_guard": "active_mismatch_selected",
            "active_signing_fingerprint_prefix": "old-fp-12345678",
            "selected_signing_fingerprint_prefix": "new-fp-12345678",
            "service_command": "fleet controller restart-service --dry-run",
            "verification_commands": [
                "fleet controller signing-rotation restart-plan --controller-url <controller-url>"
            ],
            "safety_notes": [
                "restart is executed outside the HTTP handler"
            ],
            "controller_private_key_path": "controller_private.key",
            "candidate_private_key": "private-key-secret",
            "tls_certificate_path": "tls/fullchain.pem",
            "admin_token": "admin-secret",
            "task_payload_body": "{\"program\":\"uptime\"}"
        })
        .to_string();

        let lines = render_controller_signing_rotation_restart_action_for_cli(&body)
            .expect("restart action response should render");
        let rendered = lines.join("\n");

        assert!(rendered.contains("controller_id=default-controller"));
        assert!(rendered.contains("action=external_service_manager_restart"));
        assert!(rendered.contains("action_status=audit_recorded_external_restart_required"));
        assert!(rendered.contains("service_command=fleet controller restart-service --dry-run"));
        for forbidden in [
            "controller_private.key",
            "private-key-secret",
            "tls/fullchain.pem",
            "admin-secret",
            "task_payload_body",
            "uptime",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "renderer must not expose {forbidden}"
            );
        }
    }

    #[test]
    fn controller_signing_trust_bundle_rollout_renderer_omits_key_material() {
        let body = serde_json::json!({
            "controller_id": "default-controller",
            "persisted_state": "dual_trust_active",
            "attempted_count": 2,
            "updated_count": 1,
            "skipped_count": 1,
            "failed_count": 0,
            "entries_count": 2,
            "current_fingerprint_prefix": "new-fp-12345678",
            "previous_fingerprint_prefix": "old-fp-12345678",
            "agent_results": [
                {"agent_id":"agent-1","status":"sent"},
                {"agent_id":"agent-2","status":"skipped_not_connected"}
            ],
            "previous_public_key_path": "old_controller_public.key",
            "current_public_key": "new-public-key-body",
            "previous_public_key": "old-public-key-body",
            "private_key": "private-key-secret",
            "admin_token": "admin-secret"
        })
        .to_string();

        let lines = render_controller_signing_trust_bundle_rollout_for_cli(&body)
            .expect("rollout response should render");
        let rendered = lines.join("\n");

        assert!(rendered.contains("controller_id=default-controller"));
        assert!(rendered.contains("updated_count=1"));
        assert!(rendered.contains("agent_id=agent-1\tstatus=sent"));
        for forbidden in [
            "old_controller_public.key",
            "new-public-key-body",
            "old-public-key-body",
            "private-key-secret",
            "admin-secret",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "renderer must not expose {forbidden}"
            );
        }
    }

    #[test]
    fn controller_signing_trust_bundle_staged_rollout_renderer_omits_key_material() {
        let body = serde_json::json!({
            "controller_id": "default-controller",
            "persisted_state": "dual_trust_active",
            "rollout_state": "waiting_for_ack",
            "target_count": 3,
            "planned_count": 1,
            "attempted_count": 1,
            "updated_count": 1,
            "skipped_count": 1,
            "failed_count": 0,
            "already_current_count": 1,
            "unavailable_count": 0,
            "pending_count": 1,
            "entries_count": 2,
            "current_fingerprint_prefix": "new-fp-12345678",
            "previous_fingerprint_prefix": "old-fp-12345678",
            "agent_results": [
                {"agent_id":"agent-2","status":"sent"}
            ],
            "previous_public_key_path": "old_controller_public.key",
            "current_public_key": "new-public-key-body",
            "previous_public_key": "old-public-key-body",
            "private_key": "private-key-secret",
            "admin_token": "admin-secret",
            "task_payload_body": "{\"program\":\"uptime\"}"
        })
        .to_string();

        let lines = render_controller_signing_trust_bundle_staged_rollout_for_cli(&body)
            .expect("staged rollout response should render");
        let rendered = lines.join("\n");

        assert!(rendered.contains("controller_id=default-controller"));
        assert!(rendered.contains("rollout_state=waiting_for_ack"));
        assert!(rendered.contains("target_count=3"));
        assert!(rendered.contains("planned_count=1"));
        assert!(rendered.contains("already_current_count=1"));
        assert!(rendered.contains("pending_count=1"));
        assert!(rendered.contains("agent_id=agent-2\tstatus=sent"));
        for forbidden in [
            "old_controller_public.key",
            "new-public-key-body",
            "old-public-key-body",
            "private-key-secret",
            "admin-secret",
            "task_payload_body",
            "uptime",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "renderer must not expose {forbidden}"
            );
        }
    }

    #[test]
    fn controller_signing_staged_trust_bundle_request_uses_explicit_body() {
        let body = controller_signing_staged_trust_bundle_request_body(
            Some(PathBuf::from(
                "/var/lib/fleet/controller/controller_public.key.bak",
            )),
            vec!["agent-1".to_owned(), "agent-2".to_owned()],
            10,
            1,
            30,
        );
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert_eq!(
            CONTROLLER_SIGNING_STAGED_TRUST_BUNDLE_PATH,
            "/api/controller/signing-rotation/rollout-trust-bundle/staged"
        );
        assert_eq!(
            json["previous_public_key_path"],
            "/var/lib/fleet/controller/controller_public.key.bak"
        );
        assert_eq!(json["agent_ids"], serde_json::json!(["agent-1", "agent-2"]));
        assert_eq!(json["batch_size"], 10);
        assert_eq!(json["max_failures"], 1);
        assert_eq!(json["ack_timeout_seconds"], 30);
        assert!(json.get("private_key").is_none());
        assert!(json.get("admin_token").is_none());
    }

    #[test]
    fn parses_remediation_cli_commands() {
        let remediations = Cli::try_parse_from([
            "fleet",
            "remediations",
            "list",
            "--agent-id",
            "agent-1",
            "--policy-id",
            "nginx-running",
            "--limit",
            "10",
        ])
        .expect("valid remediations list command");
        let Command::Remediations(command) = remediations.command else {
            panic!("expected remediations command");
        };
        let RemediationsSubcommand::List {
            agent_id,
            policy_id,
            limit,
            ..
        } = command.command
        else {
            panic!("expected remediation list command");
        };
        assert_eq!(agent_id.as_deref(), Some("agent-1"));
        assert_eq!(policy_id.as_deref(), Some("nginx-running"));
        assert_eq!(limit, 10);

        let remediation_approve = Cli::try_parse_from([
            "fleet",
            "remediations",
            "approve",
            "rem-1",
            "--approval-id",
            "approval-1",
            "--job-id",
            "job-1",
            "--runbook",
            "runbooks/remediate.yml",
        ])
        .expect("valid remediations approve command");
        assert!(matches!(
            remediation_approve.command,
            Command::Remediations(RemediationsCommand {
                command: RemediationsSubcommand::Approve { .. }
            })
        ));
    }

    #[test]
    fn cli_profile_save_read_roundtrip_uses_secure_permissions() {
        let dir = unique_test_dir("cli-profile");
        let path = dir.join("profile.json");
        let profile = CliProfile {
            controller_url: "https://fleet.example.com".to_owned(),
            admin_token: "admin-secret".to_owned(),
        };

        save_cli_profile(&path, &profile).unwrap();
        let loaded = read_cli_profile(&path).unwrap();

        assert_eq!(loaded, profile);
        #[cfg(unix)]
        {
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    #[cfg(unix)]
    fn cli_profile_rejects_insecure_permissions() {
        let dir = unique_test_dir("cli-profile-insecure");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("profile.json");
        fs::write(
            &path,
            r#"{"controller_url":"https://fleet.example.com","admin_token":"admin-secret"}"#,
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        assert!(matches!(read_cli_profile(&path), Err(CliError::Http(_))));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn protected_api_resolves_profile_and_allows_explicit_override() {
        let dir = unique_test_dir("cli-profile-resolve");
        let path = dir.join("profile.json");
        save_cli_profile(
            &path,
            &CliProfile {
                controller_url: "https://profile.example.com".to_owned(),
                admin_token: "profile-token".to_owned(),
            },
        )
        .unwrap();

        let from_profile = resolve_protected_api(&ProtectedApiArgs {
            controller_url: None,
            admin_token: None,
            profile_path: path.clone(),
        })
        .unwrap();
        assert_eq!(from_profile.controller_url, "https://profile.example.com");
        assert_eq!(from_profile.admin_token, "profile-token");

        let explicit = resolve_protected_api(&ProtectedApiArgs {
            controller_url: Some("https://explicit.example.com".to_owned()),
            admin_token: Some("explicit-token".to_owned()),
            profile_path: dir.join("missing.json"),
        })
        .unwrap();
        assert_eq!(explicit.controller_url, "https://explicit.example.com");
        assert_eq!(explicit.admin_token, "explicit-token");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn protected_api_client_sends_profile_bearer_token() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (sender, receiver) = mpsc::channel();
        let (ready_sender, ready_receiver) = mpsc::channel();
        let server = thread::spawn(move || {
            ready_sender.send(()).unwrap();
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = String::new();
                let mut buffer = [0_u8; 4096];
                loop {
                    let read = match stream.read(&mut buffer) {
                        Ok(read) => read,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(error) if error.kind() == std::io::ErrorKind::TimedOut => break,
                        Err(error) => panic!("mock server read failed: {error}"),
                    };
                    if read == 0 {
                        break;
                    }
                    request.push_str(&String::from_utf8_lossy(&buffer[..read]));
                    if request.contains("\r\n\r\n") {
                        break;
                    }
                }
                if request.contains("GET /api/agents HTTP/1.1") {
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                        )
                        .unwrap();
                    sender.send(request).unwrap();
                    return;
                }
            }
            panic!("mock server did not receive protected API request");
        });
        let dir = unique_test_dir("cli-profile-http");
        let path = dir.join("profile.json");
        save_cli_profile(
            &path,
            &CliProfile {
                controller_url: format!("http://127.0.0.1:{port}"),
                admin_token: "profile-token".to_owned(),
            },
        )
        .unwrap();
        ready_receiver.recv_timeout(Duration::from_secs(2)).unwrap();

        let client = resolve_protected_api(&ProtectedApiArgs {
            controller_url: None,
            admin_token: None,
            profile_path: path.clone(),
        })
        .unwrap();
        let mut last_error = None;
        let mut body = None;
        for _ in 0..3 {
            match client.get("/api/agents") {
                Ok(response) => {
                    body = Some(response);
                    break;
                }
                Err(error) => {
                    last_error = Some(error);
                    thread::sleep(Duration::from_millis(25));
                }
            }
        }
        let body = body.unwrap_or_else(|| panic!("protected API request failed: {last_error:?}"));
        let request = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        server.join().unwrap();

        assert_eq!(body, "{}");
        assert!(request.contains("GET /api/agents HTTP/1.1"));
        assert!(request.contains("authorization: Bearer profile-token"));
        assert!(!request.contains("admin-secret"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn api_error_rendering_distinguishes_common_statuses() {
        assert_eq!(
            render_http_status_error(401),
            "unauthorized: admin token is missing or invalid"
        );
        assert_eq!(
            render_http_status_error(403),
            "forbidden: admin token lacks required permission"
        );
        assert_eq!(
            render_http_status_error(404),
            "not found: requested resource does not exist"
        );
        assert_eq!(
            render_http_status_error(409),
            "conflict: request conflicts with current state"
        );
    }

    #[test]
    fn selector_preview_renderer_uses_controller_counts() {
        let body = serde_json::json!({
            "matched_count": 2,
            "selected_count": 1,
            "disabled_count": 1,
            "offline_count": 0,
            "warnings": [
                {"code": "disabled_agents_excluded", "message": "1 disabled agent excluded"}
            ],
            "agents": [
                {
                    "agent_id": "agent-1",
                    "name": "web-01",
                    "status": "online",
                    "labels": [],
                    "selected_for_dispatch": true
                }
            ]
        })
        .to_string();

        let lines = render_selector_preview_for_cli(&body).unwrap();

        assert!(lines.contains(&"matched_count=2".to_owned()));
        assert!(lines.contains(&"selected_count=1".to_owned()));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("disabled_agents_excluded"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("agent=agent-1") && line.contains("selected=true"))
        );
    }

    #[test]
    fn audit_export_path_and_jsonl_renderer_use_controller_page() {
        assert_eq!(
            audit_export_path(Some("security"), 10, Some("3:1")),
            "/api/audit/export?category=security&limit=10&before=3%3A1"
        );
        let body = serde_json::json!({
            "items": [
                {
                    "category": "security",
                    "action": "invalid_signature",
                    "actor": "system",
                    "target": "agent-1",
                    "value_kind": "secret_ref",
                    "value": "secret_ref",
                    "occurred_at_ms": 3000,
                    "cursor": "3:2"
                }
            ],
            "next_cursor": null
        })
        .to_string();

        let lines = render_audit_export_jsonl(&body).unwrap();

        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("\"category\":\"security\""));
        assert!(lines[0].contains("\"value\":\"secret_ref\""));
        assert!(!lines[0].contains("raw-secret"));
        assert!(serde_json::from_str::<serde_json::Value>(&lines[0]).is_ok());
    }

    #[test]
    fn remediation_cli_paths_and_renderer_hide_payload_bodies() {
        assert_eq!(
            remediations_list_path(Some("agent/1"), Some("nginx running"), 10),
            "/api/remediations?agent_id=agent%2F1&policy_id=nginx%20running&limit=10"
        );
        let body = serde_json::json!([
            {
                "id": "rem-1",
                "policy_id": "nginx-running",
                "policy_name": "nginx-running",
                "agent_id": "agent-1",
                "runbook_ref": "runbooks/remediate.yml",
                "status": "proposed",
                "job_id": null,
                "lifecycle_source": "persisted",
                "verification_assignment_status": "failed",
                "legacy_state": "legacy_unverified",
                "runbook_document": "kind: Runbook\n# secret-value-should-not-leak",
                "command_output": "secret-value-should-not-leak",
                "rendered_body": "secret-value-should-not-leak"
            }
        ])
        .to_string();

        let lines = render_remediation_api_for_cli(&body).unwrap();

        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("remediation_id=rem-1"));
        assert!(lines[0].contains("policy_id=nginx-running"));
        assert!(lines[0].contains("lifecycle_source=persisted"));
        assert!(lines[0].contains("verification_assignment_status=failed"));
        assert!(lines[0].contains("legacy_state=legacy_unverified"));
        assert!(!lines.join("\n").contains("kind: Runbook"));
        assert!(!lines.join("\n").contains("runbook_document"));
        assert!(!lines.join("\n").contains("secret-value-should-not-leak"));
    }

    #[test]
    fn remediation_approve_body_contains_runbook_only_in_request_payload() {
        let body = build_remediation_approve_body(
            "approval-1",
            "job-1",
            "kind: Runbook\n# secret-value-should-not-leak",
            30,
            300,
            Some("nonce-1"),
            "approved",
        );
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert_eq!(json["approval_id"], "approval-1");
        assert_eq!(json["job_id"], "job-1");
        assert_eq!(
            json["runbook_document"],
            "kind: Runbook\n# secret-value-should-not-leak"
        );
        assert_eq!(json["nonce_prefix"], "nonce-1");
    }

    #[test]
    fn remote_run_body_uses_selector_and_omits_admin_token() {
        let command = RunCommand {
            selector: Some("role=web".to_owned()),
            confirm_risk: true,
            controller_url: Some("http://127.0.0.1:7700".to_owned()),
            admin_token: Some("admin-secret".to_owned()),
            profile_path: PathBuf::from(DEFAULT_CLI_PROFILE_PATH),
            remote: true,
            job_id: Some("job-cli-1".to_owned()),
            timeout_seconds: 45,
            command: vec!["uptime".to_owned(), "-a".to_owned()],
        };

        let body =
            remote_run_request_body(&command, "job-cli-1", "uptime", &["-a".to_owned()]).unwrap();

        assert!(body.contains("\"selector\":\"role=web\""));
        assert!(body.contains("\"timeout_seconds\":45"));
        assert!(body.contains("\"expires_in_seconds\":300"));
        assert!(!body.contains("admin-secret"));
    }

    #[test]
    fn remote_run_pending_approval_response_does_not_poll_output() {
        let response = serde_json::json!({
            "job_id": "job-cli-1",
            "target_count": 1,
            "assignment_count": 1,
            "status": "pending_approval",
            "approval_request_id": "approval-1"
        });

        assert_eq!(remote_run_response_status(&response), "pending_approval");
        assert_eq!(
            remote_run_response_approval_request_id(&response),
            Some("approval-1")
        );
        assert!(remote_run_response_needs_approval(&response));
    }

    #[test]
    fn run_without_selector_uses_local_context() {
        let cli = Cli::try_parse_from(["fleet", "run", "--confirm-risk", "uptime"])
            .expect("valid local run command");

        let Command::Run(command) = cli.command else {
            panic!("expected run command");
        };

        assert_eq!(command.selector, None);
        assert_eq!(run_context_label(command.selector.as_deref()), "local");
        assert_eq!(
            run_context_label(Some("role=web")),
            "selector:role=web".to_owned()
        );
    }

    #[test]
    fn parses_apply_command() {
        let cli = Cli::try_parse_from(["fleet", "apply", "examples/runbooks/nginx-basic.yml"])
            .expect("valid command");

        let Command::Apply(command) = cli.command else {
            panic!("expected apply command");
        };

        assert_eq!(
            command.file,
            PathBuf::from("examples/runbooks/nginx-basic.yml")
        );
    }

    #[test]
    fn parses_retention_cleanup_command() {
        let cli = Cli::try_parse_from([
            "fleet",
            "retention",
            "cleanup",
            "--data-dir",
            "/tmp/fleet",
            "--older-than-days",
            "7",
            "--dry-run",
        ])
        .expect("valid command");

        let Command::Retention(RetentionCommand {
            command:
                RetentionSubcommand::Cleanup {
                    data_dir,
                    older_than_days,
                    dry_run,
                },
        }) = cli.command
        else {
            panic!("expected retention cleanup command");
        };

        assert_eq!(data_dir, PathBuf::from("/tmp/fleet"));
        assert_eq!(older_than_days, 7);
        assert!(dry_run);
    }

    #[test]
    fn retention_cleanup_execution_writes_audit_event() {
        let data_dir = unique_demo_dir();
        fs::create_dir_all(controller_dir(&data_dir)).unwrap();
        fleet_store::SqliteStore::open(controller_db_path(&data_dir)).unwrap();

        let cli = Cli::try_parse_from([
            "fleet",
            "retention",
            "cleanup",
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--older-than-days",
            "1",
        ])
        .expect("valid command");

        execute(cli).unwrap();

        let store = fleet_store::SqliteStore::open(controller_db_path(&data_dir)).unwrap();
        let audits = store
            .list_audit_events_by_category(fleet_domain::AuditCategory::Security, 10)
            .unwrap();
        assert!(
            audits
                .iter()
                .any(|event| event.action == "retention_cleanup")
        );

        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn controller_backup_creates_archive_with_metadata() {
        let data_dir = initialized_controller_backup_fixture("backup-metadata");
        let output = unique_test_dir("backup-output").join("controller-backup.json");

        execute_controller_backup(&data_dir, &output).unwrap();

        let archive = read_backup_archive(&output).unwrap();
        validate_backup_archive(&archive).unwrap();
        assert_eq!(archive.format, CONTROLLER_BACKUP_FORMAT);
        assert_eq!(archive.format_version, CONTROLLER_BACKUP_FORMAT_VERSION);
        assert_eq!(archive.schema_version, fleet_store::CURRENT_SCHEMA_VERSION);
        assert_eq!(archive.sqlite_integrity_check, "ok");
        assert_eq!(archive.source_data_dir, data_dir.display().to_string());
        assert!(archive.created_at_ms > 0);
        assert!(
            archive
                .files
                .iter()
                .any(|file| file.path == "controller/fleet.db")
        );
        assert!(
            archive
                .files
                .iter()
                .any(|file| file.path == "controller/controller_private.key")
        );

        let _ = fs::remove_dir_all(data_dir);
        let _ = fs::remove_file(output);
    }

    #[test]
    fn controller_restore_dry_run_does_not_write() {
        let data_dir = initialized_controller_backup_fixture("restore-dry-run-source");
        let output = unique_test_dir("restore-dry-run-output").join("controller-backup.json");
        let restore_dir = unique_test_dir("restore-dry-run-target");
        execute_controller_backup(&data_dir, &output).unwrap();

        execute_controller_restore(&restore_dir, &output, true, false).unwrap();

        assert!(!controller_dir(&restore_dir).exists());

        let _ = fs::remove_dir_all(data_dir);
        let _ = fs::remove_dir_all(restore_dir);
        let _ = fs::remove_file(output);
    }

    #[test]
    fn controller_backup_restore_roundtrip_restores_database_and_keys() {
        let data_dir = initialized_controller_backup_fixture("restore-roundtrip-source");
        let output = unique_test_dir("restore-roundtrip-output").join("controller-backup.json");
        let restore_dir = unique_test_dir("restore-roundtrip-target");
        execute_controller_backup(&data_dir, &output).unwrap();

        execute_controller_restore(&restore_dir, &output, false, false).unwrap();

        let restored_store =
            fleet_store::SqliteStore::open(controller_db_path(&restore_dir)).unwrap();
        assert!(
            restored_store
                .verify_admin_token_hash("backup-admin-hash")
                .unwrap()
        );
        assert!(
            controller_dir(&restore_dir)
                .join("controller_private.key")
                .is_file()
        );
        assert!(
            controller_dir(&restore_dir)
                .join("controller_public.key")
                .is_file()
        );

        let _ = fs::remove_dir_all(data_dir);
        let _ = fs::remove_dir_all(restore_dir);
        let _ = fs::remove_file(output);
    }

    #[test]
    fn controller_restore_refuses_overwrite_without_force() {
        let data_dir = initialized_controller_backup_fixture("restore-overwrite-source");
        let output = unique_test_dir("restore-overwrite-output").join("controller-backup.json");
        let restore_dir = initialized_controller_backup_fixture("restore-overwrite-target");
        execute_controller_backup(&data_dir, &output).unwrap();

        let result = execute_controller_restore(&restore_dir, &output, false, false);

        assert!(
            matches!(result, Err(CliError::Http(message)) if message.contains("refusing to overwrite"))
        );

        let _ = fs::remove_dir_all(data_dir);
        let _ = fs::remove_dir_all(restore_dir);
        let _ = fs::remove_file(output);
    }

    #[test]
    fn controller_restore_refuses_incompatible_schema_version() {
        let data_dir = initialized_controller_backup_fixture("restore-schema-source");
        let output = unique_test_dir("restore-schema-output").join("controller-backup.json");
        let restore_dir = unique_test_dir("restore-schema-target");
        execute_controller_backup(&data_dir, &output).unwrap();
        let mut archive = read_backup_archive(&output).unwrap();
        archive.schema_version = fleet_store::CURRENT_SCHEMA_VERSION + 1;
        write_backup_archive(&output, &archive).unwrap();

        let result = execute_controller_restore(&restore_dir, &output, true, false);

        assert!(
            matches!(result, Err(CliError::Http(message)) if message.contains("newer than this binary supports"))
        );

        let _ = fs::remove_dir_all(data_dir);
        let _ = fs::remove_dir_all(restore_dir);
        let _ = fs::remove_file(output);
    }

    #[test]
    fn controller_restore_rejects_corrupted_sqlite_archive() {
        let data_dir = initialized_controller_backup_fixture("restore-corrupt-source");
        let output = unique_test_dir("restore-corrupt-output").join("controller-backup.json");
        let restore_dir = unique_test_dir("restore-corrupt-target");
        execute_controller_backup(&data_dir, &output).unwrap();
        let mut archive = read_backup_archive(&output).unwrap();
        let corrupt = b"not a sqlite database";
        let db = archive
            .files
            .iter_mut()
            .find(|file| file.path == "controller/fleet.db")
            .expect("backup must include db");
        db.content_hex = hex_encode(corrupt);
        db.size_bytes = corrupt.len() as u64;
        db.sha256 = sha256_hex(corrupt);
        write_backup_archive(&output, &archive).unwrap();

        let result = execute_controller_restore(&restore_dir, &output, false, false);

        assert!(matches!(
            result,
            Err(CliError::Store(_)) | Err(CliError::Http(_))
        ));
        assert!(!controller_dir(&restore_dir).exists());

        let _ = fs::remove_dir_all(data_dir);
        let _ = fs::remove_dir_all(restore_dir);
        let _ = fs::remove_file(output);
    }

    #[test]
    fn parses_demo_command() {
        let cli = Cli::try_parse_from(["fleet", "demo", "--keep-temp", "--port", "17700"])
            .expect("valid command");

        let Command::Demo(command) = cli.command else {
            panic!("expected demo command");
        };

        assert!(command.keep_temp);
        assert_eq!(command.port, Some(17700));
    }

    #[test]
    fn demo_rejects_unavailable_port() {
        let Ok(listener) = TcpListener::bind("127.0.0.1:0") else {
            return;
        };
        let port = listener.local_addr().unwrap().port();

        assert!(matches!(
            ensure_loopback_port_available(port),
            Err(CliError::Http(_))
        ));
    }

    #[test]
    fn parses_linux_meminfo_fixture() {
        let body = "MemTotal:       16384256 kB\nMemAvailable:   8123456 kB\n";

        assert_eq!(linux_meminfo_kb(body, "MemTotal"), Some(16_384_256));
        assert_eq!(linux_meminfo_kb(body, "MemAvailable"), Some(8_123_456));
        assert_eq!(linux_meminfo_kb(body, "SwapTotal"), None);
    }

    #[test]
    fn parses_linux_network_interfaces_fixture() {
        let body = "Inter-| Receive\n face |bytes\n    lo: 1 0 0\n  eth0: 2 0 0\n";

        assert_eq!(
            linux_network_interfaces(body),
            vec!["lo".to_owned(), "eth0".to_owned()]
        );
    }

    #[test]
    fn parses_df_root_usage_fixture() {
        let body = "Filesystem 1K-blocks Used Available Use% Mounted on\n/dev/root 102400 51200 51200 50% /\n";

        assert_eq!(
            parse_df_root_usage(body),
            Some(DiskUsage {
                filesystem: "/dev/root".to_owned(),
                total_kb: 102_400,
                used_kb: 51_200,
                available_kb: 51_200,
                used_percent: 50,
            })
        );
    }

    #[test]
    fn missing_df_root_usage_is_graceful() {
        let body = "Filesystem 1K-blocks Used Available Use% Mounted on\n/dev/root 102400 51200 51200 50% /data\n";

        assert_eq!(parse_df_root_usage(body), None);
    }

    #[test]
    fn parses_linux_mounts_without_exposing_options() {
        let body = "\
/dev/sda1 / ext4 rw,relatime 0 0
/dev/disk\\040with\\040space /mnt/space ext4 ro,nosuid 0 0
server:/export /mnt/nfs nfs4 rw,vers=4.2 0 0
";

        let mounts = parse_linux_mounts(body);

        assert_eq!(
            mounts,
            vec![
                MountFact {
                    source: "/dev/sda1".to_owned(),
                    mount_point: "/".to_owned(),
                    fs_type: "ext4".to_owned(),
                    read_only: false,
                },
                MountFact {
                    source: "/dev/disk with space".to_owned(),
                    mount_point: "/mnt/space".to_owned(),
                    fs_type: "ext4".to_owned(),
                    read_only: true,
                },
                MountFact {
                    source: "server:/export".to_owned(),
                    mount_point: "/mnt/nfs".to_owned(),
                    fs_type: "nfs4".to_owned(),
                    read_only: false,
                },
            ]
        );
    }

    #[test]
    fn collects_linux_block_device_inventory_from_sysfs_fixture() {
        let dir = unique_test_dir("block-devices");
        fs::create_dir_all(dir.join("sda").join("queue")).unwrap();
        fs::write(dir.join("sda").join("size"), "2097152\n").unwrap();
        fs::write(dir.join("sda").join("removable"), "0\n").unwrap();
        fs::write(dir.join("sda").join("queue").join("rotational"), "1\n").unwrap();
        fs::create_dir_all(dir.join("sda").join("sda1")).unwrap();
        fs::write(dir.join("sda").join("sda1").join("size"), "1048576\n").unwrap();
        fs::create_dir_all(dir.join("nvme0n1").join("queue")).unwrap();
        fs::write(dir.join("nvme0n1").join("size"), "4194304\n").unwrap();
        fs::write(dir.join("nvme0n1").join("removable"), "0\n").unwrap();
        fs::write(dir.join("nvme0n1").join("queue").join("rotational"), "0\n").unwrap();
        fs::create_dir_all(dir.join("nvme0n1").join("nvme0n1p1")).unwrap();
        fs::write(
            dir.join("nvme0n1").join("nvme0n1p1").join("size"),
            "2097152\n",
        )
        .unwrap();
        fs::create_dir_all(dir.join("loop0")).unwrap();

        let devices = collect_linux_block_devices(&dir).unwrap();

        assert_eq!(
            devices,
            vec![
                BlockDeviceFact {
                    name: "nvme0n1".to_owned(),
                    kind: "disk".to_owned(),
                    size_kb: Some(2_097_152),
                    removable: Some(false),
                    rotational: Some(false),
                    partitions: vec![BlockPartitionFact {
                        name: "nvme0n1p1".to_owned(),
                        size_kb: Some(1_048_576),
                    }],
                },
                BlockDeviceFact {
                    name: "sda".to_owned(),
                    kind: "disk".to_owned(),
                    size_kb: Some(1_048_576),
                    removable: Some(false),
                    rotational: Some(true),
                    partitions: vec![BlockPartitionFact {
                        name: "sda1".to_owned(),
                        size_kb: Some(524_288),
                    }],
                },
            ]
        );
    }

    #[test]
    fn parses_systemd_failed_service_summary_fixture() {
        let body = "\
UNIT LOAD ACTIVE SUB DESCRIPTION
nginx.service loaded failed failed A high performance web server
postgresql.service loaded failed failed PostgreSQL database server

2 loaded units listed.
";

        assert_eq!(
            parse_systemd_failed_services(body),
            vec!["nginx.service".to_owned(), "postgresql.service".to_owned()]
        );
    }

    #[test]
    fn missing_systemd_service_status_is_graceful() {
        assert_eq!(
            systemd_service_summary_unavailable(),
            ServiceSummary {
                status_available: false,
                failed_units_count: None,
                failed_units: Vec::new(),
            }
        );
    }

    #[test]
    fn collect_local_facts_is_structured_and_secret_free() {
        let facts = collect_local_facts();

        assert!(facts.get("os").is_some());
        assert!(
            facts
                .get("system_time_ms")
                .and_then(serde_json::Value::as_u64)
                .is_some()
        );
        assert!(
            facts
                .get("cpu")
                .and_then(|value| value.get("logical_count"))
                .is_some()
        );
        assert!(facts.get("network").is_some());
        let memory = facts
            .get("memory")
            .expect("facts must include memory inventory");
        assert!(
            memory.get("total_kb").is_some(),
            "facts must include total memory capacity",
        );
        assert!(
            memory.get("module_count_known").is_some(),
            "facts must expose whether memory module count is known",
        );
        assert!(
            memory.get("available_kb").is_none(),
            "facts must not include current memory availability",
        );
        assert!(
            memory.get("used_kb").is_none(),
            "facts must not include current memory usage",
        );
        assert!(
            memory.get("used_percent").is_none(),
            "facts must not include current memory usage percent",
        );
        let disk = facts
            .get("disk")
            .expect("facts must include disk inventory");
        assert!(
            disk.get("root_capacity_known").is_some(),
            "facts must expose whether root disk capacity is known",
        );
        assert!(
            disk.get("device_inventory_known").is_some(),
            "facts must expose whether disk device inventory is known",
        );
        assert!(
            disk.get("device_count").is_some(),
            "facts must include disk device count",
        );
        assert!(
            disk.get("devices")
                .and_then(serde_json::Value::as_array)
                .is_some(),
            "facts must include disk device inventory",
        );
        assert!(
            disk.get("mount_inventory_known").is_some(),
            "facts must expose whether mount inventory is known",
        );
        assert!(
            disk.get("mount_count").is_some(),
            "facts must include mount count",
        );
        assert!(
            disk.get("mounts")
                .and_then(serde_json::Value::as_array)
                .is_some(),
            "facts must include mount layout",
        );
        assert!(
            disk.get("root_total_kb").is_some(),
            "facts must include root disk total capacity",
        );
        assert!(
            disk.get("used_kb").is_none(),
            "facts must not include current disk usage",
        );
        assert!(
            disk.get("available_kb").is_none(),
            "facts must not include current disk availability",
        );
        assert!(
            disk.get("used_percent").is_none(),
            "facts must not include current disk usage percent",
        );
        assert!(
            facts
                .get("degraded")
                .and_then(|value| value.get("status"))
                .is_some()
        );
        let body = facts.to_string();
        assert!(!body.contains("token="));
        assert!(!body.contains("secret="));
    }

    #[test]
    fn collect_local_metrics_is_structured_and_secret_free() {
        let metrics = collect_local_metrics();

        assert!(
            metrics
                .get("system_time_ms")
                .and_then(serde_json::Value::as_u64)
                .is_some()
        );
        assert!(
            metrics
                .get("cpu")
                .and_then(|value| value.get("logical_count"))
                .is_some()
        );
        assert!(
            metrics
                .get("cpu")
                .and_then(|value| value.get("usage_percent"))
                .is_some(),
            "metrics must include current CPU usage percent",
        );
        let memory = metrics
            .get("memory")
            .expect("metrics must include memory usage");
        assert!(memory.get("usage_available").is_some());
        assert!(memory.get("used_kb").is_some());
        assert!(memory.get("available_kb").is_some());
        assert!(memory.get("used_percent").is_some());
        assert!(metrics.get("process").is_some());
        let disk = metrics
            .get("disk")
            .expect("metrics must include disk usage");
        assert!(disk.get("usage_available").is_some());
        assert!(disk.get("used_kb").is_some());
        assert!(disk.get("available_kb").is_some());
        assert!(disk.get("used_percent").is_some());
        assert!(
            metrics
                .get("service")
                .and_then(|value| value.get("status_available"))
                .is_some()
        );
        let body = metrics.to_string();
        assert!(!body.contains("token="));
        assert!(!body.contains("secret="));
    }

    #[test]
    fn safe_only_runbook_does_not_require_package_manager() {
        let runbook = fleet_domain::parse_runbook_document(
            r#"
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Runbook
name: safe-only
selector: role=web
steps:
  - id: http
    port.check:
      host: 127.0.0.1
      port: 7700
"#,
        )
        .unwrap();

        assert_eq!(
            local_runbook_package_manager(&runbook),
            Ok(fleet_runner::LinuxPackageManager::Apt)
        );
    }

    #[test]
    fn runbook_snapshot_uses_agent_collectors_with_runbook_source() {
        let facts = collect_runbook_snapshot(&fleet_runner::SnapshotSpec {
            kind: fleet_runner::SnapshotKind::Facts,
        })
        .unwrap();
        let metrics = collect_runbook_snapshot(&fleet_runner::SnapshotSpec {
            kind: fleet_runner::SnapshotKind::Metrics,
        })
        .unwrap();
        let facts_body: serde_json::Value = serde_json::from_str(&facts.body).unwrap();
        let metrics_body: serde_json::Value = serde_json::from_str(&metrics.body).unwrap();

        assert_eq!(facts_body["source"], "runbook");
        assert_eq!(metrics_body["source"], "runbook");
        assert!(facts_body.get("os").is_some());
        assert!(metrics_body.get("cpu").is_some());
    }

    #[test]
    fn memory_usage_from_meminfo_calculates_current_usage() {
        let usage = memory_usage_from_meminfo(
            "MemTotal:       1000 kB\nMemFree:         100 kB\nMemAvailable:    250 kB\n",
        )
        .unwrap();

        assert_eq!(
            usage,
            MemoryUsage {
                total_kb: 1000,
                used_kb: 750,
                available_kb: 250,
                used_percent: 75,
            }
        );
    }

    #[test]
    fn cpu_usage_percent_uses_proc_stat_delta() {
        let first = parse_linux_cpu_sample("cpu  100 0 50 850 0 0 0 0 0 0\n").unwrap();
        let second = parse_linux_cpu_sample("cpu  130 0 70 900 0 0 0 0 0 0\n").unwrap();

        assert_eq!(first.total, 1000);
        assert_eq!(second.total, 1100);
        assert_eq!(cpu_usage_percent_between(first, second), Some(50.0));
    }

    #[test]
    fn rejects_invalid_command() {
        let error = Cli::try_parse_from(["fleet", "unknown"]).expect_err("invalid command");
        assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn help_includes_mvp_commands() {
        let mut command = Cli::command();
        let help = command.render_long_help().to_string();

        for expected in [
            "controller",
            "agent",
            "agents",
            "enroll-token",
            "run",
            "facts",
            "metrics",
            "logs",
            "drift",
            "apply",
            "retention",
            "demo",
        ] {
            assert!(help.contains(expected), "missing help entry: {expected}");
        }
    }

    #[test]
    fn version_flag_uses_package_version() {
        let command = Cli::command();
        let version = command.render_version().to_string();

        assert_eq!(
            version.trim(),
            format!("fleet {}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn local_facts_identify_the_fleet_executable() {
        assert_eq!(collect_local_facts()["runtime"]["executable"], "fleet");
    }

    #[test]
    fn agent_start_help_explains_enrollment_and_examples() {
        let mut command = Cli::command();
        let agent = command
            .find_subcommand_mut("agent")
            .expect("agent command should exist");
        let start = agent
            .find_subcommand_mut("start")
            .expect("agent start command should exist");
        let help = start.render_long_help().to_string();

        for expected in [
            "Start the enrolled local agent persistent session loop",
            "agent/agent.conf",
            "pinned controller fingerprint",
            "heartbeat liveness ticks",
            "static facts inventory",
            "metrics snapshots",
            "Heartbeat is only a liveness signal",
            "controller-signed tasks",
            "one outbound writer queue",
            "product-safe agent operational logs",
            "Connection failures are retried indefinitely by default",
            "The agent must be enrolled before this command can run",
            "Examples:",
            "Local development flow:",
            "--heartbeat-interval-seconds",
            "--facts-interval-seconds",
            "--metrics-interval-seconds",
            "--disable-log-upload",
            "--log-upload-interval-seconds",
            "0 means retry indefinitely",
        ] {
            assert!(help.contains(expected), "missing help entry: {expected}");
        }
    }

    #[test]
    fn agent_start_parses_log_upload_options() {
        let cli = Cli::try_parse_from([
            "fleet",
            "agent",
            "start",
            "--data-dir",
            ".fleet",
            "--facts-interval-seconds",
            "600",
            "--metrics-interval-seconds",
            "15",
            "--disable-log-upload",
            "--log-upload-interval-seconds",
            "45",
        ])
        .expect("agent start should parse");

        assert!(matches!(
            cli.command,
            Command::Agent(AgentCommand {
                command: AgentSubcommand::Start {
                    facts_interval_seconds: 600,
                    metrics_interval_seconds: 15,
                    disable_log_upload: true,
                    log_upload_interval_seconds: 45,
                    ..
                }
            })
        ));
    }

    #[test]
    fn agent_init_parses_enrollment() {
        let cli = Cli::try_parse_from([
            "fleet",
            "agent",
            "init",
            "--url",
            "http://127.0.0.1:7700",
            "--token",
            "token-1",
            "--name",
            "web-01",
            "--labels",
            "role=web,env=dev",
        ])
        .expect("agent init should parse");

        assert!(matches!(
            cli.command,
            Command::Agent(AgentCommand {
                command: AgentSubcommand::Init {
                    url,
                    token,
                    name,
                    labels,
                    ..
                }
            }) if url == "http://127.0.0.1:7700"
                && token == "token-1"
                && name == "web-01"
                && labels == "role=web,env=dev"
        ));
    }

    #[test]
    fn agent_init_parses_tls_ca_cert() {
        let cli = Cli::try_parse_from([
            "fleet",
            "agent",
            "init",
            "--url",
            "https://fleet.example.com",
            "--tls-ca-cert",
            "/etc/fleet/tls/ca.pem",
            "--token",
            "token-1",
            "--name",
            "web-01",
        ])
        .expect("agent init should parse");

        let Command::Agent(AgentCommand {
            command: AgentSubcommand::Init { tls_ca_cert, .. },
        }) = cli.command
        else {
            panic!("expected agent init command");
        };

        assert_eq!(
            tls_ca_cert.as_deref(),
            Some(Path::new("/etc/fleet/tls/ca.pem"))
        );
    }

    #[test]
    fn agent_enroll_remains_alias_for_init() {
        let cli = Cli::try_parse_from([
            "fleet",
            "agent",
            "enroll",
            "--url",
            "http://127.0.0.1:7700",
            "--token",
            "token-1",
            "--name",
            "web-01",
        ])
        .expect("agent enroll alias should parse");

        assert!(matches!(
            cli.command,
            Command::Agent(AgentCommand {
                command: AgentSubcommand::Init { name, .. }
            }) if name == "web-01"
        ));
    }

    #[test]
    fn high_risk_run_requires_confirmation() {
        let cli = Cli::try_parse_from(["fleet", "run", "uptime"]).expect("valid command");
        assert!(matches!(
            execute(cli),
            Err(CliError::HighRiskConfirmationRequired)
        ));
    }

    #[test]
    fn command_output_is_redacted_before_rendering() {
        let output = fleet_runner::CommandOutput {
            stdout: "token=abc123\n".to_owned(),
            stderr: "secret=def456\n".to_owned(),
            exit_code: 0,
            truncated: false,
        };

        let (stdout, stderr) = render_command_output(&output);

        assert_eq!(stdout, "token=[REDACTED]\n");
        assert_eq!(stderr, "secret=[REDACTED]\n");
    }

    #[test]
    fn job_output_renderer_prefixes_agent_stream_and_sequence() {
        let lines = render_job_output_api_for_cli(
            r#"[
                {"job_id":"job-1","agent_id":"agent-a","stream":"stdout","sequence":0,"data":"ok\n"},
                {"job_id":"job-1","agent_id":"agent-b","stream":"stderr","sequence":1,"data":"token=abc\n"}
            ]"#,
        )
        .unwrap();

        assert_eq!(lines[0], "[agent-a stdout #0] ok\n");
        assert_eq!(lines[1], "[agent-b stderr #1] token=[REDACTED]\n");
    }

    #[test]
    fn log_tail_keeps_last_lines_in_order() {
        let body = (0..60)
            .map(|index| format!("line-{index}"))
            .collect::<Vec<_>>()
            .join("\n");

        let lines = render_log_tail(&body);

        assert_eq!(lines.len(), LOG_TAIL_MAX_LINES);
        assert_eq!(lines.first().unwrap(), "line-10");
        assert_eq!(lines.last().unwrap(), "line-59");
    }

    #[test]
    fn log_tail_redacts_secret_like_values() {
        let lines = render_log_tail("ok\ntoken=abc123 password=p1\n");

        assert_eq!(lines, ["ok", "token=[REDACTED] password=[REDACTED]"]);
    }

    #[test]
    fn log_tail_truncates_oversized_lines() {
        let body = "x".repeat(LOG_TAIL_MAX_LINE_BYTES + 20);
        let lines = render_log_tail(&body);

        assert_eq!(lines.len(), 1);
        assert!(lines[0].ends_with("...[truncated]"));
        assert!(lines[0].len() < body.len());
    }

    #[test]
    fn log_follow_streams_appended_lines() {
        let dir = unique_test_dir("log-follow");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.log");
        fs::write(&path, "initial\n").unwrap();
        let append_path = path.clone();
        let appender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            let mut file = OpenOptions::new().append(true).open(append_path).unwrap();
            writeln!(file, "token=abc").unwrap();
        });
        let mut lines = Vec::new();

        stream_log_file(
            &path,
            LogStreamOptions {
                follow: true,
                max_duration: Some(Duration::from_millis(120)),
                poll_interval: Duration::from_millis(10),
            },
            |line| lines.push(line),
            || false,
        )
        .unwrap();
        appender.join().unwrap();

        assert_eq!(lines, ["initial", "token=[REDACTED]"]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn log_follow_can_be_canceled() {
        let dir = unique_test_dir("log-cancel");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.log");
        fs::write(&path, "initial\n").unwrap();
        let mut polls = 0;
        let mut lines = Vec::new();

        stream_log_file(
            &path,
            LogStreamOptions {
                follow: true,
                max_duration: None,
                poll_interval: Duration::from_millis(1),
            },
            |line| lines.push(line),
            || {
                polls += 1;
                polls > 1
            },
        )
        .unwrap();

        assert_eq!(lines, ["initial"]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn log_follow_respects_max_duration() {
        let dir = unique_test_dir("log-max-duration");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.log");
        fs::write(&path, "initial\n").unwrap();
        let started_at = Instant::now();

        stream_log_file(
            &path,
            LogStreamOptions {
                follow: true,
                max_duration: Some(Duration::from_millis(20)),
                poll_interval: Duration::from_millis(5),
            },
            |_line| {},
            || false,
        )
        .unwrap();

        assert!(started_at.elapsed() >= Duration::from_millis(20));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn builds_journald_service_shortcut_command() {
        let command = journald_command_for_service("nginx.service").unwrap();

        assert_eq!(command.program, "journalctl");
        assert_eq!(
            command.args,
            ["-u", "nginx.service", "--no-pager", "-n", "50"]
        );
        assert!(journald_command_for_service("nginx;reboot").is_none());
    }

    #[test]
    fn missing_log_file_is_reported_as_io_error() {
        let cli = Cli::try_parse_from([
            "fleet",
            "logs",
            "web-01",
            "--file",
            "/definitely/missing/fleet.log",
        ])
        .unwrap();

        assert!(matches!(execute(cli), Err(CliError::Io(_))));
    }

    #[test]
    fn parses_http_controller_url_for_remote_agent_with_warning_policy() {
        let endpoint = parse_controller_url("http://10.0.0.5:7700").unwrap();

        assert_eq!(endpoint.scheme, ControllerUrlScheme::Http);
        assert_eq!(endpoint.host, "10.0.0.5");
        assert_eq!(endpoint.port, 7700);
        assert_eq!(
            endpoint.api_url("/api/controller/identity"),
            "http://10.0.0.5:7700/api/controller/identity"
        );
        assert_eq!(
            endpoint.websocket_url("/api/agents/ws"),
            "ws://10.0.0.5:7700/api/agents/ws"
        );
    }

    #[test]
    fn parses_https_controller_url_for_remote_agent() {
        let endpoint = parse_controller_url("https://fleet.example.com").unwrap();

        assert_eq!(endpoint.scheme, ControllerUrlScheme::Https);
        assert_eq!(endpoint.host, "fleet.example.com");
        assert_eq!(endpoint.port, 443);
        assert_eq!(
            endpoint.api_url("/api/controller/identity"),
            "https://fleet.example.com:443/api/controller/identity"
        );
        assert_eq!(
            endpoint.websocket_url("/api/agents/ws"),
            "wss://fleet.example.com:443/api/agents/ws"
        );
    }

    #[test]
    fn rejects_wildcard_host_as_agent_controller_url() {
        assert!(matches!(
            parse_controller_url("http://0.0.0.0:7700"),
            Err(CliError::Http(_))
        ));
    }

    #[test]
    fn reconnect_backoff_is_capped() {
        assert_eq!(reconnect_backoff(1), Duration::from_secs(2));
        assert_eq!(reconnect_backoff(10), Duration::from_secs(32));
    }

    #[test]
    fn agent_log_upload_due_respects_disable_and_interval() {
        let enabled = AgentLogUploadOptions {
            enabled: true,
            interval: Duration::from_secs(30),
        };
        let disabled = AgentLogUploadOptions {
            enabled: false,
            interval: Duration::from_secs(30),
        };

        assert!(should_upload_agent_log(enabled, Duration::from_secs(30)));
        assert!(!should_upload_agent_log(enabled, Duration::from_secs(29)));
        assert!(!should_upload_agent_log(disabled, Duration::from_secs(30)));
    }

    #[test]
    fn agent_heartbeat_loop_uploads_log_immediately_by_default() {
        let mut upload_flags = Vec::new();

        let result = run_agent_heartbeat_loop_with(
            AgentHeartbeatOptions {
                once: true,
                heartbeat_interval: Duration::from_secs(30),
                facts_interval: Duration::from_secs(300),
                metrics_interval: Duration::from_secs(30),
                log_upload: AgentLogUploadOptions {
                    enabled: true,
                    interval: Duration::from_secs(30),
                },
                max_reconnect_attempts: 0,
            },
            |upload_log| {
                upload_flags.push(upload_log);
                Ok(())
            },
            |_| panic!("once mode must not sleep after success"),
        );

        assert!(result.is_ok());
        assert_eq!(upload_flags, vec![true]);
    }

    #[test]
    fn agent_heartbeat_loop_respects_disabled_log_upload() {
        let mut upload_flags = Vec::new();

        let result = run_agent_heartbeat_loop_with(
            AgentHeartbeatOptions {
                once: true,
                heartbeat_interval: Duration::from_secs(30),
                facts_interval: Duration::from_secs(300),
                metrics_interval: Duration::from_secs(30),
                log_upload: AgentLogUploadOptions {
                    enabled: false,
                    interval: Duration::from_secs(30),
                },
                max_reconnect_attempts: 0,
            },
            |upload_log| {
                upload_flags.push(upload_log);
                Ok(())
            },
            |_| panic!("once mode must not sleep after success"),
        );

        assert!(result.is_ok());
        assert_eq!(upload_flags, vec![false]);
    }

    #[test]
    fn agent_session_loop_preserves_runtime_state_across_reconnect() {
        let mut attempts = 0;
        let mut runtime_markers = Vec::new();
        let mut shared_runtime = 0_u8;

        run_agent_session_loop_with_state_for_test(
            &mut shared_runtime,
            AgentHeartbeatOptions {
                once: false,
                heartbeat_interval: Duration::from_secs(30),
                facts_interval: Duration::from_secs(300),
                metrics_interval: Duration::from_secs(30),
                log_upload: AgentLogUploadOptions {
                    enabled: false,
                    interval: Duration::from_secs(30),
                },
                max_reconnect_attempts: 1,
            },
            |runtime| {
                *runtime += 1;
                runtime_markers.push(*runtime);
                attempts += 1;
                if attempts == 1 {
                    Err(CliError::Http("temporary socket failure".to_owned()))
                } else {
                    Err(CliError::Http(
                        "controller signing fingerprint changed".to_owned(),
                    ))
                }
            },
            |_| {},
        )
        .unwrap_err();

        assert_eq!(runtime_markers, [1, 2]);
    }

    #[test]
    fn agent_heartbeat_loop_retries_connection_failures_until_configured_cap() {
        let mut attempts = 0;
        let mut sleeps = Vec::new();

        let result = run_agent_heartbeat_loop_with(
            AgentHeartbeatOptions {
                once: false,
                heartbeat_interval: Duration::from_secs(30),
                facts_interval: Duration::from_secs(300),
                metrics_interval: Duration::from_secs(30),
                log_upload: AgentLogUploadOptions {
                    enabled: true,
                    interval: Duration::from_secs(30),
                },
                max_reconnect_attempts: 2,
            },
            |_| {
                attempts += 1;
                Err(CliError::Http(format!("connection refused #{attempts}")))
            },
            |duration| sleeps.push(duration),
        );

        assert!(matches!(result, Err(CliError::Http(_))));
        assert_eq!(attempts, 3);
        assert_eq!(sleeps, vec![Duration::from_secs(2), Duration::from_secs(4)]);
    }

    #[test]
    fn agent_heartbeat_loop_current_lifecycle_sleeps_between_successful_heartbeats() {
        let mut attempts = 0;
        let mut sleeps = Vec::new();

        let result = run_agent_heartbeat_loop_with(
            AgentHeartbeatOptions {
                once: false,
                heartbeat_interval: Duration::from_secs(30),
                facts_interval: Duration::from_secs(300),
                metrics_interval: Duration::from_secs(30),
                log_upload: AgentLogUploadOptions {
                    enabled: true,
                    interval: Duration::from_secs(30),
                },
                max_reconnect_attempts: 1,
            },
            |_| {
                attempts += 1;
                if attempts == 1 {
                    Ok(())
                } else {
                    Err(CliError::Http(format!("connection refused #{attempts}")))
                }
            },
            |duration| sleeps.push(duration),
        );

        assert!(matches!(result, Err(CliError::Http(_))));
        assert_eq!(attempts, 3);
        assert_eq!(
            sleeps,
            vec![Duration::from_secs(30), Duration::from_secs(2)]
        );
    }

    #[test]
    fn agent_heartbeat_loop_once_exits_on_first_failure() {
        let mut attempts = 0;

        let result = run_agent_heartbeat_loop_with(
            AgentHeartbeatOptions {
                once: true,
                heartbeat_interval: Duration::from_secs(30),
                facts_interval: Duration::from_secs(300),
                metrics_interval: Duration::from_secs(30),
                log_upload: AgentLogUploadOptions {
                    enabled: true,
                    interval: Duration::from_secs(30),
                },
                max_reconnect_attempts: 0,
            },
            |_| {
                attempts += 1;
                Err(CliError::Http("connection refused".to_owned()))
            },
            |_| panic!("once mode must not sleep after a failed heartbeat"),
        );

        assert!(matches!(result, Err(CliError::Http(_))));
        assert_eq!(attempts, 1);
    }

    #[test]
    fn agent_session_loop_retries_connection_failures_until_configured_cap() {
        let mut attempts = 0;
        let mut sleeps = Vec::new();

        let result = run_agent_session_loop_with(
            AgentHeartbeatOptions {
                once: false,
                heartbeat_interval: Duration::from_secs(30),
                facts_interval: Duration::from_secs(300),
                metrics_interval: Duration::from_secs(30),
                log_upload: AgentLogUploadOptions {
                    enabled: true,
                    interval: Duration::from_secs(30),
                },
                max_reconnect_attempts: 2,
            },
            || {
                attempts += 1;
                Err(CliError::Http(format!("connection refused #{attempts}")))
            },
            |duration| sleeps.push(duration),
        );

        assert!(matches!(result, Err(CliError::Http(_))));
        assert_eq!(attempts, 3);
        assert_eq!(sleeps, vec![Duration::from_secs(2), Duration::from_secs(4)]);
    }

    #[test]
    fn agent_session_loop_reconnects_after_controller_close() {
        let mut attempts = 0;
        let mut sleeps = Vec::new();

        let result = run_agent_session_loop_with(
            AgentHeartbeatOptions {
                once: false,
                heartbeat_interval: Duration::from_secs(30),
                facts_interval: Duration::from_secs(300),
                metrics_interval: Duration::from_secs(30),
                log_upload: AgentLogUploadOptions {
                    enabled: true,
                    interval: Duration::from_secs(30),
                },
                max_reconnect_attempts: 1,
            },
            || {
                attempts += 1;
                if attempts == 1 {
                    Ok(AgentSessionEnd::ControllerClosed)
                } else {
                    Err(CliError::Http(format!("connection refused #{attempts}")))
                }
            },
            |duration| sleeps.push(duration),
        );

        assert!(matches!(result, Err(CliError::Http(_))));
        assert_eq!(attempts, 3);
        assert_eq!(
            sleeps,
            vec![Duration::from_secs(30), Duration::from_secs(2)]
        );
    }

    #[test]
    fn agent_session_loop_once_exits_after_controller_close() {
        let mut attempts = 0;

        let result = run_agent_session_loop_with(
            AgentHeartbeatOptions {
                once: true,
                heartbeat_interval: Duration::from_secs(30),
                facts_interval: Duration::from_secs(300),
                metrics_interval: Duration::from_secs(30),
                log_upload: AgentLogUploadOptions {
                    enabled: true,
                    interval: Duration::from_secs(30),
                },
                max_reconnect_attempts: 0,
            },
            || {
                attempts += 1;
                Ok(AgentSessionEnd::ControllerClosed)
            },
            |_| panic!("once mode must not sleep after controller close"),
        );

        assert!(result.is_ok());
        assert_eq!(attempts, 1);
    }

    #[test]
    fn agent_session_loop_treats_fingerprint_mismatch_as_fatal() {
        let mut attempts = 0;

        let result = run_agent_session_loop_with(
            AgentHeartbeatOptions {
                once: false,
                heartbeat_interval: Duration::from_secs(30),
                facts_interval: Duration::from_secs(300),
                metrics_interval: Duration::from_secs(30),
                log_upload: AgentLogUploadOptions {
                    enabled: true,
                    interval: Duration::from_secs(30),
                },
                max_reconnect_attempts: 0,
            },
            || {
                attempts += 1;
                Err(CliError::Http(
                    "controller signing fingerprint changed from a to b; re-enroll the agent"
                        .to_owned(),
                ))
            },
            |_| panic!("fatal fingerprint mismatch must not sleep or retry"),
        );

        assert!(matches!(result, Err(CliError::Http(_))));
        assert_eq!(attempts, 1);
    }

    #[test]
    fn agent_session_heartbeat_interval_controls_liveness_ticks() {
        let options = AgentHeartbeatOptions {
            once: false,
            heartbeat_interval: Duration::from_secs(30),
            facts_interval: Duration::from_secs(300),
            metrics_interval: Duration::from_secs(30),
            log_upload: AgentLogUploadOptions {
                enabled: true,
                interval: Duration::from_secs(45),
            },
            max_reconnect_attempts: 0,
        };

        assert_eq!(
            agent_session_tick_actions(
                Duration::from_secs(29),
                Duration::from_secs(29),
                Duration::from_secs(29),
                Duration::from_secs(44),
                options,
            ),
            AgentSessionTickActions::default()
        );
        assert_eq!(
            agent_session_tick_actions(
                Duration::from_secs(30),
                Duration::from_secs(30),
                Duration::from_secs(30),
                Duration::from_secs(44),
                options,
            ),
            AgentSessionTickActions {
                heartbeat: true,
                facts: false,
                metrics: true,
                log: false,
            }
        );
    }

    #[test]
    fn agent_session_facts_do_not_send_on_every_heartbeat() {
        let options = AgentHeartbeatOptions {
            once: false,
            heartbeat_interval: Duration::from_secs(30),
            facts_interval: Duration::from_secs(300),
            metrics_interval: Duration::from_secs(30),
            log_upload: AgentLogUploadOptions {
                enabled: true,
                interval: Duration::from_secs(30),
            },
            max_reconnect_attempts: 0,
        };

        assert_eq!(
            agent_session_tick_actions(
                Duration::from_secs(30),
                Duration::from_secs(299),
                Duration::from_secs(30),
                Duration::from_secs(30),
                options,
            ),
            AgentSessionTickActions {
                heartbeat: true,
                facts: false,
                metrics: true,
                log: true,
            }
        );
        assert_eq!(
            agent_session_tick_actions(
                Duration::from_secs(30),
                Duration::from_secs(300),
                Duration::from_secs(29),
                Duration::from_secs(29),
                options,
            ),
            AgentSessionTickActions {
                heartbeat: true,
                facts: true,
                metrics: false,
                log: false,
            }
        );
    }

    #[test]
    fn agent_session_metrics_respect_configured_interval() {
        let options = AgentHeartbeatOptions {
            once: false,
            heartbeat_interval: Duration::from_secs(10),
            facts_interval: Duration::from_secs(300),
            metrics_interval: Duration::from_secs(45),
            log_upload: AgentLogUploadOptions {
                enabled: true,
                interval: Duration::from_secs(30),
            },
            max_reconnect_attempts: 0,
        };

        assert_eq!(
            agent_session_tick_actions(
                Duration::from_secs(10),
                Duration::from_secs(299),
                Duration::from_secs(44),
                Duration::from_secs(30),
                options,
            ),
            AgentSessionTickActions {
                heartbeat: true,
                facts: false,
                metrics: false,
                log: true,
            }
        );
        assert_eq!(
            agent_session_tick_actions(
                Duration::from_secs(9),
                Duration::from_secs(299),
                Duration::from_secs(45),
                Duration::from_secs(29),
                options,
            ),
            AgentSessionTickActions {
                heartbeat: false,
                facts: false,
                metrics: true,
                log: false,
            }
        );
    }

    #[test]
    fn agent_session_outbound_queue_full_is_bounded() {
        let sender = test_outbound_queue(1);
        let config = test_agent_config();

        enqueue_wire_message(
            &sender,
            agent_heartbeat_message(&config, "corr-test").unwrap(),
        )
        .unwrap();
        let result = enqueue_wire_message(
            &sender,
            agent_metrics_snapshot_message(&config, "corr-test").unwrap(),
        );

        assert!(
            matches!(result, Err(CliError::Http(message)) if message.contains("queue is full"))
        );
    }

    #[test]
    fn failed_outbound_write_keeps_front_report_for_reconnect() {
        let queue = test_outbound_queue(1);
        let config = test_agent_config();
        enqueue_wire_message(
            &queue,
            agent_heartbeat_message(&config, "corr-test").unwrap(),
        )
        .unwrap();

        let failed = flush_agent_outbound_queue_with(&queue, |_| {
            Err(CliError::Http("socket write failed".to_owned()))
        });

        assert!(failed.is_err());
        assert!(queue.lock().unwrap().pending_report().is_some());

        let mut replayed_message_ids = Vec::new();
        flush_agent_outbound_queue_with(&queue, |message| {
            replayed_message_ids.push(message.message_id.clone());
            Ok(())
        })
        .unwrap();

        assert_eq!(replayed_message_ids.len(), 1);
        assert!(queue.lock().unwrap().pending_report().is_none());
    }

    #[test]
    fn agent_session_read_interrupt_is_idle_not_a_session_failure() {
        let interrupted =
            tungstenite::Error::Io(std::io::Error::from(std::io::ErrorKind::Interrupted));

        assert!(agent_session_read_error_is_idle(&interrupted));
    }

    #[test]
    fn handshake_read_interrupt_is_retryable_but_timeout_is_not() {
        let interrupted =
            tungstenite::Error::Io(std::io::Error::from(std::io::ErrorKind::Interrupted));
        let timed_out = tungstenite::Error::Io(std::io::Error::from(std::io::ErrorKind::TimedOut));

        assert!(handshake_read_error_is_retryable(&interrupted));
        assert!(!handshake_read_error_is_retryable(&timed_out));
    }

    #[test]
    fn agent_capability_names_use_explicit_probe_input() {
        let probe = AgentCapabilityProbe {
            privilege_level: fleet_protocol::CapabilityPrivilegeLevelWire::SudoAvailable,
            package_manager: Some(fleet_protocol::PackageManagerWire::Apt),
            service_manager: Some(fleet_protocol::ServiceManagerWire::Systemd),
        };

        let capabilities = agent_capability_names(&probe);

        assert!(capabilities.contains(&"persistent_session".to_owned()));
        assert!(capabilities.contains(&"command_execution".to_owned()));
        assert!(capabilities.contains(&"drift_check".to_owned()));
        assert!(capabilities.contains(&"runbook_execution".to_owned()));
        assert!(capabilities.contains(&"package_install".to_owned()));
        assert!(capabilities.contains(&"service_control".to_owned()));
    }

    #[test]
    fn agent_session_busy_task_rejects_new_assignment() {
        let sender = test_outbound_queue(4);
        let receiver = TestOutboundReceiver(sender.clone());
        let config = test_agent_config();
        let task_state = AgentTaskSessionState {
            busy: Arc::new(AtomicBool::new(true)),
            runtime: Arc::new(AgentTaskRuntimeState::default()),
            replay_guard: Arc::new(Mutex::new(fleet_runner::NonceReplayGuard::default())),
            controller_trust_bundle: Arc::new(Mutex::new(None)),
        };

        handle_agent_session_message(
            test_task_assignment_message(&config.agent_id),
            &config,
            "controller-public-key",
            "corr-test",
            &sender,
            &task_state,
        )
        .unwrap();

        let event = receiver.try_recv().expect("busy reject event");
        let fleet_protocol::WirePayload::TaskRejected {
            job_id,
            task_id,
            reason_code,
            reason,
        } = event.payload
        else {
            panic!("expected task rejected event");
        };
        assert_eq!(job_id, "job-test");
        assert_eq!(task_id, "task-test");
        assert_eq!(
            reason_code,
            fleet_protocol::TaskRejectionReasonCode::AgentBusy
        );
        assert_eq!(reason, "agent is busy");
        assert!(task_state.busy.load(Ordering::SeqCst));
    }

    #[test]
    fn agent_task_runtime_cancel_only_matches_current_task() {
        let runtime = AgentTaskRuntimeState::default();

        runtime.start_task("task-current").unwrap();

        assert!(!runtime.request_cancel("task-other").unwrap());
        assert!(!runtime.should_cancel("task-current"));
        assert!(runtime.request_cancel("task-current").unwrap());
        assert!(runtime.should_cancel("task-current"));
        assert!(!runtime.should_cancel("task-other"));

        runtime.finish_task("task-current");

        assert!(!runtime.should_cancel("task-current"));
    }

    #[test]
    fn command_execution_result_maps_cancel_timeout_and_failure() {
        let (_, canceled_status, _) =
            command_execution_result(Err(fleet_runner::RunnerError::Canceled));
        let (_, timeout_status, _) =
            command_execution_result(Err(fleet_runner::RunnerError::Timeout));
        let (_, failed_status, _) = command_execution_result(Ok(fleet_runner::CommandOutput {
            stdout: String::new(),
            stderr: "failed".to_owned(),
            exit_code: 2,
            truncated: false,
        }));

        assert_eq!(canceled_status, fleet_protocol::TaskResultStatus::Canceled);
        assert_eq!(timeout_status, fleet_protocol::TaskResultStatus::TimedOut);
        assert_eq!(failed_status, fleet_protocol::TaskResultStatus::Failed);
    }

    #[test]
    fn command_execution_result_preserves_program_start_failure_for_agent_output() {
        let (output, status, reason) = command_execution_result(Err(
            fleet_runner::RunnerError::Io("No such file or directory (os error 2)".to_owned()),
        ));

        assert_eq!(status, fleet_protocol::TaskResultStatus::Failed);
        assert_eq!(output.exit_code, -1);
        assert_eq!(
            output.stderr,
            "runner io error: No such file or directory (os error 2)"
        );
        assert_eq!(reason, output.stderr);
    }

    #[test]
    fn agent_session_due_telemetry_does_not_precede_inbound_task_handling() {
        let sender = test_outbound_queue(8);
        let receiver = TestOutboundReceiver(sender.clone());
        let config = test_agent_config();
        let task_state = AgentTaskSessionState {
            busy: Arc::new(AtomicBool::new(true)),
            runtime: Arc::new(AgentTaskRuntimeState::default()),
            replay_guard: Arc::new(Mutex::new(fleet_runner::NonceReplayGuard::default())),
            controller_trust_bundle: Arc::new(Mutex::new(None)),
        };

        handle_agent_session_message(
            test_task_assignment_message(&config.agent_id),
            &config,
            "controller-public-key",
            "corr-test",
            &sender,
            &task_state,
        )
        .unwrap();
        enqueue_agent_session_tick_messages(
            &sender,
            &config,
            "corr-test",
            AgentSessionTickActions {
                heartbeat: true,
                facts: true,
                metrics: true,
                log: true,
            },
        )
        .unwrap();

        assert!(matches!(
            receiver.try_recv().unwrap().payload,
            fleet_protocol::WirePayload::TaskRejected {
                reason_code: fleet_protocol::TaskRejectionReasonCode::AgentBusy,
                ..
            }
        ));
        assert!(matches!(
            receiver.try_recv().unwrap().payload,
            fleet_protocol::WirePayload::Heartbeat { .. }
        ));
        assert!(matches!(
            receiver.try_recv().unwrap().payload,
            fleet_protocol::WirePayload::FactsSnapshot { .. }
        ));
        assert!(matches!(
            receiver.try_recv().unwrap().payload,
            fleet_protocol::WirePayload::MetricsSnapshot { .. }
        ));
        assert!(matches!(
            receiver.try_recv().unwrap().payload,
            fleet_protocol::WirePayload::LogChunk { ref line, .. }
                if line.contains("agent_heartbeat_completed")
        ));
    }

    #[test]
    fn agent_session_task_worker_output_uses_outbound_queue() {
        let sender = test_outbound_queue(4);
        let receiver = TestOutboundReceiver(sender.clone());
        let config = test_agent_config();
        let envelope = test_task_envelope(&config.agent_id);

        send_agent_output_chunk_queue(
            &sender,
            &config,
            "corr-test",
            &envelope,
            fleet_runner::CommandOutputChunk {
                stream: fleet_runner::CommandOutputStream::Stdout,
                sequence: 7,
                data: "hello".to_owned(),
            },
        )
        .unwrap();
        send_agent_task_result_queue(
            &sender,
            &config,
            "corr-test",
            &envelope,
            0,
            fleet_protocol::TaskResultStatus::Succeeded,
            "",
        )
        .unwrap();
        send_agent_security_event_queue(&sender, &config, "corr-test", "task_checked", "ok")
            .unwrap();

        assert!(matches!(
            receiver.try_recv().unwrap().payload,
            fleet_protocol::WirePayload::OutputChunk { sequence: 7, .. }
        ));
        assert!(matches!(
            receiver.try_recv().unwrap().payload,
            fleet_protocol::WirePayload::TaskResult { exit_code: 0, .. }
        ));
        assert!(matches!(
            receiver.try_recv().unwrap().payload,
            fleet_protocol::WirePayload::SecurityEvent { ref action, .. } if action == "task_checked"
        ));
    }

    #[test]
    fn missing_program_task_reports_stderr_then_failed_result_through_outbound_queue() {
        let sender = test_outbound_queue(4);
        let receiver = TestOutboundReceiver(sender.clone());
        let config = test_agent_config();
        let envelope = test_task_envelope(&config.agent_id);
        let runtime = AgentTaskRuntimeState::default();

        run_signed_command_task_queue(
            &sender,
            &config,
            "corr-test",
            &envelope,
            fleet_protocol::CommandTaskWire {
                program: "fleet-program-that-does-not-exist".to_owned(),
                args: Vec::new(),
                timeout_ms: 1_000,
                max_output_bytes: 1_024,
            },
            &runtime,
        )
        .expect("failed program should report a task result");

        assert!(matches!(
            receiver.try_recv().unwrap().payload,
            fleet_protocol::WirePayload::TaskStarted { .. }
        ));
        let output = receiver.try_recv().expect("stderr output event");
        assert!(matches!(
            output.payload,
            fleet_protocol::WirePayload::OutputChunk {
                stream: fleet_protocol::OutputStream::Stderr,
                sequence: 0,
                ref data,
                ..
            } if data.contains("runner io error")
        ));
        assert!(matches!(
            receiver.try_recv().unwrap().payload,
            fleet_protocol::WirePayload::TaskResult {
                exit_code: -1,
                status: Some(fleet_protocol::TaskResultStatus::Failed),
                ref reason,
                ..
            } if reason.contains("runner io error")
        ));
    }

    #[test]
    fn trust_bundle_update_wire_maps_to_domain_current_previous_bundle() {
        let entries = vec![
            fleet_protocol::ControllerSigningTrustEntryWire {
                fingerprint: "controller-fp-new".to_owned(),
                public_key: "controller-public-new".to_owned(),
                role: fleet_protocol::ControllerSigningTrustRoleWire::Current,
                valid_from_ms: 10_000,
                valid_until_ms: None,
            },
            fleet_protocol::ControllerSigningTrustEntryWire {
                fingerprint: "controller-fp-old".to_owned(),
                public_key: "controller-public-old".to_owned(),
                role: fleet_protocol::ControllerSigningTrustRoleWire::Previous,
                valid_from_ms: 10_000,
                valid_until_ms: Some(20_000),
            },
        ];

        let bundle = controller_signing_trust_bundle_from_wire(&entries).unwrap();

        assert_eq!(bundle.entries().len(), 2);
        assert_eq!(
            bundle.entries()[0].role(),
            fleet_domain::ControllerSigningTrustRole::Current
        );
        assert_eq!(
            bundle.entries()[1].role(),
            fleet_domain::ControllerSigningTrustRole::Previous
        );
    }

    #[test]
    fn trust_bundle_update_rejects_duplicate_fingerprint_without_key_leak() {
        let entries = vec![
            fleet_protocol::ControllerSigningTrustEntryWire {
                fingerprint: "controller-fp-duplicate".to_owned(),
                public_key: "private-material-like-public-a".to_owned(),
                role: fleet_protocol::ControllerSigningTrustRoleWire::Current,
                valid_from_ms: 10_000,
                valid_until_ms: None,
            },
            fleet_protocol::ControllerSigningTrustEntryWire {
                fingerprint: "controller-fp-duplicate".to_owned(),
                public_key: "private-material-like-public-b".to_owned(),
                role: fleet_protocol::ControllerSigningTrustRoleWire::Previous,
                valid_from_ms: 10_000,
                valid_until_ms: Some(20_000),
            },
        ];

        let error = controller_signing_trust_bundle_from_wire(&entries)
            .expect_err("duplicate fingerprint should fail")
            .to_string();

        assert!(error.contains("duplicate"));
        assert!(!error.contains("private-material-like-public"));
    }

    #[test]
    fn trust_bundle_update_rejects_invalid_previous_window_without_key_leak() {
        let entries = vec![
            fleet_protocol::ControllerSigningTrustEntryWire {
                fingerprint: "controller-fp-new".to_owned(),
                public_key: "controller-public-new".to_owned(),
                role: fleet_protocol::ControllerSigningTrustRoleWire::Current,
                valid_from_ms: 10_000,
                valid_until_ms: None,
            },
            fleet_protocol::ControllerSigningTrustEntryWire {
                fingerprint: "controller-fp-old".to_owned(),
                public_key: "private-material-like-public-old".to_owned(),
                role: fleet_protocol::ControllerSigningTrustRoleWire::Previous,
                valid_from_ms: 20_000,
                valid_until_ms: Some(10_000),
            },
        ];

        let error = controller_signing_trust_bundle_from_wire(&entries)
            .expect_err("invalid previous trust window should fail")
            .to_string();

        assert!(error.contains("time window"));
        assert!(!error.contains("private-material-like-public-old"));
    }

    #[test]
    fn trust_bundle_update_rejects_previous_entry_without_expiry() {
        let entries = vec![
            fleet_protocol::ControllerSigningTrustEntryWire {
                fingerprint: "controller-fp-new".to_owned(),
                public_key: "controller-public-new".to_owned(),
                role: fleet_protocol::ControllerSigningTrustRoleWire::Current,
                valid_from_ms: 10_000,
                valid_until_ms: None,
            },
            fleet_protocol::ControllerSigningTrustEntryWire {
                fingerprint: "controller-fp-old".to_owned(),
                public_key: "controller-public-old".to_owned(),
                role: fleet_protocol::ControllerSigningTrustRoleWire::Previous,
                valid_from_ms: 10_000,
                valid_until_ms: None,
            },
        ];

        let error = controller_signing_trust_bundle_from_wire(&entries)
            .expect_err("previous trust entry without expiry should fail")
            .to_string();

        assert!(error.contains("previous controller signing trust entry requires an expiry"));
    }

    #[test]
    fn agent_controller_signing_trust_bundle_update_applies_in_memory_without_env() {
        let task_state = test_task_session_state();
        let entries = vec![fleet_protocol::ControllerSigningTrustEntryWire {
            fingerprint: "controller-fp-new".to_owned(),
            public_key: "controller-public-new".to_owned(),
            role: fleet_protocol::ControllerSigningTrustRoleWire::Current,
            valid_from_ms: 10_000,
            valid_until_ms: None,
        }];

        apply_agent_controller_signing_trust_bundle_update(&task_state, &entries, None).unwrap();
        let bundle = agent_controller_signing_trust_bundle(
            &task_state,
            &test_agent_config(),
            "legacy-controller-public",
        )
        .unwrap();

        assert_eq!(bundle.entries().len(), 1);
        assert_eq!(
            bundle.entries()[0].fingerprint().as_str(),
            "controller-fp-new"
        );
        assert_eq!(
            bundle.entries()[0].public_key().as_str(),
            "controller-public-new"
        );
    }

    #[test]
    fn agent_controller_signing_trust_bundle_update_emits_ack_without_material() {
        let sender = test_outbound_queue(4);
        let receiver = TestOutboundReceiver(sender.clone());
        let config = test_agent_config();
        fs::create_dir_all(config.controller_trust_bundle_path.parent().unwrap()).unwrap();
        let task_state = test_task_session_state();
        let entries = vec![fleet_protocol::ControllerSigningTrustEntryWire {
            fingerprint: "controller-fp-new".to_owned(),
            public_key: "controller-public-new".to_owned(),
            role: fleet_protocol::ControllerSigningTrustRoleWire::Current,
            valid_from_ms: 10_000,
            valid_until_ms: None,
        }];
        let message = fleet_protocol::WireMessage::new(
            "msg-trust-update",
            "corr-trust-update",
            Some(config.agent_id.clone()),
            10_000,
            fleet_protocol::WirePayload::ControllerSigningTrustBundleUpdate { entries },
        );

        handle_agent_session_message(
            message,
            &config,
            "legacy-controller-public",
            "corr-test",
            &sender,
            &task_state,
        )
        .unwrap();

        let ack = receiver.try_recv().expect("ack should be enqueued first");
        let event = receiver
            .try_recv()
            .expect("security event should remain enqueued");
        let encoded_ack = fleet_protocol::encode_message(&ack).unwrap();
        let fleet_protocol::WirePayload::ControllerSigningTrustBundleAck {
            agent_id,
            accepted,
            current_fingerprint,
            entries_count,
            reason_code,
        } = ack.payload
        else {
            panic!("expected trust bundle ack");
        };

        assert_eq!(agent_id, config.agent_id);
        assert!(accepted);
        assert_eq!(current_fingerprint.as_deref(), Some("controller-fp-new"));
        assert_eq!(entries_count, 1);
        assert!(reason_code.is_none());
        assert!(matches!(
            event.payload,
            fleet_protocol::WirePayload::SecurityEvent { ref action, .. }
                if action == "controller_signing_trust_bundle_update_accepted"
        ));
        assert!(!encoded_ack.contains("controller-public-new"));
        assert!(!encoded_ack.contains("private_key"));
        assert!(!encoded_ack.contains("key_path"));
        assert!(!encoded_ack.contains("tls_certificate"));
    }

    #[test]
    fn agent_controller_signing_trust_bundle_update_rejection_ack_is_bounded() {
        let sender = test_outbound_queue(4);
        let receiver = TestOutboundReceiver(sender.clone());
        let config = test_agent_config();
        fs::create_dir_all(config.controller_trust_bundle_path.parent().unwrap()).unwrap();
        let task_state = test_task_session_state();
        let entries = vec![
            fleet_protocol::ControllerSigningTrustEntryWire {
                fingerprint: "controller-fp-duplicate".to_owned(),
                public_key: "public-material-a".to_owned(),
                role: fleet_protocol::ControllerSigningTrustRoleWire::Current,
                valid_from_ms: 10_000,
                valid_until_ms: None,
            },
            fleet_protocol::ControllerSigningTrustEntryWire {
                fingerprint: "controller-fp-duplicate".to_owned(),
                public_key: "public-material-b".to_owned(),
                role: fleet_protocol::ControllerSigningTrustRoleWire::Previous,
                valid_from_ms: 10_000,
                valid_until_ms: Some(20_000),
            },
        ];
        let message = fleet_protocol::WireMessage::new(
            "msg-trust-update",
            "corr-trust-update",
            Some(config.agent_id.clone()),
            10_000,
            fleet_protocol::WirePayload::ControllerSigningTrustBundleUpdate { entries },
        );

        handle_agent_session_message(
            message,
            &config,
            "legacy-controller-public",
            "corr-test",
            &sender,
            &task_state,
        )
        .unwrap();

        let ack = receiver
            .try_recv()
            .expect("rejection ack should be enqueued");
        let event = receiver
            .try_recv()
            .expect("rejection security event should remain enqueued");
        let encoded_ack = fleet_protocol::encode_message(&ack).unwrap();
        let fleet_protocol::WirePayload::ControllerSigningTrustBundleAck {
            accepted,
            current_fingerprint,
            entries_count,
            reason_code,
            ..
        } = ack.payload
        else {
            panic!("expected trust bundle ack");
        };

        assert!(!accepted);
        assert!(current_fingerprint.is_none());
        assert_eq!(entries_count, 2);
        assert_eq!(reason_code.as_deref(), Some("invalid_trust_bundle"));
        assert!(matches!(
            event.payload,
            fleet_protocol::WirePayload::SecurityEvent { ref action, .. }
                if action == "controller_signing_trust_bundle_update_rejected"
        ));
        assert!(!encoded_ack.contains("public-material-a"));
        assert!(!encoded_ack.contains("public-material-b"));
        assert!(!encoded_ack.contains("private_key"));
        assert!(!encoded_ack.contains("key_path"));
        assert!(!encoded_ack.contains("tls_certificate"));
    }

    #[test]
    fn agent_certificate_lifecycle_update_rejects_until_runtime_support_exists() {
        let sender = test_outbound_queue(4);
        let receiver = TestOutboundReceiver(sender.clone());
        let config = test_agent_config();
        let task_state = test_task_session_state();
        let message = fleet_protocol::WireMessage::new(
            "msg-agent-cert-update",
            "corr-agent-cert-update",
            Some(config.agent_id.clone()),
            10_000,
            fleet_protocol::WirePayload::AgentCertificateLifecycleUpdate {
                agent_id: config.agent_id.clone(),
                action: fleet_protocol::AgentCertificateLifecycleActionWire::Issue,
                state: fleet_protocol::AgentCertificateLifecycleStateWire::Issued,
                current_certificate: Some(fleet_protocol::AgentCertificateMetadataWire {
                    serial: "serial-1".to_owned(),
                    fingerprint: "cert-fp-current".to_owned(),
                    not_before_ms: 10_000,
                    not_after_ms: 20_000,
                }),
                next_certificate: None,
                grace_until_ms: None,
                reason_code: None,
            },
        );

        handle_agent_session_message(
            message,
            &config,
            "legacy-controller-public",
            "corr-test",
            &sender,
            &task_state,
        )
        .unwrap();

        let ack = receiver
            .try_recv()
            .expect("certificate lifecycle rejection ack should be enqueued");
        let event = receiver
            .try_recv()
            .expect("certificate lifecycle security event should remain enqueued");
        let encoded_ack = fleet_protocol::encode_message(&ack).unwrap();
        let fleet_protocol::WirePayload::AgentCertificateLifecycleAck {
            agent_id,
            accepted,
            state,
            current_fingerprint,
            reason_code,
        } = ack.payload
        else {
            panic!("expected agent certificate lifecycle ack");
        };

        assert_eq!(agent_id, config.agent_id);
        assert!(!accepted);
        assert_eq!(
            state,
            fleet_protocol::AgentCertificateLifecycleStateWire::Issued
        );
        assert!(current_fingerprint.is_none());
        assert_eq!(
            reason_code.as_deref(),
            Some("certificate_lifecycle_runtime_not_implemented")
        );
        assert!(matches!(
            event.payload,
            fleet_protocol::WirePayload::SecurityEvent { ref action, .. }
                if action == "agent_certificate_lifecycle_update_rejected"
        ));
        assert!(!encoded_ack.contains("serial-1"));
        assert!(!encoded_ack.contains("private_key"));
        assert!(!encoded_ack.contains("certificate_body"));
        assert!(!encoded_ack.contains("ca_path"));
        assert!(!encoded_ack.contains("runtime_env"));
    }

    #[test]
    fn agent_task_verification_uses_updated_bundle_and_preserves_task_guards() {
        let old_key = fleet_core::generate_agent_key_pair().unwrap();
        let new_key = fleet_core::generate_agent_key_pair().unwrap();
        let mut config = test_agent_config();
        config.controller_fingerprint = old_key.fingerprint.clone();
        let task_state = test_task_session_state();
        let issued_at = UNIX_EPOCH + Duration::from_secs(20);
        let now = UNIX_EPOCH + Duration::from_secs(30);
        apply_agent_controller_signing_trust_bundle_update(
            &task_state,
            &[fleet_protocol::ControllerSigningTrustEntryWire {
                fingerprint: new_key.fingerprint.clone(),
                public_key: new_key.public_key_hex.clone(),
                role: fleet_protocol::ControllerSigningTrustRoleWire::Current,
                valid_from_ms: 10_000,
                valid_until_ms: None,
            }],
            None,
        )
        .unwrap();
        let envelope = signed_test_task_envelope(
            &config.agent_id,
            "nonce-updated-bundle",
            "payload-hash-updated-bundle",
            &new_key.private_key_hex,
            issued_at,
            UNIX_EPOCH + Duration::from_secs(60),
        );

        let result = verify_agent_task_envelope_once_with_session_trust(
            &envelope,
            &config,
            &old_key.public_key_hex,
            &task_state,
            now,
        )
        .unwrap();

        assert_eq!(
            result,
            Ok(fleet_domain::ControllerSigningTrustVerification::VerifiedCurrent)
        );

        let mismatched_target = signed_test_task_envelope(
            "agent-other",
            "nonce-target-mismatch",
            "payload-hash-target-mismatch",
            &new_key.private_key_hex,
            issued_at,
            UNIX_EPOCH + Duration::from_secs(60),
        );
        let mismatch = verify_agent_task_envelope_once_with_session_trust(
            &mismatched_target,
            &config,
            &old_key.public_key_hex,
            &task_state,
            now,
        )
        .unwrap();

        assert!(matches!(
            mismatch,
            Err(fleet_runner::RunnerError::Job(
                fleet_domain::JobError::TargetAgentMismatch
            ))
        ));
    }

    #[test]
    fn absent_controller_trust_bundle_sidecar_keeps_legacy_pinned_bundle() {
        let dir = unique_test_dir("agent-trust-sidecar-absent");
        fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("agent.conf");
        write_secure_file(
            &config_path,
            "url=http://127.0.0.1:7700\nagent_id=agent-web-01\nfingerprint=agent-fp-1\ncontroller_fingerprint=controller-fp-1\n",
        )
        .unwrap();
        write_secure_file(&dir.join("agent_private.key"), "agent-private-key\n").unwrap();

        let config = read_agent_config(&config_path).unwrap();
        let task_state = agent_task_session_state(&config).unwrap();
        let bundle =
            agent_controller_signing_trust_bundle(&task_state, &config, "legacy-public-key")
                .unwrap();

        assert_eq!(
            config.controller_trust_bundle_path,
            dir.join("controller_trust_bundle.json")
        );
        assert!(config.controller_trust_bundle.is_none());
        assert_eq!(bundle.entries().len(), 1);
        assert_eq!(
            bundle.entries()[0].fingerprint().as_str(),
            "controller-fp-1"
        );
        assert_eq!(
            bundle.entries()[0].public_key().as_str(),
            "legacy-public-key"
        );
    }

    #[test]
    fn controller_trust_bundle_sidecar_roundtrips_public_fields_only() {
        let dir = unique_test_dir("agent-trust-sidecar-public");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("controller_trust_bundle.json");
        let bundle = controller_signing_trust_bundle_from_wire(&[
            fleet_protocol::ControllerSigningTrustEntryWire {
                fingerprint: "controller-fp-new".to_owned(),
                public_key: "controller-public-new".to_owned(),
                role: fleet_protocol::ControllerSigningTrustRoleWire::Current,
                valid_from_ms: 10_000,
                valid_until_ms: None,
            },
        ])
        .unwrap();

        write_agent_controller_trust_bundle_sidecar(&path, &bundle).unwrap();
        let loaded = read_agent_controller_trust_bundle_sidecar(&path)
            .unwrap()
            .expect("sidecar should exist");
        let body = fs::read_to_string(path).unwrap();

        assert_eq!(loaded, bundle);
        assert!(body.contains("controller-public-new"));
        assert!(body.contains("controller-fp-new"));
        assert!(!body.contains("private_key"));
        assert!(!body.contains("key_path"));
        assert!(!body.contains("tls_certificate"));
    }

    #[test]
    fn accepted_controller_trust_bundle_update_persists_for_restart() {
        let dir = unique_test_dir("agent-trust-sidecar-restart");
        fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("agent.conf");
        write_secure_file(
            &config_path,
            "url=http://127.0.0.1:7700\nagent_id=agent-web-01\nfingerprint=agent-fp-1\ncontroller_fingerprint=controller-fp-old\n",
        )
        .unwrap();
        write_secure_file(&dir.join("agent_private.key"), "agent-private-key\n").unwrap();
        let config = read_agent_config(&config_path).unwrap();
        let task_state = agent_task_session_state(&config).unwrap();
        let update = vec![fleet_protocol::ControllerSigningTrustEntryWire {
            fingerprint: "controller-fp-new".to_owned(),
            public_key: "controller-public-new".to_owned(),
            role: fleet_protocol::ControllerSigningTrustRoleWire::Current,
            valid_from_ms: 10_000,
            valid_until_ms: None,
        }];

        apply_agent_controller_signing_trust_bundle_update(
            &task_state,
            &update,
            Some(&config.controller_trust_bundle_path),
        )
        .unwrap();
        let restarted_config = read_agent_config(&config_path).unwrap();

        assert_eq!(
            restarted_config
                .controller_trust_bundle
                .as_ref()
                .unwrap()
                .entries()[0]
                .fingerprint()
                .as_str(),
            "controller-fp-new"
        );
    }

    #[test]
    fn corrupt_controller_trust_bundle_sidecar_rejects_without_material_leak() {
        let dir = unique_test_dir("agent-trust-sidecar-corrupt");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("controller_trust_bundle.json");
        write_secure_file(
            &path,
            r#"{
              "entries": [{
                "fingerprint": "controller-fp-new",
                "public_key": "private-key-secret\nvalue",
                "role": "current",
                "valid_from_ms": 10000,
                "valid_until_ms": null
              }]
            }"#,
        )
        .unwrap();

        let error = read_agent_controller_trust_bundle_sidecar(&path)
            .expect_err("invalid sidecar should fail")
            .to_string();

        assert!(error.contains("invalid controller signing public key"));
        assert!(!error.contains("private-key-secret"));
    }

    #[test]
    fn persisted_controller_trust_bundle_verifies_task_after_restart() {
        let old_key = fleet_core::generate_agent_key_pair().unwrap();
        let new_key = fleet_core::generate_agent_key_pair().unwrap();
        let dir = unique_test_dir("agent-trust-sidecar-verify");
        fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("agent.conf");
        write_secure_file(
            &config_path,
            &format!(
                "url=http://127.0.0.1:7700\nagent_id=agent-web-01\nfingerprint=agent-fp-1\ncontroller_fingerprint={}\n",
                old_key.fingerprint
            ),
        )
        .unwrap();
        write_secure_file(&dir.join("agent_private.key"), "agent-private-key\n").unwrap();
        let persisted_bundle = controller_signing_trust_bundle_from_wire(&[
            fleet_protocol::ControllerSigningTrustEntryWire {
                fingerprint: new_key.fingerprint.clone(),
                public_key: new_key.public_key_hex.clone(),
                role: fleet_protocol::ControllerSigningTrustRoleWire::Current,
                valid_from_ms: 10_000,
                valid_until_ms: None,
            },
        ])
        .unwrap();
        write_agent_controller_trust_bundle_sidecar(
            &agent_controller_trust_bundle_path(&config_path),
            &persisted_bundle,
        )
        .unwrap();
        let restarted_config = read_agent_config(&config_path).unwrap();
        let task_state = agent_task_session_state(&restarted_config).unwrap();
        let issued_at = UNIX_EPOCH + Duration::from_secs(20);
        let now = UNIX_EPOCH + Duration::from_secs(30);
        let envelope = signed_test_task_envelope(
            &restarted_config.agent_id,
            "nonce-persisted-bundle",
            "payload-hash-persisted-bundle",
            &new_key.private_key_hex,
            issued_at,
            UNIX_EPOCH + Duration::from_secs(60),
        );

        let result = verify_agent_task_envelope_once_with_session_trust(
            &envelope,
            &restarted_config,
            &old_key.public_key_hex,
            &task_state,
            now,
        )
        .unwrap();

        assert_eq!(
            result,
            Ok(fleet_domain::ControllerSigningTrustVerification::VerifiedCurrent)
        );
    }

    #[test]
    fn reads_secure_agent_config_with_private_key() {
        let dir = unique_test_dir("secure-agent-config");
        fs::create_dir_all(&dir).unwrap();
        write_secure_file(
            &dir.join("agent.conf"),
            "url=http://127.0.0.1:7700\nagent_id=agent-web-01\nfingerprint=fp-1\ncontroller_fingerprint=controller-fp-1\n",
        )
        .unwrap();
        write_secure_file(&dir.join("agent_private.key"), "private-key-1\n").unwrap();

        let config = read_agent_config(&dir.join("agent.conf")).unwrap();

        assert_eq!(config.agent_id, "agent-web-01");
        assert!(config.tls_ca_cert.is_none());
        assert_eq!(config.private_key, "private-key-1");
        assert_eq!(config.controller_fingerprint, "controller-fp-1");
        assert_eq!(config.replay_store_path, dir.join("task_nonces.log"));
    }

    #[test]
    fn reads_agent_config_with_tls_ca_cert() {
        let dir = unique_test_dir("secure-agent-config-tls-ca");
        fs::create_dir_all(&dir).unwrap();
        write_secure_file(
            &dir.join("agent.conf"),
            "url=https://127.0.0.1:7700\ntls_ca_cert=/tmp/fleet-ca.pem\nagent_id=agent-web-01\nfingerprint=fp-1\ncontroller_fingerprint=controller-fp-1\n",
        )
        .unwrap();
        write_secure_file(&dir.join("agent_private.key"), "private-key-1\n").unwrap();

        let config = read_agent_config(&dir.join("agent.conf")).unwrap();

        assert_eq!(
            config.tls_ca_cert.as_deref(),
            Some(Path::new("/tmp/fleet-ca.pem"))
        );
    }

    #[test]
    fn agent_nonce_replay_guard_uses_configured_file_backed_store() {
        let dir = unique_test_dir("agent-replay-guard");
        fs::create_dir_all(&dir).unwrap();
        let mut config = test_agent_config();
        config.replay_store_path = dir.join("task_nonces.log");
        let mut first_guard = agent_nonce_replay_guard(&config).unwrap();

        first_guard.accept_once("nonce-restart").unwrap();
        let mut restarted_guard = agent_nonce_replay_guard(&config).unwrap();

        assert_eq!(
            restarted_guard.accept_once("nonce-restart"),
            Err(fleet_runner::RunnerError::ReplayedNonce)
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn agent_config_builds_legacy_controller_signing_trust_bundle() {
        let config = LocalAgentConfig {
            url: "http://127.0.0.1:7700".to_owned(),
            tls_ca_cert: None,
            agent_id: "agent-web-01".to_owned(),
            fingerprint: "agent-fp-1".to_owned(),
            private_key: "private-key-1".to_owned(),
            controller_fingerprint: "controller-fp-1".to_owned(),
            replay_store_path: unique_test_dir("agent-replay-store").join("task_nonces.log"),
            controller_trust_bundle_path: unique_test_dir("agent-trust-bundle")
                .join("controller_trust_bundle.json"),
            controller_trust_bundle: None,
        };

        let bundle =
            legacy_controller_signing_trust_bundle(&config, "controller-signing-public-key-1")
                .unwrap();
        let entry = bundle
            .entry_for_fingerprint(
                &fleet_domain::SigningKeyFingerprint::new("controller-fp-1").unwrap(),
                UNIX_EPOCH + Duration::from_secs(1),
                UNIX_EPOCH + Duration::from_secs(30),
            )
            .unwrap();

        assert_eq!(bundle.entries().len(), 1);
        assert_eq!(
            entry.role(),
            fleet_domain::ControllerSigningTrustRole::Current
        );
        assert_eq!(
            entry.public_key().as_str(),
            "controller-signing-public-key-1"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_agent_config_readable_by_group_or_other() {
        let dir = unique_test_dir("insecure-agent-config");
        fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("agent.conf");
        write_secure_file(
            &config_path,
            "url=http://127.0.0.1:7700\nagent_id=agent-web-01\nfingerprint=fp-1\ncontroller_fingerprint=controller-fp-1\n",
        )
        .unwrap();
        write_secure_file(&dir.join("agent_private.key"), "private-key-1\n").unwrap();
        fs::set_permissions(&config_path, fs::Permissions::from_mode(0o644)).unwrap();

        assert!(matches!(
            read_agent_config(&config_path),
            Err(CliError::Io(_))
        ));
    }

    #[test]
    fn rejects_changed_controller_fingerprint_without_reenroll() {
        let config = LocalAgentConfig {
            url: "http://127.0.0.1:7700".to_owned(),
            tls_ca_cert: None,
            agent_id: "agent-web-01".to_owned(),
            fingerprint: "agent-fp-1".to_owned(),
            private_key: "private-key-1".to_owned(),
            controller_fingerprint: "controller-fp-1".to_owned(),
            replay_store_path: unique_test_dir("agent-replay-store").join("task_nonces.log"),
            controller_trust_bundle_path: unique_test_dir("agent-trust-bundle")
                .join("controller_trust_bundle.json"),
            controller_trust_bundle: None,
        };
        let identity = fleet_controller::ControllerIdentityResponse {
            controller_public_key: "controller-public-key-2".to_owned(),
            controller_fingerprint: "controller-fp-2".to_owned(),
            controller_signing_public_key: "controller-public-key-2".to_owned(),
            controller_signing_fingerprint: "controller-fp-2".to_owned(),
            tls_endpoint: fleet_controller::ControllerTlsEndpointResponse::default(),
        };

        assert!(matches!(
            validate_pinned_controller_identity(&config, &identity),
            Err(CliError::Http(_))
        ));
    }

    #[test]
    fn controller_identity_is_created_once() {
        let dir = unique_test_dir("controller-identity-once");
        fs::create_dir_all(controller_dir(&dir)).unwrap();

        let first_fingerprint = ensure_controller_identity(&dir).unwrap();
        let first_public_key =
            fs::read_to_string(controller_dir(&dir).join("controller_public.key")).unwrap();
        let first_private_key =
            fs::read_to_string(controller_dir(&dir).join("controller_private.key")).unwrap();
        let second_fingerprint = ensure_controller_identity(&dir).unwrap();

        assert_eq!(first_fingerprint, second_fingerprint);
        assert_eq!(
            first_public_key,
            fs::read_to_string(controller_dir(&dir).join("controller_public.key")).unwrap()
        );
        assert_eq!(
            first_private_key,
            fs::read_to_string(controller_dir(&dir).join("controller_private.key")).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn controller_private_key_requires_secure_permissions() {
        let dir = unique_test_dir("controller-private-permission");
        fs::create_dir_all(controller_dir(&dir)).unwrap();
        let public_key_path = controller_dir(&dir).join("controller_public.key");
        let private_key_path = controller_dir(&dir).join("controller_private.key");
        let key_pair = fleet_core::generate_agent_key_pair().unwrap();
        fs::write(&public_key_path, format!("{}\n", key_pair.public_key_hex)).unwrap();
        fs::write(&private_key_path, format!("{}\n", key_pair.private_key_hex)).unwrap();
        fs::set_permissions(&private_key_path, fs::Permissions::from_mode(0o644)).unwrap();

        assert!(matches!(
            ensure_controller_identity(&dir),
            Err(CliError::Io(_))
        ));
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fleet-{name}-{}-{}",
            std::process::id(),
            epoch_millis()
        ))
    }

    fn initialized_controller_backup_fixture(name: &str) -> PathBuf {
        let data_dir = unique_test_dir(name);
        fs::create_dir_all(controller_dir(&data_dir)).unwrap();
        fs::create_dir_all(agent_dir(&data_dir)).unwrap();
        let store = fleet_store::SqliteStore::open(controller_db_path(&data_dir)).unwrap();
        store.insert_admin_token_hash("backup-admin-hash").unwrap();
        ensure_controller_identity(&data_dir).unwrap();
        data_dir
    }

    fn test_agent_config() -> LocalAgentConfig {
        LocalAgentConfig {
            url: "http://127.0.0.1:7700".to_owned(),
            tls_ca_cert: None,
            agent_id: "agent-web-01".to_owned(),
            fingerprint: "agent-fp-1".to_owned(),
            private_key: "private-key-1".to_owned(),
            controller_fingerprint: "controller-fp-1".to_owned(),
            replay_store_path: unique_test_dir("agent-replay-store").join("task_nonces.log"),
            controller_trust_bundle_path: unique_test_dir("agent-trust-bundle")
                .join("controller_trust_bundle.json"),
            controller_trust_bundle: None,
        }
    }

    fn test_task_envelope(agent_id: &str) -> fleet_domain::TaskEnvelope {
        fleet_domain::TaskEnvelope {
            job_id: fleet_domain::JobId::new("job-queue").unwrap(),
            task_id: fleet_domain::TaskId::new("task-queue").unwrap(),
            target_agent_id: fleet_domain::AgentId::new(agent_id).unwrap(),
            issued_at: UNIX_EPOCH + Duration::from_millis(1),
            expires_at: fleet_domain::TaskExpiry::new(UNIX_EPOCH + Duration::from_secs(60)),
            nonce: fleet_domain::TaskNonce::new("nonce-queue").unwrap(),
            payload_hash: "hash-queue".to_owned(),
            signature: Some(fleet_domain::TaskSignature::new("signature-queue").unwrap()),
        }
    }

    #[test]
    fn drift_report_payload_echoes_signed_task_envelope_correlation() {
        let config = test_agent_config();
        let envelope = test_task_envelope(&config.agent_id);

        let payload = drift_report_payload_for_envelope(
            &config.agent_id,
            &envelope,
            "drifted".to_owned(),
            "package nginx present".to_owned(),
            "package nginx missing".to_owned(),
        );

        assert!(matches!(
            payload,
            fleet_protocol::WirePayload::DriftReport {
                agent_id,
                job_id: Some(job_id),
                task_id: Some(task_id),
                status,
                ..
            } if agent_id == config.agent_id
                && job_id == "job-queue"
                && task_id == "task-queue"
                && status == "drifted"
        ));
    }

    fn signed_test_task_envelope(
        agent_id: &str,
        nonce: &str,
        payload_hash: &str,
        controller_private_key: &str,
        issued_at: SystemTime,
        expires_at: SystemTime,
    ) -> fleet_domain::TaskEnvelope {
        fleet_domain::TaskEnvelope {
            job_id: fleet_domain::JobId::new("job-signed").unwrap(),
            task_id: fleet_domain::TaskId::new(nonce).unwrap(),
            target_agent_id: fleet_domain::AgentId::new(agent_id).unwrap(),
            issued_at,
            expires_at: fleet_domain::TaskExpiry::new(expires_at),
            nonce: fleet_domain::TaskNonce::new(nonce).unwrap(),
            payload_hash: payload_hash.to_owned(),
            signature: Some(
                fleet_domain::TaskSignature::new(
                    fleet_core::sign_challenge(controller_private_key, payload_hash).unwrap(),
                )
                .unwrap(),
            ),
        }
    }

    fn test_task_session_state() -> AgentTaskSessionState {
        AgentTaskSessionState {
            busy: Arc::new(AtomicBool::new(false)),
            runtime: Arc::new(AgentTaskRuntimeState::default()),
            replay_guard: Arc::new(Mutex::new(fleet_runner::NonceReplayGuard::default())),
            controller_trust_bundle: Arc::new(Mutex::new(None)),
        }
    }

    fn test_task_assignment_message(agent_id: &str) -> fleet_protocol::WireMessage {
        fleet_protocol::WireMessage::new(
            "msg-task-test",
            "corr-test",
            Some(agent_id.to_owned()),
            1,
            fleet_protocol::WirePayload::TaskAssignment {
                envelope: fleet_protocol::SignedTaskEnvelopeWire {
                    job_id: "job-test".to_owned(),
                    task_id: "task-test".to_owned(),
                    target_agent_id: agent_id.to_owned(),
                    issued_at_ms: 1,
                    expires_at_ms: 60_000,
                    nonce: "nonce-test".to_owned(),
                    payload_hash: "hash-test".to_owned(),
                    signature: "signature-test".to_owned(),
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
}
