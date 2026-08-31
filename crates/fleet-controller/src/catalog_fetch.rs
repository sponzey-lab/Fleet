//! Public Git catalog staging decoder.
//!
//! This controller-infrastructure boundary resolves and preflights a public Git authority, then
//! reads a resolved commit and maps bounded regular YAML blobs into the application fetch result.
//! Network fetch and its worker lifecycle are kept separate so this module never accepts
//! credentials or assembles shell commands.

use fleet_application::{
    CatalogFetchCancellation, CatalogFetchResult, CatalogFetchedDocument, CatalogFetcher,
};
use fleet_domain::{
    CatalogDocumentKind, CatalogSource, parse_policy_document, parse_runbook_document,
};
use git2::{
    AutotagOption, FetchOptions, FileMode, ProxyOptions, RemoteCallbacks, RemoteRedirect,
    Repository, TreeWalkMode, build::RepoBuilder,
};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::fmt::{Display, Formatter};
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const DEFAULT_MAX_CATALOG_DOCUMENTS: usize = 1_000;
pub const DEFAULT_MAX_CATALOG_DOCUMENT_BYTES: usize = 1_048_576;
pub const DEFAULT_MAX_CATALOG_TRANSFER_BYTES: usize = 67_108_864;
const CATALOG_FETCH_HARD_DEADLINE: Duration = Duration::from_secs(30);
const PINNED_PROXY_IO_POLL_INTERVAL: Duration = Duration::from_millis(25);
const PINNED_PROXY_MAX_REQUEST_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicGitCatalogFetchLimits {
    max_documents: usize,
    max_document_bytes: usize,
}

impl PublicGitCatalogFetchLimits {
    pub fn new(max_documents: usize, max_document_bytes: usize) -> Option<Self> {
        (max_documents > 0 && max_document_bytes > 0).then_some(Self {
            max_documents,
            max_document_bytes,
        })
    }
}

impl Default for PublicGitCatalogFetchLimits {
    fn default() -> Self {
        Self {
            max_documents: DEFAULT_MAX_CATALOG_DOCUMENTS,
            max_document_bytes: DEFAULT_MAX_CATALOG_DOCUMENT_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicGitCatalogFetchError {
    InvalidStagingDirectory,
    TransferLimitExceeded,
    CredentialsRejected,
    DestinationRejected,
    Cancelled,
    CommitNotFound,
    UnsupportedGitEntry,
    DocumentLimitExceeded,
    DocumentTooLarge,
    InvalidDocument,
    RepositoryReadFailed,
}

impl Display for PublicGitCatalogFetchError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidStagingDirectory => "catalog staging directory is unavailable",
            Self::TransferLimitExceeded => "catalog Git transfer exceeds the configured size limit",
            Self::CredentialsRejected => "catalog Git source requires credentials",
            Self::DestinationRejected => "catalog Git source has no permitted public destination",
            Self::Cancelled => "catalog Git source fetch was cancelled",
            Self::CommitNotFound => "catalog Git commit was not found",
            Self::UnsupportedGitEntry => "catalog Git tree contains an unsupported entry",
            Self::DocumentLimitExceeded => "catalog document limit exceeded",
            Self::DocumentTooLarge => "catalog document exceeds the configured size limit",
            Self::InvalidDocument => "catalog contains an invalid runbook or policy document",
            Self::RepositoryReadFailed => "catalog Git repository read failed",
        })
    }
}

impl std::error::Error for PublicGitCatalogFetchError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicCatalogDestinationResolutionError {
    Unavailable,
}

impl Display for PublicCatalogDestinationResolutionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("public catalog destination lookup failed")
    }
}

impl std::error::Error for PublicCatalogDestinationResolutionError {}

/// Resolves the already validated HTTPS authority before a catalog fetch begins.
///
/// This is an infrastructure seam so deterministic tests can prove that every DNS answer must be
/// public. The active `git2` transport cannot pin its later socket resolution to this answer, so
/// callers must not treat this preflight check as complete DNS-rebinding protection.
pub trait PublicCatalogDestinationResolver {
    fn resolve_public_catalog_destination(
        &self,
        host: &str,
        port: u16,
    ) -> Result<Vec<IpAddr>, PublicCatalogDestinationResolutionError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemCatalogDestinationResolver;

impl PublicCatalogDestinationResolver for SystemCatalogDestinationResolver {
    fn resolve_public_catalog_destination(
        &self,
        host: &str,
        port: u16,
    ) -> Result<Vec<IpAddr>, PublicCatalogDestinationResolutionError> {
        (host, port)
            .to_socket_addrs()
            .map(|addresses| addresses.map(|address| address.ip()).collect())
            .map_err(|_| PublicCatalogDestinationResolutionError::Unavailable)
    }
}

/// Fetches a public HTTPS repository into an ephemeral bare staging directory.
///
/// The adapter uses a shallow, no-redirect, no-tag libgit2 fetch and drops the staging directory
/// before returning. It rejects non-public DNS answers before fetch, while connection-pinned DNS
/// rebinding prevention and worker timeout/cancellation remain controller lifecycle work.
#[derive(Debug, Clone)]
pub struct PublicGitCatalogFetcher<R = SystemCatalogDestinationResolver> {
    staging_root: PathBuf,
    limits: PublicGitCatalogFetchLimits,
    max_transfer_bytes: usize,
    destination_resolver: R,
}

impl PublicGitCatalogFetcher<SystemCatalogDestinationResolver> {
    pub fn new(staging_root: impl Into<PathBuf>) -> Self {
        Self {
            staging_root: staging_root.into(),
            limits: PublicGitCatalogFetchLimits::default(),
            max_transfer_bytes: DEFAULT_MAX_CATALOG_TRANSFER_BYTES,
            destination_resolver: SystemCatalogDestinationResolver,
        }
    }

    pub fn with_limits(
        staging_root: impl Into<PathBuf>,
        limits: PublicGitCatalogFetchLimits,
        max_transfer_bytes: usize,
    ) -> Result<Self, PublicGitCatalogFetchError> {
        if limits.max_documents == 0 || limits.max_document_bytes == 0 || max_transfer_bytes == 0 {
            return Err(PublicGitCatalogFetchError::InvalidStagingDirectory);
        }
        Ok(Self {
            staging_root: staging_root.into(),
            limits,
            max_transfer_bytes,
            destination_resolver: SystemCatalogDestinationResolver,
        })
    }
}

impl<R> PublicGitCatalogFetcher<R> {
    pub fn with_destination_resolver(
        staging_root: impl Into<PathBuf>,
        destination_resolver: R,
        limits: PublicGitCatalogFetchLimits,
        max_transfer_bytes: usize,
    ) -> Result<Self, PublicGitCatalogFetchError> {
        if limits.max_documents == 0 || limits.max_document_bytes == 0 || max_transfer_bytes == 0 {
            return Err(PublicGitCatalogFetchError::InvalidStagingDirectory);
        }
        Ok(Self {
            staging_root: staging_root.into(),
            limits,
            max_transfer_bytes,
            destination_resolver,
        })
    }
}

impl<R> PublicGitCatalogFetcher<R>
where
    R: PublicCatalogDestinationResolver,
{
    fn stage_and_decode(
        &self,
        source: &CatalogSource,
        reference: &str,
        cancellation: &dyn CatalogFetchCancellation,
    ) -> Result<CatalogFetchResult, PublicGitCatalogFetchError> {
        if cancellation.is_cancelled() {
            return Err(PublicGitCatalogFetchError::Cancelled);
        }
        let destinations = self
            .destination_resolver
            .resolve_public_catalog_destination(source.url().host(), source.url().port())
            .map_err(|_| PublicGitCatalogFetchError::DestinationRejected)?;
        if destinations.is_empty()
            || destinations
                .iter()
                .any(|address| !is_public_catalog_address(*address))
        {
            return Err(PublicGitCatalogFetchError::DestinationRejected);
        }
        std::fs::create_dir_all(&self.staging_root)
            .map_err(|_| PublicGitCatalogFetchError::InvalidStagingDirectory)?;
        let staging = tempfile::Builder::new()
            .prefix("catalog-")
            .tempdir_in(&self.staging_root)
            .map_err(|_| PublicGitCatalogFetchError::InvalidStagingDirectory)?;
        clone_public_repository(
            source.url().as_str(),
            source.url().host(),
            source.url().port(),
            &destinations,
            staging.path(),
            self.max_transfer_bytes,
            cancellation,
        )
        .and_then(|repository| {
            catalog_fetch_result_from_repository(&repository, reference, self.limits)
        })
    }
}

impl<R> CatalogFetcher for PublicGitCatalogFetcher<R>
where
    R: PublicCatalogDestinationResolver,
{
    type Error = PublicGitCatalogFetchError;

    fn fetch_public_catalog(
        &self,
        source: &CatalogSource,
        cancellation: &dyn CatalogFetchCancellation,
    ) -> Result<CatalogFetchResult, Self::Error> {
        self.stage_and_decode(source, source.reference().as_str(), cancellation)
    }
}

fn is_public_catalog_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_catalog_ipv4(address),
        IpAddr::V6(address) => {
            if let Some(mapped) = address.to_ipv4_mapped() {
                return is_public_catalog_ipv4(mapped);
            }
            !address.is_loopback()
                && !address.is_unspecified()
                && !address.is_multicast()
                && !address.is_unicast_link_local()
                && (address.segments()[0] & 0xfe00) != 0xfc00
                && !address.segments().starts_with(&[0x2001, 0x0db8])
        }
    }
}

fn is_public_catalog_ipv4(address: Ipv4Addr) -> bool {
    let [first, second, third, _] = address.octets();
    let restricted = matches!(first, 0 | 10 | 127 | 224..=255)
        || (first == 100 && (64..=127).contains(&second))
        || (first == 169 && second == 254)
        || (first == 172 && (16..=31).contains(&second))
        || (first == 192 && second == 168)
        || (first == 192 && second == 0 && matches!(third, 0 | 2))
        || (first == 198 && matches!(second, 18 | 19))
        || (first == 198 && second == 51 && third == 100)
        || (first == 203 && second == 0 && third == 113);
    !restricted
}

fn clone_public_repository(
    url: &str,
    host: &str,
    port: u16,
    destinations: &[IpAddr],
    staging_path: &Path,
    max_transfer_bytes: usize,
    cancellation: &dyn CatalogFetchCancellation,
) -> Result<Repository, PublicGitCatalogFetchError> {
    let proxy_cancellation = cancellation
        .shared_signal()
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
    let proxy = PinnedHttpsProxy::start(
        host,
        port,
        destinations.to_vec(),
        Instant::now() + CATALOG_FETCH_HARD_DEADLINE,
        proxy_cancellation,
    )
    .map_err(|_| PublicGitCatalogFetchError::DestinationRejected)?;
    let transfer_limit_hit = Cell::new(false);
    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(|_, _, _| Err(git2::Error::from_str("catalog credentials are rejected")));
    callbacks.transfer_progress(|progress| {
        let permitted =
            !cancellation.is_cancelled() && progress.received_bytes() <= max_transfer_bytes;
        if !permitted {
            transfer_limit_hit.set(true);
        }
        permitted
    });
    let mut fetch = FetchOptions::new();
    let mut proxy_options = ProxyOptions::new();
    proxy_options.url(&format!("http://{}", proxy.local_addr()));
    fetch
        .depth(1)
        .download_tags(AutotagOption::None)
        .follow_redirects(RemoteRedirect::None)
        .proxy_options(proxy_options)
        .remote_callbacks(callbacks);
    let mut builder = RepoBuilder::new();
    builder.bare(true).fetch_options(fetch);
    let result = builder.clone(url, staging_path);
    drop(proxy);
    match result {
        Ok(repository) => Ok(repository),
        Err(_) if cancellation.is_cancelled() => Err(PublicGitCatalogFetchError::Cancelled),
        Err(_) if transfer_limit_hit.get() => {
            Err(PublicGitCatalogFetchError::TransferLimitExceeded)
        }
        Err(_) => Err(PublicGitCatalogFetchError::RepositoryReadFailed),
    }
}

/// A controller-local CONNECT proxy for one HTTPS Git authority.
///
/// Libgit2 continues to perform the TLS handshake using the original hostname, while this proxy
/// opens every upstream socket directly to an already policy-checked address. The listener accepts
/// only the one expected CONNECT authority and its deadline/cancellation closes both tunnel ends.
struct PinnedHttpsProxy {
    local_addr: SocketAddr,
    stopped: Arc<AtomicBool>,
    task: Option<JoinHandle<()>>,
}

impl PinnedHttpsProxy {
    fn start(
        host: &str,
        port: u16,
        destinations: Vec<IpAddr>,
        deadline: Instant,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Self, std::io::Error> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let local_addr = listener.local_addr()?;
        let expected_authority = connect_authority(host, port);
        let worker_cancelled = Arc::clone(&cancelled);
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let task = std::thread::spawn(move || {
            serve_pinned_https_proxy(
                listener,
                expected_authority,
                port,
                destinations,
                deadline,
                worker_cancelled,
                worker_stopped,
            );
        });
        Ok(Self {
            local_addr,
            stopped,
            task: Some(task),
        })
    }

    fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

impl Drop for PinnedHttpsProxy {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(task) = self.task.take() {
            let _ = task.join();
        }
    }
}

fn serve_pinned_https_proxy(
    listener: TcpListener,
    expected_authority: String,
    port: u16,
    destinations: Vec<IpAddr>,
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
) {
    while !proxy_should_stop(deadline, &cancelled, &stopped) {
        match listener.accept() {
            Ok((stream, _)) => handle_pinned_https_proxy_connection(
                stream,
                &expected_authority,
                port,
                &destinations,
                deadline,
                &cancelled,
                &stopped,
            ),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(PINNED_PROXY_IO_POLL_INTERVAL);
            }
            Err(_) => return,
        }
    }
}

fn handle_pinned_https_proxy_connection(
    mut client: TcpStream,
    expected_authority: &str,
    port: u16,
    destinations: &[IpAddr],
    deadline: Instant,
    cancelled: &Arc<AtomicBool>,
    stopped: &Arc<AtomicBool>,
) {
    let _ = client.set_read_timeout(Some(PINNED_PROXY_IO_POLL_INTERVAL));
    let _ = client.set_write_timeout(Some(PINNED_PROXY_IO_POLL_INTERVAL));
    let Ok(authority) = read_connect_authority(&mut client, deadline, cancelled, stopped) else {
        return;
    };
    if authority != expected_authority {
        let _ = client.write_all(b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n");
        return;
    }
    let Ok(upstream) =
        connect_to_pinned_destination(destinations, port, deadline, cancelled, stopped)
    else {
        let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n");
        return;
    };
    let _ = upstream.set_read_timeout(Some(PINNED_PROXY_IO_POLL_INTERVAL));
    let _ = upstream.set_write_timeout(Some(PINNED_PROXY_IO_POLL_INTERVAL));
    if client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .is_err()
    {
        return;
    }
    relay_pinned_tunnel(client, upstream, deadline, cancelled, stopped);
}

fn read_connect_authority(
    stream: &mut TcpStream,
    deadline: Instant,
    cancelled: &AtomicBool,
    stopped: &AtomicBool,
) -> Result<String, ()> {
    let mut request = Vec::new();
    while !proxy_should_stop(deadline, cancelled, stopped)
        && request.len() < PINNED_PROXY_MAX_REQUEST_BYTES
    {
        let mut byte = [0_u8; 1];
        match stream.read(&mut byte) {
            Ok(0) => return Err(()),
            Ok(_) => {
                request.push(byte[0]);
                if request.ends_with(b"\r\n\r\n") {
                    let request = std::str::from_utf8(&request).map_err(|_| ())?;
                    let mut parts = request.lines().next().ok_or(())?.split_ascii_whitespace();
                    return match (parts.next(), parts.next(), parts.next()) {
                        (Some("CONNECT"), Some(authority), Some(_)) if parts.next().is_none() => {
                            Ok(authority.to_ascii_lowercase())
                        }
                        _ => Err(()),
                    };
                }
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return Err(()),
        }
    }
    Err(())
}

fn connect_to_pinned_destination(
    destinations: &[IpAddr],
    port: u16,
    deadline: Instant,
    cancelled: &AtomicBool,
    stopped: &AtomicBool,
) -> Result<TcpStream, ()> {
    for address in destinations {
        if proxy_should_stop(deadline, cancelled, stopped) {
            return Err(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(());
        }
        if let Ok(stream) = TcpStream::connect_timeout(&SocketAddr::new(*address, port), remaining)
        {
            return Ok(stream);
        }
    }
    Err(())
}

fn relay_pinned_tunnel(
    mut client: TcpStream,
    mut upstream: TcpStream,
    deadline: Instant,
    cancelled: &Arc<AtomicBool>,
    proxy_stopped: &Arc<AtomicBool>,
) {
    let tunnel_stopped = Arc::new(AtomicBool::new(false));
    let reader_stopped = Arc::clone(&tunnel_stopped);
    let reader_cancelled = Arc::clone(cancelled);
    let reader_proxy_stopped = Arc::clone(proxy_stopped);
    let mut client_writer = match client.try_clone() {
        Ok(stream) => stream,
        Err(_) => return,
    };
    let mut upstream_reader = match upstream.try_clone() {
        Ok(stream) => stream,
        Err(_) => return,
    };
    let reader = std::thread::spawn(move || {
        relay_pinned_direction(
            &mut upstream_reader,
            &mut client_writer,
            deadline,
            &reader_cancelled,
            &reader_proxy_stopped,
            &reader_stopped,
        );
    });
    relay_pinned_direction(
        &mut client,
        &mut upstream,
        deadline,
        cancelled,
        proxy_stopped,
        &tunnel_stopped,
    );
    tunnel_stopped.store(true, Ordering::Release);
    let _ = client.shutdown(Shutdown::Both);
    let _ = upstream.shutdown(Shutdown::Both);
    let _ = reader.join();
}

fn relay_pinned_direction(
    source: &mut TcpStream,
    destination: &mut TcpStream,
    deadline: Instant,
    cancelled: &AtomicBool,
    proxy_stopped: &AtomicBool,
    tunnel_stopped: &AtomicBool,
) {
    let mut buffer = [0_u8; 8 * 1024];
    while !tunnel_stopped.load(Ordering::Acquire)
        && !proxy_should_stop(deadline, cancelled, proxy_stopped)
    {
        match source.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) if destination.write_all(&buffer[..length]).is_err() => break,
            Ok(_) => {}
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
    }
    tunnel_stopped.store(true, Ordering::Release);
}

fn proxy_should_stop(deadline: Instant, cancelled: &AtomicBool, stopped: &AtomicBool) -> bool {
    cancelled.load(Ordering::Acquire)
        || stopped.load(Ordering::Acquire)
        || Instant::now() >= deadline
}

fn connect_authority(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
    .to_ascii_lowercase()
}

/// Decodes a previously fetched immutable Git reference without a working-tree checkout.
///
/// All Git errors are mapped to redacted error categories; callers must not log a repository URL
/// or blob body alongside the returned error.
pub fn catalog_fetch_result_from_repository(
    repository: &Repository,
    reference: &str,
    limits: PublicGitCatalogFetchLimits,
) -> Result<CatalogFetchResult, PublicGitCatalogFetchError> {
    let commit = repository
        .revparse_single(reference)
        .and_then(|object| object.peel_to_commit())
        .map_err(|_| PublicGitCatalogFetchError::CommitNotFound)?;
    let tree = commit
        .tree()
        .map_err(|_| PublicGitCatalogFetchError::RepositoryReadFailed)?;
    let mut documents = Vec::new();
    let mut rejected = None;
    let walk = tree.walk(TreeWalkMode::PreOrder, |root, entry| {
        if rejected.is_some() {
            return git2::TreeWalkResult::Abort;
        }
        let mode = entry.filemode();
        if matches!(mode, i if i == i32::from(FileMode::Link) || i == i32::from(FileMode::Commit)) {
            rejected = Some(PublicGitCatalogFetchError::UnsupportedGitEntry);
            return git2::TreeWalkResult::Abort;
        }
        if mode != i32::from(FileMode::Blob) && mode != i32::from(FileMode::BlobExecutable) {
            return git2::TreeWalkResult::Ok;
        }
        let name = match entry.name() {
            Ok(name) => name,
            Err(_) => {
                rejected = Some(PublicGitCatalogFetchError::UnsupportedGitEntry);
                return git2::TreeWalkResult::Abort;
            }
        };
        let path = format!("{root}{name}");
        if !(path.ends_with(".yaml") || path.ends_with(".yml")) {
            return git2::TreeWalkResult::Ok;
        }
        let blob = match repository.find_blob(entry.id()) {
            Ok(blob) => blob,
            Err(_) => {
                rejected = Some(PublicGitCatalogFetchError::RepositoryReadFailed);
                return git2::TreeWalkResult::Abort;
            }
        };
        let Ok(body) = std::str::from_utf8(blob.content()) else {
            rejected = Some(PublicGitCatalogFetchError::InvalidDocument);
            return git2::TreeWalkResult::Abort;
        };
        let kind = match catalog_document_kind(body) {
            Ok(Some(kind)) => kind,
            Ok(None) => return git2::TreeWalkResult::Ok,
            Err(error) => {
                rejected = Some(error);
                return git2::TreeWalkResult::Abort;
            }
        };
        if documents.len() >= limits.max_documents {
            rejected = Some(PublicGitCatalogFetchError::DocumentLimitExceeded);
            return git2::TreeWalkResult::Abort;
        }
        if blob.size() > limits.max_document_bytes {
            rejected = Some(PublicGitCatalogFetchError::DocumentTooLarge);
            return git2::TreeWalkResult::Abort;
        }
        documents.push(CatalogFetchedDocument {
            kind,
            path,
            checksum: format!("{:x}", Sha256::digest(body.as_bytes())),
            body: body.to_owned(),
        });
        git2::TreeWalkResult::Ok
    });
    if let Some(error) = rejected {
        return Err(error);
    }
    walk.map_err(|_| PublicGitCatalogFetchError::RepositoryReadFailed)?;
    Ok(CatalogFetchResult {
        commit: commit.id().to_string(),
        documents,
    })
}

fn catalog_document_kind(
    body: &str,
) -> Result<Option<CatalogDocumentKind>, PublicGitCatalogFetchError> {
    let kind = body
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("kind:").map(str::trim));
    match kind {
        Some("Policy") if parse_policy_document(body).is_ok() => {
            Ok(Some(CatalogDocumentKind::Policy))
        }
        Some("Runbook") if parse_runbook_document(body).is_ok() => {
            Ok(Some(CatalogDocumentKind::Runbook))
        }
        Some("Policy" | "Runbook") => Err(PublicGitCatalogFetchError::InvalidDocument),
        Some(_) | None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{Repository, Signature};
    use std::io::{Read, Write};
    use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    use std::time::{Duration, Instant};

    #[derive(Clone)]
    struct FixedDestinationResolver(Vec<IpAddr>);

    struct NeverCancelled;

    impl CatalogFetchCancellation for NeverCancelled {
        fn is_cancelled(&self) -> bool {
            false
        }
    }

    struct AlreadyCancelled;

    impl CatalogFetchCancellation for AlreadyCancelled {
        fn is_cancelled(&self) -> bool {
            true
        }
    }

    #[derive(Clone, Default)]
    struct SharedCancellation(Arc<AtomicBool>);

    impl CatalogFetchCancellation for SharedCancellation {
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::Acquire)
        }

        fn shared_signal(&self) -> Option<Arc<AtomicBool>> {
            Some(Arc::clone(&self.0))
        }
    }

    impl PublicCatalogDestinationResolver for FixedDestinationResolver {
        fn resolve_public_catalog_destination(
            &self,
            _host: &str,
            _port: u16,
        ) -> Result<Vec<IpAddr>, PublicCatalogDestinationResolutionError> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn decodes_a_validated_runbook_at_an_immutable_commit() {
        let directory = tempfile::tempdir().unwrap();
        let repository = Repository::init(directory.path()).unwrap();
        let body = b"apiVersion: fleet.sponzey.dev/v1alpha1\nkind: Runbook\nname: nginx\nselector: role=web\nsteps:\n  - id: nginx\n    service:\n      name: nginx\n      state: started\n";
        let blob = repository.blob(body).unwrap();
        let mut builder = repository.treebuilder(None).unwrap();
        builder
            .insert("nginx.yaml", blob, FileMode::Blob.into())
            .unwrap();
        let workflow = repository
            .blob(b"name: catalog checks\non:\n  push:\n")
            .unwrap();
        let mut github = repository.treebuilder(None).unwrap();
        github
            .insert("workflow.yml", workflow, FileMode::Blob.into())
            .unwrap();
        let github = repository.find_tree(github.write().unwrap()).unwrap();
        builder
            .insert(".github", github.id(), FileMode::Tree.into())
            .unwrap();
        let tree = repository.find_tree(builder.write().unwrap()).unwrap();
        let signature = Signature::now("Fleet test", "fleet@example.invalid").unwrap();
        let commit = repository
            .commit(Some("HEAD"), &signature, &signature, "catalog", &tree, &[])
            .unwrap();

        let result = catalog_fetch_result_from_repository(
            &repository,
            "HEAD",
            PublicGitCatalogFetchLimits::default(),
        )
        .unwrap();

        assert_eq!(result.commit, commit.to_string());
        assert_eq!(result.documents.len(), 1);
        assert_eq!(result.documents[0].kind, CatalogDocumentKind::Runbook);
        assert_eq!(result.documents[0].path, "nginx.yaml");
        assert_eq!(
            result.documents[0].checksum,
            format!("{:x}", Sha256::digest(body))
        );
    }

    #[test]
    fn public_git_fetcher_is_a_credential_free_catalog_fetcher() {
        fn assert_fetcher<F: CatalogFetcher<Error = PublicGitCatalogFetchError>>() {}

        assert_fetcher::<PublicGitCatalogFetcher>();
        assert!(PublicGitCatalogFetchLimits::new(0, 1).is_none());
        assert!(PublicGitCatalogFetchLimits::new(1, 0).is_none());
        assert!(PublicGitCatalogFetchLimits::new(1, 1).is_some());
    }

    #[test]
    fn public_git_fetcher_rejects_private_or_mixed_dns_destinations_before_network_fetch() {
        let staging = tempfile::tempdir().unwrap();
        let fetcher = PublicGitCatalogFetcher::with_destination_resolver(
            staging.path(),
            FixedDestinationResolver(vec![
                IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
                IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            ]),
            PublicGitCatalogFetchLimits::default(),
            DEFAULT_MAX_CATALOG_TRANSFER_BYTES,
        )
        .unwrap();
        let source = CatalogSource::new(
            fleet_domain::CatalogSourceId::new("public-catalog").unwrap(),
            fleet_domain::PublicCatalogUrl::new("https://example.com/catalog.git").unwrap(),
            fleet_domain::CatalogReference::new("main").unwrap(),
        );

        assert_eq!(
            fetcher.fetch_public_catalog(&source, &NeverCancelled),
            Err(PublicGitCatalogFetchError::DestinationRejected)
        );
    }

    #[test]
    fn public_destination_policy_rejects_reserved_and_private_address_ranges() {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "192.0.2.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "::1",
            "fe80::1",
            "fc00::1",
            "2001:db8::1",
        ] {
            assert!(
                !is_public_catalog_address(address.parse().unwrap()),
                "{address}"
            );
        }
        assert!(is_public_catalog_address("8.8.8.8".parse().unwrap()));
        assert!(is_public_catalog_address(
            "2606:4700:4700::1111".parse().unwrap()
        ));
    }

    #[test]
    fn public_git_fetcher_honors_cancellation_before_destination_or_network_work() {
        let staging = tempfile::tempdir().unwrap();
        let fetcher = PublicGitCatalogFetcher::with_destination_resolver(
            staging.path(),
            FixedDestinationResolver(vec![IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))]),
            PublicGitCatalogFetchLimits::default(),
            DEFAULT_MAX_CATALOG_TRANSFER_BYTES,
        )
        .unwrap();
        let source = CatalogSource::new(
            fleet_domain::CatalogSourceId::new("public-catalog").unwrap(),
            fleet_domain::PublicCatalogUrl::new("https://example.com/catalog.git").unwrap(),
            fleet_domain::CatalogReference::new("main").unwrap(),
        );

        assert_eq!(
            fetcher.fetch_public_catalog(&source, &AlreadyCancelled),
            Err(PublicGitCatalogFetchError::Cancelled)
        );
    }

    #[test]
    fn pinned_https_proxy_tunnels_only_to_the_resolved_destination() {
        let upstream_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let target_port = upstream_listener.local_addr().unwrap().port();
        let (payload_sender, payload_receiver) = mpsc::channel();
        let upstream = std::thread::spawn(move || {
            let (mut stream, _) = upstream_listener.accept().unwrap();
            let mut payload = [0_u8; 7];
            stream.read_exact(&mut payload).unwrap();
            payload_sender.send(payload).unwrap();
        });
        let proxy = PinnedHttpsProxy::start(
            "catalog.example",
            target_port,
            vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
            Instant::now() + Duration::from_secs(1),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        let mut client = TcpStream::connect(proxy.local_addr()).unwrap();
        client
            .write_all(
                format!(
                    "CONNECT catalog.example:{target_port} HTTP/1.1\r\nHost: catalog.example:{target_port}\r\n\r\n"
                )
                .as_bytes(),
            )
            .unwrap();
        assert!(read_proxy_response(&mut client).starts_with("HTTP/1.1 200"));
        client.write_all(b"payload").unwrap();
        assert_eq!(
            payload_receiver
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            *b"payload"
        );
        drop(client);
        drop(proxy);
        upstream.join().unwrap();
    }

    #[test]
    fn pinned_https_proxy_refuses_a_different_connect_authority() {
        let upstream_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        upstream_listener.set_nonblocking(true).unwrap();
        let target_port = upstream_listener.local_addr().unwrap().port();
        let proxy = PinnedHttpsProxy::start(
            "catalog.example",
            target_port,
            vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
            Instant::now() + Duration::from_secs(1),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        let mut client = TcpStream::connect(proxy.local_addr()).unwrap();
        client
            .write_all(b"CONNECT metadata.example:443 HTTP/1.1\r\nHost: metadata.example\r\n\r\n")
            .unwrap();
        assert!(read_proxy_response(&mut client).starts_with("HTTP/1.1 403"));
        drop(client);
        drop(proxy);
        assert!(
            matches!(upstream_listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    }

    #[test]
    fn pinned_https_proxy_closes_a_stalled_tunnel_at_the_hard_deadline() {
        let upstream_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let target_port = upstream_listener.local_addr().unwrap().port();
        let upstream = std::thread::spawn(move || {
            let (_stream, _) = upstream_listener.accept().unwrap();
            std::thread::sleep(Duration::from_secs(3));
        });
        let proxy = PinnedHttpsProxy::start(
            "catalog.example",
            target_port,
            vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
            Instant::now() + Duration::from_secs(2),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        let mut client = TcpStream::connect(proxy.local_addr()).unwrap();
        client
            .write_all(
                format!("CONNECT catalog.example:{target_port} HTTP/1.1\r\nHost: catalog.example\r\n\r\n")
                    .as_bytes(),
            )
            .unwrap();
        assert!(read_proxy_response(&mut client).starts_with("HTTP/1.1 200"));
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut byte = [0_u8; 1];
        assert_eq!(client.read(&mut byte).unwrap(), 0);
        drop(client);
        drop(proxy);
        upstream.join().unwrap();
    }

    #[test]
    fn pinned_https_proxy_closes_a_stalled_tunnel_when_cancelled() {
        let upstream_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let target_port = upstream_listener.local_addr().unwrap().port();
        let upstream = std::thread::spawn(move || {
            let (_stream, _) = upstream_listener.accept().unwrap();
            std::thread::sleep(Duration::from_secs(2));
        });
        let cancellation = Arc::new(AtomicBool::new(false));
        let proxy = PinnedHttpsProxy::start(
            "catalog.example",
            target_port,
            vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
            Instant::now() + Duration::from_secs(5),
            Arc::clone(&cancellation),
        )
        .unwrap();
        let mut client = TcpStream::connect(proxy.local_addr()).unwrap();
        client
            .write_all(
                format!("CONNECT catalog.example:{target_port} HTTP/1.1\r\nHost: catalog.example\r\n\r\n")
                    .as_bytes(),
            )
            .unwrap();
        assert!(read_proxy_response(&mut client).starts_with("HTTP/1.1 200"));
        cancellation.store(true, Ordering::Release);
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut byte = [0_u8; 1];
        assert_eq!(client.read(&mut byte).unwrap(), 0);
        drop(client);
        drop(proxy);
        upstream.join().unwrap();
    }

    #[test]
    fn libgit2_fetch_reaches_only_the_pinned_proxy_destination() {
        let upstream_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let target_port = upstream_listener.local_addr().unwrap().port();
        let (connection_sender, connection_receiver) = mpsc::channel();
        let upstream = std::thread::spawn(move || {
            let (mut stream, _) = upstream_listener.accept().unwrap();
            let mut first_byte = [0_u8; 1];
            stream.read_exact(&mut first_byte).unwrap();
            connection_sender.send(()).unwrap();
        });
        let staging = tempfile::tempdir().unwrap();
        let cancellation = SharedCancellation::default();

        assert!(matches!(
            clone_public_repository(
                &format!("https://catalog.example:{target_port}/catalog.git"),
                "catalog.example",
                target_port,
                &[IpAddr::V4(Ipv4Addr::LOCALHOST)],
                staging.path(),
                DEFAULT_MAX_CATALOG_TRANSFER_BYTES,
                &cancellation,
            ),
            Err(PublicGitCatalogFetchError::RepositoryReadFailed)
        ));
        connection_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert!(!cancellation.is_cancelled());
        upstream.join().unwrap();
    }

    #[test]
    #[ignore = "requires FLEET_TEST_PUBLIC_CATALOG_URL and public network access"]
    fn public_git_fetcher_syncs_a_public_catalog_from_test_bootstrap() {
        let url = std::env::var("FLEET_TEST_PUBLIC_CATALOG_URL")
            .expect("test bootstrap must provide FLEET_TEST_PUBLIC_CATALOG_URL");
        let source = CatalogSource::new(
            fleet_domain::CatalogSourceId::new("external-smoke").unwrap(),
            fleet_domain::PublicCatalogUrl::new(url).unwrap(),
            fleet_domain::CatalogReference::new("main").unwrap(),
        );
        let staging = tempfile::tempdir().unwrap();
        let result = PublicGitCatalogFetcher::new(staging.path())
            .fetch_public_catalog(&source, &SharedCancellation::default())
            .unwrap();

        assert!(!result.commit.is_empty());
        assert!(!result.documents.is_empty());
    }

    fn read_proxy_response(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut response = Vec::new();
        loop {
            let mut byte = [0_u8; 1];
            stream.read_exact(&mut byte).unwrap();
            response.push(byte[0]);
            if response.ends_with(b"\r\n\r\n") {
                return String::from_utf8(response).unwrap();
            }
        }
    }
}
