use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditCategory {
    Agent,
    Enrollment,
    Job,
    Approval,
    Drift,
    Policy,
    Security,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditActor(String);

impl AuditActor {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditTarget(String);

impl AuditTarget {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditValue {
    Plain(String),
    SecretRef(String),
    Redacted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    pub category: AuditCategory,
    pub action: String,
    pub actor: AuditActor,
    pub target: AuditTarget,
    pub value: AuditValue,
    pub occurred_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    pub id: u64,
    pub event: AuditEvent,
}

impl AuditEvent {
    pub fn security(action: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            category: AuditCategory::Security,
            action: action.into(),
            actor: AuditActor::new("system"),
            target: AuditTarget::new(target),
            value: AuditValue::Redacted,
            occurred_at: SystemTime::UNIX_EPOCH,
        }
    }

    pub fn contains_secret_plaintext(&self) -> bool {
        matches!(self.value, AuditValue::Plain(ref value) if value.contains("token=") || value.contains("secret="))
    }
}

impl AuditCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Enrollment => "enrollment",
            Self::Job => "job",
            Self::Approval => "approval",
            Self::Drift => "drift",
            Self::Policy => "policy",
            Self::Security => "security",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "agent" => Some(Self::Agent),
            "enrollment" => Some(Self::Enrollment),
            "job" => Some(Self::Job),
            "approval" => Some(Self::Approval),
            "drift" => Some(Self::Drift),
            "policy" => Some(Self::Policy),
            "security" => Some(Self::Security),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_event_does_not_need_secret_plaintext() {
        let event = AuditEvent::security("invalid_signature", "agent-1");
        assert!(!event.contains_secret_plaintext());
    }

    #[test]
    fn creates_security_audit_event() {
        let event = AuditEvent::security("invalid_signature", "agent-1");
        assert_eq!(event.category, AuditCategory::Security);
    }
}
