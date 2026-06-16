use crate::{Selector, SelectorError};
use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

pub const RUNBOOK_API_VERSION: &str = "fleet.sponzey.dev/v1alpha1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runbook {
    pub api_version: String,
    pub name: String,
    pub description: Option<String>,
    pub target_selector: Selector,
    pub strategy: RunbookStrategy,
    pub check_mode: bool,
    pub dry_run: bool,
    pub tasks: Vec<RunbookTask>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunbookStrategy {
    pub concurrency: u32,
    pub max_failures: Option<u32>,
}

impl Default for RunbookStrategy {
    fn default() -> Self {
        Self {
            concurrency: 1,
            max_failures: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunbookTask {
    Package(PackagePrimitive),
    Service(ServicePrimitive),
    FileCopy(FileCopyPrimitive),
    PortCheck(PortCheckPrimitive),
    ProcessCheck(ProcessCheckPrimitive),
    FactsCollect(FactsCollectPrimitive),
    MetricsSnapshot(MetricsSnapshotPrimitive),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagePrimitive {
    pub id: String,
    pub name: String,
    pub state: PackageState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageState {
    Present,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicePrimitive {
    pub id: String,
    pub name: String,
    pub state: ServicePrimitiveState,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServicePrimitiveState {
    Started,
    Restarted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCopyPrimitive {
    pub id: String,
    pub dest: String,
    pub content: String,
    pub mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortCheckPrimitive {
    pub id: String,
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessCheckPrimitive {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactsCollectPrimitive {
    pub id: String,
    pub scope: SnapshotScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsSnapshotPrimitive {
    pub id: String,
    pub scope: SnapshotScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotScope {
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunbookParseError {
    MissingField(&'static str),
    UnsupportedApiVersion(String),
    UnsupportedKind(String),
    UnknownTopLevelField(String),
    UnknownSpecField(String),
    UnknownTaskField { task_id: String, field: String },
    UnsupportedTask(String),
    UnsupportedPackageState(String),
    UnsupportedServiceState(String),
    UnsupportedSnapshotScope(String),
    InvalidPort(String),
    InvalidSelector(String),
    InvalidYaml(String),
    InvalidBoolean { field: &'static str, value: String },
    InvalidStrategy { field: &'static str, value: String },
    UnsafeFileDestination(String),
}

impl Display for RunbookParseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField(field) => {
                write!(formatter, "runbook missing required field: {field}")
            }
            Self::UnsupportedApiVersion(version) => {
                write!(formatter, "unsupported runbook apiVersion: {version}")
            }
            Self::UnsupportedKind(kind) => write!(formatter, "unsupported runbook kind: {kind}"),
            Self::UnknownTopLevelField(field) => {
                write!(formatter, "unknown runbook top-level field: {field}")
            }
            Self::UnknownSpecField(field) => {
                write!(formatter, "unknown runbook spec field: {field}")
            }
            Self::UnknownTaskField { task_id, field } => {
                write!(formatter, "unknown runbook task field: {task_id}.{field}")
            }
            Self::UnsupportedTask(task) => write!(formatter, "unsupported runbook task: {task}"),
            Self::UnsupportedPackageState(state) => {
                write!(formatter, "unsupported package state: {state}")
            }
            Self::UnsupportedServiceState(state) => {
                write!(formatter, "unsupported service state: {state}")
            }
            Self::UnsupportedSnapshotScope(scope) => {
                write!(formatter, "unsupported snapshot scope: {scope}")
            }
            Self::InvalidPort(value) => write!(formatter, "invalid port value: {value}"),
            Self::InvalidSelector(selector) => {
                write!(formatter, "invalid runbook selector: {selector}")
            }
            Self::InvalidYaml(message) => write!(formatter, "invalid runbook yaml: {message}"),
            Self::InvalidBoolean { field, value } => {
                write!(formatter, "invalid boolean for {field}: {value}")
            }
            Self::InvalidStrategy { field, value } => {
                write!(formatter, "invalid strategy value for {field}: {value}")
            }
            Self::UnsafeFileDestination(path) => {
                write!(formatter, "unsafe file destination: {path}")
            }
        }
    }
}

impl std::error::Error for RunbookParseError {}

impl From<SelectorError> for RunbookParseError {
    fn from(value: SelectorError) -> Self {
        Self::InvalidSelector(value.to_string())
    }
}

pub fn runbook_schema_json() -> &'static str {
    r#"{
  "type": "object",
  "required": ["apiVersion", "kind", "name", "steps"],
  "properties": {
    "apiVersion": {"const": "fleet.sponzey.dev/v1alpha1"},
    "kind": {"const": "Runbook"},
    "name": {"type": "string"},
    "description": {"type": "string"},
    "selector": {"type": "string"},
    "matchLabels": {
      "type": "object",
      "additionalProperties": {"type": "string"}
    },
    "strategy": {
      "type": "object",
      "properties": {
        "concurrency": {"type": "integer", "minimum": 1},
        "maxFailures": {"type": "integer", "minimum": 1}
      },
      "additionalProperties": false
    },
    "checkMode": {"type": "boolean"},
    "dryRun": {"type": "boolean"},
    "steps": {
      "type": "array",
      "items": {
        "oneOf": [
          {"required": ["id", "package"]},
          {"required": ["id", "service"]},
          {"required": ["id", "file.copy"]},
          {"required": ["id", "port.check"]},
          {"required": ["id", "process.check"]},
          {"required": ["id", "facts.collect"]},
          {"required": ["id", "metrics.snapshot"]}
        ]
      }
    },
    "metadata": {
      "description": "Legacy compatibility block. Prefer top-level name and description.",
      "type": "object",
      "required": ["name"],
      "properties": {
        "name": {"type": "string"},
        "description": {"type": "string"}
      },
      "additionalProperties": false
    },
    "spec": {
      "description": "Legacy compatibility block. Prefer top-level selector, matchLabels, strategy, checkMode, dryRun, and steps.",
      "type": "object",
      "required": ["targets", "tasks"],
      "properties": {
        "targets": {
          "type": "object",
          "properties": {
            "selector": {"type": "string"},
            "matchLabels": {
              "type": "object",
              "additionalProperties": {"type": "string"}
            }
          },
          "additionalProperties": false
        },
        "strategy": {
          "type": "object",
          "properties": {
            "concurrency": {"type": "integer", "minimum": 1},
            "maxFailures": {"type": "integer", "minimum": 1}
          },
          "additionalProperties": false
        },
        "checkMode": {"type": "boolean"},
        "dryRun": {"type": "boolean"},
        "tasks": {
          "type": "array",
          "items": {
            "oneOf": [
              {"required": ["id", "package"]},
              {"required": ["id", "service"]},
              {"required": ["id", "file.copy"]},
              {"required": ["id", "port.check"]},
              {"required": ["id", "process.check"]},
              {"required": ["id", "facts.collect"]},
              {"required": ["id", "metrics.snapshot"]}
            ]
          }
        }
      },
      "additionalProperties": false
    }
  },
  "additionalProperties": false
}"#
}

pub fn parse_runbook_document(body: &str) -> Result<Runbook, RunbookParseError> {
    let mut api_version = None;
    let mut kind = None;
    let mut name = None;
    let mut description = None;
    let mut target_selector = None;
    let mut match_labels = BTreeMap::new();
    let mut strategy = RunbookStrategy::default();
    let mut check_mode = false;
    let mut dry_run = false;
    let mut tasks = Vec::new();
    let mut current_task: Option<TaskBuilder> = None;
    let mut root_section: Option<String> = None;
    let mut spec_section: Option<String> = None;
    let mut match_labels_indent: Option<usize> = None;
    let mut strategy_indent: Option<usize> = None;
    let mut task_section_indent: Option<usize> = None;

    for raw_line in body.lines() {
        let without_comment = raw_line.split('#').next().unwrap_or_default();
        if without_comment.trim().is_empty() {
            continue;
        }
        if without_comment.contains('\t') {
            return Err(RunbookParseError::InvalidYaml(
                "tabs are not supported in MVP runbooks".to_owned(),
            ));
        }
        let indent = without_comment
            .chars()
            .take_while(|character| *character == ' ')
            .count();
        let line = without_comment.trim();

        if match_labels_indent.is_some_and(|section_indent| indent <= section_indent)
            && line != "matchLabels:"
        {
            match_labels_indent = None;
        }
        if strategy_indent.is_some_and(|section_indent| indent <= section_indent)
            && line != "strategy:"
        {
            strategy_indent = None;
        }

        if indent == 0 {
            let key = line
                .split_once(':')
                .map(|(key, _)| key)
                .unwrap_or(line)
                .trim();
            if !matches!(
                key,
                "apiVersion"
                    | "kind"
                    | "metadata"
                    | "spec"
                    | "name"
                    | "description"
                    | "selector"
                    | "matchLabels"
                    | "strategy"
                    | "checkMode"
                    | "dryRun"
                    | "steps"
            ) {
                return Err(RunbookParseError::UnknownTopLevelField(key.to_owned()));
            }
            root_section = Some(key.to_owned());
            spec_section = None;
            if key == "matchLabels" {
                match_labels_indent = Some(indent);
            }
            if key == "strategy" {
                strategy_indent = Some(indent);
            }
            if key == "steps" {
                task_section_indent = Some(indent);
            }
        } else if indent == 2 && root_section.as_deref() == Some("spec") {
            let key = line
                .split_once(':')
                .map(|(key, _)| key)
                .unwrap_or(line)
                .trim();
            if !matches!(
                key,
                "targets" | "tasks" | "strategy" | "checkMode" | "dryRun"
            ) {
                return Err(RunbookParseError::UnknownSpecField(key.to_owned()));
            }
            spec_section = Some(key.to_owned());
            if key == "strategy" {
                strategy_indent = Some(indent);
            }
            if key == "tasks" {
                task_section_indent = Some(indent);
            }
        } else if indent == 4
            && root_section.as_deref() == Some("spec")
            && spec_section.as_deref() == Some("targets")
        {
            let key = line
                .split_once(':')
                .map(|(key, _)| key)
                .unwrap_or(line)
                .trim();
            if !matches!(key, "selector" | "matchLabels") {
                return Err(RunbookParseError::UnknownSpecField(format!(
                    "targets.{key}"
                )));
            }
            if key == "matchLabels" {
                match_labels_indent = Some(indent);
            }
        }

        if let Some(value) = scalar_value(line, "apiVersion") {
            api_version = Some(clean_scalar(value));
            continue;
        }
        if let Some(value) = scalar_value(line, "kind") {
            kind = Some(clean_scalar(value));
            continue;
        }
        if indent == 0
            && let Some(value) = scalar_value(line, "name")
        {
            name = Some(clean_scalar(value));
            continue;
        }
        if indent == 2
            && root_section.as_deref() == Some("metadata")
            && name.is_none()
            && let Some(value) = scalar_value(line, "name")
        {
            name = Some(clean_scalar(value));
            continue;
        }
        if indent == 0
            && let Some(value) = scalar_value(line, "description")
        {
            description = Some(clean_scalar(value));
            continue;
        }
        if indent == 2
            && root_section.as_deref() == Some("metadata")
            && description.is_none()
            && let Some(value) = scalar_value(line, "description")
        {
            description = Some(clean_scalar(value));
            continue;
        }
        if (indent == 0
            || (indent >= 4
                && root_section.as_deref() == Some("spec")
                && spec_section.as_deref() == Some("targets")))
            && let Some(value) = scalar_value(line, "selector")
        {
            target_selector = Some(Selector::parse(value)?);
            continue;
        }
        if (indent == 0 || (indent == 2 && root_section.as_deref() == Some("spec")))
            && let Some(value) = scalar_value(line, "checkMode")
        {
            check_mode = parse_bool("checkMode", value)?;
            continue;
        }
        if (indent == 0 || (indent == 2 && root_section.as_deref() == Some("spec")))
            && let Some(value) = scalar_value(line, "dryRun")
        {
            dry_run = parse_bool("dryRun", value)?;
            continue;
        }
        if line == "matchLabels:" {
            match_labels_indent = Some(indent);
            continue;
        }
        if match_labels_indent.is_some_and(|section_indent| indent > section_indent) {
            let Some((key, value)) = line.split_once(':') else {
                return Err(RunbookParseError::InvalidYaml(format!(
                    "expected key: value inside matchLabels near `{line}`"
                )));
            };
            match_labels.insert(key.trim().to_owned(), clean_scalar(value.trim()));
            continue;
        }
        if line == "strategy:" {
            strategy_indent = Some(indent);
            continue;
        }
        if strategy_indent.is_some_and(|section_indent| indent > section_indent) {
            let Some((key, value)) = line.split_once(':') else {
                return Err(RunbookParseError::InvalidYaml(format!(
                    "expected key: value inside strategy near `{line}`"
                )));
            };
            match key.trim() {
                "concurrency" => {
                    strategy.concurrency = parse_positive_u32("strategy.concurrency", value)?;
                }
                "maxFailures" | "max_failures" => {
                    strategy.max_failures =
                        Some(parse_positive_u32("strategy.maxFailures", value)?);
                }
                field => {
                    return Err(RunbookParseError::UnknownSpecField(format!(
                        "strategy.{field}"
                    )));
                }
            }
            continue;
        }
        if line == "steps:" || line == "tasks:" {
            task_section_indent = Some(indent);
            continue;
        }

        if task_section_indent.is_some_and(|section_indent| indent > section_indent)
            && let Some(value) = line.strip_prefix("- id:")
        {
            if let Some(builder) = current_task.take() {
                tasks.push(builder.build()?);
            }
            current_task = Some(TaskBuilder::new(value.trim(), indent));
            continue;
        }

        if let Some(builder) = current_task.as_mut()
            && indent > builder.indent
        {
            if line == "package:" {
                builder.kind = Some("package".to_owned());
                continue;
            }
            if line == "service:" {
                builder.kind = Some("service".to_owned());
                continue;
            }
            if line == "file.copy:" {
                builder.kind = Some("file.copy".to_owned());
                continue;
            }
            if line == "port.check:" {
                builder.kind = Some("port.check".to_owned());
                continue;
            }
            if line == "process.check:" {
                builder.kind = Some("process.check".to_owned());
                continue;
            }
            if line == "facts.collect:" {
                builder.kind = Some("facts.collect".to_owned());
                continue;
            }
            if line == "metrics.snapshot:" {
                builder.kind = Some("metrics.snapshot".to_owned());
                continue;
            }
            if line.ends_with(':') {
                builder.kind = Some(line.trim_end_matches(':').to_owned());
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                return Err(RunbookParseError::InvalidYaml(format!(
                    "expected key: value near `{line}`"
                )));
            };
            builder
                .fields
                .insert(key.trim().to_owned(), clean_scalar(value.trim()));
            continue;
        }

        if indent > 0
            && let Some(builder) = current_task.as_mut()
        {
            return Err(RunbookParseError::InvalidYaml(format!(
                "unexpected line in task {}: `{line}`",
                builder.id
            )));
        }
    }

    if let Some(builder) = current_task.take() {
        tasks.push(builder.build()?);
    }

    let api_version = api_version.ok_or(RunbookParseError::MissingField("apiVersion"))?;
    if api_version != RUNBOOK_API_VERSION {
        return Err(RunbookParseError::UnsupportedApiVersion(api_version));
    }
    let kind = kind.ok_or(RunbookParseError::MissingField("kind"))?;
    if kind != "Runbook" {
        return Err(RunbookParseError::UnsupportedKind(kind));
    }
    let name = name.ok_or(RunbookParseError::MissingField("metadata.name"))?;
    if target_selector.is_some() && !match_labels.is_empty() {
        return Err(RunbookParseError::InvalidSelector(
            "selector and matchLabels cannot be used together".to_owned(),
        ));
    }
    let target_selector = match target_selector {
        Some(selector) => selector,
        None if !match_labels.is_empty() => Selector::from_match_labels(match_labels)?,
        None => return Err(RunbookParseError::MissingField("selector")),
    };
    if tasks.is_empty() {
        return Err(RunbookParseError::MissingField("steps"));
    }

    Ok(Runbook {
        api_version,
        name,
        description,
        target_selector,
        strategy,
        check_mode,
        dry_run,
        tasks,
    })
}

fn scalar_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.strip_prefix(key)?
        .strip_prefix(':')
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn clean_scalar(value: &str) -> String {
    let value = value.trim();
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value)
        .to_owned()
}

fn parse_bool(field: &'static str, value: &str) -> Result<bool, RunbookParseError> {
    match clean_scalar(value).as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        value => Err(RunbookParseError::InvalidBoolean {
            field,
            value: value.to_owned(),
        }),
    }
}

fn parse_positive_u32(field: &'static str, value: &str) -> Result<u32, RunbookParseError> {
    let value = clean_scalar(value);
    let parsed = value
        .parse::<u32>()
        .map_err(|_| RunbookParseError::InvalidStrategy {
            field,
            value: value.clone(),
        })?;
    if parsed == 0 {
        Err(RunbookParseError::InvalidStrategy { field, value })
    } else {
        Ok(parsed)
    }
}

struct TaskBuilder {
    id: String,
    indent: usize,
    kind: Option<String>,
    fields: BTreeMap<String, String>,
}

impl TaskBuilder {
    fn new(id: &str, indent: usize) -> Self {
        Self {
            id: id.to_owned(),
            indent,
            kind: None,
            fields: BTreeMap::new(),
        }
    }

    fn build(self) -> Result<RunbookTask, RunbookParseError> {
        match self.kind.as_deref() {
            Some("package") => {
                self.reject_unknown_fields(&["name", "state"])?;
                let name = required_field(&self.fields, "name", "package.name")?;
                let state = required_field(&self.fields, "state", "package.state")?;
                if state != "present" {
                    return Err(RunbookParseError::UnsupportedPackageState(state));
                }
                Ok(RunbookTask::Package(PackagePrimitive {
                    id: self.id,
                    name,
                    state: PackageState::Present,
                }))
            }
            Some("service") => {
                self.reject_unknown_fields(&["name", "state", "enabled"])?;
                let name = required_field(&self.fields, "name", "service.name")?;
                let state = required_field(&self.fields, "state", "service.state")?;
                let state = match state.as_str() {
                    "started" => ServicePrimitiveState::Started,
                    "restarted" => ServicePrimitiveState::Restarted,
                    _ => return Err(RunbookParseError::UnsupportedServiceState(state)),
                };
                Ok(RunbookTask::Service(ServicePrimitive {
                    id: self.id,
                    name,
                    state,
                    enabled: self
                        .fields
                        .get("enabled")
                        .map(|value| parse_bool("service.enabled", value))
                        .transpose()?,
                }))
            }
            Some("file.copy") => {
                self.reject_unknown_fields(&["dest", "content", "mode"])?;
                let dest = required_field(&self.fields, "dest", "file.copy.dest")?;
                validate_file_destination(&dest)?;
                Ok(RunbookTask::FileCopy(FileCopyPrimitive {
                    id: self.id,
                    dest,
                    content: required_field(&self.fields, "content", "file.copy.content")?,
                    mode: self.fields.get("mode").cloned(),
                }))
            }
            Some("port.check") => {
                self.reject_unknown_fields(&["host", "port"])?;
                Ok(RunbookTask::PortCheck(PortCheckPrimitive {
                    id: self.id,
                    host: self
                        .fields
                        .get("host")
                        .cloned()
                        .unwrap_or_else(|| "127.0.0.1".to_owned()),
                    port: parse_port(required_field(&self.fields, "port", "port.check.port")?)?,
                }))
            }
            Some("process.check") => {
                self.reject_unknown_fields(&["name"])?;
                Ok(RunbookTask::ProcessCheck(ProcessCheckPrimitive {
                    id: self.id,
                    name: required_field(&self.fields, "name", "process.check.name")?,
                }))
            }
            Some("facts.collect") => {
                self.reject_unknown_fields(&["scope"])?;
                Ok(RunbookTask::FactsCollect(FactsCollectPrimitive {
                    id: self.id,
                    scope: parse_snapshot_scope(self.fields.get("scope"))?,
                }))
            }
            Some("metrics.snapshot") => {
                self.reject_unknown_fields(&["scope"])?;
                Ok(RunbookTask::MetricsSnapshot(MetricsSnapshotPrimitive {
                    id: self.id,
                    scope: parse_snapshot_scope(self.fields.get("scope"))?,
                }))
            }
            Some(kind) => Err(RunbookParseError::UnsupportedTask(kind.to_owned())),
            None => Err(RunbookParseError::MissingField("task kind")),
        }
    }

    fn reject_unknown_fields(&self, allowed: &[&str]) -> Result<(), RunbookParseError> {
        if let Some(field) = self
            .fields
            .keys()
            .find(|field| !allowed.contains(&field.as_str()))
        {
            Err(RunbookParseError::UnknownTaskField {
                task_id: self.id.clone(),
                field: field.clone(),
            })
        } else {
            Ok(())
        }
    }
}

fn required_field(
    fields: &BTreeMap<String, String>,
    key: &str,
    field: &'static str,
) -> Result<String, RunbookParseError> {
    fields
        .get(key)
        .cloned()
        .filter(|value| !value.is_empty())
        .ok_or(RunbookParseError::MissingField(field))
}

fn validate_file_destination(path: &str) -> Result<(), RunbookParseError> {
    if !path.starts_with('/') || path == "/" || path.contains("/../") || path.ends_with("/..") {
        Err(RunbookParseError::UnsafeFileDestination(path.to_owned()))
    } else {
        Ok(())
    }
}

fn parse_port(value: String) -> Result<u16, RunbookParseError> {
    let port = value
        .parse::<u16>()
        .map_err(|_| RunbookParseError::InvalidPort(value.clone()))?;
    if port == 0 {
        Err(RunbookParseError::InvalidPort(value))
    } else {
        Ok(port)
    }
}

fn parse_snapshot_scope(value: Option<&String>) -> Result<SnapshotScope, RunbookParseError> {
    match value.map(|value| value.as_str()).unwrap_or("local") {
        "local" => Ok(SnapshotScope::Local),
        scope => Err(RunbookParseError::UnsupportedSnapshotScope(
            scope.to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NGINX_RUNBOOK: &str = r#"
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Runbook
name: nginx-basic
description: Install nginx and make sure the service is running.
selector: role=web
strategy:
  concurrency: 2
  maxFailures: 1
checkMode: true
dryRun: false
steps:
  - id: nginx-package
    package:
      name: nginx
      state: present
  - id: nginx-service
    service:
      name: nginx
      state: started
      enabled: true
"#;

    #[test]
    fn parses_valid_nginx_runbook() {
        let runbook = parse_runbook_document(NGINX_RUNBOOK).unwrap();

        assert_eq!(runbook.api_version, RUNBOOK_API_VERSION);
        assert_eq!(runbook.name, "nginx-basic");
        assert_eq!(
            runbook.description.as_deref(),
            Some("Install nginx and make sure the service is running.")
        );
        assert!(matches!(runbook.target_selector, Selector::Labels(_)));
        assert_eq!(runbook.strategy.concurrency, 2);
        assert_eq!(runbook.strategy.max_failures, Some(1));
        assert!(runbook.check_mode);
        assert!(!runbook.dry_run);
        assert_eq!(runbook.tasks.len(), 2);
    }

    #[test]
    fn rejects_missing_targets() {
        let body = NGINX_RUNBOOK.replace("selector: role=web", "");

        assert!(matches!(
            parse_runbook_document(&body),
            Err(RunbookParseError::MissingField("selector"))
        ));
    }

    #[test]
    fn rejects_missing_tasks() {
        let body = NGINX_RUNBOOK
            .lines()
            .take_while(|line| !line.trim().starts_with("steps:"))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(matches!(
            parse_runbook_document(&body),
            Err(RunbookParseError::MissingField("steps"))
        ));
    }

    #[test]
    fn rejects_unsupported_task() {
        let body = NGINX_RUNBOOK.replace("package:", "shell:");

        assert!(matches!(
            parse_runbook_document(&body),
            Err(RunbookParseError::UnsupportedTask(_))
        ));
    }

    #[test]
    fn rejects_invalid_yaml_tabs() {
        let body = NGINX_RUNBOOK.replace("name: nginx-basic", "\tname: nginx-basic");

        assert!(matches!(
            parse_runbook_document(&body),
            Err(RunbookParseError::InvalidYaml(_))
        ));
    }

    #[test]
    fn invalid_yaml_error_is_user_friendly() {
        let body = NGINX_RUNBOOK.replace("state: present", "state present");
        let error = parse_runbook_document(&body).unwrap_err();

        assert!(error.to_string().contains("expected key: value"));
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let body = format!("{NGINX_RUNBOOK}\nvars:\n  answer: 42\n");

        assert!(matches!(
            parse_runbook_document(&body),
            Err(RunbookParseError::UnknownTopLevelField(_))
        ));
    }

    #[test]
    fn parses_file_copy_task_and_rejects_unsafe_destination() {
        let body = r#"
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Runbook
name: file-copy
selector: role=web
steps:
  - id: copy-index
    file.copy:
      dest: /tmp/index.html
      content: hello
      mode: "0644"
"#;
        let runbook = parse_runbook_document(body).unwrap();
        assert!(matches!(runbook.tasks[0], RunbookTask::FileCopy(_)));

        let unsafe_body = body.replace("/tmp/index.html", "../index.html");
        assert!(matches!(
            parse_runbook_document(&unsafe_body),
            Err(RunbookParseError::UnsafeFileDestination(_))
        ));
    }

    #[test]
    fn parses_safe_check_and_snapshot_tasks() {
        let body = r#"
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Runbook
name: safe-checks
selector: role=web
steps:
  - id: http
    port.check:
      host: 127.0.0.1
      port: 8080
  - id: nginx-process
    process.check:
      name: nginx
  - id: facts-now
    facts.collect:
      scope: local
  - id: metrics-now
    metrics.snapshot:
      scope: local
"#;

        let runbook = parse_runbook_document(body).unwrap();

        assert!(matches!(
            &runbook.tasks[0],
            RunbookTask::PortCheck(task)
                if task.host == "127.0.0.1" && task.port == 8080
        ));
        assert!(matches!(
            &runbook.tasks[1],
            RunbookTask::ProcessCheck(task) if task.name == "nginx"
        ));
        assert!(matches!(
            &runbook.tasks[2],
            RunbookTask::FactsCollect(task) if task.scope == SnapshotScope::Local
        ));
        assert!(matches!(
            &runbook.tasks[3],
            RunbookTask::MetricsSnapshot(task) if task.scope == SnapshotScope::Local
        ));
    }

    #[test]
    fn rejects_invalid_safe_primitive_fields() {
        let invalid_port = r#"
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Runbook
name: invalid-port
selector: role=web
steps:
  - id: http
    port.check:
      port: 0
"#;
        assert!(matches!(
            parse_runbook_document(invalid_port),
            Err(RunbookParseError::InvalidPort(_))
        ));

        let invalid_scope = r#"
apiVersion: fleet.sponzey.dev/v1alpha1
kind: Runbook
name: invalid-scope
selector: role=web
steps:
  - id: facts-now
    facts.collect:
      scope: controller
"#;
        assert!(matches!(
            parse_runbook_document(invalid_scope),
            Err(RunbookParseError::UnsupportedSnapshotScope(_))
        ));
    }

    #[test]
    fn parses_match_labels_selector() {
        let body = NGINX_RUNBOOK.replace(
            "selector: role=web",
            "matchLabels:\n  role: web\n  env: prod",
        );
        let runbook = parse_runbook_document(&body).unwrap();

        let Selector::Labels(labels) = runbook.target_selector else {
            panic!("matchLabels must parse to label selector");
        };
        assert_eq!(labels.len(), 2);
    }

    #[test]
    fn parses_legacy_metadata_spec_fixture() {
        let runbook = parse_runbook_document(include_str!(
            "../../../examples/runbooks/legacy-nginx-basic.yml"
        ))
        .unwrap();

        assert_eq!(runbook.name, "nginx-basic");
        assert_eq!(runbook.strategy, RunbookStrategy::default());
        assert!(!runbook.check_mode);
        assert!(!runbook.dry_run);
        assert_eq!(runbook.tasks.len(), 2);
    }

    #[test]
    fn rejects_unknown_task_field() {
        let body = NGINX_RUNBOOK.replace("state: present", "state: present\n      version: latest");

        assert!(matches!(
            parse_runbook_document(&body),
            Err(RunbookParseError::UnknownTaskField { .. })
        ));
    }

    #[test]
    fn rejects_invalid_strategy_and_boolean_values() {
        let invalid_strategy = NGINX_RUNBOOK.replace("concurrency: 2", "concurrency: 0");
        assert!(matches!(
            parse_runbook_document(&invalid_strategy),
            Err(RunbookParseError::InvalidStrategy {
                field: "strategy.concurrency",
                ..
            })
        ));

        let invalid_boolean = NGINX_RUNBOOK.replace("checkMode: true", "checkMode: maybe");
        assert!(matches!(
            parse_runbook_document(&invalid_boolean),
            Err(RunbookParseError::InvalidBoolean {
                field: "checkMode",
                ..
            })
        ));
    }

    #[test]
    fn schema_export_mentions_required_fields() {
        let schema = runbook_schema_json();
        assert!(schema.contains("\"apiVersion\""));
        assert!(schema.contains("\"strategy\""));
        assert!(schema.contains("\"checkMode\""));
        assert!(schema.contains("\"dryRun\""));
        assert!(schema.contains("\"steps\""));
        assert!(schema.contains("\"matchLabels\""));
        assert!(schema.contains("\"port.check\""));
        assert!(schema.contains("\"metrics.snapshot\""));
        assert!(!schema.contains("ansible"));
    }
}
