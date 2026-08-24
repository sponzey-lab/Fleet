use crate::{Selector, SelectorError};
use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub source: String,
    pub selector: Selector,
    pub checks: Vec<PolicyCheck>,
    pub remediation: Option<PolicyRemediation>,
    pub schedule: Option<DriftSchedule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyCheck {
    Service {
        id: String,
        name: String,
        state: ServiceState,
    },
    Package {
        id: String,
        name: String,
        present: bool,
    },
    FileChecksum {
        id: String,
        path: String,
        sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceState {
    Running,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftStatus {
    Compliant,
    Drifted,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftSeverity {
    None,
    Warning,
    Critical,
    Unknown,
}

impl DriftSeverity {
    pub fn for_status(status: DriftStatus) -> Self {
        match status {
            DriftStatus::Compliant => Self::None,
            DriftStatus::Drifted => Self::Warning,
            DriftStatus::Unknown => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftAcknowledgement {
    Open,
    Acknowledged { by: String, at: SystemTime },
    Resolved { job_id: String, at: SystemTime },
}

impl DriftAcknowledgement {
    pub fn is_acknowledged(&self) -> bool {
        matches!(self, Self::Acknowledged { .. } | Self::Resolved { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftReport {
    pub policy_name: String,
    pub status: DriftStatus,
    pub severity: DriftSeverity,
    pub acknowledgement: DriftAcknowledgement,
    pub expected: String,
    pub actual: String,
}

impl DriftReport {
    pub fn drifted(
        policy_name: impl Into<String>,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> Self {
        Self {
            policy_name: policy_name.into(),
            status: DriftStatus::Drifted,
            severity: DriftSeverity::Warning,
            acknowledgement: DriftAcknowledgement::Open,
            expected: expected.into(),
            actual: actual.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRemediation {
    pub runbook_ref: String,
    pub approval_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemediationPlan {
    pub runbook_ref: String,
    pub approval_required: bool,
    pub risk_summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemediationStatus {
    Proposed,
    PendingApproval,
    Approved,
    JobCreated,
    Running,
    SucceededPendingVerify,
    Resolved,
    Failed,
    Rejected,
    Expired,
    Canceled,
}

impl RemediationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::PendingApproval => "pending_approval",
            Self::Approved => "approved",
            Self::JobCreated => "job_created",
            Self::Running => "running",
            Self::SucceededPendingVerify => "succeeded_pending_verify",
            Self::Resolved => "resolved",
            Self::Failed => "failed",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
            Self::Canceled => "canceled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "proposed" => Some(Self::Proposed),
            "pending_approval" => Some(Self::PendingApproval),
            "approved" => Some(Self::Approved),
            "job_created" => Some(Self::JobCreated),
            "running" => Some(Self::Running),
            "succeeded_pending_verify" => Some(Self::SucceededPendingVerify),
            "resolved" => Some(Self::Resolved),
            "failed" => Some(Self::Failed),
            "rejected" => Some(Self::Rejected),
            "expired" => Some(Self::Expired),
            "canceled" => Some(Self::Canceled),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Resolved | Self::Failed | Self::Rejected | Self::Expired | Self::Canceled
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemediationResult {
    pub job_id: String,
    pub status: RemediationStatus,
    pub evidence: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriftSchedule {
    pub interval: Duration,
}

impl DriftSchedule {
    pub fn new(interval: Duration) -> Result<Self, PolicyParseError> {
        if interval.is_zero() {
            Err(PolicyParseError::InvalidSchedule(
                "interval must be positive".to_owned(),
            ))
        } else {
            Ok(Self { interval })
        }
    }

    pub fn next_due_after(self, checked_at: SystemTime) -> SystemTime {
        checked_at + self.interval
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyAssignment {
    pub policy_id: String,
    pub target: PolicyAssignmentTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyAssignmentTarget {
    Agent(String),
    Selector(Selector),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledDriftCheck {
    pub policy_id: String,
    pub agent_id: String,
    pub due_at: SystemTime,
    pub missed: bool,
}

pub fn scheduled_drift_due(
    policy_id: impl Into<String>,
    agent_id: impl Into<String>,
    next_due_at: SystemTime,
    now: SystemTime,
    grace: Duration,
) -> Option<ScheduledDriftCheck> {
    if now < next_due_at {
        return None;
    }
    Some(ScheduledDriftCheck {
        policy_id: policy_id.into(),
        agent_id: agent_id.into(),
        due_at: next_due_at,
        missed: now
            .duration_since(next_due_at)
            .map(|duration| duration > grace)
            .unwrap_or(false),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemediationRequest {
    pub id: String,
    pub policy_id: String,
    pub policy_name: String,
    pub agent_id: String,
    pub runbook_ref: String,
    pub approval_required: bool,
    pub status: RemediationStatus,
    pub risk_summary: String,
    pub job_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemediationError {
    MissingRemediation,
    ApprovalRequired,
    DriftNotEligible,
    InvalidTransition {
        from: RemediationStatus,
        event: &'static str,
    },
}

impl RemediationRequest {
    pub fn from_policy(
        policy: &Policy,
        agent_id: impl Into<String>,
    ) -> Result<Self, PolicyParseError> {
        let agent_id = agent_id.into();
        let remediation = policy
            .remediation
            .as_ref()
            .ok_or(PolicyParseError::MissingField("remediation"))?;
        if !remediation.approval_required {
            return Err(PolicyParseError::RemediationRequiresApproval);
        }
        Ok(Self {
            id: format!("remediation-{}-{agent_id}", policy.id),
            policy_id: policy.id.clone(),
            policy_name: policy.name.clone(),
            agent_id,
            runbook_ref: remediation.runbook_ref.clone(),
            approval_required: true,
            status: RemediationStatus::Proposed,
            risk_summary: "policy remediation requires approval".to_owned(),
            job_id: None,
        })
    }

    pub fn propose_from_drift(
        id: impl Into<String>,
        policy: &Policy,
        agent_id: impl Into<String>,
        drift: &DriftReport,
    ) -> Result<Self, RemediationError> {
        if drift.status != DriftStatus::Drifted || drift.policy_name != policy.name {
            return Err(RemediationError::DriftNotEligible);
        }
        let remediation = policy
            .remediation
            .as_ref()
            .ok_or(RemediationError::MissingRemediation)?;
        if !remediation.approval_required {
            return Err(RemediationError::ApprovalRequired);
        }
        Ok(Self {
            id: id.into(),
            policy_id: policy.id.clone(),
            policy_name: policy.name.clone(),
            agent_id: agent_id.into(),
            runbook_ref: remediation.runbook_ref.clone(),
            approval_required: true,
            status: RemediationStatus::Proposed,
            risk_summary: format!(
                "drifted policy {} requires approved runbook remediation",
                policy.id
            ),
            job_id: None,
        })
    }

    pub fn plan(&self) -> RemediationPlan {
        RemediationPlan {
            runbook_ref: self.runbook_ref.clone(),
            approval_required: self.approval_required,
            risk_summary: self.risk_summary.clone(),
        }
    }

    pub fn request_approval(&mut self) -> Result<(), RemediationError> {
        self.transition(
            "request_approval",
            &[RemediationStatus::Proposed],
            RemediationStatus::PendingApproval,
        )
    }

    pub fn approve(&mut self) -> Result<(), RemediationError> {
        self.transition(
            "approve",
            &[RemediationStatus::PendingApproval],
            RemediationStatus::Approved,
        )
    }

    pub fn create_job(&mut self, job_id: impl Into<String>) -> Result<(), RemediationError> {
        self.transition(
            "create_job",
            &[RemediationStatus::Approved],
            RemediationStatus::JobCreated,
        )?;
        self.job_id = Some(job_id.into());
        Ok(())
    }

    pub fn mark_running(&mut self) -> Result<(), RemediationError> {
        self.transition(
            "mark_running",
            &[RemediationStatus::JobCreated],
            RemediationStatus::Running,
        )
    }

    pub fn job_succeeded(&mut self) -> Result<(), RemediationError> {
        self.transition(
            "job_succeeded",
            &[RemediationStatus::Running],
            RemediationStatus::SucceededPendingVerify,
        )
    }

    pub fn verify_resolved(&mut self) -> Result<RemediationResult, RemediationError> {
        self.transition(
            "verify_resolved",
            &[RemediationStatus::SucceededPendingVerify],
            RemediationStatus::Resolved,
        )?;
        Ok(RemediationResult {
            job_id: self.job_id.clone().unwrap_or_default(),
            status: self.status,
            evidence: "drift evidence verified".to_owned(),
        })
    }

    pub fn reject(&mut self) -> Result<(), RemediationError> {
        self.transition(
            "reject",
            &[
                RemediationStatus::Proposed,
                RemediationStatus::PendingApproval,
            ],
            RemediationStatus::Rejected,
        )
    }

    pub fn expire(&mut self) -> Result<(), RemediationError> {
        self.transition(
            "expire",
            &[
                RemediationStatus::Proposed,
                RemediationStatus::PendingApproval,
            ],
            RemediationStatus::Expired,
        )
    }

    pub fn cancel(&mut self) -> Result<(), RemediationError> {
        self.transition(
            "cancel",
            &[
                RemediationStatus::Proposed,
                RemediationStatus::PendingApproval,
                RemediationStatus::Approved,
                RemediationStatus::JobCreated,
                RemediationStatus::Running,
            ],
            RemediationStatus::Canceled,
        )
    }

    pub fn mark_failed(&mut self) -> Result<(), RemediationError> {
        self.transition(
            "fail",
            &[
                RemediationStatus::JobCreated,
                RemediationStatus::Running,
                RemediationStatus::SucceededPendingVerify,
            ],
            RemediationStatus::Failed,
        )
    }

    fn transition(
        &mut self,
        event: &'static str,
        allowed_from: &[RemediationStatus],
        to: RemediationStatus,
    ) -> Result<(), RemediationError> {
        if self.status.is_terminal() || !allowed_from.contains(&self.status) {
            return Err(RemediationError::InvalidTransition {
                from: self.status,
                event,
            });
        }
        self.status = to;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyParseError {
    MissingField(&'static str),
    UnsupportedKind(String),
    UnsupportedCheck(String),
    UnsupportedServiceState(String),
    InvalidChecksum(String),
    InvalidVersion(String),
    InvalidSchedule(String),
    InvalidSelector(String),
    RemediationRequiresApproval,
}

impl Display for PolicyParseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField(field) => {
                write!(formatter, "policy missing required field: {field}")
            }
            Self::UnsupportedKind(kind) => write!(formatter, "unsupported policy kind: {kind}"),
            Self::UnsupportedCheck(check) => write!(formatter, "unsupported policy check: {check}"),
            Self::UnsupportedServiceState(state) => {
                write!(formatter, "unsupported service state: {state}")
            }
            Self::InvalidChecksum(value) => write!(formatter, "invalid file sha256: {value}"),
            Self::InvalidVersion(value) => write!(formatter, "invalid policy version: {value}"),
            Self::InvalidSchedule(value) => write!(formatter, "invalid drift schedule: {value}"),
            Self::InvalidSelector(selector) => {
                write!(formatter, "invalid policy selector: {selector}")
            }
            Self::RemediationRequiresApproval => {
                write!(
                    formatter,
                    "policy remediation requires approvalRequired: true in MVP"
                )
            }
        }
    }
}

impl std::error::Error for PolicyParseError {}

impl From<SelectorError> for PolicyParseError {
    fn from(value: SelectorError) -> Self {
        Self::InvalidSelector(value.to_string())
    }
}

pub fn parse_policy_document(body: &str) -> Result<Policy, PolicyParseError> {
    let mut api_version = None;
    let mut kind = None;
    let mut id = None;
    let mut name = None;
    let mut version = 1;
    let mut selector_labels = BTreeMap::new();
    let mut checks = Vec::new();
    let mut current_check: Option<CheckBuilder> = None;
    let mut in_match_labels = false;
    let mut in_checks = false;
    let mut in_remediation = false;
    let mut in_schedule = false;
    let mut remediation_declared = false;
    let mut remediation_runbook_ref = None;
    let mut remediation_approval_required = false;
    let mut schedule = None;

    for raw_line in body.lines() {
        let without_comment = raw_line.split('#').next().unwrap_or_default();
        if without_comment.trim().is_empty() {
            continue;
        }
        let indent = without_comment
            .chars()
            .take_while(|character| *character == ' ')
            .count();
        let line = without_comment.trim();

        if indent == 0 {
            in_match_labels = false;
            in_checks = line == "checks:" || line == "spec:";
            in_remediation = line == "remediation:";
            remediation_declared |= in_remediation;
            in_schedule = false;
        }

        if let Some(value) = scalar_value(line, "apiVersion") {
            api_version = Some(value.to_owned());
            continue;
        }
        if let Some(value) = scalar_value(line, "kind") {
            kind = Some(value.to_owned());
            continue;
        }
        if indent >= 2 && line == "matchLabels:" {
            in_match_labels = true;
            continue;
        }
        if indent >= 2 && line == "checks:" {
            in_checks = true;
            in_match_labels = false;
            in_schedule = false;
            continue;
        }
        if indent >= 2 && line == "remediation:" {
            in_remediation = true;
            remediation_declared = true;
            in_match_labels = false;
            in_checks = false;
            in_schedule = false;
            continue;
        }
        if indent >= 2 && line == "schedule:" {
            in_schedule = true;
            in_remediation = false;
            in_match_labels = false;
            in_checks = false;
            continue;
        }
        if indent >= 2 && matches!(line, "approvalRequired: true") {
            remediation_approval_required = true;
            continue;
        }
        if indent >= 2 && matches!(line, "approvalRequired: false") {
            remediation_approval_required = false;
            continue;
        }
        if indent >= 2
            && id.is_none()
            && let Some(value) = scalar_value(line, "id")
        {
            id = Some(value.to_owned());
            continue;
        }
        if indent >= 2
            && name.is_none()
            && let Some(value) = scalar_value(line, "name")
        {
            name = Some(value.to_owned());
            continue;
        }
        if indent >= 2
            && let Some(value) = scalar_value(line, "version")
        {
            version = parse_positive_u32(value)
                .map_err(|_| PolicyParseError::InvalidVersion(value.to_owned()))?;
            continue;
        }
        if in_remediation
            && indent >= 2
            && let Some(value) = scalar_value(line, "runbookRef")
        {
            remediation_runbook_ref = Some(value.to_owned());
            continue;
        }
        if in_schedule
            && indent >= 2
            && let Some(value) = scalar_value(line, "intervalSeconds")
        {
            let seconds = parse_positive_u64(value)
                .map_err(|_| PolicyParseError::InvalidSchedule(value.to_owned()))?;
            schedule = Some(DriftSchedule::new(Duration::from_secs(seconds))?);
            continue;
        }

        if in_match_labels
            && indent >= 6
            && let Some((key, value)) = line.split_once(':')
        {
            selector_labels.insert(key.trim().to_owned(), value.trim().to_owned());
            continue;
        }

        if in_checks && indent >= 4 {
            if let Some(value) = line.strip_prefix("- id:") {
                if let Some(builder) = current_check.take() {
                    checks.push(builder.build()?);
                }
                current_check = Some(CheckBuilder::new(value.trim()));
                continue;
            }
            if let Some(builder) = current_check.as_mut() {
                match line {
                    "service:" => builder.kind = Some("service".to_owned()),
                    "package:" => builder.kind = Some("package".to_owned()),
                    "file:" => builder.kind = Some("file".to_owned()),
                    value if value.ends_with(':') => {
                        let key = value.trim_end_matches(':').trim();
                        if matches!(key, "name" | "state" | "path" | "sha256") {
                            builder.fields.insert(key.to_owned(), String::new());
                        } else {
                            builder.kind = Some(key.to_owned());
                        }
                    }
                    _ => {
                        if let Some((key, value)) = line.split_once(':') {
                            builder
                                .fields
                                .insert(key.trim().to_owned(), value.trim().to_owned());
                        }
                    }
                }
            }
        }
    }

    if let Some(builder) = current_check.take() {
        checks.push(builder.build()?);
    }
    if remediation_declared && !remediation_approval_required {
        return Err(PolicyParseError::RemediationRequiresApproval);
    }

    let _api_version = api_version.ok_or(PolicyParseError::MissingField("apiVersion"))?;
    let kind = kind.ok_or(PolicyParseError::MissingField("kind"))?;
    if kind != "Policy" {
        return Err(PolicyParseError::UnsupportedKind(kind));
    }
    let name = name.ok_or(PolicyParseError::MissingField("metadata.name"))?;
    let id = id.unwrap_or_else(|| name.clone());
    if selector_labels.is_empty() {
        return Err(PolicyParseError::MissingField("spec.selector.matchLabels"));
    }
    if checks.is_empty() {
        return Err(PolicyParseError::MissingField("spec.checks"));
    }
    let selector = Selector::parse(
        &selector_labels
            .into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(","),
    )?;

    Ok(Policy {
        id,
        name,
        version,
        source: body.to_owned(),
        selector,
        checks,
        remediation: remediation_runbook_ref.map(|runbook_ref| PolicyRemediation {
            runbook_ref,
            approval_required: remediation_approval_required,
        }),
        schedule,
    })
}

fn scalar_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.strip_prefix(key)?
        .strip_prefix(':')
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn parse_positive_u32(value: &str) -> Result<u32, ()> {
    let parsed = value.parse::<u32>().map_err(|_| ())?;
    if parsed == 0 { Err(()) } else { Ok(parsed) }
}

fn parse_positive_u64(value: &str) -> Result<u64, ()> {
    let parsed = value.parse::<u64>().map_err(|_| ())?;
    if parsed == 0 { Err(()) } else { Ok(parsed) }
}

struct CheckBuilder {
    id: String,
    kind: Option<String>,
    fields: BTreeMap<String, String>,
}

impl CheckBuilder {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            kind: None,
            fields: BTreeMap::new(),
        }
    }

    fn build(self) -> Result<PolicyCheck, PolicyParseError> {
        match self.kind.as_deref() {
            Some("service") => {
                let name = self
                    .fields
                    .get("name")
                    .ok_or(PolicyParseError::MissingField("service.name"))?
                    .to_owned();
                let state = self
                    .fields
                    .get("state")
                    .ok_or(PolicyParseError::MissingField("service.state"))?;
                if state != "running" {
                    return Err(PolicyParseError::UnsupportedServiceState(state.to_owned()));
                }
                Ok(PolicyCheck::Service {
                    id: self.id,
                    name,
                    state: ServiceState::Running,
                })
            }
            Some("package") => {
                let name = self
                    .fields
                    .get("name")
                    .ok_or(PolicyParseError::MissingField("package.name"))?
                    .to_owned();
                let present = self
                    .fields
                    .get("state")
                    .is_none_or(|value| value == "present");
                Ok(PolicyCheck::Package {
                    id: self.id,
                    name,
                    present,
                })
            }
            Some("file") => {
                let path = self
                    .fields
                    .get("path")
                    .ok_or(PolicyParseError::MissingField("file.path"))?
                    .to_owned();
                let sha256 = self
                    .fields
                    .get("sha256")
                    .ok_or(PolicyParseError::MissingField("file.sha256"))?
                    .to_owned();
                validate_file_sha256(&sha256)?;
                Ok(PolicyCheck::FileChecksum {
                    id: self.id,
                    path,
                    sha256,
                })
            }
            Some(kind) => Err(PolicyParseError::UnsupportedCheck(kind.to_owned())),
            None => Err(PolicyParseError::MissingField("check kind")),
        }
    }
}

fn validate_file_sha256(value: &str) -> Result<(), PolicyParseError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err(PolicyParseError::InvalidChecksum(
            "file.sha256 must be 64 lowercase hex characters".to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NGINX_RUNNING: &str = r#"
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

    #[test]
    fn parses_valid_service_policy() {
        let policy = parse_policy_document(NGINX_RUNNING).unwrap();

        assert_eq!(policy.id, "nginx-running");
        assert_eq!(policy.name, "nginx-running");
        assert_eq!(policy.version, 1);
        assert!(policy.source.contains("kind: Policy"));
        assert!(matches!(policy.selector, Selector::Labels(_)));
        assert!(matches!(
            policy.checks[0],
            PolicyCheck::Service {
                ref name,
                state: ServiceState::Running,
                ..
            } if name == "nginx"
        ));
    }

    #[test]
    fn rejects_unsupported_check() {
        let body = NGINX_RUNNING.replace("service:", "shell:");

        assert!(matches!(
            parse_policy_document(&body),
            Err(PolicyParseError::UnsupportedCheck(_))
        ));
    }

    #[test]
    fn rejects_invalid_selector() {
        let body = NGINX_RUNNING.replace("role: web", "bad key: web");

        assert!(matches!(
            parse_policy_document(&body),
            Err(PolicyParseError::InvalidSelector(_))
        ));
    }

    #[test]
    fn rejects_remediation_without_approval() {
        let body = format!("{NGINX_RUNNING}\nremediation:\n  run: restart nginx\n");

        assert!(matches!(
            parse_policy_document(&body),
            Err(PolicyParseError::RemediationRequiresApproval)
        ));
    }

    #[test]
    fn parses_policy_identity_schedule_and_remediation_reference() {
        let body = r#"
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Policy
metadata:
  id: policy-nginx-running
  name: nginx-running
  version: 2
spec:
  selector:
    matchLabels:
      role: web
  schedule:
    intervalSeconds: 300
  checks:
    - id: nginx-service
      service:
        name: nginx
        state: running
  remediation:
    runbookRef: runbooks/nginx-remediate.yml
    approvalRequired: true
"#;

        let policy = parse_policy_document(body).unwrap();

        assert_eq!(policy.id, "policy-nginx-running");
        assert_eq!(policy.version, 2);
        assert_eq!(
            policy.schedule.map(|schedule| schedule.interval),
            Some(Duration::from_secs(300))
        );
        assert_eq!(
            policy
                .remediation
                .as_ref()
                .map(|value| value.runbook_ref.as_str()),
            Some("runbooks/nginx-remediate.yml")
        );
    }

    #[test]
    fn scheduled_drift_marks_missed_after_grace() {
        let due_at = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let not_due = scheduled_drift_due(
            "policy-1",
            "agent-1",
            due_at,
            SystemTime::UNIX_EPOCH + Duration::from_secs(99),
            Duration::from_secs(10),
        );
        assert_eq!(not_due, None);

        let due = scheduled_drift_due(
            "policy-1",
            "agent-1",
            due_at,
            SystemTime::UNIX_EPOCH + Duration::from_secs(111),
            Duration::from_secs(10),
        )
        .unwrap();

        assert_eq!(due.policy_id, "policy-1");
        assert!(due.missed);
    }

    #[test]
    fn remediation_request_requires_approval() {
        let policy = parse_policy_document(&format!(
            "{NGINX_RUNNING}\nremediation:\n  runbookRef: fix-nginx.yml\n  approvalRequired: true\n"
        ))
        .unwrap();

        let request = RemediationRequest::from_policy(&policy, "agent-1").unwrap();

        assert_eq!(request.policy_id, "nginx-running");
        assert_eq!(request.agent_id, "agent-1");
        assert!(request.approval_required);
    }

    #[test]
    fn remediation_request_requires_drifted_policy_with_remediation() {
        let policy = parse_policy_document(&format!(
            "{NGINX_RUNNING}\nremediation:\n  runbookRef: fix-nginx.yml\n  approvalRequired: true\n"
        ))
        .unwrap();
        let compliant = DriftReport {
            policy_name: "nginx-running".to_owned(),
            status: DriftStatus::Compliant,
            severity: DriftSeverity::None,
            acknowledgement: DriftAcknowledgement::Open,
            expected: "service nginx running".to_owned(),
            actual: "active".to_owned(),
        };
        let unknown = DriftReport {
            policy_name: "nginx-running".to_owned(),
            status: DriftStatus::Unknown,
            severity: DriftSeverity::Unknown,
            acknowledgement: DriftAcknowledgement::Open,
            expected: "service nginx running".to_owned(),
            actual: "systemd unavailable".to_owned(),
        };
        let drifted = DriftReport::drifted("nginx-running", "expected", "actual");
        let no_remediation = parse_policy_document(NGINX_RUNNING).unwrap();

        assert!(matches!(
            RemediationRequest::propose_from_drift("rem-1", &policy, "agent-1", &compliant),
            Err(RemediationError::DriftNotEligible)
        ));
        assert!(matches!(
            RemediationRequest::propose_from_drift("rem-1", &policy, "agent-1", &unknown),
            Err(RemediationError::DriftNotEligible)
        ));
        assert!(matches!(
            RemediationRequest::propose_from_drift("rem-1", &no_remediation, "agent-1", &drifted),
            Err(RemediationError::MissingRemediation)
        ));
    }

    #[test]
    fn remediation_request_state_machine_rejects_invalid_transitions() {
        let policy = parse_policy_document(&format!(
            "{NGINX_RUNNING}\nremediation:\n  runbookRef: fix-nginx.yml\n  approvalRequired: true\n"
        ))
        .unwrap();
        let drifted = DriftReport::drifted("nginx-running", "expected", "actual");
        let mut request =
            RemediationRequest::propose_from_drift("rem-1", &policy, "agent-1", &drifted).unwrap();

        assert_eq!(request.status, RemediationStatus::Proposed);
        assert!(matches!(
            request.create_job("job-1"),
            Err(RemediationError::InvalidTransition { .. })
        ));

        request.request_approval().unwrap();
        assert_eq!(request.status, RemediationStatus::PendingApproval);
        request.approve().unwrap();
        request.create_job("job-1").unwrap();
        request.mark_running().unwrap();
        request.job_succeeded().unwrap();
        request.verify_resolved().unwrap();
        assert_eq!(request.status, RemediationStatus::Resolved);
        assert_eq!(request.job_id.as_deref(), Some("job-1"));
        assert!(matches!(
            request.reject(),
            Err(RemediationError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn remediation_status_parse_roundtrips_persisted_values() {
        for status in [
            RemediationStatus::Proposed,
            RemediationStatus::PendingApproval,
            RemediationStatus::Approved,
            RemediationStatus::JobCreated,
            RemediationStatus::Running,
            RemediationStatus::SucceededPendingVerify,
            RemediationStatus::Resolved,
            RemediationStatus::Failed,
            RemediationStatus::Rejected,
            RemediationStatus::Expired,
            RemediationStatus::Canceled,
        ] {
            assert_eq!(RemediationStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(RemediationStatus::parse("unknown"), None);
    }

    #[test]
    fn parses_file_checksum_check() {
        let body = r#"
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Policy
metadata:
  name: file-check
spec:
  selector:
    matchLabels:
      role: web
  checks:
    - id: config-file
      file:
        path: /etc/nginx/nginx.conf
        sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
"#;

        let policy = parse_policy_document(body).unwrap();

        assert!(matches!(
            policy.checks[0],
            PolicyCheck::FileChecksum { ref path, ref sha256, .. }
                if path == "/etc/nginx/nginx.conf"
                    && sha256 == "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ));
    }

    #[test]
    fn rejects_invalid_file_checksum_values() {
        for invalid in [
            "",
            "abc123",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        ] {
            let body = format!(
                r#"
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Policy
metadata:
  name: file-check
spec:
  selector:
    matchLabels:
      role: web
  checks:
    - id: config-file
      file:
        path: /etc/nginx/nginx.conf
        sha256: {invalid}
"#
            );

            assert!(matches!(
                parse_policy_document(&body),
                Err(PolicyParseError::InvalidChecksum(_))
            ));
        }
    }
}
