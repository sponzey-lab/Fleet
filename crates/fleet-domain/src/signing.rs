use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Debug, Display, Formatter};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SigningKeyFingerprint(String);

impl SigningKeyFingerprint {
    pub fn new(value: impl Into<String>) -> Result<Self, SigningKeyRotationError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(SigningKeyRotationError::InvalidFingerprint);
        }
        if value.contains('\n') || value.contains('\r') {
            return Err(SigningKeyRotationError::InvalidFingerprint);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigningKeyRotationState {
    Steady,
    RotationRequested,
    NewMaterialValidated,
    DualTrustActive,
    OldKeyRetired,
    RotationFailed,
    CanceledBeforeActivation,
}

impl SigningKeyRotationState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Steady => "steady",
            Self::RotationRequested => "rotation_requested",
            Self::NewMaterialValidated => "new_material_validated",
            Self::DualTrustActive => "dual_trust_active",
            Self::OldKeyRetired => "old_key_retired",
            Self::RotationFailed => "rotation_failed",
            Self::CanceledBeforeActivation => "canceled_before_activation",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "steady" => Some(Self::Steady),
            "rotation_requested" => Some(Self::RotationRequested),
            "new_material_validated" => Some(Self::NewMaterialValidated),
            "dual_trust_active" => Some(Self::DualTrustActive),
            "old_key_retired" => Some(Self::OldKeyRetired),
            "rotation_failed" => Some(Self::RotationFailed),
            "canceled_before_activation" => Some(Self::CanceledBeforeActivation),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::OldKeyRetired | Self::RotationFailed | Self::CanceledBeforeActivation
        )
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ControllerSigningPublicKey(String);

impl ControllerSigningPublicKey {
    pub fn new(value: impl Into<String>) -> Result<Self, SigningTrustBundleError> {
        let value = value.into();
        if value.trim().is_empty() || value.contains('\n') || value.contains('\r') {
            return Err(SigningTrustBundleError::InvalidPublicKey);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Debug for ControllerSigningPublicKey {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ControllerSigningPublicKey([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerSigningTrustRole {
    Current,
    Previous,
}

impl ControllerSigningTrustRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Previous => "previous",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ControllerSigningTrustEntry {
    role: ControllerSigningTrustRole,
    fingerprint: SigningKeyFingerprint,
    public_key: ControllerSigningPublicKey,
    valid_from: SystemTime,
    valid_until: Option<SystemTime>,
}

impl Debug for ControllerSigningTrustEntry {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControllerSigningTrustEntry")
            .field("role", &self.role)
            .field("fingerprint", &self.fingerprint)
            .field("public_key", &"[REDACTED]")
            .field("valid_from", &self.valid_from)
            .field("valid_until", &self.valid_until)
            .finish()
    }
}

impl ControllerSigningTrustEntry {
    pub fn new(
        role: ControllerSigningTrustRole,
        fingerprint: SigningKeyFingerprint,
        public_key: ControllerSigningPublicKey,
        valid_from: SystemTime,
        valid_until: Option<SystemTime>,
    ) -> Result<Self, SigningTrustBundleError> {
        if let Some(valid_until) = valid_until
            && valid_until < valid_from
        {
            return Err(SigningTrustBundleError::InvalidTimeWindow);
        }
        if role == ControllerSigningTrustRole::Previous && valid_until.is_none() {
            return Err(SigningTrustBundleError::PreviousTrustRequiresExpiry);
        }
        Ok(Self {
            role,
            fingerprint,
            public_key,
            valid_from,
            valid_until,
        })
    }

    pub fn role(&self) -> ControllerSigningTrustRole {
        self.role
    }

    pub fn fingerprint(&self) -> &SigningKeyFingerprint {
        &self.fingerprint
    }

    pub fn public_key(&self) -> &ControllerSigningPublicKey {
        &self.public_key
    }

    pub fn valid_from(&self) -> SystemTime {
        self.valid_from
    }

    pub fn valid_until(&self) -> Option<SystemTime> {
        self.valid_until
    }

    pub fn allows_signature_time(
        &self,
        signed_at: SystemTime,
        verification_at: SystemTime,
    ) -> bool {
        match self.role {
            ControllerSigningTrustRole::Current => {
                signed_at >= self.valid_from
                    && self
                        .valid_until
                        .is_none_or(|valid_until| verification_at <= valid_until)
            }
            ControllerSigningTrustRole::Previous => {
                signed_at < self.valid_from
                    && self
                        .valid_until
                        .is_some_and(|valid_until| verification_at <= valid_until)
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ControllerSigningTrustBundle {
    entries: Vec<ControllerSigningTrustEntry>,
}

impl Debug for ControllerSigningTrustBundle {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControllerSigningTrustBundle")
            .field("entries", &self.entries)
            .finish()
    }
}

impl ControllerSigningTrustBundle {
    pub fn new(entries: Vec<ControllerSigningTrustEntry>) -> Result<Self, SigningTrustBundleError> {
        if entries.is_empty() {
            return Err(SigningTrustBundleError::EmptyBundle);
        }
        let current_count = entries
            .iter()
            .filter(|entry| entry.role == ControllerSigningTrustRole::Current)
            .count();
        if current_count != 1 {
            return Err(SigningTrustBundleError::InvalidCurrentEntryCount);
        }
        for (index, entry) in entries.iter().enumerate() {
            if entries
                .iter()
                .skip(index + 1)
                .any(|other| other.fingerprint == entry.fingerprint)
            {
                return Err(SigningTrustBundleError::DuplicateFingerprint);
            }
        }
        Ok(Self { entries })
    }

    pub fn from_legacy_pinned(
        fingerprint: SigningKeyFingerprint,
        public_key: ControllerSigningPublicKey,
    ) -> Result<Self, SigningTrustBundleError> {
        Self::new(vec![ControllerSigningTrustEntry::new(
            ControllerSigningTrustRole::Current,
            fingerprint,
            public_key,
            SystemTime::UNIX_EPOCH,
            None,
        )?])
    }

    pub fn entries(&self) -> &[ControllerSigningTrustEntry] {
        &self.entries
    }

    pub fn entry_for_fingerprint(
        &self,
        fingerprint: &SigningKeyFingerprint,
        signed_at: SystemTime,
        verification_at: SystemTime,
    ) -> Result<&ControllerSigningTrustEntry, SigningTrustBundleError> {
        let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.fingerprint() == fingerprint)
        else {
            return Err(SigningTrustBundleError::UnknownFingerprint);
        };
        if !entry.allows_signature_time(signed_at, verification_at) {
            return Err(SigningTrustBundleError::ExpiredTrust);
        }
        Ok(entry)
    }

    pub fn valid_entries(
        &self,
        signed_at: SystemTime,
        verification_at: SystemTime,
    ) -> impl Iterator<Item = &ControllerSigningTrustEntry> {
        self.entries
            .iter()
            .filter(move |entry| entry.allows_signature_time(signed_at, verification_at))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerSigningTrustVerification {
    VerifiedCurrent,
    VerifiedPrevious,
}

impl ControllerSigningTrustVerification {
    pub fn from_role(role: ControllerSigningTrustRole) -> Self {
        match role {
            ControllerSigningTrustRole::Current => Self::VerifiedCurrent,
            ControllerSigningTrustRole::Previous => Self::VerifiedPrevious,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControllerSigningStagedRolloutConfig {
    pub batch_size: usize,
    pub max_failures: usize,
    pub ack_timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerSigningStagedRolloutState {
    Planned,
    DispatchingBatch,
    WaitingForAck,
    Completed,
    Failed,
}

impl ControllerSigningStagedRolloutState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::DispatchingBatch => "dispatching_batch",
            Self::WaitingForAck => "waiting_for_ack",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "planned" => Some(Self::Planned),
            "dispatching_batch" => Some(Self::DispatchingBatch),
            "waiting_for_ack" => Some(Self::WaitingForAck),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSigningStagedRolloutTarget {
    pub agent_id: String,
    pub connected: bool,
    pub accepted_current: bool,
    pub acknowledged_at: Option<SystemTime>,
}

impl ControllerSigningStagedRolloutTarget {
    pub fn observed(
        agent_id: impl Into<String>,
        connected: bool,
        accepted_current: bool,
        acknowledged_at: Option<SystemTime>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            connected,
            accepted_current,
            acknowledged_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSigningStagedRolloutBatchPlan {
    pub agent_ids: Vec<String>,
    pub already_current_count: usize,
    pub unavailable_count: usize,
    pub pending_count: usize,
    pub planned_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSigningStagedRolloutTimeout {
    pub timed_out_agent_ids: Vec<String>,
    pub failed_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ControllerSigningStagedRolloutAttempt {
    agent_id: String,
    dispatched_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSigningStagedRolloutAttemptSnapshot {
    pub agent_id: String,
    pub dispatched_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSigningStagedRolloutSnapshot {
    pub state: ControllerSigningStagedRolloutState,
    pub target_ids: Vec<String>,
    pub config: ControllerSigningStagedRolloutConfig,
    pub acknowledged_agent_ids: Vec<String>,
    pub unavailable_agent_ids: Vec<String>,
    pub failed_agent_ids: Vec<String>,
    pub in_flight: Vec<ControllerSigningStagedRolloutAttemptSnapshot>,
    pub failure_reason_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSigningStagedRollout {
    state: ControllerSigningStagedRolloutState,
    target_ids: Vec<String>,
    config: ControllerSigningStagedRolloutConfig,
    acknowledged: BTreeSet<String>,
    unavailable: BTreeSet<String>,
    failed: BTreeSet<String>,
    in_flight: BTreeMap<String, ControllerSigningStagedRolloutAttempt>,
    failure_reason_code: Option<String>,
}

impl ControllerSigningStagedRollout {
    pub fn new(
        target_ids: Vec<String>,
        config: ControllerSigningStagedRolloutConfig,
    ) -> Result<Self, SigningStagedRolloutError> {
        if target_ids.is_empty() || config.batch_size == 0 || config.ack_timeout == Duration::ZERO {
            return Err(SigningStagedRolloutError::InvalidConfig);
        }
        let mut seen = BTreeSet::new();
        for target_id in &target_ids {
            if target_id.trim().is_empty() || !seen.insert(target_id.clone()) {
                return Err(SigningStagedRolloutError::InvalidConfig);
            }
        }
        Ok(Self {
            state: ControllerSigningStagedRolloutState::Planned,
            target_ids,
            config,
            acknowledged: BTreeSet::new(),
            unavailable: BTreeSet::new(),
            failed: BTreeSet::new(),
            in_flight: BTreeMap::new(),
            failure_reason_code: None,
        })
    }

    pub fn from_snapshot(
        snapshot: ControllerSigningStagedRolloutSnapshot,
    ) -> Result<Self, SigningStagedRolloutError> {
        let mut rollout = Self::new(snapshot.target_ids.clone(), snapshot.config)?;
        let target_set = rollout.target_ids.iter().cloned().collect::<BTreeSet<_>>();
        let acknowledged =
            staged_rollout_agent_set_from_snapshot(&snapshot.acknowledged_agent_ids, &target_set)?;
        let unavailable =
            staged_rollout_agent_set_from_snapshot(&snapshot.unavailable_agent_ids, &target_set)?;
        let failed =
            staged_rollout_agent_set_from_snapshot(&snapshot.failed_agent_ids, &target_set)?;
        if !acknowledged.is_disjoint(&unavailable)
            || !acknowledged.is_disjoint(&failed)
            || !unavailable.is_disjoint(&failed)
        {
            return Err(SigningStagedRolloutError::InvalidTransition);
        }
        let mut in_flight = BTreeMap::new();
        for attempt in snapshot.in_flight {
            if attempt.agent_id.trim().is_empty()
                || !target_set.contains(&attempt.agent_id)
                || acknowledged.contains(&attempt.agent_id)
                || unavailable.contains(&attempt.agent_id)
                || failed.contains(&attempt.agent_id)
                || in_flight.contains_key(&attempt.agent_id)
            {
                return Err(SigningStagedRolloutError::InvalidTransition);
            }
            in_flight.insert(
                attempt.agent_id.clone(),
                ControllerSigningStagedRolloutAttempt {
                    agent_id: attempt.agent_id,
                    dispatched_at: attempt.dispatched_at,
                },
            );
        }
        match snapshot.state {
            ControllerSigningStagedRolloutState::WaitingForAck if in_flight.is_empty() => {
                return Err(SigningStagedRolloutError::InvalidTransition);
            }
            ControllerSigningStagedRolloutState::Completed
            | ControllerSigningStagedRolloutState::Failed
                if !in_flight.is_empty() =>
            {
                return Err(SigningStagedRolloutError::InvalidTransition);
            }
            ControllerSigningStagedRolloutState::Completed
                if !rollout.target_ids.iter().all(|target_id| {
                    acknowledged.contains(target_id)
                        || unavailable.contains(target_id)
                        || failed.contains(target_id)
                }) =>
            {
                return Err(SigningStagedRolloutError::InvalidTransition);
            }
            ControllerSigningStagedRolloutState::Failed
                if snapshot
                    .failure_reason_code
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty() =>
            {
                return Err(SigningStagedRolloutError::InvalidTransition);
            }
            _ => {}
        }
        rollout.state = snapshot.state;
        rollout.acknowledged = acknowledged;
        rollout.unavailable = unavailable;
        rollout.failed = failed;
        rollout.in_flight = in_flight;
        rollout.failure_reason_code = snapshot.failure_reason_code;
        Ok(rollout)
    }

    pub fn snapshot(&self) -> ControllerSigningStagedRolloutSnapshot {
        ControllerSigningStagedRolloutSnapshot {
            state: self.state,
            target_ids: self.target_ids.clone(),
            config: self.config,
            acknowledged_agent_ids: self.acknowledged.iter().cloned().collect(),
            unavailable_agent_ids: self.unavailable.iter().cloned().collect(),
            failed_agent_ids: self.failed.iter().cloned().collect(),
            in_flight: self
                .in_flight
                .values()
                .map(|attempt| ControllerSigningStagedRolloutAttemptSnapshot {
                    agent_id: attempt.agent_id.clone(),
                    dispatched_at: attempt.dispatched_at,
                })
                .collect(),
            failure_reason_code: self.failure_reason_code.clone(),
        }
    }

    pub fn state(&self) -> ControllerSigningStagedRolloutState {
        self.state
    }

    pub fn failure_reason_code(&self) -> Option<&str> {
        self.failure_reason_code.as_deref()
    }

    pub fn plan_next_batch(
        &mut self,
        observations: &[ControllerSigningStagedRolloutTarget],
        planned_at: SystemTime,
    ) -> Result<ControllerSigningStagedRolloutBatchPlan, SigningStagedRolloutError> {
        self.require_non_terminal()?;
        if self.state == ControllerSigningStagedRolloutState::WaitingForAck
            && !self.in_flight.is_empty()
        {
            return Err(SigningStagedRolloutError::InvalidTransition);
        }

        self.apply_observations(observations)?;
        let connected = observations
            .iter()
            .filter(|target| target.connected)
            .map(|target| target.agent_id.as_str())
            .collect::<BTreeSet<_>>();
        let mut candidates = Vec::new();
        for target_id in &self.target_ids {
            if self.acknowledged.contains(target_id)
                || self.unavailable.contains(target_id)
                || self.failed.contains(target_id)
                || self.in_flight.contains_key(target_id)
            {
                continue;
            }
            candidates.push(target_id.clone());
        }
        let agent_ids = candidates
            .iter()
            .filter(|agent_id| connected.contains(agent_id.as_str()))
            .take(self.config.batch_size)
            .cloned()
            .collect::<Vec<_>>();
        let pending_count = candidates.len().saturating_sub(agent_ids.len());
        if agent_ids.is_empty() {
            self.refresh_terminal_state();
        } else {
            self.state = ControllerSigningStagedRolloutState::DispatchingBatch;
        }
        Ok(ControllerSigningStagedRolloutBatchPlan {
            agent_ids,
            already_current_count: self.acknowledged.len(),
            unavailable_count: self.unavailable.len(),
            pending_count,
            planned_at,
        })
    }

    pub fn batch_dispatched(
        &mut self,
        agent_ids: &[String],
        dispatched_at: SystemTime,
    ) -> Result<(), SigningStagedRolloutError> {
        self.require_non_terminal()?;
        if self.state != ControllerSigningStagedRolloutState::DispatchingBatch
            || agent_ids.is_empty()
            || agent_ids.len() > self.config.batch_size
        {
            return Err(SigningStagedRolloutError::InvalidTransition);
        }
        for agent_id in agent_ids {
            if !self.target_ids.contains(agent_id)
                || self.acknowledged.contains(agent_id)
                || self.unavailable.contains(agent_id)
                || self.failed.contains(agent_id)
            {
                return Err(SigningStagedRolloutError::UnknownTarget);
            }
        }
        self.in_flight.clear();
        for agent_id in agent_ids {
            self.in_flight.insert(
                agent_id.clone(),
                ControllerSigningStagedRolloutAttempt {
                    agent_id: agent_id.clone(),
                    dispatched_at,
                },
            );
        }
        self.state = ControllerSigningStagedRolloutState::WaitingForAck;
        Ok(())
    }

    pub fn ack_observed(
        &mut self,
        agent_id: &str,
        _acknowledged_at: SystemTime,
    ) -> Result<(), SigningStagedRolloutError> {
        self.require_non_terminal()?;
        if !self
            .target_ids
            .iter()
            .any(|target_id| target_id == agent_id)
        {
            return Err(SigningStagedRolloutError::UnknownTarget);
        }
        self.acknowledged.insert(agent_id.to_owned());
        self.unavailable.remove(agent_id);
        self.failed.remove(agent_id);
        self.in_flight.remove(agent_id);
        if self.in_flight.is_empty() {
            self.state = ControllerSigningStagedRolloutState::Planned;
            self.refresh_terminal_state();
        }
        Ok(())
    }

    pub fn ack_timeout(
        &mut self,
        now: SystemTime,
    ) -> Result<ControllerSigningStagedRolloutTimeout, SigningStagedRolloutError> {
        self.require_non_terminal()?;
        if self.state != ControllerSigningStagedRolloutState::WaitingForAck {
            return Err(SigningStagedRolloutError::InvalidTransition);
        }
        let timed_out_agent_ids = self
            .in_flight
            .values()
            .filter(|attempt| {
                now.duration_since(attempt.dispatched_at)
                    .unwrap_or(Duration::ZERO)
                    >= self.config.ack_timeout
            })
            .map(|attempt| attempt.agent_id.clone())
            .collect::<Vec<_>>();
        for agent_id in &timed_out_agent_ids {
            self.in_flight.remove(agent_id);
            self.failed.insert(agent_id.clone());
        }
        if self.failed.len() > self.config.max_failures {
            self.state = ControllerSigningStagedRolloutState::Failed;
            self.failure_reason_code = Some("ack_timeout".to_owned());
        } else if self.in_flight.is_empty() {
            self.state = ControllerSigningStagedRolloutState::Planned;
            self.refresh_terminal_state();
        }
        Ok(ControllerSigningStagedRolloutTimeout {
            timed_out_agent_ids,
            failed_count: self.failed.len(),
        })
    }

    fn apply_observations(
        &mut self,
        observations: &[ControllerSigningStagedRolloutTarget],
    ) -> Result<(), SigningStagedRolloutError> {
        let target_set = self.target_ids.iter().collect::<BTreeSet<_>>();
        for observation in observations {
            if !target_set.contains(&observation.agent_id) {
                return Err(SigningStagedRolloutError::UnknownTarget);
            }
            if observation.accepted_current {
                self.acknowledged.insert(observation.agent_id.clone());
                self.unavailable.remove(&observation.agent_id);
                self.in_flight.remove(&observation.agent_id);
            } else if observation.connected {
                self.unavailable.remove(&observation.agent_id);
            } else if !self.in_flight.contains_key(&observation.agent_id)
                && !self.acknowledged.contains(&observation.agent_id)
            {
                self.unavailable.insert(observation.agent_id.clone());
            }
        }
        Ok(())
    }

    fn refresh_terminal_state(&mut self) {
        if self.failed.len() > self.config.max_failures {
            self.state = ControllerSigningStagedRolloutState::Failed;
            self.failure_reason_code = Some("ack_timeout".to_owned());
            return;
        }
        if self.in_flight.is_empty()
            && self.target_ids.iter().all(|target_id| {
                self.acknowledged.contains(target_id)
                    || self.unavailable.contains(target_id)
                    || self.failed.contains(target_id)
            })
        {
            self.state = ControllerSigningStagedRolloutState::Completed;
        }
    }

    fn require_non_terminal(&self) -> Result<(), SigningStagedRolloutError> {
        if self.state.is_terminal() {
            Err(SigningStagedRolloutError::TerminalState)
        } else {
            Ok(())
        }
    }
}

fn staged_rollout_agent_set_from_snapshot(
    agent_ids: &[String],
    target_set: &BTreeSet<String>,
) -> Result<BTreeSet<String>, SigningStagedRolloutError> {
    let mut set = BTreeSet::new();
    for agent_id in agent_ids {
        if agent_id.trim().is_empty()
            || !target_set.contains(agent_id)
            || !set.insert(agent_id.clone())
        {
            return Err(SigningStagedRolloutError::InvalidTransition);
        }
    }
    Ok(set)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigningStagedRolloutError {
    InvalidConfig,
    InvalidTransition,
    TerminalState,
    UnknownTarget,
}

impl Display for SigningStagedRolloutError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig => formatter.write_str("invalid staged rollout configuration"),
            Self::InvalidTransition => formatter.write_str("invalid staged rollout transition"),
            Self::TerminalState => formatter.write_str("staged rollout is terminal"),
            Self::UnknownTarget => formatter.write_str("staged rollout target is unknown"),
        }
    }
}

impl std::error::Error for SigningStagedRolloutError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSigningKeyRotation {
    state: SigningKeyRotationState,
    old_fingerprint: SigningKeyFingerprint,
    new_fingerprint: Option<SigningKeyFingerprint>,
    requested_at: Option<SystemTime>,
    validated_at: Option<SystemTime>,
    activated_at: Option<SystemTime>,
    old_key_verifies_until: Option<SystemTime>,
    retired_at: Option<SystemTime>,
    failed_at: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSigningKeyRotationSnapshot {
    pub state: SigningKeyRotationState,
    pub old_fingerprint: SigningKeyFingerprint,
    pub new_fingerprint: Option<SigningKeyFingerprint>,
    pub requested_at: Option<SystemTime>,
    pub validated_at: Option<SystemTime>,
    pub activated_at: Option<SystemTime>,
    pub old_key_verifies_until: Option<SystemTime>,
    pub retired_at: Option<SystemTime>,
    pub failed_at: Option<SystemTime>,
}

impl ControllerSigningKeyRotation {
    pub fn steady(active_fingerprint: SigningKeyFingerprint) -> Self {
        Self {
            state: SigningKeyRotationState::Steady,
            old_fingerprint: active_fingerprint,
            new_fingerprint: None,
            requested_at: None,
            validated_at: None,
            activated_at: None,
            old_key_verifies_until: None,
            retired_at: None,
            failed_at: None,
        }
    }

    pub fn state(&self) -> SigningKeyRotationState {
        self.state
    }

    pub fn old_fingerprint(&self) -> &SigningKeyFingerprint {
        &self.old_fingerprint
    }

    pub fn new_fingerprint(&self) -> Option<&SigningKeyFingerprint> {
        self.new_fingerprint.as_ref()
    }

    pub fn requested_at(&self) -> Option<SystemTime> {
        self.requested_at
    }

    pub fn validated_at(&self) -> Option<SystemTime> {
        self.validated_at
    }

    pub fn activated_at(&self) -> Option<SystemTime> {
        self.activated_at
    }

    pub fn old_key_verifies_until(&self) -> Option<SystemTime> {
        self.old_key_verifies_until
    }

    pub fn retired_at(&self) -> Option<SystemTime> {
        self.retired_at
    }

    pub fn failed_at(&self) -> Option<SystemTime> {
        self.failed_at
    }

    pub fn snapshot(&self) -> ControllerSigningKeyRotationSnapshot {
        ControllerSigningKeyRotationSnapshot {
            state: self.state,
            old_fingerprint: self.old_fingerprint.clone(),
            new_fingerprint: self.new_fingerprint.clone(),
            requested_at: self.requested_at,
            validated_at: self.validated_at,
            activated_at: self.activated_at,
            old_key_verifies_until: self.old_key_verifies_until,
            retired_at: self.retired_at,
            failed_at: self.failed_at,
        }
    }

    pub fn from_snapshot(
        snapshot: ControllerSigningKeyRotationSnapshot,
    ) -> Result<Self, SigningKeyRotationError> {
        validate_rotation_snapshot(&snapshot)?;
        Ok(Self {
            state: snapshot.state,
            old_fingerprint: snapshot.old_fingerprint,
            new_fingerprint: snapshot.new_fingerprint,
            requested_at: snapshot.requested_at,
            validated_at: snapshot.validated_at,
            activated_at: snapshot.activated_at,
            old_key_verifies_until: snapshot.old_key_verifies_until,
            retired_at: snapshot.retired_at,
            failed_at: snapshot.failed_at,
        })
    }

    pub fn current_signing_fingerprint(&self, now: SystemTime) -> &SigningKeyFingerprint {
        match self.state {
            SigningKeyRotationState::DualTrustActive | SigningKeyRotationState::OldKeyRetired
                if self
                    .activated_at
                    .is_some_and(|activated_at| now >= activated_at) =>
            {
                self.new_fingerprint
                    .as_ref()
                    .unwrap_or(&self.old_fingerprint)
            }
            _ => &self.old_fingerprint,
        }
    }

    pub fn request_rotation(
        &mut self,
        new_fingerprint: SigningKeyFingerprint,
        requested_at: SystemTime,
        old_key_verifies_until: SystemTime,
    ) -> Result<(), SigningKeyRotationError> {
        self.require_state(SigningKeyRotationState::Steady)?;
        if self.old_fingerprint == new_fingerprint {
            return Err(SigningKeyRotationError::FingerprintRoleConflict);
        }
        if old_key_verifies_until <= requested_at {
            return Err(SigningKeyRotationError::InvalidTimeOrder);
        }
        self.state = SigningKeyRotationState::RotationRequested;
        self.new_fingerprint = Some(new_fingerprint);
        self.requested_at = Some(requested_at);
        self.old_key_verifies_until = Some(old_key_verifies_until);
        Ok(())
    }

    pub fn validate_new_material(
        &mut self,
        validated_at: SystemTime,
    ) -> Result<(), SigningKeyRotationError> {
        self.require_state(SigningKeyRotationState::RotationRequested)?;
        let requested_at = self
            .requested_at
            .ok_or(SigningKeyRotationError::MissingRotationTime)?;
        if validated_at < requested_at {
            return Err(SigningKeyRotationError::InvalidTimeOrder);
        }
        self.state = SigningKeyRotationState::NewMaterialValidated;
        self.validated_at = Some(validated_at);
        Ok(())
    }

    pub fn activate_dual_trust(
        &mut self,
        activated_at: SystemTime,
    ) -> Result<(), SigningKeyRotationError> {
        self.require_state(SigningKeyRotationState::NewMaterialValidated)?;
        let validated_at = self
            .validated_at
            .ok_or(SigningKeyRotationError::MissingRotationTime)?;
        if activated_at < validated_at {
            return Err(SigningKeyRotationError::InvalidTimeOrder);
        }
        self.state = SigningKeyRotationState::DualTrustActive;
        self.activated_at = Some(activated_at);
        Ok(())
    }

    pub fn retire_old_key(
        &mut self,
        retired_at: SystemTime,
    ) -> Result<(), SigningKeyRotationError> {
        self.require_state(SigningKeyRotationState::DualTrustActive)?;
        let old_key_verifies_until = self
            .old_key_verifies_until
            .ok_or(SigningKeyRotationError::MissingRotationTime)?;
        if retired_at < old_key_verifies_until {
            return Err(SigningKeyRotationError::RetirementGuardNotSatisfied);
        }
        self.state = SigningKeyRotationState::OldKeyRetired;
        self.retired_at = Some(retired_at);
        Ok(())
    }

    pub fn fail_rotation(&mut self, failed_at: SystemTime) -> Result<(), SigningKeyRotationError> {
        if self.state.is_terminal() {
            return Err(SigningKeyRotationError::TerminalState);
        }
        self.state = SigningKeyRotationState::RotationFailed;
        self.failed_at = Some(failed_at);
        Ok(())
    }

    pub fn cancel_before_activation(&mut self) -> Result<(), SigningKeyRotationError> {
        match self.state {
            SigningKeyRotationState::RotationRequested
            | SigningKeyRotationState::NewMaterialValidated => {
                self.state = SigningKeyRotationState::CanceledBeforeActivation;
                Ok(())
            }
            _ => Err(SigningKeyRotationError::InvalidTransition {
                from: self.state,
                expected: SigningKeyRotationState::RotationRequested,
            }),
        }
    }

    pub fn can_verify_signature_from(
        &self,
        fingerprint: &SigningKeyFingerprint,
        signed_at: SystemTime,
        verification_at: SystemTime,
    ) -> bool {
        match self.state {
            SigningKeyRotationState::Steady
            | SigningKeyRotationState::RotationRequested
            | SigningKeyRotationState::NewMaterialValidated => fingerprint == &self.old_fingerprint,
            SigningKeyRotationState::DualTrustActive => {
                self.can_verify_new_key(fingerprint, signed_at)
                    || self.can_verify_old_key(fingerprint, signed_at, verification_at)
            }
            SigningKeyRotationState::OldKeyRetired => {
                self.can_verify_new_key(fingerprint, signed_at)
            }
            SigningKeyRotationState::RotationFailed
            | SigningKeyRotationState::CanceledBeforeActivation => {
                fingerprint == &self.old_fingerprint
            }
        }
    }

    fn can_verify_new_key(
        &self,
        fingerprint: &SigningKeyFingerprint,
        signed_at: SystemTime,
    ) -> bool {
        self.new_fingerprint.as_ref() == Some(fingerprint)
            && self
                .activated_at
                .is_some_and(|activated_at| signed_at >= activated_at)
    }

    fn can_verify_old_key(
        &self,
        fingerprint: &SigningKeyFingerprint,
        signed_at: SystemTime,
        verification_at: SystemTime,
    ) -> bool {
        fingerprint == &self.old_fingerprint
            && self
                .activated_at
                .is_some_and(|activated_at| signed_at < activated_at)
            && self
                .old_key_verifies_until
                .is_some_and(|expires_at| verification_at <= expires_at)
    }

    fn require_state(
        &self,
        expected: SigningKeyRotationState,
    ) -> Result<(), SigningKeyRotationError> {
        if self.state == expected {
            Ok(())
        } else if self.state.is_terminal() {
            Err(SigningKeyRotationError::TerminalState)
        } else {
            Err(SigningKeyRotationError::InvalidTransition {
                from: self.state,
                expected,
            })
        }
    }
}

fn validate_rotation_snapshot(
    snapshot: &ControllerSigningKeyRotationSnapshot,
) -> Result<(), SigningKeyRotationError> {
    if snapshot
        .new_fingerprint
        .as_ref()
        .is_some_and(|new| new == &snapshot.old_fingerprint)
    {
        return Err(SigningKeyRotationError::FingerprintRoleConflict);
    }

    match snapshot.state {
        SigningKeyRotationState::Steady => Ok(()),
        SigningKeyRotationState::RotationRequested => {
            require_snapshot_fields(snapshot, true, false, false, true, false, false)
        }
        SigningKeyRotationState::NewMaterialValidated => {
            require_snapshot_fields(snapshot, true, true, false, true, false, false)?;
            require_order(snapshot.requested_at, snapshot.validated_at)
        }
        SigningKeyRotationState::DualTrustActive => {
            require_snapshot_fields(snapshot, true, true, true, true, false, false)?;
            require_order(snapshot.requested_at, snapshot.validated_at)?;
            require_order(snapshot.validated_at, snapshot.activated_at)
        }
        SigningKeyRotationState::OldKeyRetired => {
            require_snapshot_fields(snapshot, true, true, true, true, true, false)?;
            require_order(snapshot.requested_at, snapshot.validated_at)?;
            require_order(snapshot.validated_at, snapshot.activated_at)?;
            require_order(snapshot.old_key_verifies_until, snapshot.retired_at)
        }
        SigningKeyRotationState::RotationFailed => {
            require_snapshot_fields(snapshot, true, false, false, false, false, true)
        }
        SigningKeyRotationState::CanceledBeforeActivation => {
            require_snapshot_fields(snapshot, true, false, false, false, false, false)
        }
    }
}

fn require_snapshot_fields(
    snapshot: &ControllerSigningKeyRotationSnapshot,
    new_fingerprint: bool,
    validated_at: bool,
    activated_at: bool,
    old_key_verifies_until: bool,
    retired_at: bool,
    failed_at: bool,
) -> Result<(), SigningKeyRotationError> {
    if snapshot.requested_at.is_none()
        || (new_fingerprint && snapshot.new_fingerprint.is_none())
        || (validated_at && snapshot.validated_at.is_none())
        || (activated_at && snapshot.activated_at.is_none())
        || (old_key_verifies_until && snapshot.old_key_verifies_until.is_none())
        || (retired_at && snapshot.retired_at.is_none())
        || (failed_at && snapshot.failed_at.is_none())
    {
        return Err(SigningKeyRotationError::MissingRotationTime);
    }
    Ok(())
}

fn require_order(
    earlier: Option<SystemTime>,
    later: Option<SystemTime>,
) -> Result<(), SigningKeyRotationError> {
    match (earlier, later) {
        (Some(earlier), Some(later)) if later >= earlier => Ok(()),
        _ => Err(SigningKeyRotationError::InvalidTimeOrder),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigningKeyRotationError {
    InvalidFingerprint,
    FingerprintRoleConflict,
    InvalidTimeOrder,
    MissingRotationTime,
    RetirementGuardNotSatisfied,
    TerminalState,
    InvalidTransition {
        from: SigningKeyRotationState,
        expected: SigningKeyRotationState,
    },
}

impl Display for SigningKeyRotationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFingerprint => formatter.write_str("invalid signing key fingerprint"),
            Self::FingerprintRoleConflict => {
                formatter.write_str("old and new signing key fingerprints must be distinct")
            }
            Self::InvalidTimeOrder => formatter.write_str("signing key rotation times are invalid"),
            Self::MissingRotationTime => {
                formatter.write_str("signing key rotation is missing a required timestamp")
            }
            Self::RetirementGuardNotSatisfied => formatter.write_str(
                "old signing key cannot be retired before the dual-trust window expires",
            ),
            Self::TerminalState => formatter.write_str("signing key rotation is already terminal"),
            Self::InvalidTransition { from, expected } => write!(
                formatter,
                "invalid signing key rotation transition from {} to expected {}",
                from.as_str(),
                expected.as_str()
            ),
        }
    }
}

impl std::error::Error for SigningKeyRotationError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigningTrustBundleError {
    EmptyBundle,
    InvalidCurrentEntryCount,
    DuplicateFingerprint,
    InvalidPublicKey,
    InvalidTimeWindow,
    PreviousTrustRequiresExpiry,
    UnknownFingerprint,
    ExpiredTrust,
}

impl Display for SigningTrustBundleError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyBundle => formatter.write_str("controller signing trust bundle is empty"),
            Self::InvalidCurrentEntryCount => formatter.write_str(
                "controller signing trust bundle must contain exactly one current entry",
            ),
            Self::DuplicateFingerprint => {
                formatter.write_str("controller signing trust bundle has duplicate fingerprints")
            }
            Self::InvalidPublicKey => formatter.write_str("invalid controller signing public key"),
            Self::InvalidTimeWindow => {
                formatter.write_str("controller signing trust time window is invalid")
            }
            Self::PreviousTrustRequiresExpiry => {
                formatter.write_str("previous controller signing trust entry requires an expiry")
            }
            Self::UnknownFingerprint => {
                formatter.write_str("controller signing fingerprint is not trusted")
            }
            Self::ExpiredTrust => {
                formatter.write_str("controller signing trust entry is outside its valid window")
            }
        }
    }
}

impl std::error::Error for SigningTrustBundleError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn fingerprint(value: &str) -> SigningKeyFingerprint {
        SigningKeyFingerprint::new(value).unwrap()
    }

    fn public_key(value: &str) -> ControllerSigningPublicKey {
        ControllerSigningPublicKey::new(value).unwrap()
    }

    fn rotated_trust_bundle() -> ControllerSigningTrustBundle {
        ControllerSigningTrustBundle::new(vec![
            ControllerSigningTrustEntry::new(
                ControllerSigningTrustRole::Previous,
                fingerprint("old-controller-key"),
                public_key("old-controller-public-key"),
                at(20),
                Some(at(40)),
            )
            .unwrap(),
            ControllerSigningTrustEntry::new(
                ControllerSigningTrustRole::Current,
                fingerprint("new-controller-key"),
                public_key("new-controller-public-key"),
                at(20),
                None,
            )
            .unwrap(),
        ])
        .unwrap()
    }

    #[test]
    fn legacy_pinned_controller_key_becomes_single_current_trust_bundle() {
        let bundle = ControllerSigningTrustBundle::from_legacy_pinned(
            fingerprint("legacy-controller-key"),
            public_key("legacy-controller-public-key"),
        )
        .unwrap();

        let entry = bundle
            .entry_for_fingerprint(&fingerprint("legacy-controller-key"), at(1), at(50))
            .unwrap();

        assert_eq!(bundle.entries().len(), 1);
        assert_eq!(entry.role(), ControllerSigningTrustRole::Current);
        assert_eq!(entry.public_key().as_str(), "legacy-controller-public-key");
        assert_eq!(entry.valid_from(), SystemTime::UNIX_EPOCH);
        assert_eq!(entry.valid_until(), None);
    }

    #[test]
    fn controller_signing_trust_bundle_rejects_duplicate_or_missing_current_entries() {
        let duplicate_error = ControllerSigningTrustBundle::new(vec![
            ControllerSigningTrustEntry::new(
                ControllerSigningTrustRole::Current,
                fingerprint("same-controller-key"),
                public_key("first-public-key"),
                at(1),
                None,
            )
            .unwrap(),
            ControllerSigningTrustEntry::new(
                ControllerSigningTrustRole::Previous,
                fingerprint("same-controller-key"),
                public_key("second-public-key"),
                at(20),
                Some(at(40)),
            )
            .unwrap(),
        ])
        .expect_err("duplicate fingerprints should be rejected");

        let missing_current_error = ControllerSigningTrustBundle::new(vec![
            ControllerSigningTrustEntry::new(
                ControllerSigningTrustRole::Previous,
                fingerprint("old-controller-key"),
                public_key("old-public-key"),
                at(20),
                Some(at(40)),
            )
            .unwrap(),
        ])
        .expect_err("bundle without exactly one current key should be rejected");

        assert_eq!(
            duplicate_error,
            SigningTrustBundleError::DuplicateFingerprint
        );
        assert_eq!(
            missing_current_error,
            SigningTrustBundleError::InvalidCurrentEntryCount
        );
    }

    #[test]
    fn previous_controller_signing_trust_accepts_only_before_expiry_window() {
        let bundle = rotated_trust_bundle();

        let previous = bundle
            .entry_for_fingerprint(&fingerprint("old-controller-key"), at(19), at(39))
            .unwrap();
        let expired = bundle
            .entry_for_fingerprint(&fingerprint("old-controller-key"), at(19), at(41))
            .expect_err("previous key should expire at verification time");
        let signed_after_rotation = bundle
            .entry_for_fingerprint(&fingerprint("old-controller-key"), at(20), at(21))
            .expect_err("previous key should not sign envelopes issued after rotation");

        assert_eq!(previous.role(), ControllerSigningTrustRole::Previous);
        assert_eq!(expired, SigningTrustBundleError::ExpiredTrust);
        assert_eq!(signed_after_rotation, SigningTrustBundleError::ExpiredTrust);
    }

    #[test]
    fn controller_signing_trust_bundle_rejects_unknown_fingerprint() {
        let bundle = rotated_trust_bundle();

        let error = bundle
            .entry_for_fingerprint(&fingerprint("unknown-controller-key"), at(19), at(21))
            .expect_err("unknown controller signing key should fail");

        assert_eq!(error, SigningTrustBundleError::UnknownFingerprint);
    }

    #[test]
    fn controller_signing_trust_bundle_debug_does_not_expose_key_material() {
        let bundle = rotated_trust_bundle();

        let debug = format!("{bundle:?}");
        let error = ControllerSigningPublicKey::new("private-key-secret\nvalue")
            .expect_err("invalid public key should fail");
        let message = error.to_string();

        assert!(!debug.contains("old-controller-public-key"));
        assert!(!debug.contains("new-controller-public-key"));
        assert!(!message.contains("private-key-secret"));
        assert!(!message.contains("value"));
    }

    #[test]
    fn staged_rollout_skips_already_current_and_plans_bounded_batch() {
        let mut rollout = ControllerSigningStagedRollout::new(
            vec![
                "agent-a".to_owned(),
                "agent-b".to_owned(),
                "agent-c".to_owned(),
            ],
            ControllerSigningStagedRolloutConfig {
                batch_size: 2,
                max_failures: 1,
                ack_timeout: Duration::from_secs(30),
            },
        )
        .unwrap();

        let plan = rollout
            .plan_next_batch(
                &[
                    ControllerSigningStagedRolloutTarget::observed(
                        "agent-a",
                        true,
                        true,
                        Some(at(5)),
                    ),
                    ControllerSigningStagedRolloutTarget::observed("agent-b", true, false, None),
                    ControllerSigningStagedRolloutTarget::observed("agent-c", true, false, None),
                ],
                at(10),
            )
            .unwrap();

        assert_eq!(
            rollout.state(),
            ControllerSigningStagedRolloutState::DispatchingBatch
        );
        assert_eq!(plan.agent_ids, vec!["agent-b", "agent-c"]);
        assert_eq!(plan.already_current_count, 1);
        assert_eq!(plan.unavailable_count, 0);
        assert_eq!(plan.pending_count, 0);
    }

    #[test]
    fn staged_rollout_timeout_fails_when_max_failures_is_exceeded() {
        let mut rollout = ControllerSigningStagedRollout::new(
            vec!["agent-a".to_owned()],
            ControllerSigningStagedRolloutConfig {
                batch_size: 1,
                max_failures: 0,
                ack_timeout: Duration::from_secs(5),
            },
        )
        .unwrap();
        let plan = rollout
            .plan_next_batch(
                &[ControllerSigningStagedRolloutTarget::observed(
                    "agent-a", true, false, None,
                )],
                at(10),
            )
            .unwrap();
        rollout.batch_dispatched(&plan.agent_ids, at(10)).unwrap();

        let timeout = rollout.ack_timeout(at(16)).unwrap();

        assert_eq!(timeout.timed_out_agent_ids, vec!["agent-a"]);
        assert_eq!(rollout.state(), ControllerSigningStagedRolloutState::Failed);
        assert_eq!(rollout.failure_reason_code(), Some("ack_timeout"));
    }

    #[test]
    fn staged_rollout_ack_observed_allows_next_batch_and_completion() {
        let mut rollout = ControllerSigningStagedRollout::new(
            vec!["agent-a".to_owned(), "agent-b".to_owned()],
            ControllerSigningStagedRolloutConfig {
                batch_size: 1,
                max_failures: 1,
                ack_timeout: Duration::from_secs(30),
            },
        )
        .unwrap();
        let first = rollout
            .plan_next_batch(
                &[
                    ControllerSigningStagedRolloutTarget::observed("agent-a", true, false, None),
                    ControllerSigningStagedRolloutTarget::observed("agent-b", true, false, None),
                ],
                at(10),
            )
            .unwrap();
        rollout.batch_dispatched(&first.agent_ids, at(10)).unwrap();

        rollout.ack_observed("agent-a", at(12)).unwrap();
        let second = rollout
            .plan_next_batch(
                &[ControllerSigningStagedRolloutTarget::observed(
                    "agent-b", true, false, None,
                )],
                at(13),
            )
            .unwrap();
        rollout.batch_dispatched(&second.agent_ids, at(13)).unwrap();
        rollout.ack_observed("agent-b", at(14)).unwrap();

        assert_eq!(first.agent_ids, vec!["agent-a"]);
        assert_eq!(second.agent_ids, vec!["agent-b"]);
        assert_eq!(
            rollout.state(),
            ControllerSigningStagedRolloutState::Completed
        );
    }

    #[test]
    fn staged_rollout_completes_when_all_targets_are_already_current() {
        let mut rollout = ControllerSigningStagedRollout::new(
            vec!["agent-a".to_owned(), "agent-b".to_owned()],
            ControllerSigningStagedRolloutConfig {
                batch_size: 1,
                max_failures: 1,
                ack_timeout: Duration::from_secs(5),
            },
        )
        .unwrap();

        let plan = rollout
            .plan_next_batch(
                &[
                    ControllerSigningStagedRolloutTarget::observed(
                        "agent-a",
                        true,
                        true,
                        Some(at(1)),
                    ),
                    ControllerSigningStagedRolloutTarget::observed(
                        "agent-b",
                        true,
                        true,
                        Some(at(1)),
                    ),
                ],
                at(2),
            )
            .unwrap();
        let error = rollout
            .plan_next_batch(&[], at(3))
            .expect_err("terminal state must reject further planning");

        assert!(plan.agent_ids.is_empty());
        assert_eq!(
            rollout.state(),
            ControllerSigningStagedRolloutState::Completed
        );
        assert_eq!(error, SigningStagedRolloutError::TerminalState);
    }

    #[test]
    fn staged_rollout_rejects_invalid_config_and_terminal_dispatch() {
        let invalid = ControllerSigningStagedRollout::new(
            vec!["agent-a".to_owned()],
            ControllerSigningStagedRolloutConfig {
                batch_size: 0,
                max_failures: 1,
                ack_timeout: Duration::from_secs(5),
            },
        )
        .expect_err("zero batch size should fail");
        let mut rollout = ControllerSigningStagedRollout::new(
            vec!["agent-a".to_owned()],
            ControllerSigningStagedRolloutConfig {
                batch_size: 1,
                max_failures: 1,
                ack_timeout: Duration::from_secs(5),
            },
        )
        .unwrap();
        rollout
            .plan_next_batch(
                &[ControllerSigningStagedRolloutTarget::observed(
                    "agent-a", false, false, None,
                )],
                at(1),
            )
            .unwrap();

        assert_eq!(invalid, SigningStagedRolloutError::InvalidConfig);
        assert_eq!(
            rollout.batch_dispatched(&["agent-a".to_owned()], at(2)),
            Err(SigningStagedRolloutError::TerminalState)
        );
        assert!(
            !format!("{rollout:?}").contains("private_key"),
            "debug output must not contain key-material field names"
        );
    }

    #[test]
    fn staged_rollout_snapshot_roundtrips_in_flight_and_terminal_state() {
        let mut rollout = ControllerSigningStagedRollout::new(
            vec![
                "agent-a".to_owned(),
                "agent-b".to_owned(),
                "agent-c".to_owned(),
            ],
            ControllerSigningStagedRolloutConfig {
                batch_size: 1,
                max_failures: 0,
                ack_timeout: Duration::from_secs(30),
            },
        )
        .unwrap();
        let first = rollout
            .plan_next_batch(
                &[
                    ControllerSigningStagedRolloutTarget::observed(
                        "agent-a",
                        true,
                        true,
                        Some(at(5)),
                    ),
                    ControllerSigningStagedRolloutTarget::observed("agent-b", true, false, None),
                    ControllerSigningStagedRolloutTarget::observed("agent-c", false, false, None),
                ],
                at(10),
            )
            .unwrap();
        rollout.batch_dispatched(&first.agent_ids, at(10)).unwrap();

        let restored = ControllerSigningStagedRollout::from_snapshot(rollout.snapshot()).unwrap();

        assert_eq!(restored.snapshot(), rollout.snapshot());
        assert_eq!(
            restored.state(),
            ControllerSigningStagedRolloutState::WaitingForAck
        );

        let mut failed = restored;
        failed.ack_timeout(at(41)).unwrap();
        let failed_restored =
            ControllerSigningStagedRollout::from_snapshot(failed.snapshot()).unwrap();

        assert_eq!(
            failed_restored.state(),
            ControllerSigningStagedRolloutState::Failed
        );
        assert_eq!(failed_restored.failure_reason_code(), Some("ack_timeout"));
        assert_eq!(failed_restored.snapshot(), failed.snapshot());
    }

    #[test]
    fn staged_rollout_restore_rejects_inconsistent_snapshot() {
        let invalid = ControllerSigningStagedRolloutSnapshot {
            state: ControllerSigningStagedRolloutState::WaitingForAck,
            target_ids: vec!["agent-a".to_owned()],
            config: ControllerSigningStagedRolloutConfig {
                batch_size: 1,
                max_failures: 1,
                ack_timeout: Duration::from_secs(30),
            },
            acknowledged_agent_ids: vec!["agent-a".to_owned()],
            unavailable_agent_ids: Vec::new(),
            failed_agent_ids: Vec::new(),
            in_flight: vec![ControllerSigningStagedRolloutAttemptSnapshot {
                agent_id: "agent-a".to_owned(),
                dispatched_at: at(10),
            }],
            failure_reason_code: None,
        };

        assert_eq!(
            ControllerSigningStagedRollout::from_snapshot(invalid),
            Err(SigningStagedRolloutError::InvalidTransition)
        );
    }

    #[test]
    fn signing_key_rotation_starts_steady_with_active_fingerprint() {
        let rotation = ControllerSigningKeyRotation::steady(fingerprint("old-key-fingerprint"));

        assert_eq!(rotation.state(), SigningKeyRotationState::Steady);
        assert_eq!(
            rotation.current_signing_fingerprint(at(10)).as_str(),
            "old-key-fingerprint"
        );
        assert!(rotation.can_verify_signature_from(
            &fingerprint("old-key-fingerprint"),
            at(1),
            at(10)
        ));
    }

    #[test]
    fn signing_key_rotation_rejects_same_old_and_new_fingerprint() {
        let mut rotation = ControllerSigningKeyRotation::steady(fingerprint("same-fingerprint"));

        let error = rotation
            .request_rotation(fingerprint("same-fingerprint"), at(10), at(30))
            .expect_err("same old/new fingerprint should fail");

        assert_eq!(error, SigningKeyRotationError::FingerprintRoleConflict);
    }

    #[test]
    fn signing_key_rotation_cannot_activate_before_validation() {
        let mut rotation = ControllerSigningKeyRotation::steady(fingerprint("old"));
        rotation
            .request_rotation(fingerprint("new"), at(10), at(40))
            .unwrap();

        let error = rotation
            .activate_dual_trust(at(20))
            .expect_err("activation before validation should fail");

        assert_eq!(
            error,
            SigningKeyRotationError::InvalidTransition {
                from: SigningKeyRotationState::RotationRequested,
                expected: SigningKeyRotationState::NewMaterialValidated,
            }
        );
        assert_eq!(rotation.state(), SigningKeyRotationState::RotationRequested);
    }

    #[test]
    fn signing_key_rotation_new_key_signs_after_dual_trust_activation() {
        let mut rotation = ControllerSigningKeyRotation::steady(fingerprint("old"));
        rotation
            .request_rotation(fingerprint("new"), at(10), at(40))
            .unwrap();
        rotation.validate_new_material(at(11)).unwrap();
        rotation.activate_dual_trust(at(20)).unwrap();

        assert_eq!(rotation.current_signing_fingerprint(at(19)).as_str(), "old");
        assert_eq!(rotation.current_signing_fingerprint(at(20)).as_str(), "new");
        assert!(rotation.can_verify_signature_from(&fingerprint("new"), at(20), at(21)));
    }

    #[test]
    fn signing_key_rotation_old_key_verifies_only_until_expiry() {
        let mut rotation = ControllerSigningKeyRotation::steady(fingerprint("old"));
        rotation
            .request_rotation(fingerprint("new"), at(10), at(40))
            .unwrap();
        rotation.validate_new_material(at(12)).unwrap();
        rotation.activate_dual_trust(at(20)).unwrap();

        assert!(rotation.can_verify_signature_from(&fingerprint("old"), at(19), at(40)));
        assert!(!rotation.can_verify_signature_from(&fingerprint("old"), at(19), at(41)));
        assert!(!rotation.can_verify_signature_from(&fingerprint("old"), at(20), at(21)));
    }

    #[test]
    fn signing_key_rotation_cannot_retire_old_key_before_guard() {
        let mut rotation = ControllerSigningKeyRotation::steady(fingerprint("old"));
        rotation
            .request_rotation(fingerprint("new"), at(10), at(40))
            .unwrap();
        rotation.validate_new_material(at(12)).unwrap();
        rotation.activate_dual_trust(at(20)).unwrap();

        let error = rotation
            .retire_old_key(at(39))
            .expect_err("old key cannot retire before trust expiry");

        assert_eq!(error, SigningKeyRotationError::RetirementGuardNotSatisfied);
        assert_eq!(rotation.state(), SigningKeyRotationState::DualTrustActive);
        rotation.retire_old_key(at(40)).unwrap();
        assert_eq!(rotation.state(), SigningKeyRotationState::OldKeyRetired);
        assert!(!rotation.can_verify_signature_from(&fingerprint("old"), at(19), at(40)));
    }

    #[test]
    fn signing_key_rotation_errors_are_redacted() {
        let error = SigningKeyFingerprint::new("private-key-secret\nvalue")
            .expect_err("invalid fingerprint should fail");

        let message = error.to_string();
        assert!(!message.contains("private-key-secret"));
        assert!(!message.contains("value"));
    }
}
