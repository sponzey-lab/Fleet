use crate::agent::{Agent, AgentLabel};
use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    Agent(String),
    Labels(Vec<AgentLabel>),
}

impl Selector {
    pub fn parse(value: &str) -> Result<Self, SelectorError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(SelectorError::Invalid(value.to_owned()));
        }

        if let Some(agent) = value.strip_prefix("agent:") {
            if agent.trim().is_empty() {
                return Err(SelectorError::Invalid(value.to_owned()));
            }
            return Ok(Self::Agent(agent.trim().to_owned()));
        }

        let label_selector = value.strip_prefix("label:").unwrap_or(value);
        Self::parse_labels(label_selector)
    }

    pub fn from_match_labels(labels: BTreeMap<String, String>) -> Result<Self, SelectorError> {
        if labels.is_empty() {
            return Err(SelectorError::InvalidMatchLabels);
        }
        labels
            .into_iter()
            .map(|(key, value)| {
                AgentLabel::new(key.trim(), value.trim())
                    .map_err(|_| SelectorError::InvalidMatchLabels)
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Self::Labels)
    }

    fn parse_labels(value: &str) -> Result<Self, SelectorError> {
        let mut labels = Vec::new();
        for part in value.split(',') {
            let part = part.trim();
            let Some((key, label_value)) = part.split_once('=') else {
                return Err(SelectorError::Invalid(value.to_owned()));
            };
            labels.push(
                AgentLabel::new(key.trim(), label_value.trim())
                    .map_err(|_| SelectorError::Invalid(value.to_owned()))?,
            );
        }
        if labels.is_empty() {
            Err(SelectorError::Invalid(value.to_owned()))
        } else {
            Ok(Self::Labels(labels))
        }
    }

    pub fn matches(&self, agent: &Agent) -> bool {
        match self {
            Self::Agent(value) => agent.name().as_str() == value || agent.id().as_str() == value,
            Self::Labels(expected) => expected.iter().all(|label| {
                agent
                    .labels()
                    .iter()
                    .any(|actual| actual.key() == label.key() && actual.value() == label.value())
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorError {
    Invalid(String),
    InvalidMatchLabels,
}

impl Display for SelectorError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(value) => write!(
                f,
                "invalid selector: {value}; expected agent:<name-or-id>, label:key=value, or key=value[,key=value]"
            ),
            Self::InvalidMatchLabels => write!(
                f,
                "invalid matchLabels selector; expected a non-empty object of string label keys and values"
            ),
        }
    }
}

impl std::error::Error for SelectorError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentFingerprint, AgentId, AgentIdentity, AgentName, AgentPublicKey};

    fn agent() -> Agent {
        let mut agent = Agent::new(
            AgentId::new("a1").unwrap(),
            AgentName::new("web-01").unwrap(),
            AgentIdentity {
                public_key: AgentPublicKey::new("pk").unwrap(),
                fingerprint: AgentFingerprint::new("0123456789abcdef").unwrap(),
            },
        );
        agent.set_labels(vec![
            AgentLabel::new("role", "web").unwrap(),
            AgentLabel::new("env", "prod").unwrap(),
        ]);
        agent
    }

    #[test]
    fn parses_label_selector() {
        assert!(matches!(
            Selector::parse("role=web").unwrap(),
            Selector::Labels(_)
        ));
    }

    #[test]
    fn rejects_invalid_selector() {
        assert!(Selector::parse("role:web").is_err());
    }

    #[test]
    fn matches_label_selector() {
        assert!(Selector::parse("role=web").unwrap().matches(&agent()));
    }

    #[test]
    fn rejects_label_mismatch() {
        assert!(!Selector::parse("role=db").unwrap().matches(&agent()));
    }

    #[test]
    fn matches_agent_name_selector() {
        assert!(Selector::parse("agent:web-01").unwrap().matches(&agent()));
    }

    #[test]
    fn matches_agent_id_selector() {
        assert!(Selector::parse("agent:a1").unwrap().matches(&agent()));
    }

    #[test]
    fn parses_label_prefix_selector() {
        assert!(Selector::parse("label:role=web").unwrap().matches(&agent()));
    }

    #[test]
    fn parses_match_labels_selector() {
        let mut labels = BTreeMap::new();
        labels.insert("role".to_owned(), "web".to_owned());
        labels.insert("env".to_owned(), "prod".to_owned());

        assert!(
            Selector::from_match_labels(labels)
                .unwrap()
                .matches(&agent())
        );
    }

    #[test]
    fn rejects_empty_match_labels_selector() {
        assert_eq!(
            Selector::from_match_labels(BTreeMap::new()).unwrap_err(),
            SelectorError::InvalidMatchLabels
        );
    }
}
