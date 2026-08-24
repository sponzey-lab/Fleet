use crate::{AgentId, JobId, TaskId};
use std::fmt::{Display, Formatter};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArtifactId(String);

impl ArtifactId {
    pub fn new(value: impl Into<String>) -> Result<Self, ArtifactError> {
        non_empty(value.into(), "artifact id").map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactChecksum(String);

impl ArtifactChecksum {
    pub fn sha256(value: impl Into<String>) -> Result<Self, ArtifactError> {
        let value = value.into();
        if value.len() != 64 || !value.chars().all(|character| character.is_ascii_hexdigit()) {
            return Err(ArtifactError::InvalidSha256(value));
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_sha256(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactRetentionClass {
    RenderedTemplate,
}

impl ArtifactRetentionClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RenderedTemplate => "rendered_template",
        }
    }

    pub fn parse(value: &str) -> Result<Self, ArtifactError> {
        match value {
            "rendered_template" => Ok(Self::RenderedTemplate),
            value => Err(ArtifactError::UnsupportedRetentionClass(value.to_owned())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedArtifactMetadata {
    pub id: ArtifactId,
    pub job_id: JobId,
    pub agent_id: AgentId,
    pub task_id: TaskId,
    pub destination: String,
    pub checksum: ArtifactChecksum,
    pub size_bytes: u64,
    pub retention_class: ArtifactRetentionClass,
    pub created_at: SystemTime,
}

impl RenderedArtifactMetadata {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: ArtifactId,
        job_id: JobId,
        agent_id: AgentId,
        task_id: TaskId,
        destination: impl Into<String>,
        checksum: ArtifactChecksum,
        size_bytes: u64,
        retention_class: ArtifactRetentionClass,
        created_at: SystemTime,
    ) -> Result<Self, ArtifactError> {
        let destination = non_empty(destination.into(), "artifact destination")?;
        if size_bytes == 0 {
            return Err(ArtifactError::InvalidSizeBytes(size_bytes));
        }
        Ok(Self {
            id,
            job_id,
            agent_id,
            task_id,
            destination,
            checksum,
            size_bytes,
            retention_class,
            created_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactError {
    EmptyField(&'static str),
    InvalidSha256(String),
    InvalidSizeBytes(u64),
    UnsupportedRetentionClass(String),
}

impl Display for ArtifactError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyField(field) => write!(formatter, "{field} cannot be empty"),
            Self::InvalidSha256(value) => write!(formatter, "invalid sha256 checksum: {value}"),
            Self::InvalidSizeBytes(value) => {
                write!(formatter, "artifact size_bytes must be positive: {value}")
            }
            Self::UnsupportedRetentionClass(value) => {
                write!(formatter, "unsupported artifact retention class: {value}")
            }
        }
    }
}

impl std::error::Error for ArtifactError {}

fn non_empty(value: String, field: &'static str) -> Result<String, ArtifactError> {
    if value.trim().is_empty() {
        Err(ArtifactError::EmptyField(field))
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentId, JobId, TaskId};

    #[test]
    fn rendered_artifact_metadata_requires_valid_fields() {
        let checksum = ArtifactChecksum::sha256(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap();
        let metadata = RenderedArtifactMetadata::new(
            ArtifactId::new("artifact-1").unwrap(),
            JobId::new("job-1").unwrap(),
            AgentId::new("agent-1").unwrap(),
            TaskId::new("task-1").unwrap(),
            "/etc/app.conf",
            checksum,
            42,
            ArtifactRetentionClass::RenderedTemplate,
            SystemTime::UNIX_EPOCH,
        )
        .unwrap();

        assert_eq!(metadata.id.as_str(), "artifact-1");
        assert_eq!(metadata.retention_class.as_str(), "rendered_template");
    }

    #[test]
    fn rejects_invalid_rendered_artifact_metadata() {
        assert_eq!(
            ArtifactId::new(""),
            Err(ArtifactError::EmptyField("artifact id"))
        );
        assert!(matches!(
            ArtifactChecksum::sha256("abc"),
            Err(ArtifactError::InvalidSha256(_))
        ));
        assert!(matches!(
            ArtifactRetentionClass::parse("job_output"),
            Err(ArtifactError::UnsupportedRetentionClass(_))
        ));
        let checksum = ArtifactChecksum::sha256(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap();
        assert!(matches!(
            RenderedArtifactMetadata::new(
                ArtifactId::new("artifact-1").unwrap(),
                JobId::new("job-1").unwrap(),
                AgentId::new("agent-1").unwrap(),
                TaskId::new("task-1").unwrap(),
                "/etc/app.conf",
                checksum,
                0,
                ArtifactRetentionClass::RenderedTemplate,
                SystemTime::UNIX_EPOCH,
            ),
            Err(ArtifactError::InvalidSizeBytes(0))
        ));
    }
}
