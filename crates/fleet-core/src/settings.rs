use std::fmt::{Display, Formatter};
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogProfile {
    #[default]
    Product,
    FieldDebug,
    Development,
}

impl FromStr for LogProfile {
    type Err = SettingsError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "product" | "Product" => Ok(Self::Product),
            "field-debug" | "field_debug" | "FieldDebug" => Ok(Self::FieldDebug),
            "development" | "dev" | "Development" => Ok(Self::Development),
            _ => Err(SettingsError::InvalidLogProfile(value.to_owned())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatabaseBackend {
    Sqlite {
        path: PathBuf,
    },
    Postgres {
        settings: PostgresConnectionSettings,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseSettings {
    backend: DatabaseBackend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactStoreBackend {
    Local { root: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactStoreSettings {
    backend: ArtifactStoreBackend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretProviderBackend {
    Disabled,
    StaticTest { fixture_path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretProviderSettings {
    backend: SecretProviderBackend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsServerIdentitySettings {
    cert_path: PathBuf,
    key_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSigningIdentitySettings {
    public_key_path: PathBuf,
    private_key_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentClientCertificateTrust {
    Disabled,
    Required { ca_cert_path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerTrustSettings {
    tls_server: Option<TlsServerIdentitySettings>,
    controller_signing: ControllerSigningIdentitySettings,
    agent_client_certificate: AgentClientCertificateTrust,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PostgresSslMode {
    #[default]
    Disable,
    Prefer,
    Require,
}

impl FromStr for PostgresSslMode {
    type Err = SettingsError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "disable" => Ok(Self::Disable),
            "prefer" => Ok(Self::Prefer),
            "require" => Ok(Self::Require),
            _ => Err(SettingsError::InvalidDatabaseUrl(
                "postgres sslmode must be disable, prefer, or require".to_owned(),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostgresConnectionSettings {
    url: String,
    ssl_mode: PostgresSslMode,
    connect_timeout: Duration,
    pool_max_connections: usize,
    pool_checkout_timeout: Duration,
}

impl PostgresConnectionSettings {
    pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
    pub const DEFAULT_POOL_MAX_CONNECTIONS: usize = 4;
    pub const DEFAULT_POOL_CHECKOUT_TIMEOUT: Duration = Duration::from_secs(5);

    pub fn new(url: String) -> Result<Self, SettingsError> {
        let Some(rest) = url
            .strip_prefix("postgres://")
            .or_else(|| url.strip_prefix("postgresql://"))
        else {
            return Err(SettingsError::UnsupportedDatabaseUrlScheme(
                database_url_scheme(&url).to_owned(),
            ));
        };
        if rest.trim().is_empty() {
            return Err(SettingsError::InvalidDatabaseUrl(
                "postgres URL cannot be empty".to_owned(),
            ));
        }

        let ssl_mode = postgres_query_param(&url, "sslmode")
            .map(str::parse)
            .transpose()?
            .unwrap_or_default();
        let connect_timeout = postgres_query_param(&url, "connect_timeout")
            .map(parse_postgres_connect_timeout)
            .transpose()?
            .unwrap_or(Self::DEFAULT_CONNECT_TIMEOUT);
        let pool_max_connections = postgres_query_param(&url, "pool_max_connections")
            .map(parse_postgres_pool_max_connections)
            .transpose()?
            .unwrap_or(Self::DEFAULT_POOL_MAX_CONNECTIONS);
        let pool_checkout_timeout = postgres_query_param(&url, "pool_checkout_timeout")
            .map(parse_postgres_pool_checkout_timeout)
            .transpose()?
            .unwrap_or(Self::DEFAULT_POOL_CHECKOUT_TIMEOUT);

        Ok(Self {
            url,
            ssl_mode,
            connect_timeout,
            pool_max_connections,
            pool_checkout_timeout,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn ssl_mode(&self) -> PostgresSslMode {
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

impl DatabaseSettings {
    pub fn parse_optional(
        value: Option<&str>,
        default_sqlite_path: PathBuf,
    ) -> Result<Self, SettingsError> {
        match value {
            Some(url) => Self::parse_url(url),
            None => Self::sqlite(default_sqlite_path),
        }
    }

    pub fn sqlite(path: PathBuf) -> Result<Self, SettingsError> {
        if path.as_os_str().is_empty() {
            return Err(SettingsError::InvalidDatabaseUrl(
                "sqlite path cannot be empty".to_owned(),
            ));
        }

        Ok(Self {
            backend: DatabaseBackend::Sqlite { path },
        })
    }

    pub fn postgres(url: String) -> Result<Self, SettingsError> {
        Ok(Self {
            backend: DatabaseBackend::Postgres {
                settings: PostgresConnectionSettings::new(url)?,
            },
        })
    }

    pub fn backend(&self) -> &DatabaseBackend {
        &self.backend
    }

    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            DatabaseBackend::Sqlite { .. } => "sqlite",
            DatabaseBackend::Postgres { .. } => "postgres",
        }
    }

    pub fn sqlite_path(&self) -> Option<&Path> {
        match &self.backend {
            DatabaseBackend::Sqlite { path } => Some(path.as_path()),
            DatabaseBackend::Postgres { .. } => None,
        }
    }

    fn parse_url(url: &str) -> Result<Self, SettingsError> {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            return Err(SettingsError::InvalidDatabaseUrl(
                "database URL cannot be empty".to_owned(),
            ));
        }
        if let Some(path) = trimmed.strip_prefix("sqlite://") {
            if path.trim().is_empty() {
                return Err(SettingsError::InvalidDatabaseUrl(
                    "sqlite path cannot be empty".to_owned(),
                ));
            }
            return Self::sqlite(PathBuf::from(path));
        }
        if trimmed.starts_with("postgres://") || trimmed.starts_with("postgresql://") {
            return Self::postgres(trimmed.to_owned());
        }

        Err(SettingsError::UnsupportedDatabaseUrlScheme(
            database_url_scheme(trimmed).to_owned(),
        ))
    }
}

impl ArtifactStoreSettings {
    pub fn default_local(data_dir: impl AsRef<Path>) -> Result<Self, SettingsError> {
        Self::local(data_dir.as_ref().join("controller").join("artifacts"))
    }

    pub fn local(root: PathBuf) -> Result<Self, SettingsError> {
        if root.as_os_str().is_empty() {
            return Err(SettingsError::InvalidArtifactStore(
                "local artifact store root cannot be empty".to_owned(),
            ));
        }

        Ok(Self {
            backend: ArtifactStoreBackend::Local { root },
        })
    }

    pub fn backend(&self) -> &ArtifactStoreBackend {
        &self.backend
    }

    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            ArtifactStoreBackend::Local { .. } => "local",
        }
    }

    pub fn local_root(&self) -> &Path {
        match &self.backend {
            ArtifactStoreBackend::Local { root } => root.as_path(),
        }
    }
}

impl SecretProviderSettings {
    pub fn parse_optional(
        kind: Option<&str>,
        static_fixture_path: Option<PathBuf>,
    ) -> Result<Self, SettingsError> {
        match kind.map(str::trim).filter(|value| !value.is_empty()) {
            None | Some("disabled") => {
                if static_fixture_path.is_some() {
                    return Err(SettingsError::InvalidSecretProvider(
                        "disabled secret provider cannot define a static fixture path".to_owned(),
                    ));
                }
                Ok(Self::disabled())
            }
            Some("static") | Some("static-test") => {
                let Some(path) = static_fixture_path else {
                    return Err(SettingsError::InvalidSecretProvider(
                        "static test provider requires a json fixture path".to_owned(),
                    ));
                };
                Self::static_test(path)
            }
            Some(value) => Err(SettingsError::UnsupportedSecretProvider(
                secret_provider_kind(value).to_owned(),
            )),
        }
    }

    pub fn disabled() -> Self {
        Self {
            backend: SecretProviderBackend::Disabled,
        }
    }

    pub fn static_test(fixture_path: PathBuf) -> Result<Self, SettingsError> {
        validate_static_secret_fixture_path(&fixture_path)?;
        Ok(Self {
            backend: SecretProviderBackend::StaticTest { fixture_path },
        })
    }

    pub fn backend(&self) -> &SecretProviderBackend {
        &self.backend
    }

    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            SecretProviderBackend::Disabled => "disabled",
            SecretProviderBackend::StaticTest { .. } => "static-test",
        }
    }

    pub fn static_test_fixture_path(&self) -> Option<&Path> {
        match &self.backend {
            SecretProviderBackend::Disabled => None,
            SecretProviderBackend::StaticTest { fixture_path } => Some(fixture_path.as_path()),
        }
    }
}

impl Default for SecretProviderSettings {
    fn default() -> Self {
        Self::disabled()
    }
}

impl TlsServerIdentitySettings {
    pub fn parse_optional(
        cert_path: Option<PathBuf>,
        key_path: Option<PathBuf>,
    ) -> Result<Option<Self>, SettingsError> {
        match (cert_path, key_path) {
            (Some(cert_path), Some(key_path)) => Ok(Some(Self::new(cert_path, key_path)?)),
            (None, None) => Ok(None),
            _ => Err(SettingsError::InvalidTrustSettings(
                "TLS server certificate and private key paths must be provided together".to_owned(),
            )),
        }
    }

    pub fn new(cert_path: PathBuf, key_path: PathBuf) -> Result<Self, SettingsError> {
        validate_trust_file_path(&cert_path, "TLS server certificate path")?;
        validate_trust_file_path(&key_path, "TLS server private key path")?;
        if cert_path == key_path {
            return Err(SettingsError::InvalidTrustSettings(
                "TLS server certificate path and private key path must be distinct".to_owned(),
            ));
        }
        Ok(Self {
            cert_path,
            key_path,
        })
    }

    pub fn cert_path(&self) -> &Path {
        self.cert_path.as_path()
    }

    pub fn key_path(&self) -> &Path {
        self.key_path.as_path()
    }
}

impl ControllerSigningIdentitySettings {
    pub fn new(public_key_path: PathBuf, private_key_path: PathBuf) -> Result<Self, SettingsError> {
        validate_trust_file_path(&public_key_path, "controller signing public key path")?;
        validate_trust_file_path(&private_key_path, "controller signing private key path")?;
        if public_key_path == private_key_path {
            return Err(SettingsError::InvalidTrustSettings(
                "controller signing public key path and private key path must be distinct"
                    .to_owned(),
            ));
        }
        Ok(Self {
            public_key_path,
            private_key_path,
        })
    }

    pub fn default_data_dir(data_dir: impl AsRef<Path>) -> Result<Self, SettingsError> {
        let controller_dir = data_dir.as_ref().join("controller");
        Self::new(
            controller_dir.join("controller_public.key"),
            controller_dir.join("controller_private.key"),
        )
    }

    pub fn public_key_path(&self) -> &Path {
        self.public_key_path.as_path()
    }

    pub fn private_key_path(&self) -> &Path {
        self.private_key_path.as_path()
    }
}

impl AgentClientCertificateTrust {
    pub fn disabled() -> Self {
        Self::Disabled
    }

    pub fn required(ca_cert_path: PathBuf) -> Result<Self, SettingsError> {
        validate_trust_file_path(&ca_cert_path, "agent client certificate CA path")?;
        if is_wildcard_trust_target(&ca_cert_path) {
            return Err(SettingsError::InvalidTrustSettings(
                "agent client certificate trust must reference a CA certificate path, not a wildcard target"
                    .to_owned(),
            ));
        }
        Ok(Self::Required { ca_cert_path })
    }

    pub fn ca_cert_path(&self) -> Option<&Path> {
        match self {
            Self::Disabled => None,
            Self::Required { ca_cert_path } => Some(ca_cert_path.as_path()),
        }
    }
}

impl ControllerTrustSettings {
    pub fn new(
        tls_server: Option<TlsServerIdentitySettings>,
        controller_signing: ControllerSigningIdentitySettings,
        agent_client_certificate: AgentClientCertificateTrust,
    ) -> Result<Self, SettingsError> {
        if let Some(tls_server) = &tls_server {
            if tls_server.key_path() == controller_signing.private_key_path() {
                return Err(SettingsError::InvalidTrustSettings(
                    "TLS server private key and controller signing private key must be separate files"
                        .to_owned(),
                ));
            }
            if tls_server.cert_path() == controller_signing.public_key_path() {
                return Err(SettingsError::InvalidTrustSettings(
                    "TLS server certificate and controller signing public key must be separate files"
                        .to_owned(),
                ));
            }
        }
        Ok(Self {
            tls_server,
            controller_signing,
            agent_client_certificate,
        })
    }

    pub fn from_parts(
        tls_cert_path: Option<PathBuf>,
        tls_key_path: Option<PathBuf>,
        controller_signing_public_key_path: PathBuf,
        controller_signing_private_key_path: PathBuf,
        agent_client_certificate: AgentClientCertificateTrust,
    ) -> Result<Self, SettingsError> {
        Self::new(
            TlsServerIdentitySettings::parse_optional(tls_cert_path, tls_key_path)?,
            ControllerSigningIdentitySettings::new(
                controller_signing_public_key_path,
                controller_signing_private_key_path,
            )?,
            agent_client_certificate,
        )
    }

    pub fn tls_server(&self) -> Option<&TlsServerIdentitySettings> {
        self.tls_server.as_ref()
    }

    pub fn controller_signing(&self) -> &ControllerSigningIdentitySettings {
        &self.controller_signing
    }

    pub fn agent_client_certificate(&self) -> &AgentClientCertificateTrust {
        &self.agent_client_certificate
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub bind_addr: SocketAddr,
    pub controller_url: Option<String>,
    pub log_profile: LogProfile,
}

impl Settings {
    pub fn new(
        bind_addr: SocketAddr,
        controller_url: Option<String>,
        log_profile: LogProfile,
    ) -> Result<Self, SettingsError> {
        if let Some(url) = controller_url.as_deref() {
            validate_controller_url(url)?;
        }

        Ok(Self {
            bind_addr,
            controller_url,
            log_profile,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    InvalidBindAddr(String),
    InvalidArtifactStore(String),
    InvalidDatabaseUrl(String),
    InvalidLogProfile(String),
    InvalidSecretProvider(String),
    InvalidTrustSettings(String),
    UnsupportedDatabaseUrlScheme(String),
    UnsupportedSecretProvider(String),
    UnsupportedUrlScheme(String),
}

impl Display for SettingsError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBindAddr(value) => write!(formatter, "invalid bind address: {value}"),
            Self::InvalidArtifactStore(message) => {
                write!(formatter, "invalid artifact store settings: {message}")
            }
            Self::InvalidDatabaseUrl(message) => {
                write!(formatter, "invalid database URL: {message}")
            }
            Self::InvalidLogProfile(value) => write!(formatter, "invalid log profile: {value}"),
            Self::InvalidSecretProvider(message) => {
                write!(formatter, "invalid secret provider settings: {message}")
            }
            Self::InvalidTrustSettings(message) => {
                write!(formatter, "invalid trust settings: {message}")
            }
            Self::UnsupportedDatabaseUrlScheme(scheme) => {
                write!(formatter, "database URL scheme is not supported: {scheme}")
            }
            Self::UnsupportedSecretProvider(kind) => {
                write!(formatter, "secret provider is not supported: {kind}")
            }
            Self::UnsupportedUrlScheme(value) => write!(
                formatter,
                "controller URL must start with http:// or https://: {value}"
            ),
        }
    }
}

impl std::error::Error for SettingsError {}

pub fn parse_bind_addr(value: &str) -> Result<SocketAddr, SettingsError> {
    value
        .parse()
        .map_err(|_| SettingsError::InvalidBindAddr(value.to_owned()))
}

fn validate_controller_url(url: &str) -> Result<(), SettingsError> {
    if is_https_url(url) {
        return Ok(());
    }

    if is_http_url(url) {
        return Ok(());
    }

    Err(SettingsError::UnsupportedUrlScheme(url.to_owned()))
}

fn database_url_scheme(url: &str) -> &str {
    url.split_once("://")
        .map(|(scheme, _)| scheme)
        .filter(|scheme| !scheme.is_empty())
        .unwrap_or("missing")
}

fn secret_provider_kind(value: &str) -> &str {
    value
        .split_once("://")
        .map(|(scheme, _)| scheme)
        .or_else(|| value.split_once('=').map(|(kind, _)| kind))
        .map(str::trim)
        .filter(|kind| !kind.is_empty())
        .unwrap_or("missing")
}

fn validate_static_secret_fixture_path(path: &Path) -> Result<(), SettingsError> {
    if path.as_os_str().is_empty() {
        return Err(SettingsError::InvalidSecretProvider(
            "static test provider requires a json fixture path".to_owned(),
        ));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(SettingsError::InvalidSecretProvider(
            "static test provider fixture path cannot contain parent directory segments".to_owned(),
        ));
    }
    let value = path.to_string_lossy();
    if value.contains("://") || value.contains('=') || value.contains('\n') {
        return Err(SettingsError::InvalidSecretProvider(
            "static test provider source must be a json fixture path".to_owned(),
        ));
    }
    if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
        return Err(SettingsError::InvalidSecretProvider(
            "static test provider source must be a json fixture path".to_owned(),
        ));
    }
    Ok(())
}

fn validate_trust_file_path(path: &Path, label: &str) -> Result<(), SettingsError> {
    if path.as_os_str().is_empty() {
        return Err(SettingsError::InvalidTrustSettings(format!(
            "{label} cannot be empty"
        )));
    }
    if path.to_string_lossy().contains('\n') {
        return Err(SettingsError::InvalidTrustSettings(format!(
            "{label} cannot contain line breaks"
        )));
    }
    Ok(())
}

fn is_wildcard_trust_target(path: &Path) -> bool {
    matches!(
        path.to_string_lossy().trim(),
        "*" | "0.0.0.0" | "::" | "[::]"
    )
}

fn postgres_query_param<'a>(url: &'a str, name: &str) -> Option<&'a str> {
    url.split_once('?')?.1.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == name { Some(value) } else { None }
    })
}

fn parse_postgres_connect_timeout(value: &str) -> Result<Duration, SettingsError> {
    let seconds = value.parse::<u64>().map_err(|_| {
        SettingsError::InvalidDatabaseUrl(
            "postgres connect_timeout must be a positive integer".to_owned(),
        )
    })?;
    if seconds == 0 {
        return Err(SettingsError::InvalidDatabaseUrl(
            "postgres connect_timeout must be greater than zero".to_owned(),
        ));
    }
    Ok(Duration::from_secs(seconds))
}

fn parse_postgres_pool_max_connections(value: &str) -> Result<usize, SettingsError> {
    let max_connections = value.parse::<usize>().map_err(|_| {
        SettingsError::InvalidDatabaseUrl(
            "postgres pool_max_connections must be a positive integer".to_owned(),
        )
    })?;
    if max_connections == 0 {
        return Err(SettingsError::InvalidDatabaseUrl(
            "postgres pool_max_connections must be greater than zero".to_owned(),
        ));
    }
    Ok(max_connections)
}

fn parse_postgres_pool_checkout_timeout(value: &str) -> Result<Duration, SettingsError> {
    let seconds = value.parse::<u64>().map_err(|_| {
        SettingsError::InvalidDatabaseUrl(
            "postgres pool_checkout_timeout must be a positive integer".to_owned(),
        )
    })?;
    if seconds == 0 {
        return Err(SettingsError::InvalidDatabaseUrl(
            "postgres pool_checkout_timeout must be greater than zero".to_owned(),
        ));
    }
    Ok(Duration::from_secs(seconds))
}

fn is_https_url(url: &str) -> bool {
    url.starts_with("https://")
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_bind_addr() {
        let addr = parse_bind_addr("127.0.0.1:7700").expect("bind address should parse");
        assert_eq!(addr.to_string(), "127.0.0.1:7700");
    }

    #[test]
    fn rejects_invalid_bind_addr() {
        let error = parse_bind_addr("not-an-address").expect_err("bind address should fail");
        assert_eq!(
            error,
            SettingsError::InvalidBindAddr("not-an-address".to_owned())
        );
    }

    #[test]
    fn rejects_invalid_log_profile() {
        let error = "verbose"
            .parse::<LogProfile>()
            .expect_err("log profile should fail");
        assert_eq!(
            error,
            SettingsError::InvalidLogProfile("verbose".to_owned())
        );
    }

    #[test]
    fn database_settings_default_to_sqlite_path() {
        let settings = DatabaseSettings::parse_optional(
            None,
            PathBuf::from("/var/lib/fleet/controller/fleet.db"),
        )
        .expect("default sqlite settings should parse");

        assert_eq!(settings.backend_name(), "sqlite");
        assert_eq!(
            settings.sqlite_path(),
            Some(Path::new("/var/lib/fleet/controller/fleet.db"))
        );
    }

    #[test]
    fn artifact_store_settings_default_to_local_controller_artifacts_path() {
        let settings = ArtifactStoreSettings::default_local(Path::new("/var/lib/fleet"))
            .expect("default artifact store settings should parse");

        assert_eq!(settings.backend_name(), "local");
        assert_eq!(
            settings.local_root(),
            Path::new("/var/lib/fleet/controller/artifacts")
        );
        assert_eq!(
            settings.backend(),
            &ArtifactStoreBackend::Local {
                root: PathBuf::from("/var/lib/fleet/controller/artifacts")
            }
        );
    }

    #[test]
    fn artifact_store_settings_reject_empty_local_root() {
        let error =
            ArtifactStoreSettings::local(PathBuf::new()).expect_err("empty local root should fail");

        assert_eq!(
            error,
            SettingsError::InvalidArtifactStore(
                "local artifact store root cannot be empty".to_owned()
            )
        );
    }

    #[test]
    fn secret_provider_settings_default_to_disabled_mode() {
        let settings =
            SecretProviderSettings::parse_optional(None, None).expect("default should parse");

        assert_eq!(settings.backend_name(), "disabled");
        assert_eq!(settings.backend(), &SecretProviderBackend::Disabled);
        assert_eq!(settings.static_test_fixture_path(), None);
    }

    #[test]
    fn secret_provider_settings_accept_static_test_fixture_path() {
        let settings = SecretProviderSettings::parse_optional(
            Some("static-test"),
            Some(PathBuf::from("fixtures/secrets.json")),
        )
        .expect("static test fixture path should parse");

        assert_eq!(settings.backend_name(), "static-test");
        assert_eq!(
            settings.static_test_fixture_path(),
            Some(Path::new("fixtures/secrets.json"))
        );
    }

    #[test]
    fn secret_provider_settings_reject_unsupported_kind_without_secret_leak() {
        let error = SecretProviderSettings::parse_optional(
            Some("vault://token=raw-secret-value"),
            Some(PathBuf::from("fixtures/secrets.json")),
        )
        .expect_err("unsupported provider should fail");

        assert_eq!(
            error,
            SettingsError::UnsupportedSecretProvider("vault".to_owned())
        );
        let message = error.to_string();
        assert!(!message.contains("raw-secret-value"));
        assert!(!message.contains("token="));
    }

    #[test]
    fn secret_provider_settings_reject_inline_raw_secret_candidate() {
        let error = SecretProviderSettings::parse_optional(
            Some("static-test"),
            Some(PathBuf::from("api_token=raw-secret-value")),
        )
        .expect_err("inline raw secret should fail");

        assert_eq!(
            error,
            SettingsError::InvalidSecretProvider(
                "static test provider source must be a json fixture path".to_owned()
            )
        );
        assert!(!error.to_string().contains("raw-secret-value"));
    }

    #[test]
    fn database_settings_parse_sqlite_url() {
        let settings = DatabaseSettings::parse_optional(
            Some("sqlite:///tmp/fleet.db"),
            PathBuf::from("/ignored/default.db"),
        )
        .expect("sqlite URL should parse");

        assert_eq!(
            settings,
            DatabaseSettings::sqlite(PathBuf::from("/tmp/fleet.db")).unwrap()
        );
    }

    #[test]
    fn database_settings_classify_postgres_url_without_path_conversion() {
        let settings = DatabaseSettings::parse_optional(
            Some("postgresql://fleet:secret@db.example.com/fleet"),
            PathBuf::from("/ignored/default.db"),
        )
        .expect("postgres URL should be recognized");

        assert_eq!(settings.backend_name(), "postgres");
        assert!(matches!(
            settings.backend(),
            DatabaseBackend::Postgres { .. }
        ));
        assert_eq!(settings.sqlite_path(), None);
    }

    #[test]
    fn database_settings_parse_postgres_connection_options() {
        let settings = DatabaseSettings::parse_optional(
            Some(
                "postgresql://fleet:secret@db.example.com/fleet?sslmode=require&connect_timeout=7&pool_max_connections=8&pool_checkout_timeout=3",
            ),
            PathBuf::from("/ignored/default.db"),
        )
        .expect("postgres settings should parse");

        let DatabaseBackend::Postgres { settings } = settings.backend() else {
            panic!("postgres settings expected");
        };
        assert_eq!(settings.ssl_mode(), PostgresSslMode::Require);
        assert_eq!(settings.connect_timeout(), Duration::from_secs(7));
        assert_eq!(settings.pool_max_connections(), 8);
        assert_eq!(settings.pool_checkout_timeout(), Duration::from_secs(3));
        assert!(settings.url().contains("sslmode=require"));
    }

    #[test]
    fn database_settings_reject_invalid_postgres_sslmode_without_url_leak() {
        let error = DatabaseSettings::parse_optional(
            Some("postgresql://fleet:secret@db.example.com/fleet?sslmode=verify-full"),
            PathBuf::from("/ignored/default.db"),
        )
        .expect_err("invalid sslmode should fail");

        assert_eq!(
            error,
            SettingsError::InvalidDatabaseUrl(
                "postgres sslmode must be disable, prefer, or require".to_owned()
            )
        );
        let message = error.to_string();
        assert!(!message.contains("secret"));
        assert!(!message.contains("db.example.com"));
    }

    #[test]
    fn database_settings_reject_zero_postgres_connect_timeout() {
        let error = DatabaseSettings::parse_optional(
            Some("postgresql://fleet:secret@db.example.com/fleet?connect_timeout=0"),
            PathBuf::from("/ignored/default.db"),
        )
        .expect_err("zero timeout should fail");

        assert_eq!(
            error,
            SettingsError::InvalidDatabaseUrl(
                "postgres connect_timeout must be greater than zero".to_owned()
            )
        );
        assert!(!error.to_string().contains("secret"));
    }

    #[test]
    fn database_settings_reject_zero_postgres_pool_settings_without_url_leak() {
        let error = DatabaseSettings::parse_optional(
            Some("postgresql://fleet:secret@db.example.com/fleet?pool_max_connections=0"),
            PathBuf::from("/ignored/default.db"),
        )
        .expect_err("zero pool size should fail");

        assert_eq!(
            error,
            SettingsError::InvalidDatabaseUrl(
                "postgres pool_max_connections must be greater than zero".to_owned()
            )
        );
        let message = error.to_string();
        assert!(!message.contains("secret"));
        assert!(!message.contains("db.example.com"));

        let error = DatabaseSettings::parse_optional(
            Some("postgresql://fleet:secret@db.example.com/fleet?pool_checkout_timeout=0"),
            PathBuf::from("/ignored/default.db"),
        )
        .expect_err("zero checkout timeout should fail");

        assert_eq!(
            error,
            SettingsError::InvalidDatabaseUrl(
                "postgres pool_checkout_timeout must be greater than zero".to_owned()
            )
        );
        let message = error.to_string();
        assert!(!message.contains("secret"));
        assert!(!message.contains("db.example.com"));
    }

    #[test]
    fn database_settings_reject_unsupported_scheme_without_storing_full_url() {
        let error = DatabaseSettings::parse_optional(
            Some("mysql://user:secret@db.example.com/fleet"),
            PathBuf::from("/ignored/default.db"),
        )
        .expect_err("unsupported database URL should fail");

        assert_eq!(
            error,
            SettingsError::UnsupportedDatabaseUrlScheme("mysql".to_owned())
        );
        assert!(!error.to_string().contains("secret"));
    }

    #[test]
    fn trust_settings_keep_tls_and_signing_identities_distinct() {
        let settings = ControllerTrustSettings::from_parts(
            Some(PathBuf::from("/etc/fleet/tls/server.crt")),
            Some(PathBuf::from("/etc/fleet/tls/server.key")),
            PathBuf::from("/var/lib/fleet/controller/controller_public.key"),
            PathBuf::from("/var/lib/fleet/controller/controller_private.key"),
            AgentClientCertificateTrust::disabled(),
        )
        .expect("separate trust settings should parse");

        let tls = settings.tls_server().expect("tls settings expected");
        assert_eq!(tls.cert_path(), Path::new("/etc/fleet/tls/server.crt"));
        assert_eq!(tls.key_path(), Path::new("/etc/fleet/tls/server.key"));
        assert_eq!(
            settings.controller_signing().public_key_path(),
            Path::new("/var/lib/fleet/controller/controller_public.key")
        );
        assert_eq!(
            settings.controller_signing().private_key_path(),
            Path::new("/var/lib/fleet/controller/controller_private.key")
        );
        assert_eq!(
            settings.agent_client_certificate(),
            &AgentClientCertificateTrust::Disabled
        );
    }

    #[test]
    fn trust_settings_reject_missing_tls_key_pair_without_path_leak() {
        let error = ControllerTrustSettings::from_parts(
            Some(PathBuf::from("/etc/fleet/secret-server.crt")),
            None,
            PathBuf::from("/var/lib/fleet/controller/controller_public.key"),
            PathBuf::from("/var/lib/fleet/controller/controller_private.key"),
            AgentClientCertificateTrust::disabled(),
        )
        .expect_err("partial TLS identity should fail");

        assert_eq!(
            error,
            SettingsError::InvalidTrustSettings(
                "TLS server certificate and private key paths must be provided together".to_owned()
            )
        );
        assert!(!error.to_string().contains("secret-server"));
    }

    #[test]
    fn trust_settings_reject_tls_private_key_reused_as_signing_private_key() {
        let error = ControllerTrustSettings::from_parts(
            Some(PathBuf::from("/etc/fleet/tls/server.crt")),
            Some(PathBuf::from("/etc/fleet/shared-private.key")),
            PathBuf::from("/var/lib/fleet/controller/controller_public.key"),
            PathBuf::from("/etc/fleet/shared-private.key"),
            AgentClientCertificateTrust::disabled(),
        )
        .expect_err("TLS key reuse for signing should fail");

        assert_eq!(
            error,
            SettingsError::InvalidTrustSettings(
                "TLS server private key and controller signing private key must be separate files"
                    .to_owned()
            )
        );
        assert!(!error.to_string().contains("shared-private"));
    }

    #[test]
    fn trust_settings_do_not_derive_controller_signing_from_tls_fingerprint() {
        let settings = ControllerTrustSettings::from_parts(
            Some(PathBuf::from("/etc/fleet/tls/server.crt")),
            Some(PathBuf::from("/etc/fleet/tls/server.key")),
            PathBuf::from("/var/lib/fleet/controller/controller_public.key"),
            PathBuf::from("/var/lib/fleet/controller/controller_private.key"),
            AgentClientCertificateTrust::disabled(),
        )
        .expect("separate identity settings should parse");

        assert_ne!(
            settings.tls_server().unwrap().cert_path(),
            settings.controller_signing().public_key_path()
        );
    }

    #[test]
    fn agent_client_certificate_trust_rejects_wildcard_target_without_leak() {
        let error = AgentClientCertificateTrust::required(PathBuf::from("*"))
            .expect_err("wildcard trust target should fail");

        assert_eq!(
            error,
            SettingsError::InvalidTrustSettings(
                "agent client certificate trust must reference a CA certificate path, not a wildcard target"
                    .to_owned()
            )
        );
        assert!(!error.to_string().contains('*'));
    }

    #[test]
    fn accepts_http_controller_url() {
        let bind_addr = parse_bind_addr("127.0.0.1:7700").expect("valid bind address");
        let settings = Settings::new(
            bind_addr,
            Some("http://10.0.0.5:7700".to_owned()),
            LogProfile::Product,
        )
        .expect("http URL should be allowed with warnings at runtime boundaries");

        assert_eq!(
            settings.controller_url.as_deref(),
            Some("http://10.0.0.5:7700")
        );
    }

    #[test]
    fn rejects_unsupported_controller_url_scheme() {
        let bind_addr = parse_bind_addr("127.0.0.1:7700").expect("valid bind address");
        let error = Settings::new(
            bind_addr,
            Some("ftp://controller.example.com".to_owned()),
            LogProfile::Product,
        )
        .expect_err("unsupported scheme should fail");

        assert_eq!(
            error,
            SettingsError::UnsupportedUrlScheme("ftp://controller.example.com".to_owned())
        );
    }
}
