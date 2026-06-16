use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

pub const PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrelationId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireMessage {
    pub protocol_version: u16,
    pub message_id: MessageId,
    pub correlation_id: CorrelationId,
    pub agent_id: Option<String>,
    pub timestamp_ms: u64,
    pub payload: WirePayload,
}

impl WireMessage {
    pub fn new(
        message_id: impl Into<String>,
        correlation_id: impl Into<String>,
        agent_id: Option<String>,
        timestamp_ms: u64,
        payload: WirePayload,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            message_id: MessageId(message_id.into()),
            correlation_id: CorrelationId(correlation_id.into()),
            agent_id,
            timestamp_ms,
            payload,
        }
    }

    pub fn validate_version(&self) -> Result<(), ProtocolError> {
        if self.protocol_version == PROTOCOL_VERSION {
            Ok(())
        } else {
            Err(ProtocolError::VersionMismatch {
                expected: PROTOCOL_VERSION,
                actual: self.protocol_version,
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum WirePayload {
    EnrollRequest {
        agent_name: String,
        token: String,
        public_key: String,
        fingerprint: String,
        labels: Vec<WireLabel>,
    },
    EnrollResponse {
        agent_id: String,
        controller_public_key: String,
        controller_fingerprint: String,
    },
    AgentHello {
        agent_id: String,
        fingerprint: String,
    },
    AuthChallenge {
        nonce: String,
    },
    AuthResponse {
        nonce: String,
        signature: String,
    },
    AuthAccepted,
    Heartbeat {
        agent_id: String,
        status: String,
    },
    TaskAssignment {
        envelope: SignedTaskEnvelopeWire,
        task: TaskWire,
    },
    TaskAck {
        job_id: String,
        task_id: String,
    },
    TaskStarted {
        job_id: String,
        task_id: String,
    },
    TaskRejected {
        job_id: String,
        task_id: String,
        reason_code: TaskRejectionReasonCode,
        reason: String,
    },
    TaskCancel {
        job_id: String,
        task_id: String,
        reason: String,
    },
    OutputChunk {
        job_id: String,
        task_id: String,
        stream: OutputStream,
        sequence: u64,
        data: String,
    },
    TaskResult {
        job_id: String,
        task_id: String,
        exit_code: i32,
        #[serde(default)]
        status: Option<TaskResultStatus>,
        #[serde(default)]
        reason: String,
    },
    SecurityEvent {
        agent_id: String,
        action: String,
        detail: String,
    },
    FactsSnapshot {
        agent_id: String,
        body: String,
    },
    MetricsSnapshot {
        agent_id: String,
        body: String,
    },
    LogChunk {
        agent_id: String,
        line: String,
    },
    DriftReport {
        agent_id: String,
        status: String,
        expected: String,
        actual: String,
    },
}

impl WirePayload {
    pub fn channel(&self) -> ProtocolChannel {
        match self {
            Self::EnrollRequest { .. }
            | Self::EnrollResponse { .. }
            | Self::AgentHello { .. }
            | Self::AuthChallenge { .. }
            | Self::AuthResponse { .. }
            | Self::AuthAccepted
            | Self::Heartbeat { .. } => ProtocolChannel::AuthSession,
            Self::TaskAssignment { .. }
            | Self::TaskAck { .. }
            | Self::TaskStarted { .. }
            | Self::TaskRejected { .. }
            | Self::TaskCancel { .. }
            | Self::OutputChunk { .. }
            | Self::TaskResult { .. }
            | Self::SecurityEvent { .. }
            | Self::FactsSnapshot { .. }
            | Self::MetricsSnapshot { .. }
            | Self::LogChunk { .. }
            | Self::DriftReport { .. } => ProtocolChannel::TaskData,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolChannel {
    AuthSession,
    TaskData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireLabel {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedTaskEnvelopeWire {
    pub job_id: String,
    pub task_id: String,
    pub target_agent_id: String,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub nonce: String,
    pub payload_hash: String,
    pub signature: String,
}

impl SignedTaskEnvelopeWire {
    pub fn targets_agent(&self, agent_id: &str) -> bool {
        self.target_agent_id == agent_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum TaskWire {
    Command(CommandTaskWire),
    DriftCheck(DriftCheckTaskWire),
    RunbookExecution(RunbookExecutionTaskWire),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandTaskWire {
    pub program: String,
    pub args: Vec<String>,
    pub timeout_ms: u64,
    pub max_output_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriftCheckTaskWire {
    pub policy_document: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunbookExecutionTaskWire {
    pub runbook_document: String,
    pub timeout_ms: u64,
    pub confirmed_high_risk: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskRejectionReasonCode {
    AgentBusy,
    InvalidSignature,
    Expired,
    Replay,
    TargetMismatch,
    InvalidTask,
    CapabilityUnsupported,
    LocalPolicy,
    InternalError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskResultStatus {
    Succeeded,
    Failed,
    Canceled,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    Json(String),
    VersionMismatch { expected: u16, actual: u16 },
}

impl Display for ProtocolError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "protocol json error: {error}"),
            Self::VersionMismatch { expected, actual } => {
                write!(
                    formatter,
                    "protocol version mismatch: expected {expected}, actual {actual}"
                )
            }
        }
    }
}

impl std::error::Error for ProtocolError {}

pub fn encode_message(message: &WireMessage) -> Result<String, ProtocolError> {
    message.validate_version()?;
    serde_json::to_string(message).map_err(|error| ProtocolError::Json(error.to_string()))
}

pub fn decode_message(body: &str) -> Result<WireMessage, ProtocolError> {
    let message: WireMessage =
        serde_json::from_str(body).map_err(|error| ProtocolError::Json(error.to_string()))?;
    message.validate_version()?;
    Ok(message)
}

pub fn protocol_version() -> u16 {
    PROTOCOL_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heartbeat() -> WireMessage {
        WireMessage::new(
            "msg-1",
            "corr-1",
            Some("agent-1".to_owned()),
            1,
            WirePayload::Heartbeat {
                agent_id: "agent-1".to_owned(),
                status: "online".to_owned(),
            },
        )
    }

    #[test]
    fn exposes_protocol_version() {
        assert_eq!(protocol_version(), 1);
    }

    #[test]
    fn serializes_and_deserializes_wire_message() {
        let encoded = encode_message(&heartbeat()).unwrap();
        let decoded = decode_message(&encoded).unwrap();

        assert_eq!(decoded, heartbeat());
        assert!(encoded.contains("\"protocol_version\":1"));
        assert!(encoded.contains("\"heartbeat\""));
    }

    #[test]
    fn rejects_malformed_payload() {
        assert!(matches!(
            decode_message("{not-json"),
            Err(ProtocolError::Json(_))
        ));
    }

    #[test]
    fn rejects_unknown_message_type() {
        let body = r#"{
            "protocol_version": 1,
            "message_id": "msg-1",
            "correlation_id": "corr-1",
            "agent_id": "agent-1",
            "timestamp_ms": 1,
            "payload": {
                "type": "unknown_message",
                "payload": {}
            }
        }"#;

        assert!(matches!(decode_message(body), Err(ProtocolError::Json(_))));
    }

    #[test]
    fn rejects_protocol_version_mismatch() {
        let mut message = heartbeat();
        message.protocol_version = 999;

        assert_eq!(
            encode_message(&message),
            Err(ProtocolError::VersionMismatch {
                expected: 1,
                actual: 999,
            })
        );
    }

    #[test]
    fn auth_challenge_roundtrips() {
        let message = WireMessage::new(
            "msg-auth",
            "corr-auth",
            Some("agent-1".to_owned()),
            1,
            WirePayload::AuthChallenge {
                nonce: "nonce-1".to_owned(),
            },
        );

        assert_eq!(
            decode_message(&encode_message(&message).unwrap()).unwrap(),
            message
        );
    }

    #[test]
    fn signed_task_envelope_serializes_target_agent() {
        let message = WireMessage::new(
            "msg-task",
            "corr-task",
            Some("agent-1".to_owned()),
            1,
            WirePayload::TaskAssignment {
                envelope: SignedTaskEnvelopeWire {
                    job_id: "job-1".to_owned(),
                    task_id: "task-1".to_owned(),
                    target_agent_id: "agent-1".to_owned(),
                    issued_at_ms: 1,
                    expires_at_ms: 60_000,
                    nonce: "nonce-1".to_owned(),
                    payload_hash: "hash".to_owned(),
                    signature: "sig".to_owned(),
                },
                task: TaskWire::Command(CommandTaskWire {
                    program: "uptime".to_owned(),
                    args: Vec::new(),
                    timeout_ms: 30_000,
                    max_output_bytes: 1024,
                }),
            },
        );

        let decoded = decode_message(&encode_message(&message).unwrap()).unwrap();
        let WirePayload::TaskAssignment { envelope, task } = decoded.payload else {
            panic!("expected task assignment");
        };

        assert!(envelope.targets_agent("agent-1"));
        assert!(!envelope.targets_agent("agent-2"));
        assert!(matches!(task, TaskWire::Command(_)));
    }

    #[test]
    fn drift_check_task_roundtrips() {
        let message = WireMessage::new(
            "msg-drift",
            "corr-drift",
            Some("agent-1".to_owned()),
            1,
            WirePayload::TaskAssignment {
                envelope: SignedTaskEnvelopeWire {
                    job_id: "job-drift".to_owned(),
                    task_id: "task-drift".to_owned(),
                    target_agent_id: "agent-1".to_owned(),
                    issued_at_ms: 1,
                    expires_at_ms: 60_000,
                    nonce: "nonce-drift".to_owned(),
                    payload_hash: "hash".to_owned(),
                    signature: "sig".to_owned(),
                },
                task: TaskWire::DriftCheck(DriftCheckTaskWire {
                    policy_document: "apiVersion: fleet.sponzey.dev/v1alpha1".to_owned(),
                }),
            },
        );

        let decoded = decode_message(&encode_message(&message).unwrap()).unwrap();
        let WirePayload::TaskAssignment { task, .. } = decoded.payload else {
            panic!("expected task assignment");
        };

        assert!(matches!(task, TaskWire::DriftCheck(_)));
    }

    #[test]
    fn runbook_execution_task_roundtrips() {
        let message = WireMessage::new(
            "msg-runbook",
            "corr-runbook",
            Some("agent-1".to_owned()),
            1,
            WirePayload::TaskAssignment {
                envelope: SignedTaskEnvelopeWire {
                    job_id: "job-runbook".to_owned(),
                    task_id: "task-runbook".to_owned(),
                    target_agent_id: "agent-1".to_owned(),
                    issued_at_ms: 1,
                    expires_at_ms: 60_000,
                    nonce: "nonce-runbook".to_owned(),
                    payload_hash: "hash".to_owned(),
                    signature: "sig".to_owned(),
                },
                task: TaskWire::RunbookExecution(RunbookExecutionTaskWire {
                    runbook_document: "apiVersion: fleet.sponzey.dev/v1alpha1".to_owned(),
                    timeout_ms: 30_000,
                    confirmed_high_risk: true,
                }),
            },
        );

        let decoded = decode_message(&encode_message(&message).unwrap()).unwrap();
        let WirePayload::TaskAssignment { task, .. } = decoded.payload else {
            panic!("expected task assignment");
        };

        assert!(matches!(task, TaskWire::RunbookExecution(_)));
    }

    #[test]
    fn task_lifecycle_events_roundtrip() {
        for payload in [
            WirePayload::TaskAck {
                job_id: "job-1".to_owned(),
                task_id: "task-1".to_owned(),
            },
            WirePayload::TaskStarted {
                job_id: "job-1".to_owned(),
                task_id: "task-1".to_owned(),
            },
            WirePayload::TaskRejected {
                job_id: "job-1".to_owned(),
                task_id: "task-1".to_owned(),
                reason_code: TaskRejectionReasonCode::InvalidSignature,
                reason: "signature verification failed".to_owned(),
            },
            WirePayload::TaskCancel {
                job_id: "job-1".to_owned(),
                task_id: "task-1".to_owned(),
                reason: "operator requested cancel".to_owned(),
            },
            WirePayload::TaskResult {
                job_id: "job-1".to_owned(),
                task_id: "task-1".to_owned(),
                exit_code: 0,
                status: Some(TaskResultStatus::Succeeded),
                reason: String::new(),
            },
        ] {
            let message = WireMessage::new(
                "msg-task-event",
                "task-1",
                Some("agent-1".to_owned()),
                1,
                payload.clone(),
            );

            let decoded = decode_message(&encode_message(&message).unwrap()).unwrap();

            assert_eq!(decoded.payload, payload);
            assert_eq!(decoded.payload.channel(), ProtocolChannel::TaskData);
        }
    }

    #[test]
    fn legacy_task_result_without_status_still_decodes() {
        let body = r#"{
            "protocol_version":1,
            "message_id":"msg-result",
            "correlation_id":"corr-result",
            "agent_id":"agent-1",
            "timestamp_ms":1,
            "payload":{
                "type":"task_result",
                "payload":{
                    "job_id":"job-1",
                    "task_id":"task-1",
                    "exit_code":0
                }
            }
        }"#;

        let decoded = decode_message(body).unwrap();
        let WirePayload::TaskResult { status, reason, .. } = decoded.payload else {
            panic!("expected task result");
        };

        assert_eq!(status, None);
        assert_eq!(reason, "");
    }

    #[test]
    fn task_ack_ignores_unknown_compatible_fields() {
        let body = r#"{
            "protocol_version": 1,
            "message_id": "msg-ack",
            "correlation_id": "task-1",
            "agent_id": "agent-1",
            "timestamp_ms": 1,
            "payload": {
                "type": "task_ack",
                "payload": {
                    "job_id": "job-1",
                    "task_id": "task-1",
                    "future_field": "ignored"
                }
            }
        }"#;

        let decoded = decode_message(body).unwrap();

        assert!(matches!(
            decoded.payload,
            WirePayload::TaskAck { job_id, task_id }
                if job_id == "job-1" && task_id == "task-1"
        ));
    }

    #[test]
    fn separates_auth_and_task_channels() {
        assert_eq!(
            WirePayload::AuthChallenge { nonce: "n1".into() }.channel(),
            ProtocolChannel::AuthSession
        );
        assert_eq!(
            WirePayload::OutputChunk {
                job_id: "job-1".into(),
                task_id: "task-1".into(),
                stream: OutputStream::Stdout,
                sequence: 0,
                data: "ok".into(),
            }
            .channel(),
            ProtocolChannel::TaskData
        );
        assert_eq!(
            WirePayload::TaskAck {
                job_id: "job-1".into(),
                task_id: "task-1".into(),
            }
            .channel(),
            ProtocolChannel::TaskData
        );
        assert_eq!(
            WirePayload::SecurityEvent {
                agent_id: "agent-1".into(),
                action: "task_verification_failed".into(),
                detail: "invalid signature".into(),
            }
            .channel(),
            ProtocolChannel::TaskData
        );
    }
}
