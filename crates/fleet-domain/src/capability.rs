use crate::job::TaskKind;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentCapability {
    PersistentSession,
    CommandExecution,
    DriftCheck,
    RunbookExecution,
    PackageInstall,
    ServiceControl,
    FileCopy,
}

impl AgentCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PersistentSession => "persistent_session",
            Self::CommandExecution => "command_execution",
            Self::DriftCheck => "drift_check",
            Self::RunbookExecution => "runbook_execution",
            Self::PackageInstall => "package_install",
            Self::ServiceControl => "service_control",
            Self::FileCopy => "file_copy",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "persistent_session" => Some(Self::PersistentSession),
            "command_execution" => Some(Self::CommandExecution),
            "drift_check" => Some(Self::DriftCheck),
            "runbook_execution" => Some(Self::RunbookExecution),
            "package_install" => Some(Self::PackageInstall),
            "service_control" => Some(Self::ServiceControl),
            "file_copy" => Some(Self::FileCopy),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PrivilegeLevel {
    Unprivileged,
    SudoAvailable,
    Root,
}

impl PrivilegeLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unprivileged => "unprivileged",
            Self::SudoAvailable => "sudo_available",
            Self::Root => "root",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "unprivileged" => Some(Self::Unprivileged),
            "sudo_available" => Some(Self::SudoAvailable),
            "root" => Some(Self::Root),
            _ => None,
        }
    }

    fn satisfies(self, required: Self) -> bool {
        self >= required
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackageManager {
    Apt,
    Dnf,
    Yum,
    Apk,
    Brew,
}

impl PackageManager {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apt => "apt",
            Self::Dnf => "dnf",
            Self::Yum => "yum",
            Self::Apk => "apk",
            Self::Brew => "brew",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "apt" => Some(Self::Apt),
            "dnf" => Some(Self::Dnf),
            "yum" => Some(Self::Yum),
            "apk" => Some(Self::Apk),
            "brew" => Some(Self::Brew),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceManager {
    Systemd,
    Launchd,
    OpenRc,
}

impl ServiceManager {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Systemd => "systemd",
            Self::Launchd => "launchd",
            Self::OpenRc => "openrc",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "systemd" => Some(Self::Systemd),
            "launchd" => Some(Self::Launchd),
            "openrc" => Some(Self::OpenRc),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimePrimitive {
    Command,
    DriftCheck,
    RunbookExecution,
    PackageInstall,
    ServiceControl,
    FileCopy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapabilityRequirement {
    Capability(AgentCapability),
    PrivilegeAtLeast(PrivilegeLevel),
    PackageManager,
    ServiceManager,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilitySnapshotStatus {
    Unknown,
    Reported,
    Stale,
    Unsupported,
    Compatible,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRuntimeProfile {
    privilege: PrivilegeLevel,
    package_manager: Option<PackageManager>,
    service_manager: Option<ServiceManager>,
    capabilities: Vec<AgentCapability>,
}

impl AgentRuntimeProfile {
    pub fn new(
        privilege: PrivilegeLevel,
        package_manager: Option<PackageManager>,
        service_manager: Option<ServiceManager>,
        capabilities: Vec<AgentCapability>,
    ) -> Self {
        Self {
            privilege,
            package_manager,
            service_manager,
            capabilities,
        }
    }

    pub fn privilege(&self) -> PrivilegeLevel {
        self.privilege
    }

    pub fn package_manager(&self) -> Option<PackageManager> {
        self.package_manager
    }

    pub fn service_manager(&self) -> Option<ServiceManager> {
        self.service_manager
    }

    pub fn capabilities(&self) -> &[AgentCapability] {
        &self.capabilities
    }

    fn satisfies(&self, requirement: CapabilityRequirement) -> bool {
        match requirement {
            CapabilityRequirement::Capability(capability) => {
                self.capabilities.contains(&capability)
            }
            CapabilityRequirement::PrivilegeAtLeast(level) => self.privilege.satisfies(level),
            CapabilityRequirement::PackageManager => self.package_manager.is_some(),
            CapabilityRequirement::ServiceManager => self.service_manager.is_some(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCapabilitySnapshot {
    status: CapabilitySnapshotStatus,
    profile: Option<AgentRuntimeProfile>,
    reported_at: Option<SystemTime>,
}

impl AgentCapabilitySnapshot {
    pub fn unknown() -> Self {
        Self {
            status: CapabilitySnapshotStatus::Unknown,
            profile: None,
            reported_at: None,
        }
    }

    pub fn reported(profile: AgentRuntimeProfile, reported_at: SystemTime) -> Self {
        Self {
            status: CapabilitySnapshotStatus::Reported,
            profile: Some(profile),
            reported_at: Some(reported_at),
        }
    }

    pub fn status(&self) -> CapabilitySnapshotStatus {
        self.status
    }

    pub fn profile(&self) -> Option<&AgentRuntimeProfile> {
        self.profile.as_ref()
    }

    pub fn reported_at(&self) -> Option<SystemTime> {
        self.reported_at
    }

    pub fn stale_if_older_than(mut self, now: SystemTime, max_age: Duration) -> Self {
        if let Some(reported_at) = self.reported_at
            && now
                .duration_since(reported_at)
                .is_ok_and(|age| age > max_age)
        {
            self.status = CapabilitySnapshotStatus::Stale;
        }
        self
    }

    pub fn evaluate(&self, primitive: RuntimePrimitive) -> CapabilityEvaluation {
        if self.status == CapabilitySnapshotStatus::Unknown {
            return CapabilityEvaluation::new(CapabilitySnapshotStatus::Unknown, Vec::new());
        }
        if self.status == CapabilitySnapshotStatus::Stale {
            return CapabilityEvaluation::new(CapabilitySnapshotStatus::Stale, Vec::new());
        }
        let Some(profile) = &self.profile else {
            return CapabilityEvaluation::new(CapabilitySnapshotStatus::Unknown, Vec::new());
        };
        let missing = primitive
            .requirements()
            .into_iter()
            .filter(|requirement| !profile.satisfies(*requirement))
            .collect::<Vec<_>>();
        if missing.is_empty() {
            CapabilityEvaluation::new(CapabilitySnapshotStatus::Compatible, missing)
        } else {
            CapabilityEvaluation::new(CapabilitySnapshotStatus::Unsupported, missing)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityEvaluation {
    pub status: CapabilitySnapshotStatus,
    pub missing: Vec<CapabilityRequirement>,
}

impl CapabilityEvaluation {
    fn new(status: CapabilitySnapshotStatus, missing: Vec<CapabilityRequirement>) -> Self {
        Self { status, missing }
    }
}

impl RuntimePrimitive {
    pub fn for_task(task: &TaskKind) -> Self {
        match task {
            TaskKind::Command(_) => Self::Command,
            TaskKind::DriftCheck(_) => Self::DriftCheck,
            TaskKind::RunbookExecution(_) => Self::RunbookExecution,
        }
    }

    pub fn requirements(self) -> Vec<CapabilityRequirement> {
        match self {
            Self::Command => vec![CapabilityRequirement::Capability(
                AgentCapability::CommandExecution,
            )],
            Self::DriftCheck => {
                vec![CapabilityRequirement::Capability(
                    AgentCapability::DriftCheck,
                )]
            }
            Self::RunbookExecution => {
                vec![CapabilityRequirement::Capability(
                    AgentCapability::RunbookExecution,
                )]
            }
            Self::PackageInstall => vec![
                CapabilityRequirement::Capability(AgentCapability::PackageInstall),
                CapabilityRequirement::PrivilegeAtLeast(PrivilegeLevel::SudoAvailable),
                CapabilityRequirement::PackageManager,
            ],
            Self::ServiceControl => vec![
                CapabilityRequirement::Capability(AgentCapability::ServiceControl),
                CapabilityRequirement::PrivilegeAtLeast(PrivilegeLevel::SudoAvailable),
                CapabilityRequirement::ServiceManager,
            ],
            Self::FileCopy => vec![CapabilityRequirement::Capability(AgentCapability::FileCopy)],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_profile_satisfies_package_and_service_primitives() {
        let snapshot = AgentCapabilitySnapshot::reported(
            AgentRuntimeProfile::new(
                PrivilegeLevel::Root,
                Some(PackageManager::Apt),
                Some(ServiceManager::Systemd),
                vec![
                    AgentCapability::PackageInstall,
                    AgentCapability::ServiceControl,
                ],
            ),
            SystemTime::UNIX_EPOCH + Duration::from_secs(10),
        );

        assert_eq!(
            snapshot.evaluate(RuntimePrimitive::PackageInstall).status,
            CapabilitySnapshotStatus::Compatible
        );
        assert_eq!(
            snapshot.evaluate(RuntimePrimitive::ServiceControl).status,
            CapabilitySnapshotStatus::Compatible
        );
    }

    #[test]
    fn unprivileged_profile_reports_missing_capabilities() {
        let snapshot = AgentCapabilitySnapshot::reported(
            AgentRuntimeProfile::new(PrivilegeLevel::Unprivileged, None, None, Vec::new()),
            SystemTime::UNIX_EPOCH,
        );

        let result = snapshot.evaluate(RuntimePrimitive::PackageInstall);

        assert_eq!(result.status, CapabilitySnapshotStatus::Unsupported);
        assert!(result.missing.contains(&CapabilityRequirement::Capability(
            AgentCapability::PackageInstall
        )));
        assert!(
            result
                .missing
                .contains(&CapabilityRequirement::PrivilegeAtLeast(
                    PrivilegeLevel::SudoAvailable
                ))
        );
        assert!(
            result
                .missing
                .contains(&CapabilityRequirement::PackageManager)
        );
    }

    #[test]
    fn unknown_and_stale_snapshots_are_explicit_states() {
        let unknown = AgentCapabilitySnapshot::unknown();
        assert_eq!(
            unknown.evaluate(RuntimePrimitive::Command).status,
            CapabilitySnapshotStatus::Unknown
        );

        let stale = AgentCapabilitySnapshot::reported(
            AgentRuntimeProfile::new(
                PrivilegeLevel::SudoAvailable,
                Some(PackageManager::Apt),
                Some(ServiceManager::Systemd),
                vec![AgentCapability::CommandExecution],
            ),
            SystemTime::UNIX_EPOCH,
        )
        .stale_if_older_than(
            SystemTime::UNIX_EPOCH + Duration::from_secs(301),
            Duration::from_secs(300),
        );

        assert_eq!(
            stale.evaluate(RuntimePrimitive::Command).status,
            CapabilitySnapshotStatus::Stale
        );
    }
}
