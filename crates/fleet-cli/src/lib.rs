use clap::{Args, Parser, Subcommand};
use fleet_core::{
    LogProfile, format_error_message, format_warning_message, init_logging, redact_secret,
};
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
    mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio_rustls::rustls::{ClientConfig as RustlsClientConfig, RootCertStore};
use tungstenite::client::IntoClientRequest;
use tungstenite::{Connector, Message};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

const LOG_TAIL_MAX_LINES: usize = 50;
const LOG_TAIL_MAX_LINE_BYTES: usize = 4096;
const LOG_TAIL_POLL_INTERVAL: Duration = Duration::from_millis(100);
const AGENT_SESSION_OUTBOUND_QUEUE_CAPACITY: usize = 64;
const AGENT_SESSION_READ_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Parser)]
#[command(name = "sponzey")]
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
    Controller(ControllerCommand),
    Agent(AgentCommand),
    Agents(AgentsCommand),
    EnrollToken(EnrollTokenCommand),
    Run(RunCommand),
    Facts(FactsCommand),
    Metrics(MetricsCommand),
    Logs(LogsCommand),
    Drift(DriftCommand),
    Apply(ApplyCommand),
    Retention(RetentionCommand),
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
pub struct ControllerCommand {
    #[command(subcommand)]
    pub command: ControllerSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum ControllerSubcommand {
    Init {
        #[arg(long, default_value = ".sponzey")]
        data_dir: PathBuf,
    },
    #[command(
        about = "Start the Sponzey Fleet controller",
        long_about = "Start the Sponzey Fleet controller API, Web Admin UI, and agent gateway.\n\nThe bind host controls where the process listens. The external URL is the public URL agents and operators should use. Do not use 0.0.0.0 as an agent URL. HTTP URLs are allowed for tests only, but every HTTP use prints a warning because traffic is not encrypted. Product and production environments must use HTTPS.",
        after_help = "Examples:\n  Local loopback demo:\n    sponzey controller start --host 127.0.0.1 --port 7700 --data-dir .sponzey --external-url http://127.0.0.1:7700\n\n  Test-only HTTP remote controller with warning:\n    sponzey controller start --host 0.0.0.0 --port 7700 --data-dir /var/lib/sponzey-fleet --external-url http://192.168.0.10:7700\n\n  HTTPS behind DNS/reverse proxy:\n    sponzey controller start --host 127.0.0.1 --port 7700 --data-dir /var/lib/sponzey-fleet --external-url https://fleet.example.com\n\n  Built-in HTTPS listener:\n    sponzey controller start --host 0.0.0.0 --port 7700 --data-dir /var/lib/sponzey-fleet --external-url https://fleet.example.com --tls-cert /etc/sponzey/tls/fullchain.pem --tls-key /etc/sponzey/tls/privkey.pem"
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
            help = "SQLite database URL, for example sqlite:///var/lib/sponzey-fleet/controller/fleet.db"
        )]
        db: Option<String>,
        #[arg(long, help = "PEM certificate chain for built-in HTTPS listener")]
        tls_cert: Option<PathBuf>,
        #[arg(long, help = "PEM private key for built-in HTTPS listener")]
        tls_key: Option<PathBuf>,
        #[arg(long, default_value = ".sponzey")]
        data_dir: PathBuf,
    },
    InstallService {
        #[arg(long)]
        binary: Option<PathBuf>,
        #[arg(long, default_value = "/var/lib/sponzey-fleet")]
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
    UninstallService {
        #[arg(long)]
        dry_run: bool,
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
        after_help = "Examples:\n  Local loopback development:\n    sponzey agent init --url http://127.0.0.1:7700 --token <token> --name web-01 --labels role=web,env=dev\n\n  Test-only remote HTTP with warning:\n    sponzey agent init --data-dir /var/lib/sponzey-fleet --url http://192.168.0.10:7700 --token <token> --name test-web-01 --labels role=web,env=test\n\n  HTTPS:\n    sponzey agent init --data-dir /var/lib/sponzey-fleet --url https://fleet.example.com --token <token> --name prod-web-01 --labels role=web,env=prod\n\n  HTTPS with a private CA:\n    sponzey agent init --data-dir /var/lib/sponzey-fleet --url https://fleet.example.com --tls-ca-cert /etc/sponzey/tls/ca.pem --token <token> --name prod-web-01"
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
        #[arg(long, default_value = ".sponzey", help = "Agent data directory")]
        data_dir: PathBuf,
    },
    #[command(
        about = "Start the enrolled local agent",
        long_about = "Start the enrolled local agent persistent session loop.\n\nThe agent reads its local identity from <data-dir>/agent/agent.conf, verifies the pinned controller fingerprint before opening the session, sends heartbeat liveness ticks, static facts inventory, metrics snapshots, and product-safe agent operational logs through one outbound writer queue, and receives controller-signed tasks on the same session. Heartbeat is only a liveness signal; facts, metrics, and logs each have their own bootstrap CLI interval. Connection failures are retried indefinitely by default. The agent must be enrolled before this command can run.",
        after_help = "Examples:\n  sponzey agent start --data-dir .sponzey\n  sponzey agent start --data-dir /var/lib/sponzey-fleet\n  sponzey agent start --data-dir .sponzey --once\n  sponzey agent start --data-dir .sponzey --facts-interval-seconds 300 --metrics-interval-seconds 30 --log-upload-interval-seconds 30\n\nLocal development flow:\n  sponzey controller init --data-dir .sponzey\n  sponzey enroll-token create --data-dir .sponzey --labels role=web,env=dev\n  sponzey agent init --data-dir .sponzey --url http://127.0.0.1:7700 --token <token> --name web-01 --labels role=web,env=dev\n  sponzey agent start --data-dir .sponzey"
    )]
    Start {
        #[arg(
            long,
            default_value = ".sponzey",
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
        long_about = "Render or install the Linux systemd unit for running 'sponzey agent start'.\n\nDry-run is safe on every platform. Writing service files requires Linux and root privileges."
    )]
    InstallService {
        #[arg(long, help = "Absolute sponzey binary path to pin in the service unit")]
        binary: Option<PathBuf>,
        #[arg(
            long,
            default_value = "/var/lib/sponzey-fleet",
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
        #[arg(long, default_value = ".sponzey")]
        data_dir: PathBuf,
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
        #[arg(long, default_value = ".sponzey")]
        data_dir: PathBuf,
    },
    List {
        #[arg(long, default_value = ".sponzey")]
        data_dir: PathBuf,
    },
    Revoke {
        id: String,
        #[arg(long, default_value = ".sponzey")]
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
        #[arg(long, default_value = ".sponzey")]
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
                    "controller is not initialized for data dir: {}\n\nInitialize it once before starting the controller:\n\n  sponzey controller init --data-dir \"{}\"\n  sponzey controller start --host 127.0.0.1 --port 7700 --data-dir \"{}\" --external-url http://127.0.0.1:7700\n\nIf you use local scripts:\n\n  ./scripts/run_controller.sh --host 127.0.0.1 --port 7700 --data-dir \"{}\" --external-url http://127.0.0.1:7700",
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
        Command::Controller(command) => execute_controller(command),
        Command::Agent(command) => execute_agent(command),
        Command::Agents(command) => execute_agents(command),
        Command::EnrollToken(command) => execute_enroll_token(command),
        Command::Run(command) => execute_run(command),
        Command::Facts(command) => execute_facts(command),
        Command::Metrics(command) => execute_metrics(command),
        Command::Logs(command) => execute_logs(command),
        Command::Drift(command) => execute_drift(command),
        Command::Apply(command) => execute_apply(command),
        Command::Retention(command) => execute_retention(command),
        Command::Demo(command) => execute_demo(command),
    }
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
            data_dir,
        } => {
            let database_path = db.as_deref().map(parse_sqlite_database_url).transpose()?;
            ensure_controller_initialized_for_start(&data_dir)?;
            fleet_controller::start_controller_server(fleet_controller::ControllerServerConfig {
                host,
                port,
                external_url,
                tls_cert_path: tls_cert,
                tls_key_path: tls_key,
                data_dir,
                database_path,
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
        ControllerSubcommand::UninstallService { dry_run } => {
            uninstall_systemd_service(ServiceRole::Controller, dry_run)
        }
    }
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
                    "sponzey agent init --url {} --token {} --name {} --labels {}",
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
    }
}

fn execute_run(command: RunCommand) -> Result<(), CliError> {
    if !command.confirm_risk {
        return Err(CliError::HighRiskConfirmationRequired);
    }
    let Some((program, args)) = command.command.split_first() else {
        return Err(CliError::EmptyCommand);
    };
    if command.controller_url.is_some() || command.admin_token.is_some() {
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
    let controller_url = command
        .controller_url
        .as_deref()
        .ok_or_else(|| CliError::Http("--controller-url is required for remote run".to_owned()))?;
    let admin_token = command
        .admin_token
        .as_deref()
        .ok_or_else(|| CliError::Http("--admin-token is required for remote run".to_owned()))?;
    if command.selector.is_none() {
        return Err(CliError::Http(
            "remote run requires --selector in MVP".to_owned(),
        ));
    }
    let job_id = command.job_id.clone().unwrap_or(prefixed_ulid("job-cli")?);
    let body = remote_run_request_body(command, &job_id, program, args)?;
    let response = http_request_url(
        controller_url,
        "POST",
        "/api/jobs/command",
        Some(admin_token),
        Some(&body),
    )?;
    if !response.starts_with("HTTP/1.1 201") {
        return Err(CliError::Http(
            response
                .lines()
                .next()
                .unwrap_or("request failed")
                .to_owned(),
        ));
    }
    let response_body = response.split("\r\n\r\n").nth(1).unwrap_or_default();
    let response_json: serde_json::Value =
        serde_json::from_str(response_body).map_err(|error| CliError::Http(error.to_string()))?;
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

    let output_path = format!("/api/jobs/{job_id}/output");
    let output = http_get_url(controller_url, &output_path, Some(admin_token))?;
    for line in render_job_output_api_for_cli(&output)? {
        println!("{line}");
    }
    Ok(())
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
        "expires_in_seconds": 60,
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
            let cutoff = SystemTime::now()
                .checked_sub(Duration::from_secs(older_than_days.saturating_mul(86_400)))
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let summary = store.cleanup_retention(cutoff, dry_run)?;
            if !dry_run {
                store.write_audit_event(fleet_domain::AuditEvent {
                    category: fleet_domain::AuditCategory::Security,
                    action: "retention_cleanup".to_owned(),
                    actor: fleet_domain::AuditActor::new("cli"),
                    target: fleet_domain::AuditTarget::new("controller-store"),
                    value: fleet_domain::AuditValue::Plain(format!(
                        "job_output_chunks={},facts_snapshots={},metrics_snapshots={},total={}",
                        summary.job_output_chunks,
                        summary.facts_snapshots,
                        summary.metrics_snapshots,
                        summary.total()
                    )),
                    occurred_at: SystemTime::now(),
                })?;
            }
            println!("job_output_chunks={}", summary.job_output_chunks);
            println!("facts_snapshots={}", summary.facts_snapshots);
            println!("metrics_snapshots={}", summary.metrics_snapshots);
            println!("total={}", summary.total());
            println!("dry_run={dry_run}");
            Ok(())
        }
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
                data_dir: server_data_dir,
                database_path: None,
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
    if !output.contains("\"data\":\"demo-ok\"") {
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
        "target_agent_ids": [],
        "selector": "role=web",
        "program": "printf",
        "args": ["demo-ok"],
        "timeout_seconds": 30,
        "confirmed_high_risk": true,
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

fn http_get_url(url: &str, path: &str, bearer_token: Option<&str>) -> Result<String, CliError> {
    let response = http_request_url(url, "GET", path, bearer_token, None)?;
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
        "sponzey-fleet-demo-{}-{}",
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

fn parse_sqlite_database_url(value: &str) -> Result<PathBuf, CliError> {
    let Some(path) = value.strip_prefix("sqlite://") else {
        return Err(CliError::Http(
            "controller --db currently supports sqlite:// paths only".to_owned(),
        ));
    };
    if path.trim().is_empty() {
        return Err(CliError::Http(
            "controller --db sqlite path cannot be empty".to_owned(),
        ));
    }
    Ok(PathBuf::from(path))
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
        ServiceRole::Controller => "sponzey-fleet-controller.service",
        ServiceRole::Agent => "sponzey-fleet-agent.service",
    }
}

fn systemd_unit_path(role: ServiceRole) -> PathBuf {
    Path::new("/etc/systemd/system").join(service_unit_name(role))
}

fn render_systemctl_command(action: &str, role: ServiceRole) -> String {
    format!("systemctl {action} {}", service_unit_name(role))
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
    if std::env::consts::OS != "linux" {
        return Err(CliError::ServiceOperationRequiresLinux);
    }
    if effective_user_id()? != 0 {
        return Err(CliError::ServiceOperationRequiresRoot);
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
    Ok(LocalAgentConfig {
        url: value("url")?,
        tls_ca_cert: optional_value("tls_ca_cert"),
        agent_id: value("agent_id")?,
        fingerprint: value("fingerprint")?,
        private_key,
        controller_fingerprint: value("controller_fingerprint")?,
    })
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
    run_agent_session_loop_with(
        options,
        || run_agent_session_once(config, options),
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
type AgentOutboundSender = SyncSender<fleet_protocol::WireMessage>;
type AgentOutboundReceiver = Receiver<fleet_protocol::WireMessage>;

fn run_agent_session_once(
    config: &LocalAgentConfig,
    options: AgentHeartbeatOptions,
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

    let (outbound_sender, outbound_receiver) =
        mpsc::sync_channel(AGENT_SESSION_OUTBOUND_QUEUE_CAPACITY);
    let task_busy = Arc::new(AtomicBool::new(false));
    let replay_guard = Arc::new(Mutex::new(fleet_runner::NonceReplayGuard::default()));
    enqueue_initial_agent_session_messages(&outbound_sender, config, &correlation_id, options)?;

    let mut last_heartbeat = Instant::now();
    let mut last_facts = Instant::now();
    let mut last_metrics = Instant::now();
    let mut last_log = if options.log_upload.enabled {
        Instant::now()
    } else {
        Instant::now() - options.log_upload.interval
    };

    loop {
        flush_agent_outbound_queue(&mut socket, &outbound_receiver)?;

        match read_agent_session_message(&mut socket)? {
            AgentSessionInbound::Message(message) => handle_agent_session_message(
                *message,
                config,
                controller_signing_public_key(&identity),
                &correlation_id,
                &outbound_sender,
                &task_busy,
                &replay_guard,
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
        enqueue_agent_session_tick_messages(&outbound_sender, config, &correlation_id, actions)?;
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
        flush_agent_outbound_queue(&mut socket, &outbound_receiver)?;
    }
}

fn enqueue_agent_session_tick_messages(
    outbound_sender: &AgentOutboundSender,
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
    outbound_sender: &AgentOutboundSender,
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
    outbound_receiver: &AgentOutboundReceiver,
) -> Result<(), CliError> {
    loop {
        match outbound_receiver.try_recv() {
            Ok(message) => send_wire_message_to_socket(socket, &message)?,
            Err(TryRecvError::Empty) => return Ok(()),
            Err(TryRecvError::Disconnected) => {
                return Err(CliError::Http(
                    "agent session outbound queue disconnected".to_owned(),
                ));
            }
        }
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
        Err(tungstenite::Error::Io(error))
            if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            Ok(AgentSessionInbound::Idle)
        }
        Err(error) => Err(CliError::Http(error.to_string())),
    }
}

fn handle_agent_session_message(
    message: fleet_protocol::WireMessage,
    config: &LocalAgentConfig,
    controller_public_key: &str,
    correlation_id: &str,
    outbound_sender: &AgentOutboundSender,
    task_busy: &Arc<AtomicBool>,
    replay_guard: &Arc<Mutex<fleet_runner::NonceReplayGuard>>,
) -> Result<(), CliError> {
    let fleet_protocol::WirePayload::TaskAssignment { envelope, task } = message.payload else {
        return Ok(());
    };
    if task_busy.swap(true, Ordering::SeqCst) {
        enqueue_wire_message(
            outbound_sender,
            agent_security_event_message(config, correlation_id, "task_rejected", "agent is busy")?,
        )?;
        return Ok(());
    }

    let worker_config = config.clone();
    let worker_public_key = controller_public_key.to_owned();
    let worker_correlation_id = correlation_id.to_owned();
    let worker_sender = outbound_sender.clone();
    let worker_busy = task_busy.clone();
    let worker_replay_guard = replay_guard.clone();
    thread::spawn(move || {
        if let Err(error) = handle_task_assignment_with_queue(
            &worker_sender,
            &worker_config,
            &worker_public_key,
            &worker_correlation_id,
            envelope,
            task,
            &worker_replay_guard,
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
        worker_busy.store(false, Ordering::SeqCst);
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
    outbound_sender: &AgentOutboundSender,
    message: fleet_protocol::WireMessage,
) -> Result<(), CliError> {
    outbound_sender
        .try_send(message)
        .map_err(|error| match error {
            TrySendError::Full(_) => CliError::Http("agent outbound queue is full".to_owned()),
            TrySendError::Disconnected(_) => {
                CliError::Http("agent outbound queue disconnected".to_owned())
            }
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
            "executable": "sponzey",
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
        name,
    })
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
    let verifier = ControllerSignatureVerifier {
        controller_public_key,
    };
    let agent_id = fleet_domain::AgentId::new(config.agent_id.clone())
        .map_err(|error| CliError::Http(error.to_string()))?;
    let mut replay_guard = fleet_runner::NonceReplayGuard::default();
    if let Err(error) = fleet_runner::verify_signed_envelope_once(
        &envelope,
        &agent_id,
        SystemTime::now(),
        &verifier,
        &mut replay_guard,
    ) {
        send_agent_security_event(
            socket,
            config,
            correlation_id,
            "task_verification_failed",
            &error.to_string(),
        )?;
        return Ok(());
    }

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
    outbound_sender: &AgentOutboundSender,
    config: &LocalAgentConfig,
    controller_public_key: &str,
    correlation_id: &str,
    envelope: fleet_protocol::SignedTaskEnvelopeWire,
    task: fleet_protocol::TaskWire,
    replay_guard: &Arc<Mutex<fleet_runner::NonceReplayGuard>>,
) -> Result<(), CliError> {
    let envelope = task_envelope_from_wire(envelope)?;
    let verifier = ControllerSignatureVerifier {
        controller_public_key,
    };
    let agent_id = fleet_domain::AgentId::new(config.agent_id.clone())
        .map_err(|error| CliError::Http(error.to_string()))?;
    let verification = {
        let mut replay_guard = replay_guard
            .lock()
            .map_err(|_| CliError::Http("agent nonce replay guard lock poisoned".to_owned()))?;
        fleet_runner::verify_signed_envelope_once(
            &envelope,
            &agent_id,
            SystemTime::now(),
            &verifier,
            &mut replay_guard,
        )
    };
    if let Err(error) = verification {
        send_agent_security_event_queue(
            outbound_sender,
            config,
            correlation_id,
            "task_verification_failed",
            &error.to_string(),
        )?;
        return Ok(());
    }

    match task {
        fleet_protocol::TaskWire::Command(command) => {
            run_signed_command_task_queue(
                outbound_sender,
                config,
                correlation_id,
                &envelope,
                command,
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
    outbound_sender: &AgentOutboundSender,
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
    let mut streamed_any = false;
    let output = match fleet_runner::run_command_streaming(spec, |chunk| {
        streamed_any = true;
        send_agent_output_chunk_queue(outbound_sender, config, correlation_id, envelope, chunk)
            .map_err(|error| fleet_runner::RunnerError::Stream(error.to_string()))
    }) {
        Ok(output) => output,
        Err(error) => fleet_runner::CommandOutput {
            stdout: String::new(),
            stderr: error.to_string(),
            exit_code: -1,
            truncated: false,
        },
    };
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
    )?;
    Ok(())
}

fn run_signed_drift_check_task_queue(
    outbound_sender: &AgentOutboundSender,
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
            send_agent_task_result_queue(outbound_sender, config, correlation_id, envelope, -1)?;
            return Ok(());
        }
    };
    let report = fleet_runner::evaluate_policy_drift(&policy, &fleet_runner::LocalDriftProbe);
    enqueue_wire_message(
        outbound_sender,
        fleet_protocol::WireMessage::new(
            prefixed_ulid("msg")?,
            correlation_id.to_owned(),
            Some(config.agent_id.clone()),
            epoch_millis() as u64,
            fleet_protocol::WirePayload::DriftReport {
                agent_id: config.agent_id.clone(),
                status: drift_status_to_cli(&report.status).to_owned(),
                expected: report.expected,
                actual: report.actual,
            },
        ),
    )?;
    send_agent_task_result_queue(outbound_sender, config, correlation_id, envelope, 0)?;
    Ok(())
}

fn run_signed_runbook_task_queue(
    outbound_sender: &AgentOutboundSender,
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
            send_agent_task_result_queue(outbound_sender, config, correlation_id, envelope, -1)?;
            return Ok(());
        }
    };
    let Some(package_manager) = fleet_runner::detect_local_linux_package_manager() else {
        send_agent_output_chunk_queue(
            outbound_sender,
            config,
            correlation_id,
            envelope,
            fleet_runner::CommandOutputChunk {
                stream: fleet_runner::CommandOutputStream::Stderr,
                sequence: 0,
                data: "no supported Linux package manager detected".to_owned(),
            },
        )?;
        send_agent_task_result_queue(outbound_sender, config, correlation_id, envelope, -1)?;
        return Ok(());
    };
    let plan = match fleet_runner::build_runbook_execution_plan(&runbook, package_manager) {
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
            send_agent_task_result_queue(outbound_sender, config, correlation_id, envelope, -1)?;
            return Ok(());
        }
    };
    let report = match fleet_runner::execute_runbook_execution_plan(
        &plan,
        fleet_runner::RunbookExecutionOptions {
            confirmed_high_risk: task.confirmed_high_risk,
            command_timeout: Duration::from_millis(task.timeout_ms),
            ..fleet_runner::RunbookExecutionOptions::default()
        },
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
            send_agent_task_result_queue(outbound_sender, config, correlation_id, envelope, -1)?;
            return Ok(());
        }
    };
    let mut sequence = 0;
    for outcome in report.outcomes {
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
    send_agent_task_result_queue(outbound_sender, config, correlation_id, envelope, 0)?;
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
    let mut streamed_any = false;
    let output = match fleet_runner::run_command_streaming(spec, |chunk| {
        streamed_any = true;
        send_agent_output_chunk(socket, config, correlation_id, envelope, chunk)
            .map_err(|error| fleet_runner::RunnerError::Stream(error.to_string()))
    }) {
        Ok(output) => output,
        Err(error) => fleet_runner::CommandOutput {
            stdout: String::new(),
            stderr: error.to_string(),
            exit_code: -1,
            truncated: false,
        },
    };
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
    send_agent_task_result(socket, config, correlation_id, envelope, output.exit_code)?;
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
            send_agent_task_result(socket, config, correlation_id, envelope, -1)?;
            return Ok(());
        }
    };
    let report = fleet_runner::evaluate_policy_drift(&policy, &fleet_runner::LocalDriftProbe);
    let message = fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::DriftReport {
            agent_id: config.agent_id.clone(),
            status: drift_status_to_cli(&report.status).to_owned(),
            expected: report.expected,
            actual: report.actual,
        },
    );
    socket
        .send(Message::Text(
            fleet_protocol::encode_message(&message)
                .map_err(|error| CliError::Http(error.to_string()))?,
        ))
        .map_err(|error| CliError::Http(error.to_string()))?;
    send_agent_task_result(socket, config, correlation_id, envelope, 0)?;
    Ok(())
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
            send_agent_task_result(socket, config, correlation_id, envelope, -1)?;
            return Ok(());
        }
    };
    let Some(package_manager) = fleet_runner::detect_local_linux_package_manager() else {
        send_agent_output_chunk(
            socket,
            config,
            correlation_id,
            envelope,
            fleet_runner::CommandOutputChunk {
                stream: fleet_runner::CommandOutputStream::Stderr,
                sequence: 0,
                data: "no supported Linux package manager detected".to_owned(),
            },
        )?;
        send_agent_task_result(socket, config, correlation_id, envelope, -1)?;
        return Ok(());
    };
    let plan = match fleet_runner::build_runbook_execution_plan(&runbook, package_manager) {
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
            send_agent_task_result(socket, config, correlation_id, envelope, -1)?;
            return Ok(());
        }
    };
    let report = match fleet_runner::execute_runbook_execution_plan(
        &plan,
        fleet_runner::RunbookExecutionOptions {
            confirmed_high_risk: task.confirmed_high_risk,
            command_timeout: Duration::from_millis(task.timeout_ms),
            ..fleet_runner::RunbookExecutionOptions::default()
        },
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
            send_agent_task_result(socket, config, correlation_id, envelope, -1)?;
            return Ok(());
        }
    };
    let mut sequence = 0;
    for outcome in report.outcomes {
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
    send_agent_task_result(socket, config, correlation_id, envelope, 0)?;
    Ok(())
}

fn send_agent_task_result(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
    exit_code: i32,
) -> Result<(), CliError> {
    let result = fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::TaskResult {
            job_id: envelope.job_id.as_str().to_owned(),
            task_id: envelope.task_id.as_str().to_owned(),
            exit_code,
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
    outbound_sender: &AgentOutboundSender,
    config: &LocalAgentConfig,
    correlation_id: &str,
    envelope: &fleet_domain::TaskEnvelope,
    exit_code: i32,
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
                exit_code,
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
    outbound_sender: &AgentOutboundSender,
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

fn send_agent_security_event(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    config: &LocalAgentConfig,
    correlation_id: &str,
    action: &str,
    detail: &str,
) -> Result<(), CliError> {
    let message = fleet_protocol::WireMessage::new(
        prefixed_ulid("msg")?,
        correlation_id.to_owned(),
        Some(config.agent_id.clone()),
        epoch_millis() as u64,
        fleet_protocol::WirePayload::SecurityEvent {
            agent_id: config.agent_id.clone(),
            action: action.to_owned(),
            detail: detail.to_owned(),
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

fn send_agent_security_event_queue(
    outbound_sender: &AgentOutboundSender,
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

struct ControllerSignatureVerifier<'a> {
    controller_public_key: &'a str,
}

impl fleet_runner::TaskSignatureVerifier for ControllerSignatureVerifier<'_> {
    fn verify(&self, payload_hash: &str, signature: &str) -> bool {
        fleet_core::verify_challenge_signature(self.controller_public_key, payload_hash, signature)
            .unwrap_or(false)
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
    let message = socket
        .read()
        .map_err(|error| CliError::Http(error.to_string()))?;
    let body = message
        .to_text()
        .map_err(|error| CliError::Http(error.to_string()))?;
    fleet_protocol::decode_message(body).map_err(|error| CliError::Http(error.to_string()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

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
        let cli = Cli::try_parse_from(["sponzey", "controller", "init"]).expect("valid command");
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
            "sponzey",
            "controller",
            "start",
            "--db",
            "sqlite:///tmp/sponzey-fleet.db",
        ])
        .expect("valid command");

        let Command::Controller(ControllerCommand {
            command: ControllerSubcommand::Start { db, .. },
        }) = cli.command
        else {
            panic!("expected controller start command");
        };

        assert_eq!(db.as_deref(), Some("sqlite:///tmp/sponzey-fleet.db"));
        assert_eq!(
            parse_sqlite_database_url(db.as_deref().unwrap()).unwrap(),
            PathBuf::from("/tmp/sponzey-fleet.db")
        );
    }

    #[test]
    fn parses_controller_start_external_https_url() {
        let cli = Cli::try_parse_from([
            "sponzey",
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
            "sponzey",
            "controller",
            "start",
            "--external-url",
            "https://fleet.example.com",
            "--tls-cert",
            "/etc/sponzey/tls/fullchain.pem",
            "--tls-key",
            "/etc/sponzey/tls/privkey.pem",
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
            Some(Path::new("/etc/sponzey/tls/fullchain.pem"))
        );
        assert_eq!(
            tls_key.as_deref(),
            Some(Path::new("/etc/sponzey/tls/privkey.pem"))
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
        assert!(message.contains("sponzey controller init --data-dir"));
        assert!(message.contains("./scripts/run_controller.sh"));
    }

    #[test]
    fn enroll_token_create_persists_scope_and_audit() {
        let data_dir = unique_test_dir("enroll-token-create");
        let cli = Cli::try_parse_from([
            "sponzey",
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
            "sponzey",
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
    fn rejects_non_sqlite_controller_db_url() {
        assert!(matches!(
            parse_sqlite_database_url("postgres://localhost/fleet"),
            Err(CliError::Http(_))
        ));
    }

    #[test]
    fn renders_controller_service_unit_with_absolute_binary() {
        let unit = render_service_unit(
            ServiceRole::Controller,
            Path::new("/usr/local/bin/sponzey"),
            Path::new("/var/lib/sponzey-fleet"),
            Some("sponzey"),
            Some("sponzey"),
        )
        .unwrap();

        assert!(unit.contains("Description=Sponzey Fleet Controller"));
        assert!(unit.contains(
            "ExecStart=/usr/local/bin/sponzey controller start --data-dir /var/lib/sponzey-fleet"
        ));
        assert!(unit.contains("User=sponzey"));
        assert!(unit.contains("Group=sponzey"));
    }

    #[test]
    fn renders_agent_service_unit_with_quoted_paths() {
        let unit = render_service_unit(
            ServiceRole::Agent,
            Path::new("/opt/Sponzey Fleet/bin/sponzey"),
            Path::new("/var/lib/sponzey fleet"),
            None,
            None,
        )
        .unwrap();

        assert!(unit.contains("Description=Sponzey Fleet Agent"));
        assert!(unit.contains(
            "ExecStart=\"/opt/Sponzey Fleet/bin/sponzey\" agent start --data-dir \"/var/lib/sponzey fleet\""
        ));
    }

    #[test]
    fn service_unit_rejects_relative_binary_path() {
        assert!(matches!(
            render_service_unit(
                ServiceRole::Agent,
                Path::new("target/debug/sponzey"),
                Path::new("/var/lib/sponzey-fleet"),
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
                Path::new("/usr/local/bin/sponzey"),
                Path::new("/var/lib/sponzey-fleet"),
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
        let cli = Cli::try_parse_from(["sponzey", "agent", "start-service", "--dry-run"]).unwrap();

        assert!(execute(cli).is_ok());
        assert_eq!(
            render_systemctl_command("start", ServiceRole::Agent),
            "systemctl start sponzey-fleet-agent.service"
        );
    }

    #[test]
    fn agent_uninstall_service_dry_run_renders_safe_commands() {
        let cli =
            Cli::try_parse_from(["sponzey", "agent", "uninstall-service", "--dry-run"]).unwrap();

        assert!(execute(cli).is_ok());
        assert_eq!(
            render_uninstall_service_commands(ServiceRole::Agent),
            vec![
                "systemctl disable --now sponzey-fleet-agent.service".to_owned(),
                "rm /etc/systemd/system/sponzey-fleet-agent.service".to_owned(),
                "systemctl daemon-reload".to_owned(),
            ]
        );
    }

    #[test]
    fn controller_service_unit_path_is_systemd_path() {
        assert_eq!(
            systemd_unit_path(ServiceRole::Controller),
            PathBuf::from("/etc/systemd/system/sponzey-fleet-controller.service")
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
    fn parses_run_command() {
        let cli = Cli::try_parse_from([
            "sponzey",
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
            "sponzey",
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
    fn remote_run_body_uses_selector_and_omits_admin_token() {
        let command = RunCommand {
            selector: Some("role=web".to_owned()),
            confirm_risk: true,
            controller_url: Some("http://127.0.0.1:7700".to_owned()),
            admin_token: Some("admin-secret".to_owned()),
            job_id: Some("job-cli-1".to_owned()),
            timeout_seconds: 45,
            command: vec!["uptime".to_owned(), "-a".to_owned()],
        };

        let body =
            remote_run_request_body(&command, "job-cli-1", "uptime", &["-a".to_owned()]).unwrap();

        assert!(body.contains("\"selector\":\"role=web\""));
        assert!(body.contains("\"timeout_seconds\":45"));
        assert!(!body.contains("admin-secret"));
    }

    #[test]
    fn run_without_selector_uses_local_context() {
        let cli = Cli::try_parse_from(["sponzey", "run", "--confirm-risk", "uptime"])
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
        let cli = Cli::try_parse_from(["sponzey", "apply", "examples/runbooks/nginx-basic.yml"])
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
            "sponzey",
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
            "sponzey",
            "retention",
            "cleanup",
            "--data-dir",
            data_dir.to_str().unwrap(),
            "--older-than-days",
            "0",
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
    fn parses_demo_command() {
        let cli = Cli::try_parse_from(["sponzey", "demo", "--keep-temp", "--port", "17700"])
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
        fs::create_dir_all(dir.join("nvme0n1").join("queue")).unwrap();
        fs::write(dir.join("nvme0n1").join("size"), "4194304\n").unwrap();
        fs::write(dir.join("nvme0n1").join("removable"), "0\n").unwrap();
        fs::write(dir.join("nvme0n1").join("queue").join("rotational"), "0\n").unwrap();
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
                },
                BlockDeviceFact {
                    name: "sda".to_owned(),
                    kind: "disk".to_owned(),
                    size_kb: Some(1_048_576),
                    removable: Some(false),
                    rotational: Some(true),
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
        let error = Cli::try_parse_from(["sponzey", "unknown"]).expect_err("invalid command");
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
            format!("sponzey {}", env!("CARGO_PKG_VERSION"))
        );
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
            "sponzey",
            "agent",
            "start",
            "--data-dir",
            ".sponzey",
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
            "sponzey",
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
            "sponzey",
            "agent",
            "init",
            "--url",
            "https://fleet.example.com",
            "--tls-ca-cert",
            "/etc/sponzey/tls/ca.pem",
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
            Some(Path::new("/etc/sponzey/tls/ca.pem"))
        );
    }

    #[test]
    fn agent_enroll_remains_alias_for_init() {
        let cli = Cli::try_parse_from([
            "sponzey",
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
        let cli = Cli::try_parse_from(["sponzey", "run", "uptime"]).expect("valid command");
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
            "sponzey",
            "logs",
            "web-01",
            "--file",
            "/definitely/missing/sponzey.log",
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
        let (sender, _receiver) = mpsc::sync_channel(1);
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
    fn agent_session_busy_task_rejects_new_assignment() {
        let (sender, receiver) = mpsc::sync_channel(4);
        let config = test_agent_config();
        let task_busy = Arc::new(AtomicBool::new(true));
        let replay_guard = Arc::new(Mutex::new(fleet_runner::NonceReplayGuard::default()));

        handle_agent_session_message(
            test_task_assignment_message(&config.agent_id),
            &config,
            "controller-public-key",
            "corr-test",
            &sender,
            &task_busy,
            &replay_guard,
        )
        .unwrap();

        let event = receiver.try_recv().expect("busy reject event");
        let fleet_protocol::WirePayload::SecurityEvent {
            agent_id,
            action,
            detail,
        } = event.payload
        else {
            panic!("expected security event");
        };
        assert_eq!(agent_id, config.agent_id);
        assert_eq!(action, "task_rejected");
        assert_eq!(detail, "agent is busy");
        assert!(task_busy.load(Ordering::SeqCst));
    }

    #[test]
    fn agent_session_due_telemetry_does_not_precede_inbound_task_handling() {
        let (sender, receiver) = mpsc::sync_channel(8);
        let config = test_agent_config();
        let task_busy = Arc::new(AtomicBool::new(true));
        let replay_guard = Arc::new(Mutex::new(fleet_runner::NonceReplayGuard::default()));

        handle_agent_session_message(
            test_task_assignment_message(&config.agent_id),
            &config,
            "controller-public-key",
            "corr-test",
            &sender,
            &task_busy,
            &replay_guard,
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
            fleet_protocol::WirePayload::SecurityEvent { ref action, .. } if action == "task_rejected"
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
        let (sender, receiver) = mpsc::sync_channel(4);
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
        send_agent_task_result_queue(&sender, &config, "corr-test", &envelope, 0).unwrap();
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
    }

    #[test]
    fn reads_agent_config_with_tls_ca_cert() {
        let dir = unique_test_dir("secure-agent-config-tls-ca");
        fs::create_dir_all(&dir).unwrap();
        write_secure_file(
            &dir.join("agent.conf"),
            "url=https://127.0.0.1:7700\ntls_ca_cert=/tmp/sponzey-ca.pem\nagent_id=agent-web-01\nfingerprint=fp-1\ncontroller_fingerprint=controller-fp-1\n",
        )
        .unwrap();
        write_secure_file(&dir.join("agent_private.key"), "private-key-1\n").unwrap();

        let config = read_agent_config(&dir.join("agent.conf")).unwrap();

        assert_eq!(
            config.tls_ca_cert.as_deref(),
            Some(Path::new("/tmp/sponzey-ca.pem"))
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
            "sponzey-{name}-{}-{}",
            std::process::id(),
            epoch_millis()
        ))
    }

    fn test_agent_config() -> LocalAgentConfig {
        LocalAgentConfig {
            url: "http://127.0.0.1:7700".to_owned(),
            tls_ca_cert: None,
            agent_id: "agent-web-01".to_owned(),
            fingerprint: "agent-fp-1".to_owned(),
            private_key: "private-key-1".to_owned(),
            controller_fingerprint: "controller-fp-1".to_owned(),
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
