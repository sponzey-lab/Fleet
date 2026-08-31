//! Pure catalog identities, revision and sync-operation state, and activation guards.
//!
//! The catalog boundary records only validated public-source metadata and immutable
//! commit provenance. Fetching, persistence, and HTTP mapping belong to outer layers.

use std::fmt::{Display, Formatter};
use std::net::IpAddr;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CatalogSourceId(String);

impl CatalogSourceId {
    pub fn new(value: impl Into<String>) -> Result<Self, CatalogError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || value.trim() != value
            || !value.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
        {
            return Err(CatalogError::InvalidSourceId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for CatalogSourceId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicCatalogUrl {
    value: String,
    host: String,
    port: u16,
}

impl PublicCatalogUrl {
    /// Accepts only a normalized HTTPS URL without embedded credentials.
    ///
    /// Obvious non-public IP literals are rejected here; DNS and redirect destination
    /// policy remain infrastructure checks.
    pub fn new(value: impl Into<String>) -> Result<Self, CatalogError> {
        let value = value.into();
        let Some(authority_and_path) = value.strip_prefix("https://") else {
            return Err(CatalogError::InvalidPublicUrl);
        };
        let authority = authority_and_path
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default();
        if value.trim() != value
            || authority.is_empty()
            || authority.contains('@')
            || authority.chars().any(char::is_whitespace)
        {
            return Err(CatalogError::InvalidPublicUrl);
        }
        let (host, port) = parse_public_catalog_authority(authority)?;
        if is_obviously_non_public_host(&host) {
            return Err(CatalogError::InvalidPublicUrl);
        }
        Ok(Self { value, host, port })
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }

    /// Returns the normalized authority hostname for infrastructure destination checks.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Returns the validated HTTPS authority port, defaulting to 443 when omitted.
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Display for PublicCatalogUrl {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.value)
    }
}

fn parse_public_catalog_authority(authority: &str) -> Result<(String, u16), CatalogError> {
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let Some((host, suffix)) = bracketed.split_once(']') else {
            return Err(CatalogError::InvalidPublicUrl);
        };
        let port = match suffix {
            "" => 443,
            suffix if suffix.starts_with(':') => parse_catalog_port(&suffix[1..])?,
            _ => return Err(CatalogError::InvalidPublicUrl),
        };
        if host.is_empty() || host.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(CatalogError::InvalidPublicUrl);
        }
        (host.to_ascii_lowercase(), port)
    } else {
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (host, parse_catalog_port(port)?),
            Some(_) => return Err(CatalogError::InvalidPublicUrl),
            None => (authority, 443),
        };
        if !is_valid_catalog_hostname(host) {
            return Err(CatalogError::InvalidPublicUrl);
        }
        (host.to_ascii_lowercase(), port)
    };
    Ok((host, port))
}

fn parse_catalog_port(value: &str) -> Result<u16, CatalogError> {
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or(CatalogError::InvalidPublicUrl)
}

fn is_valid_catalog_hostname(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
}

fn is_obviously_non_public_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let Ok(address) = host.parse::<IpAddr>() else {
        return false;
    };
    match address {
        IpAddr::V4(address) => {
            address.is_loopback()
                || address.is_private()
                || address.is_link_local()
                || address.is_unspecified()
        }
        IpAddr::V6(address) => {
            address.is_loopback()
                || address.is_unspecified()
                || address.is_unicast_link_local()
                || ((address.segments()[0] & 0xfe00) == 0xfc00)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogReference(String);

impl CatalogReference {
    pub fn new(value: impl Into<String>) -> Result<Self, CatalogError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 255
            || value.trim() != value
            || value.chars().any(char::is_control)
        {
            return Err(CatalogError::InvalidReference);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CatalogCommitId(String);

impl CatalogCommitId {
    pub fn new(value: impl Into<String>) -> Result<Self, CatalogError> {
        let value = value.into();
        if !matches!(value.len(), 40 | 64)
            || !value
                .chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
        {
            return Err(CatalogError::InvalidCommitId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for CatalogCommitId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogSource {
    id: CatalogSourceId,
    url: PublicCatalogUrl,
    reference: CatalogReference,
    active_revision: Option<CatalogCommitId>,
}

impl CatalogSource {
    pub fn new(id: CatalogSourceId, url: PublicCatalogUrl, reference: CatalogReference) -> Self {
        Self {
            id,
            url,
            reference,
            active_revision: None,
        }
    }

    pub fn id(&self) -> &CatalogSourceId {
        &self.id
    }

    pub fn url(&self) -> &PublicCatalogUrl {
        &self.url
    }

    pub fn reference(&self) -> &CatalogReference {
        &self.reference
    }

    pub fn active_revision(&self) -> Option<&CatalogCommitId> {
        self.active_revision.as_ref()
    }

    pub fn begin_sync(&self, commit: CatalogCommitId) -> CatalogRevision {
        CatalogRevision {
            source_id: self.id.clone(),
            commit,
            state: CatalogRevisionState::Fetching,
        }
    }

    /// Changes the active pointer only for a validated immutable revision from this source.
    pub fn activate(&mut self, revision: &CatalogRevision) -> Result<(), CatalogError> {
        if revision.source_id != self.id {
            return Err(CatalogError::RevisionSourceMismatch);
        }
        if !matches!(revision.state, CatalogRevisionState::Ready { .. }) {
            return Err(CatalogError::RevisionNotReady);
        }
        self.active_revision = Some(revision.commit.clone());
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogRevision {
    source_id: CatalogSourceId,
    commit: CatalogCommitId,
    state: CatalogRevisionState,
}

impl CatalogRevision {
    /// Rehydrates a revision snapshot after a persistence adapter has decoded its stored state.
    ///
    /// Persistence adapters must preserve the source/commit pair and cannot use this constructor
    /// to manufacture an empty ready revision.
    pub fn restore(
        source_id: CatalogSourceId,
        commit: CatalogCommitId,
        state: CatalogRevisionState,
    ) -> Result<Self, CatalogError> {
        if matches!(state, CatalogRevisionState::Ready { document_count: 0 }) {
            return Err(CatalogError::EmptyRevision);
        }
        Ok(Self {
            source_id,
            commit,
            state,
        })
    }

    pub fn source_id(&self) -> &CatalogSourceId {
        &self.source_id
    }

    pub fn commit(&self) -> &CatalogCommitId {
        &self.commit
    }

    pub fn state(&self) -> CatalogRevisionState {
        self.state
    }

    pub fn begin_validation(&mut self) -> Result<(), CatalogError> {
        if self.state != CatalogRevisionState::Fetching {
            return Err(CatalogError::InvalidRevisionTransition);
        }
        self.state = CatalogRevisionState::Validating;
        Ok(())
    }

    pub fn mark_ready(&mut self, document_count: usize) -> Result<(), CatalogError> {
        if self.state != CatalogRevisionState::Validating {
            return Err(CatalogError::InvalidRevisionTransition);
        }
        if document_count == 0 {
            return Err(CatalogError::EmptyRevision);
        }
        self.state = CatalogRevisionState::Ready { document_count };
        Ok(())
    }

    pub fn fail(&mut self, failure: CatalogSyncFailure) -> Result<(), CatalogError> {
        if !matches!(
            self.state,
            CatalogRevisionState::Fetching | CatalogRevisionState::Validating
        ) {
            return Err(CatalogError::InvalidRevisionTransition);
        }
        self.state = CatalogRevisionState::Failed(failure);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogRevisionState {
    Fetching,
    Validating,
    Ready { document_count: usize },
    Failed(CatalogSyncFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogSyncFailure {
    FetchRejected,
    FetchFailed,
    ValidationFailed,
    LimitExceeded,
    Cancelled,
}

/// Identifies one durable source-sync request without encoding a repository URL or credential.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CatalogSyncOperationId(String);

impl CatalogSyncOperationId {
    pub fn new(value: impl Into<String>) -> Result<Self, CatalogError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || value.trim() != value
            || !value.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
        {
            return Err(CatalogError::InvalidSyncOperationId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for CatalogSyncOperationId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Tracks one source synchronization independently from the immutable revision it discovers.
///
/// The operation may bind one commit once fetching resolves its requested reference. A terminal
/// operation cannot be reused, so retries and later sync requests receive a new operation id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogSyncOperation {
    id: CatalogSyncOperationId,
    source_id: CatalogSourceId,
    commit: Option<CatalogCommitId>,
    state: CatalogSyncOperationState,
}

impl CatalogSyncOperation {
    pub fn new(id: CatalogSyncOperationId, source_id: CatalogSourceId) -> Self {
        Self {
            id,
            source_id,
            commit: None,
            state: CatalogSyncOperationState::InProgress,
        }
    }

    /// Rehydrates a persisted operation after its source, commit, and terminal state are decoded.
    pub fn restore(
        id: CatalogSyncOperationId,
        source_id: CatalogSourceId,
        commit: Option<CatalogCommitId>,
        state: CatalogSyncOperationState,
    ) -> Result<Self, CatalogError> {
        if state == CatalogSyncOperationState::Completed && commit.is_none() {
            return Err(CatalogError::SyncOperationCommitRequired);
        }
        Ok(Self {
            id,
            source_id,
            commit,
            state,
        })
    }

    pub fn id(&self) -> &CatalogSyncOperationId {
        &self.id
    }

    pub fn source_id(&self) -> &CatalogSourceId {
        &self.source_id
    }

    pub fn commit(&self) -> Option<&CatalogCommitId> {
        self.commit.as_ref()
    }

    pub fn state(&self) -> CatalogSyncOperationState {
        self.state
    }

    pub fn is_active(&self) -> bool {
        self.state == CatalogSyncOperationState::InProgress
    }

    /// Binds the commit resolved for this request; repeated binding of the same commit is safe.
    pub fn bind_commit(&mut self, commit: CatalogCommitId) -> Result<(), CatalogError> {
        if !self.is_active() {
            return Err(CatalogError::InvalidSyncOperationTransition);
        }
        match &self.commit {
            None => {
                self.commit = Some(commit);
                Ok(())
            }
            Some(existing) if existing == &commit => Ok(()),
            Some(_) => Err(CatalogError::SyncOperationCommitAlreadyBound),
        }
    }

    /// Completes only the ready revision that belongs to the operation's source and commit.
    pub fn complete(&mut self, revision: &CatalogRevision) -> Result<(), CatalogError> {
        if !self.is_active() {
            return Err(CatalogError::InvalidSyncOperationTransition);
        }
        if revision.source_id != self.source_id || self.commit.as_ref() != Some(&revision.commit) {
            return Err(CatalogError::SyncOperationRevisionMismatch);
        }
        if !matches!(revision.state, CatalogRevisionState::Ready { .. }) {
            return Err(CatalogError::RevisionNotReady);
        }
        self.state = CatalogSyncOperationState::Completed;
        Ok(())
    }

    pub fn fail(&mut self, failure: CatalogSyncFailure) -> Result<(), CatalogError> {
        if !self.is_active() || failure == CatalogSyncFailure::Cancelled {
            return Err(CatalogError::InvalidSyncOperationTransition);
        }
        self.state = CatalogSyncOperationState::Failed(failure);
        Ok(())
    }

    pub fn cancel(&mut self) -> Result<(), CatalogError> {
        if !self.is_active() {
            return Err(CatalogError::InvalidSyncOperationTransition);
        }
        self.state = CatalogSyncOperationState::Cancelled;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogSyncOperationState {
    InProgress,
    Completed,
    Failed(CatalogSyncFailure),
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogDocumentKind {
    Policy,
    Runbook,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogDocument {
    source_id: CatalogSourceId,
    commit: CatalogCommitId,
    kind: CatalogDocumentKind,
    path: String,
    checksum: String,
}

impl CatalogDocument {
    pub fn new(
        source_id: CatalogSourceId,
        commit: CatalogCommitId,
        kind: CatalogDocumentKind,
        path: impl Into<String>,
        checksum: impl Into<String>,
    ) -> Result<Self, CatalogError> {
        let path = path.into();
        let checksum = checksum.into();
        if path.is_empty()
            || path.starts_with('/')
            || path.contains('\\')
            || path
                .split('/')
                .any(|part| matches!(part, "" | "." | ".." | ".git"))
            || !(path.ends_with(".yaml") || path.ends_with(".yml"))
        {
            return Err(CatalogError::InvalidDocumentPath);
        }
        if checksum.len() != 64
            || !checksum
                .chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
        {
            return Err(CatalogError::InvalidChecksum);
        }
        Ok(Self {
            source_id,
            commit,
            kind,
            path,
            checksum,
        })
    }

    pub fn source_id(&self) -> &CatalogSourceId {
        &self.source_id
    }

    pub fn commit(&self) -> &CatalogCommitId {
        &self.commit
    }

    pub fn kind(&self) -> CatalogDocumentKind {
        self.kind
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn checksum(&self) -> &str {
        &self.checksum
    }
}

/// Immutable catalog identity attached to a Runbook Job snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogRunbookProvenance {
    source_id: CatalogSourceId,
    commit: CatalogCommitId,
    path: String,
    checksum: String,
}

impl CatalogRunbookProvenance {
    pub fn from_document(document: &CatalogDocument) -> Result<Self, CatalogError> {
        if document.kind() != CatalogDocumentKind::Runbook {
            return Err(CatalogError::CatalogDocumentIsNotRunbook);
        }
        Ok(Self {
            source_id: document.source_id().clone(),
            commit: document.commit().clone(),
            path: document.path().to_owned(),
            checksum: document.checksum().to_owned(),
        })
    }
    pub fn source_id(&self) -> &CatalogSourceId {
        &self.source_id
    }
    pub fn commit(&self) -> &CatalogCommitId {
        &self.commit
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn checksum(&self) -> &str {
        &self.checksum
    }
}

/// Immutable catalog identity attached to a published Policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogPolicyProvenance {
    source_id: CatalogSourceId,
    commit: CatalogCommitId,
    path: String,
    checksum: String,
}

impl CatalogPolicyProvenance {
    pub fn from_document(document: &CatalogDocument) -> Result<Self, CatalogError> {
        if document.kind() != CatalogDocumentKind::Policy {
            return Err(CatalogError::CatalogDocumentIsNotPolicy);
        }
        Ok(Self {
            source_id: document.source_id().clone(),
            commit: document.commit().clone(),
            path: document.path().to_owned(),
            checksum: document.checksum().to_owned(),
        })
    }

    pub fn source_id(&self) -> &CatalogSourceId {
        &self.source_id
    }

    pub fn commit(&self) -> &CatalogCommitId {
        &self.commit
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn checksum(&self) -> &str {
        &self.checksum
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogError {
    InvalidSourceId,
    InvalidPublicUrl,
    InvalidReference,
    InvalidCommitId,
    InvalidSyncOperationId,
    InvalidDocumentPath,
    InvalidChecksum,
    CatalogDocumentIsNotRunbook,
    CatalogDocumentIsNotPolicy,
    InvalidRevisionTransition,
    EmptyRevision,
    RevisionNotReady,
    RevisionSourceMismatch,
    InvalidSyncOperationTransition,
    SyncOperationCommitAlreadyBound,
    SyncOperationRevisionMismatch,
    SyncOperationCommitRequired,
}

impl Display for CatalogError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::InvalidSourceId => "catalog source id is invalid",
            Self::InvalidPublicUrl => {
                "catalog source must be a public HTTPS URL without credentials"
            }
            Self::InvalidReference => "catalog reference is invalid",
            Self::InvalidCommitId => "catalog commit id is invalid",
            Self::InvalidSyncOperationId => "catalog sync operation id is invalid",
            Self::InvalidDocumentPath => "catalog document path is invalid",
            Self::InvalidChecksum => "catalog document checksum is invalid",
            Self::CatalogDocumentIsNotRunbook => "catalog document is not a Runbook",
            Self::CatalogDocumentIsNotPolicy => "catalog document is not a Policy",
            Self::InvalidRevisionTransition => "catalog revision transition is invalid",
            Self::EmptyRevision => "catalog revision contains no documents",
            Self::RevisionNotReady => "catalog revision is not ready for activation",
            Self::RevisionSourceMismatch => "catalog revision belongs to a different source",
            Self::InvalidSyncOperationTransition => "catalog sync operation transition is invalid",
            Self::SyncOperationCommitAlreadyBound => {
                "catalog sync operation already belongs to a different commit"
            }
            Self::SyncOperationRevisionMismatch => {
                "catalog revision does not match the sync operation source and commit"
            }
            Self::SyncOperationCommitRequired => {
                "completed catalog sync operation requires a commit"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for CatalogError {}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMIT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const COMMIT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn source(id: &str) -> CatalogSource {
        CatalogSource::new(
            CatalogSourceId::new(id).unwrap(),
            PublicCatalogUrl::new("https://example.com/fleet/catalog.git").unwrap(),
            CatalogReference::new("main").unwrap(),
        )
    }

    #[test]
    fn public_catalog_url_rejects_non_https_credentials_and_localhost() {
        for value in [
            "http://example.com/catalog.git",
            "https://token@example.com/catalog.git",
            "https://localhost/catalog.git",
            "https://127.0.0.1/catalog.git",
            "https://169.254.169.254/catalog.git",
            "https://[::1]/catalog.git",
            " file:///catalog.git",
        ] {
            assert_eq!(
                PublicCatalogUrl::new(value),
                Err(CatalogError::InvalidPublicUrl)
            );
        }
    }

    #[test]
    fn public_catalog_url_exposes_a_validated_authority_for_destination_policy() {
        let url =
            PublicCatalogUrl::new("https://Catalog.Example.com:8443/fleet/catalog.git?ref=main")
                .unwrap();

        assert_eq!(url.host(), "catalog.example.com");
        assert_eq!(url.port(), 8443);
        for value in [
            "https://example.com:not-a-port/catalog.git",
            "https://example.com:0/catalog.git",
            "https://example..com/catalog.git",
            "https://[2001:db8::1/catalog.git",
        ] {
            assert_eq!(
                PublicCatalogUrl::new(value),
                Err(CatalogError::InvalidPublicUrl)
            );
        }
    }

    #[test]
    fn revision_requires_validation_before_activation() {
        let mut source = source("public-catalog");
        let mut revision = source.begin_sync(CatalogCommitId::new(COMMIT_A).unwrap());

        assert_eq!(
            source.activate(&revision),
            Err(CatalogError::RevisionNotReady)
        );
        revision.begin_validation().unwrap();
        revision.mark_ready(2).unwrap();
        source.activate(&revision).unwrap();

        assert_eq!(source.active_revision(), Some(revision.commit()));
    }

    #[test]
    fn failed_revision_preserves_previously_active_revision() {
        let mut source = source("public-catalog");
        let mut ready = source.begin_sync(CatalogCommitId::new(COMMIT_A).unwrap());
        ready.begin_validation().unwrap();
        ready.mark_ready(1).unwrap();
        source.activate(&ready).unwrap();

        let mut failed = source.begin_sync(CatalogCommitId::new(COMMIT_B).unwrap());
        failed.fail(CatalogSyncFailure::FetchFailed).unwrap();

        assert_eq!(
            source.activate(&failed),
            Err(CatalogError::RevisionNotReady)
        );
        assert_eq!(source.active_revision(), Some(ready.commit()));
    }

    #[test]
    fn source_rejects_activation_of_another_sources_revision() {
        let source_a = source("catalog-a");
        let mut revision = source_a.begin_sync(CatalogCommitId::new(COMMIT_A).unwrap());
        revision.begin_validation().unwrap();
        revision.mark_ready(1).unwrap();
        let mut source_b = source("catalog-b");

        assert_eq!(
            source_b.activate(&revision),
            Err(CatalogError::RevisionSourceMismatch)
        );
    }

    #[test]
    fn document_rejects_unsafe_path_and_accepts_immutable_provenance() {
        let source = CatalogSourceId::new("public-catalog").unwrap();
        let commit = CatalogCommitId::new(COMMIT_A).unwrap();
        let checksum = "c".repeat(64);

        let document = CatalogDocument::new(
            source.clone(),
            commit.clone(),
            CatalogDocumentKind::Runbook,
            "runbooks/nginx.yaml",
            checksum,
        )
        .unwrap();
        assert_eq!(document.source_id(), &source);
        assert_eq!(document.commit(), &commit);
        assert_eq!(document.path(), "runbooks/nginx.yaml");
        assert_eq!(
            CatalogDocument::new(
                source,
                commit,
                CatalogDocumentKind::Policy,
                "../policies/nginx.yaml",
                "d".repeat(64),
            ),
            Err(CatalogError::InvalidDocumentPath)
        );
    }

    #[test]
    fn runbook_provenance_rejects_policy_documents_and_keeps_immutable_identity() {
        let source = CatalogSourceId::new("public-catalog").unwrap();
        let commit = CatalogCommitId::new(COMMIT_A).unwrap();
        let runbook = CatalogDocument::new(
            source.clone(),
            commit.clone(),
            CatalogDocumentKind::Runbook,
            "runbooks/nginx.yaml",
            "c".repeat(64),
        )
        .unwrap();
        let provenance = CatalogRunbookProvenance::from_document(&runbook).unwrap();
        assert_eq!(provenance.source_id(), &source);
        assert_eq!(provenance.commit(), &commit);
        assert_eq!(provenance.path(), "runbooks/nginx.yaml");
        assert_eq!(
            provenance.checksum(),
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        );
        let policy = CatalogDocument::new(
            source,
            commit,
            CatalogDocumentKind::Policy,
            "policies/nginx.yaml",
            "d".repeat(64),
        )
        .unwrap();
        assert_eq!(
            CatalogRunbookProvenance::from_document(&policy),
            Err(CatalogError::CatalogDocumentIsNotRunbook)
        );
        let policy_provenance = CatalogPolicyProvenance::from_document(&policy).unwrap();
        assert_eq!(policy_provenance.path(), "policies/nginx.yaml");
        assert_eq!(
            CatalogPolicyProvenance::from_document(&runbook),
            Err(CatalogError::CatalogDocumentIsNotPolicy)
        );
    }

    #[test]
    fn sync_operation_binds_one_immutable_commit_and_completes_only_ready_revision() {
        let source = source("public-catalog");
        let commit_a = CatalogCommitId::new(COMMIT_A).unwrap();
        let commit_b = CatalogCommitId::new(COMMIT_B).unwrap();
        let mut operation = CatalogSyncOperation::new(
            CatalogSyncOperationId::new("sync-20260830-001").unwrap(),
            source.id().clone(),
        );

        assert!(operation.is_active());
        operation.bind_commit(commit_a.clone()).unwrap();
        assert_eq!(operation.commit(), Some(&commit_a));
        assert_eq!(
            operation.bind_commit(commit_b),
            Err(CatalogError::SyncOperationCommitAlreadyBound)
        );

        let mut revision = source.begin_sync(commit_a);
        revision.begin_validation().unwrap();
        revision.mark_ready(2).unwrap();
        operation.complete(&revision).unwrap();

        assert_eq!(operation.state(), CatalogSyncOperationState::Completed);
        assert!(!operation.is_active());
        assert_eq!(
            operation.fail(CatalogSyncFailure::FetchFailed),
            Err(CatalogError::InvalidSyncOperationTransition)
        );
    }

    #[test]
    fn cancelled_operation_cannot_change_the_source_active_revision() {
        let mut source = source("public-catalog");
        let mut active = source.begin_sync(CatalogCommitId::new(COMMIT_A).unwrap());
        active.begin_validation().unwrap();
        active.mark_ready(1).unwrap();
        source.activate(&active).unwrap();

        let commit = CatalogCommitId::new(COMMIT_B).unwrap();
        let mut operation = CatalogSyncOperation::new(
            CatalogSyncOperationId::new("sync-20260830-002").unwrap(),
            source.id().clone(),
        );
        operation.bind_commit(commit.clone()).unwrap();
        operation.cancel().unwrap();

        assert_eq!(operation.state(), CatalogSyncOperationState::Cancelled);
        assert_eq!(source.active_revision(), Some(active.commit()));
        assert_eq!(
            operation.bind_commit(commit),
            Err(CatalogError::InvalidSyncOperationTransition)
        );
    }

    #[test]
    fn revision_restore_rejects_an_empty_persisted_ready_state() {
        assert_eq!(
            CatalogRevision::restore(
                CatalogSourceId::new("public-catalog").unwrap(),
                CatalogCommitId::new(COMMIT_A).unwrap(),
                CatalogRevisionState::Ready { document_count: 0 },
            ),
            Err(CatalogError::EmptyRevision)
        );
    }

    #[test]
    fn sync_operation_restore_rejects_a_completed_operation_without_commit() {
        assert_eq!(
            CatalogSyncOperation::restore(
                CatalogSyncOperationId::new("sync-001").unwrap(),
                CatalogSourceId::new("public-catalog").unwrap(),
                None,
                CatalogSyncOperationState::Completed,
            ),
            Err(CatalogError::SyncOperationCommitRequired)
        );
    }
}
