use std::fmt::{Display, Formatter};
use std::time::{Duration, SystemTime};

use crate::AgentId;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentCertificateFingerprint(String);

impl AgentCertificateFingerprint {
    pub fn new(value: impl Into<String>) -> Result<Self, AgentCertificateLifecycleError> {
        let value = value.into();
        if value.trim().is_empty()
            || value.len() < 16
            || value.contains('\n')
            || value.contains('\r')
            || !value.chars().all(|c| c.is_ascii_hexdigit() || c == ':')
        {
            return Err(AgentCertificateLifecycleError::InvalidFingerprint);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentCertificateSerial(String);

impl AgentCertificateSerial {
    pub fn new(value: impl Into<String>) -> Result<Self, AgentCertificateLifecycleError> {
        let value = value.into();
        if value.trim().is_empty()
            || value.len() > 128
            || value.contains('\n')
            || value.contains('\r')
            || !value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
        {
            return Err(AgentCertificateLifecycleError::InvalidSerial);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCertificateValidity {
    not_before: SystemTime,
    not_after: SystemTime,
}

impl AgentCertificateValidity {
    pub fn new(
        not_before: SystemTime,
        not_after: SystemTime,
    ) -> Result<Self, AgentCertificateLifecycleError> {
        if not_after <= not_before {
            return Err(AgentCertificateLifecycleError::InvalidValidityWindow);
        }
        Ok(Self {
            not_before,
            not_after,
        })
    }

    pub fn not_before(&self) -> SystemTime {
        self.not_before
    }

    pub fn not_after(&self) -> SystemTime {
        self.not_after
    }

    pub fn is_valid_at(&self, at: SystemTime) -> bool {
        at >= self.not_before && at <= self.not_after
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCertificate {
    serial: AgentCertificateSerial,
    fingerprint: AgentCertificateFingerprint,
    validity: AgentCertificateValidity,
}

impl AgentCertificate {
    pub fn new(
        serial: AgentCertificateSerial,
        fingerprint: AgentCertificateFingerprint,
        validity: AgentCertificateValidity,
    ) -> Result<Self, AgentCertificateLifecycleError> {
        Ok(Self {
            serial,
            fingerprint,
            validity,
        })
    }

    pub fn serial(&self) -> &AgentCertificateSerial {
        &self.serial
    }

    pub fn fingerprint(&self) -> &AgentCertificateFingerprint {
        &self.fingerprint
    }

    pub fn validity(&self) -> &AgentCertificateValidity {
        &self.validity
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentCertificateRenewalPolicy {
    renew_before_expiry: Duration,
    rotation_grace_period: Duration,
}

impl AgentCertificateRenewalPolicy {
    pub fn new(
        renew_before_expiry: Duration,
        rotation_grace_period: Duration,
    ) -> Result<Self, AgentCertificateLifecycleError> {
        if renew_before_expiry.is_zero() || rotation_grace_period.is_zero() {
            return Err(AgentCertificateLifecycleError::InvalidRenewalPolicy);
        }
        Ok(Self {
            renew_before_expiry,
            rotation_grace_period,
        })
    }

    pub fn renew_before_expiry(&self) -> Duration {
        self.renew_before_expiry
    }

    pub fn rotation_grace_period(&self) -> Duration {
        self.rotation_grace_period
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCertificateLifecycleState {
    NotIssued,
    IssuanceRequested,
    Issued,
    RenewalRequested,
    DualCertificateActive,
    Revoked,
    Expired,
    Failed,
}

impl AgentCertificateLifecycleState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotIssued => "not_issued",
            Self::IssuanceRequested => "issuance_requested",
            Self::Issued => "issued",
            Self::RenewalRequested => "renewal_requested",
            Self::DualCertificateActive => "dual_certificate_active",
            Self::Revoked => "revoked",
            Self::Expired => "expired",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "not_issued" => Some(Self::NotIssued),
            "issuance_requested" => Some(Self::IssuanceRequested),
            "issued" => Some(Self::Issued),
            "renewal_requested" => Some(Self::RenewalRequested),
            "dual_certificate_active" => Some(Self::DualCertificateActive),
            "revoked" => Some(Self::Revoked),
            "expired" => Some(Self::Expired),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Revoked | Self::Expired | Self::Failed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCertificateRevocationReason {
    OperatorRevoked,
    AgentDisabled,
    CertificateCompromised,
}

impl AgentCertificateRevocationReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OperatorRevoked => "operator_revoked",
            Self::AgentDisabled => "agent_disabled",
            Self::CertificateCompromised => "certificate_compromised",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "operator_revoked" => Some(Self::OperatorRevoked),
            "agent_disabled" => Some(Self::AgentDisabled),
            "certificate_compromised" => Some(Self::CertificateCompromised),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCertificateLifecycle {
    agent_id: AgentId,
    state: AgentCertificateLifecycleState,
    current_certificate: Option<AgentCertificate>,
    next_certificate: Option<AgentCertificate>,
    grace_until: Option<SystemTime>,
    revocation_reason: Option<AgentCertificateRevocationReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCertificateLifecycleSnapshot {
    pub agent_id: AgentId,
    pub state: AgentCertificateLifecycleState,
    pub current_certificate: Option<AgentCertificate>,
    pub next_certificate: Option<AgentCertificate>,
    pub grace_until: Option<SystemTime>,
    pub revocation_reason: Option<AgentCertificateRevocationReason>,
}

impl AgentCertificateLifecycle {
    pub fn new(agent_id: AgentId) -> Self {
        Self {
            agent_id,
            state: AgentCertificateLifecycleState::NotIssued,
            current_certificate: None,
            next_certificate: None,
            grace_until: None,
            revocation_reason: None,
        }
    }

    pub fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }

    pub fn state(&self) -> AgentCertificateLifecycleState {
        self.state
    }

    pub fn current_certificate(&self) -> Option<&AgentCertificate> {
        self.current_certificate.as_ref()
    }

    pub fn next_certificate(&self) -> Option<&AgentCertificate> {
        self.next_certificate.as_ref()
    }

    pub fn grace_until(&self) -> Option<SystemTime> {
        self.grace_until
    }

    pub fn revocation_reason(&self) -> Option<AgentCertificateRevocationReason> {
        self.revocation_reason
    }

    pub fn snapshot(&self) -> AgentCertificateLifecycleSnapshot {
        AgentCertificateLifecycleSnapshot {
            agent_id: self.agent_id.clone(),
            state: self.state,
            current_certificate: self.current_certificate.clone(),
            next_certificate: self.next_certificate.clone(),
            grace_until: self.grace_until,
            revocation_reason: self.revocation_reason,
        }
    }

    pub fn from_snapshot(
        snapshot: AgentCertificateLifecycleSnapshot,
    ) -> Result<Self, AgentCertificateLifecycleError> {
        validate_agent_certificate_lifecycle_snapshot(&snapshot)?;
        Ok(Self {
            agent_id: snapshot.agent_id,
            state: snapshot.state,
            current_certificate: snapshot.current_certificate,
            next_certificate: snapshot.next_certificate,
            grace_until: snapshot.grace_until,
            revocation_reason: snapshot.revocation_reason,
        })
    }

    pub fn request_issuance(
        &mut self,
        _requested_at: SystemTime,
    ) -> Result<(), AgentCertificateLifecycleError> {
        self.ensure_non_terminal()?;
        if self.state != AgentCertificateLifecycleState::NotIssued {
            return Err(invalid_transition(
                self.state,
                AgentCertificateLifecycleState::NotIssued,
            ));
        }
        self.state = AgentCertificateLifecycleState::IssuanceRequested;
        Ok(())
    }

    pub fn issue(
        &mut self,
        certificate: AgentCertificate,
        issued_at: SystemTime,
    ) -> Result<(), AgentCertificateLifecycleError> {
        self.ensure_expected_state(AgentCertificateLifecycleState::IssuanceRequested)?;
        ensure_certificate_valid_at(&certificate, issued_at)?;
        self.current_certificate = Some(certificate);
        self.next_certificate = None;
        self.grace_until = None;
        self.state = AgentCertificateLifecycleState::Issued;
        Ok(())
    }

    pub fn requires_renewal(
        &self,
        now: SystemTime,
        policy: &AgentCertificateRenewalPolicy,
    ) -> bool {
        if self.state != AgentCertificateLifecycleState::Issued {
            return false;
        }
        self.current_certificate
            .as_ref()
            .is_some_and(|certificate| {
                match certificate.validity().not_after().duration_since(now) {
                    Ok(remaining) => remaining <= policy.renew_before_expiry(),
                    Err(_) => true,
                }
            })
    }

    pub fn request_renewal(
        &mut self,
        requested_at: SystemTime,
        policy: &AgentCertificateRenewalPolicy,
    ) -> Result<(), AgentCertificateLifecycleError> {
        self.ensure_non_terminal()?;
        self.ensure_expected_state(AgentCertificateLifecycleState::Issued)?;
        if !self.requires_renewal(requested_at, policy) {
            return Err(AgentCertificateLifecycleError::NotWithinRenewalWindow);
        }
        self.state = AgentCertificateLifecycleState::RenewalRequested;
        Ok(())
    }

    pub fn activate_renewal(
        &mut self,
        next_certificate: AgentCertificate,
        activated_at: SystemTime,
        policy: &AgentCertificateRenewalPolicy,
    ) -> Result<(), AgentCertificateLifecycleError> {
        self.ensure_non_terminal()?;
        self.ensure_expected_state(AgentCertificateLifecycleState::RenewalRequested)?;
        ensure_certificate_valid_at(&next_certificate, activated_at)?;

        let current = self
            .current_certificate
            .as_ref()
            .ok_or(AgentCertificateLifecycleError::MissingCurrentCertificate)?;
        if current.serial() == next_certificate.serial()
            || current.fingerprint() == next_certificate.fingerprint()
        {
            return Err(AgentCertificateLifecycleError::CertificateIdentityConflict);
        }

        let grace_until = activated_at
            .checked_add(policy.rotation_grace_period())
            .ok_or(AgentCertificateLifecycleError::InvalidTimeOrder)?;

        self.next_certificate = Some(next_certificate);
        self.grace_until = Some(grace_until);
        self.state = AgentCertificateLifecycleState::DualCertificateActive;
        Ok(())
    }

    pub fn complete_rotation(
        &mut self,
        completed_at: SystemTime,
    ) -> Result<(), AgentCertificateLifecycleError> {
        self.ensure_non_terminal()?;
        self.ensure_expected_state(AgentCertificateLifecycleState::DualCertificateActive)?;
        let grace_until = self
            .grace_until
            .ok_or(AgentCertificateLifecycleError::MissingGracePeriod)?;
        if completed_at < grace_until {
            return Err(AgentCertificateLifecycleError::GracePeriodNotElapsed);
        }

        let next_certificate = self
            .next_certificate
            .take()
            .ok_or(AgentCertificateLifecycleError::MissingNextCertificate)?;
        self.current_certificate = Some(next_certificate);
        self.grace_until = None;
        self.state = AgentCertificateLifecycleState::Issued;
        Ok(())
    }

    pub fn revoke(
        &mut self,
        reason: AgentCertificateRevocationReason,
        _revoked_at: SystemTime,
    ) -> Result<(), AgentCertificateLifecycleError> {
        self.ensure_non_terminal()?;
        if self.current_certificate.is_none() && self.next_certificate.is_none() {
            return Err(invalid_transition(
                self.state,
                AgentCertificateLifecycleState::Issued,
            ));
        }
        self.next_certificate = None;
        self.grace_until = None;
        self.revocation_reason = Some(reason);
        self.state = AgentCertificateLifecycleState::Revoked;
        Ok(())
    }

    pub fn expire(&mut self, expired_at: SystemTime) -> Result<(), AgentCertificateLifecycleError> {
        self.ensure_non_terminal()?;
        let current = self
            .current_certificate
            .as_ref()
            .ok_or(AgentCertificateLifecycleError::MissingCurrentCertificate)?;
        if current.validity().not_after() > expired_at {
            return Err(AgentCertificateLifecycleError::CertificateStillValid);
        }
        self.next_certificate = None;
        self.grace_until = None;
        self.state = AgentCertificateLifecycleState::Expired;
        Ok(())
    }

    pub fn fail(&mut self) -> Result<(), AgentCertificateLifecycleError> {
        self.ensure_non_terminal()?;
        self.next_certificate = None;
        self.grace_until = None;
        self.state = AgentCertificateLifecycleState::Failed;
        Ok(())
    }

    pub fn trusts_certificate(
        &self,
        fingerprint: &AgentCertificateFingerprint,
        at: SystemTime,
    ) -> bool {
        match self.state {
            AgentCertificateLifecycleState::Issued => self
                .current_certificate
                .as_ref()
                .is_some_and(|certificate| certificate_matches(certificate, fingerprint, at)),
            AgentCertificateLifecycleState::DualCertificateActive => {
                let trusts_current = self
                    .current_certificate
                    .as_ref()
                    .is_some_and(|certificate| {
                        certificate_matches(certificate, fingerprint, at)
                            && self
                                .grace_until
                                .is_some_and(|grace_until| at <= grace_until)
                    });
                let trusts_next = self
                    .next_certificate
                    .as_ref()
                    .is_some_and(|certificate| certificate_matches(certificate, fingerprint, at));
                trusts_current || trusts_next
            }
            _ => false,
        }
    }

    fn ensure_expected_state(
        &self,
        expected: AgentCertificateLifecycleState,
    ) -> Result<(), AgentCertificateLifecycleError> {
        if self.state == expected {
            Ok(())
        } else {
            Err(invalid_transition(self.state, expected))
        }
    }

    fn ensure_non_terminal(&self) -> Result<(), AgentCertificateLifecycleError> {
        if self.state.is_terminal() {
            Err(AgentCertificateLifecycleError::TerminalState)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentCertificateLifecycleError {
    InvalidFingerprint,
    InvalidSerial,
    InvalidValidityWindow,
    InvalidRenewalPolicy,
    InvalidTimeOrder,
    CertificateNotValidAtOperationTime,
    CertificateIdentityConflict,
    NotWithinRenewalWindow,
    GracePeriodNotElapsed,
    MissingCurrentCertificate,
    MissingNextCertificate,
    MissingGracePeriod,
    CertificateStillValid,
    TerminalState,
    InvalidSnapshot,
    InvalidTransition {
        from: AgentCertificateLifecycleState,
        expected: AgentCertificateLifecycleState,
    },
}

impl Display for AgentCertificateLifecycleError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFingerprint => {
                formatter.write_str("invalid agent certificate fingerprint")
            }
            Self::InvalidSerial => formatter.write_str("invalid agent certificate serial"),
            Self::InvalidValidityWindow => {
                formatter.write_str("agent certificate validity window is invalid")
            }
            Self::InvalidRenewalPolicy => {
                formatter.write_str("agent certificate renewal policy is invalid")
            }
            Self::InvalidTimeOrder => {
                formatter.write_str("agent certificate lifecycle times are invalid")
            }
            Self::CertificateNotValidAtOperationTime => formatter
                .write_str("agent certificate is not valid at the lifecycle operation time"),
            Self::CertificateIdentityConflict => formatter
                .write_str("current and next agent certificate identities must be distinct"),
            Self::NotWithinRenewalWindow => {
                formatter.write_str("agent certificate is not within the renewal window")
            }
            Self::GracePeriodNotElapsed => {
                formatter.write_str("agent certificate rotation grace period has not elapsed")
            }
            Self::MissingCurrentCertificate => {
                formatter.write_str("agent certificate lifecycle is missing current certificate")
            }
            Self::MissingNextCertificate => {
                formatter.write_str("agent certificate lifecycle is missing next certificate")
            }
            Self::MissingGracePeriod => {
                formatter.write_str("agent certificate lifecycle is missing grace period")
            }
            Self::CertificateStillValid => formatter.write_str("agent certificate is still valid"),
            Self::TerminalState => {
                formatter.write_str("agent certificate lifecycle is already terminal")
            }
            Self::InvalidSnapshot => {
                formatter.write_str("agent certificate lifecycle snapshot is invalid")
            }
            Self::InvalidTransition { from, expected } => write!(
                formatter,
                "invalid agent certificate lifecycle transition from {} to expected {}",
                from.as_str(),
                expected.as_str()
            ),
        }
    }
}

impl std::error::Error for AgentCertificateLifecycleError {}

fn ensure_certificate_valid_at(
    certificate: &AgentCertificate,
    at: SystemTime,
) -> Result<(), AgentCertificateLifecycleError> {
    if certificate.validity().is_valid_at(at) {
        Ok(())
    } else {
        Err(AgentCertificateLifecycleError::CertificateNotValidAtOperationTime)
    }
}

fn certificate_matches(
    certificate: &AgentCertificate,
    fingerprint: &AgentCertificateFingerprint,
    at: SystemTime,
) -> bool {
    certificate.fingerprint() == fingerprint && certificate.validity().is_valid_at(at)
}

fn validate_agent_certificate_lifecycle_snapshot(
    snapshot: &AgentCertificateLifecycleSnapshot,
) -> Result<(), AgentCertificateLifecycleError> {
    match snapshot.state {
        AgentCertificateLifecycleState::NotIssued
        | AgentCertificateLifecycleState::IssuanceRequested => ensure_snapshot_fields(
            snapshot.current_certificate.is_none()
                && snapshot.next_certificate.is_none()
                && snapshot.grace_until.is_none()
                && snapshot.revocation_reason.is_none(),
        ),
        AgentCertificateLifecycleState::Issued
        | AgentCertificateLifecycleState::RenewalRequested => ensure_snapshot_fields(
            snapshot.current_certificate.is_some()
                && snapshot.next_certificate.is_none()
                && snapshot.grace_until.is_none()
                && snapshot.revocation_reason.is_none(),
        ),
        AgentCertificateLifecycleState::DualCertificateActive => {
            let current = snapshot
                .current_certificate
                .as_ref()
                .ok_or(AgentCertificateLifecycleError::InvalidSnapshot)?;
            let next = snapshot
                .next_certificate
                .as_ref()
                .ok_or(AgentCertificateLifecycleError::InvalidSnapshot)?;
            ensure_snapshot_fields(
                snapshot.grace_until.is_some()
                    && snapshot.revocation_reason.is_none()
                    && current.serial() != next.serial()
                    && current.fingerprint() != next.fingerprint(),
            )
        }
        AgentCertificateLifecycleState::Revoked => ensure_snapshot_fields(
            snapshot.current_certificate.is_some()
                && snapshot.next_certificate.is_none()
                && snapshot.grace_until.is_none()
                && snapshot.revocation_reason.is_some(),
        ),
        AgentCertificateLifecycleState::Expired => ensure_snapshot_fields(
            snapshot.current_certificate.is_some()
                && snapshot.next_certificate.is_none()
                && snapshot.grace_until.is_none()
                && snapshot.revocation_reason.is_none(),
        ),
        AgentCertificateLifecycleState::Failed => ensure_snapshot_fields(
            snapshot.next_certificate.is_none()
                && snapshot.grace_until.is_none()
                && snapshot.revocation_reason.is_none(),
        ),
    }
}

fn ensure_snapshot_fields(valid: bool) -> Result<(), AgentCertificateLifecycleError> {
    if valid {
        Ok(())
    } else {
        Err(AgentCertificateLifecycleError::InvalidSnapshot)
    }
}

fn invalid_transition(
    from: AgentCertificateLifecycleState,
    expected: AgentCertificateLifecycleState,
) -> AgentCertificateLifecycleError {
    AgentCertificateLifecycleError::InvalidTransition { from, expected }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;
    use crate::AgentId;

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn fingerprint(value: &str) -> AgentCertificateFingerprint {
        AgentCertificateFingerprint::new(value).unwrap()
    }

    fn certificate(
        serial: &str,
        fingerprint: &str,
        not_before: u64,
        not_after: u64,
    ) -> AgentCertificate {
        AgentCertificate::new(
            AgentCertificateSerial::new(serial).unwrap(),
            AgentCertificateFingerprint::new(fingerprint).unwrap(),
            AgentCertificateValidity::new(at(not_before), at(not_after)).unwrap(),
        )
        .unwrap()
    }

    fn policy() -> AgentCertificateRenewalPolicy {
        AgentCertificateRenewalPolicy::new(Duration::from_secs(30), Duration::from_secs(10))
            .unwrap()
    }

    #[test]
    fn agent_certificate_lifecycle_initial_issue_and_renewal_rotation() {
        let mut lifecycle = AgentCertificateLifecycle::new(AgentId::new("agent-1").unwrap());

        assert_eq!(lifecycle.state(), AgentCertificateLifecycleState::NotIssued);
        lifecycle.request_issuance(at(10)).unwrap();
        lifecycle
            .issue(certificate("serial-1", "0123456789abcdef", 10, 110), at(11))
            .unwrap();

        assert_eq!(lifecycle.state(), AgentCertificateLifecycleState::Issued);
        assert!(!lifecycle.requires_renewal(at(70), &policy()));
        assert!(lifecycle.requires_renewal(at(80), &policy()));

        lifecycle.request_renewal(at(80), &policy()).unwrap();
        lifecycle
            .activate_renewal(
                certificate("serial-2", "fedcba9876543210", 80, 200),
                at(81),
                &policy(),
            )
            .unwrap();

        assert_eq!(
            lifecycle.state(),
            AgentCertificateLifecycleState::DualCertificateActive
        );
        assert!(lifecycle.trusts_certificate(&fingerprint("0123456789abcdef"), at(85)));
        assert!(lifecycle.trusts_certificate(&fingerprint("fedcba9876543210"), at(85)));
        assert!(!lifecycle.trusts_certificate(&fingerprint("0123456789abcdef"), at(92)));

        lifecycle.complete_rotation(at(91)).unwrap();

        assert_eq!(lifecycle.state(), AgentCertificateLifecycleState::Issued);
        assert_eq!(
            lifecycle.current_certificate().unwrap().serial().as_str(),
            "serial-2"
        );
        assert!(lifecycle.trusts_certificate(&fingerprint("fedcba9876543210"), at(92)));
    }

    #[test]
    fn agent_certificate_lifecycle_rejects_invalid_transition() {
        let mut lifecycle = AgentCertificateLifecycle::new(AgentId::new("agent-1").unwrap());

        let error = lifecycle
            .issue(certificate("serial-1", "0123456789abcdef", 10, 110), at(11))
            .expect_err("certificate cannot be issued before issuance is requested");

        assert_eq!(
            error,
            AgentCertificateLifecycleError::InvalidTransition {
                from: AgentCertificateLifecycleState::NotIssued,
                expected: AgentCertificateLifecycleState::IssuanceRequested,
            }
        );

        lifecycle.request_issuance(at(10)).unwrap();
        lifecycle
            .issue(certificate("serial-1", "0123456789abcdef", 10, 110), at(11))
            .unwrap();

        let error = lifecycle
            .request_issuance(at(12))
            .expect_err("issued certificate lifecycle cannot request initial issuance again");

        assert_eq!(
            error,
            AgentCertificateLifecycleError::InvalidTransition {
                from: AgentCertificateLifecycleState::Issued,
                expected: AgentCertificateLifecycleState::NotIssued,
            }
        );
    }

    #[test]
    fn agent_certificate_lifecycle_rejects_invalid_material_without_leak() {
        let fingerprint_error =
            AgentCertificateFingerprint::new("-----BEGIN PRIVATE KEY-----\nsecret")
                .expect_err("fingerprint must reject body-like material");
        let serial_error = AgentCertificateSerial::new("serial\nsecret-path")
            .expect_err("serial must reject multiline material");

        let fingerprint_message = fingerprint_error.to_string();
        let serial_message = serial_error.to_string();

        assert!(!fingerprint_message.contains("PRIVATE KEY"));
        assert!(!fingerprint_message.contains("secret"));
        assert!(!serial_message.contains("secret-path"));
    }

    #[test]
    fn agent_certificate_lifecycle_snapshot_roundtrips_public_state_only() {
        let mut lifecycle = AgentCertificateLifecycle::new(AgentId::new("agent-1").unwrap());
        lifecycle.request_issuance(at(10)).unwrap();
        lifecycle
            .issue(certificate("serial-1", "0123456789abcdef", 10, 110), at(11))
            .unwrap();
        lifecycle.request_renewal(at(80), &policy()).unwrap();
        lifecycle
            .activate_renewal(
                certificate("serial-2", "fedcba9876543210", 80, 200),
                at(81),
                &policy(),
            )
            .unwrap();

        let snapshot = lifecycle.snapshot();
        let restored = AgentCertificateLifecycle::from_snapshot(snapshot.clone()).unwrap();

        assert_eq!(restored.snapshot(), snapshot);
        let debug = format!("{snapshot:?}");
        for forbidden in ["PRIVATE KEY", "BEGIN CERTIFICATE", "/etc/fleet", "CA_PATH"] {
            assert!(
                !debug.contains(forbidden),
                "certificate lifecycle snapshot must not expose {forbidden}"
            );
        }
    }

    #[test]
    fn agent_certificate_lifecycle_restore_rejects_inconsistent_snapshot() {
        let snapshot = AgentCertificateLifecycleSnapshot {
            agent_id: AgentId::new("agent-1").unwrap(),
            state: AgentCertificateLifecycleState::NotIssued,
            current_certificate: Some(certificate("serial-1", "0123456789abcdef", 10, 110)),
            next_certificate: None,
            grace_until: None,
            revocation_reason: None,
        };

        let error = AgentCertificateLifecycle::from_snapshot(snapshot)
            .expect_err("not_issued snapshot cannot contain current certificate");

        assert_eq!(error, AgentCertificateLifecycleError::InvalidSnapshot);
    }
}
