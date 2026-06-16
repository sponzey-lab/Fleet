use fleet_application::{
    AdminTokenRecord as AppAdminTokenRecord, AdminTokenRepository,
    AgentIdentityRecord as AppAgentIdentityRecord, AgentIdentityRepository,
    AgentLogChunkPageRecord as AppAgentLogChunkPageRecord, AgentLogRepository, AgentRepository,
    ApprovalRepository, ApprovalRequestRecord as AppApprovalRequestRecord, AuditRepository,
    AuditWriter, CommandJobRepository, ControllerIdentityMetadata, ControllerIdentityRepository,
    DispatchAssignmentRepository, DriftCheckJobRepository,
    DriftReportPageRecord as AppDriftReportPageRecord, DriftReportRecord as AppDriftReportRecord,
    DriftRepository, EnrollmentTokenRecord as AppEnrollmentTokenRecord, EnrollmentTokenRepository,
    FactsRepository, FactsSnapshotPageRecord as AppFactsSnapshotPageRecord,
    FactsSnapshotRecord as AppFactsSnapshotRecord, JobDispatchGate as AppJobDispatchGate,
    JobOutputChunk, JobOutputRepository, JobOutputStream, JobQueryRepository, JobRepository,
    JobSummaryRecord as AppJobSummaryRecord, JobTargetSummaryRecord as AppJobTargetSummaryRecord,
    MetricsRepository, MetricsSnapshotPageRecord as AppMetricsSnapshotPageRecord,
    MetricsSnapshotRecord as AppMetricsSnapshotRecord,
    PendingTaskAssignment as AppPendingTaskAssignment,
    PolicyAssignmentRecord as AppPolicyAssignmentRecord, PolicyRecord as AppPolicyRecord,
    PolicyRepository, RunbookJobRepository, ScheduledDriftRecord as AppScheduledDriftRecord,
    SnapshotPageCursor, TaskAssignmentRepository,
};
use fleet_domain::{
    Agent, AgentError, AgentFingerprint, AgentId, AgentIdentity, AgentLabel, AgentName,
    AgentPublicKey, AgentStatus, AssignmentStatus, AuditActor, AuditCategory, AuditEvent,
    AuditTarget, AuditValue, CommandTask, ControllerPublicKey, DriftAcknowledgement,
    DriftCheckTask, DriftReport, DriftSeverity, DriftStatus, Job, JobId, JobStatus,
    RunbookExecutionTask, TaskEnvelope, TaskExpiry, TaskId, TaskKind, TaskNonce, TaskSignature,
    aggregate_job_status,
};
use rusqlite::{Connection, ErrorCode, OptionalExtension, params};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const CURRENT_SCHEMA_VERSION: i64 = 9;

#[derive(Debug)]
pub enum StoreError {
    DuplicateAgent,
    ConstraintViolation(String),
    NotFound,
    Sqlite(rusqlite::Error),
    Domain(String),
}

impl PartialEq for StoreError {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::DuplicateAgent, Self::DuplicateAgent)
                | (Self::NotFound, Self::NotFound)
                | (Self::ConstraintViolation(_), Self::ConstraintViolation(_))
                | (Self::Sqlite(_), Self::Sqlite(_))
                | (Self::Domain(_), Self::Domain(_))
        )
    }
}

impl Eq for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(value: rusqlite::Error) -> Self {
        if let rusqlite::Error::SqliteFailure(error, Some(message)) = &value
            && error.code == ErrorCode::ConstraintViolation
        {
            return Self::ConstraintViolation(message.clone());
        }
        Self::Sqlite(value)
    }
}

impl From<AgentError> for StoreError {
    fn from(value: AgentError) -> Self {
        Self::Domain(value.to_string())
    }
}

#[derive(Default)]
pub struct MemoryAgentRepository {
    agents: BTreeMap<String, Agent>,
}

impl AgentRepository for MemoryAgentRepository {
    type Error = StoreError;

    fn save(&mut self, agent: Agent) -> Result<(), Self::Error> {
        let key = agent.id().as_str().to_owned();
        if self.agents.contains_key(&key) {
            return Err(StoreError::DuplicateAgent);
        }
        self.agents.insert(key, agent);
        Ok(())
    }

    fn find_by_id(&self, id: &AgentId) -> Result<Option<Agent>, Self::Error> {
        Ok(self.agents.get(id.as_str()).cloned())
    }

    fn list(&self) -> Result<Vec<Agent>, Self::Error> {
        Ok(self.agents.values().cloned().collect())
    }
}

pub struct SqliteStore {
    connection: Connection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollmentTokenRecord {
    pub id: String,
    pub default_labels: String,
    pub expires_at: SystemTime,
    pub max_uses: u32,
    pub used_count: u32,
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCommandAssignment {
    pub envelope: TaskEnvelope,
    pub command: CommandTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDriftCheckAssignment {
    pub envelope: TaskEnvelope,
    pub drift_check: DriftCheckTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRunbookAssignment {
    pub envelope: TaskEnvelope,
    pub runbook: RunbookExecutionTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAssignmentStateRecord {
    pub job_id: String,
    pub task_id: String,
    pub agent_id: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAssignmentSummaryRecord {
    pub job_id: String,
    pub task_id: String,
    pub agent_id: String,
    pub status: String,
    pub last_error: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobStrategyRecord {
    pub concurrency: u32,
    pub max_failures: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobDispatchGateRecord {
    pub concurrency: u32,
    pub max_failures: Option<u32>,
    pub active_count: usize,
    pub failure_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequestRecord {
    pub id: String,
    pub job_id: String,
    pub requester: String,
    pub approver: Option<String>,
    pub reason: String,
    pub status: String,
    pub expires_at: SystemTime,
    pub created_at: SystemTime,
    pub decided_at: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobSummaryRecord {
    pub id: String,
    pub status: String,
    pub risk: String,
    pub command_program: Option<String>,
    pub command_args: Vec<String>,
    pub selector_kind: String,
    pub selector_source: String,
    pub strategy_concurrency: u32,
    pub strategy_max_failures: Option<u32>,
    pub target_count: usize,
    pub target_agents: Vec<JobTargetSummaryRecord>,
    pub created_at: SystemTime,
    pub expires_at: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobTargetSummaryRecord {
    pub agent_id: String,
    pub agent_name: String,
    pub status: String,
    pub labels: Vec<(String, String)>,
    pub task_id: Option<String>,
    pub assignment_status: Option<String>,
    pub last_error: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactsSnapshotRecord {
    pub agent_id: String,
    pub body: String,
    pub collected_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactsSnapshotPageRecord {
    pub agent_id: String,
    pub body: String,
    pub collected_at: SystemTime,
    pub cursor: SnapshotPageCursor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsSnapshotRecord {
    pub agent_id: String,
    pub body: String,
    pub collected_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsSnapshotPageRecord {
    pub agent_id: String,
    pub body: String,
    pub collected_at: SystemTime,
    pub cursor: SnapshotPageCursor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftReportRecord {
    pub agent_id: String,
    pub report: DriftReport,
    pub checked_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftReportPageRecord {
    pub agent_id: String,
    pub report: DriftReport,
    pub checked_at: SystemTime,
    pub cursor: SnapshotPageCursor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRecord {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub source: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyAssignmentRecord {
    pub policy_id: String,
    pub agent_id: String,
    pub assigned_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledDriftRecord {
    pub policy_id: String,
    pub agent_id: String,
    pub interval_seconds: u64,
    pub next_due_at: SystemTime,
    pub last_checked_at: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLogChunkRecord {
    pub agent_id: String,
    pub line: String,
    pub collected_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLogChunkPageRecord {
    pub agent_id: String,
    pub line: String,
    pub collected_at: SystemTime,
    pub cursor: SnapshotPageCursor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionCleanupSummary {
    pub job_output_chunks: usize,
    pub facts_snapshots: usize,
    pub metrics_snapshots: usize,
    pub agent_log_chunks: usize,
}

impl RetentionCleanupSummary {
    pub fn total(self) -> usize {
        self.job_output_chunks
            + self.facts_snapshots
            + self.metrics_snapshots
            + self.agent_log_chunks
    }
}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self, StoreError> {
        let connection = Connection::open_in_memory()?;
        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    pub fn migrate(&self) -> Result<(), StoreError> {
        self.connection.execute_batch(SCHEMA_SQL)?;
        self.ensure_column(
            "jobs",
            "drift_policy_document",
            "ALTER TABLE jobs ADD COLUMN drift_policy_document TEXT",
        )?;
        self.ensure_column(
            "jobs",
            "runbook_document",
            "ALTER TABLE jobs ADD COLUMN runbook_document TEXT",
        )?;
        self.ensure_column(
            "jobs",
            "selector_kind",
            "ALTER TABLE jobs ADD COLUMN selector_kind TEXT NOT NULL DEFAULT 'explicit_ids'",
        )?;
        self.ensure_column(
            "jobs",
            "selector_source",
            "ALTER TABLE jobs ADD COLUMN selector_source TEXT NOT NULL DEFAULT ''",
        )?;
        self.ensure_column(
            "jobs",
            "strategy_concurrency",
            "ALTER TABLE jobs ADD COLUMN strategy_concurrency INTEGER NOT NULL DEFAULT 1",
        )?;
        self.ensure_column(
            "jobs",
            "strategy_max_failures",
            "ALTER TABLE jobs ADD COLUMN strategy_max_failures INTEGER",
        )?;
        self.ensure_column(
            "job_targets",
            "agent_display_name",
            "ALTER TABLE job_targets ADD COLUMN agent_display_name TEXT NOT NULL DEFAULT ''",
        )?;
        self.ensure_column(
            "job_targets",
            "agent_status_snapshot",
            "ALTER TABLE job_targets ADD COLUMN agent_status_snapshot TEXT NOT NULL DEFAULT 'unknown'",
        )?;
        self.ensure_column(
            "job_targets",
            "labels_snapshot",
            "ALTER TABLE job_targets ADD COLUMN labels_snapshot TEXT NOT NULL DEFAULT ''",
        )?;
        self.ensure_column(
            "task_assignments",
            "status",
            "ALTER TABLE task_assignments ADD COLUMN status TEXT NOT NULL DEFAULT 'queued'",
        )?;
        self.ensure_column(
            "task_assignments",
            "dispatched_at",
            "ALTER TABLE task_assignments ADD COLUMN dispatched_at INTEGER",
        )?;
        self.ensure_column(
            "task_assignments",
            "accepted_at",
            "ALTER TABLE task_assignments ADD COLUMN accepted_at INTEGER",
        )?;
        self.ensure_column(
            "task_assignments",
            "started_at",
            "ALTER TABLE task_assignments ADD COLUMN started_at INTEGER",
        )?;
        self.ensure_column(
            "task_assignments",
            "completed_at",
            "ALTER TABLE task_assignments ADD COLUMN completed_at INTEGER",
        )?;
        self.ensure_column(
            "task_assignments",
            "last_error",
            "ALTER TABLE task_assignments ADD COLUMN last_error TEXT NOT NULL DEFAULT ''",
        )?;
        self.ensure_column(
            "admin_tokens",
            "actor_id",
            "ALTER TABLE admin_tokens ADD COLUMN actor_id TEXT NOT NULL DEFAULT 'bootstrap-admin'",
        )?;
        self.ensure_column(
            "admin_tokens",
            "role",
            "ALTER TABLE admin_tokens ADD COLUMN role TEXT NOT NULL DEFAULT 'owner'",
        )?;
        self.ensure_column(
            "drift_reports",
            "severity",
            "ALTER TABLE drift_reports ADD COLUMN severity TEXT NOT NULL DEFAULT 'unknown'",
        )?;
        self.ensure_column(
            "drift_reports",
            "acknowledged_at",
            "ALTER TABLE drift_reports ADD COLUMN acknowledged_at INTEGER",
        )?;
        self.ensure_column(
            "drift_reports",
            "acknowledged_by",
            "ALTER TABLE drift_reports ADD COLUMN acknowledged_by TEXT",
        )?;
        self.ensure_column(
            "drift_reports",
            "resolved_at",
            "ALTER TABLE drift_reports ADD COLUMN resolved_at INTEGER",
        )?;
        self.ensure_column(
            "drift_reports",
            "resolution_job_id",
            "ALTER TABLE drift_reports ADD COLUMN resolution_job_id TEXT",
        )?;
        self.record_schema_version()?;
        Ok(())
    }

    fn record_schema_version(&self) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO schema_migrations (name, version, applied_at)
             VALUES ('fleet_store', ?1, unixepoch())
             ON CONFLICT(name) DO UPDATE SET
                version = excluded.version,
                applied_at = CASE
                    WHEN schema_migrations.version = excluded.version
                    THEN schema_migrations.applied_at
                    ELSE excluded.applied_at
                END",
            params![CURRENT_SCHEMA_VERSION],
        )?;
        Ok(())
    }

    pub fn current_schema_version(&self) -> Result<Option<i64>, StoreError> {
        self.connection
            .query_row(
                "SELECT version FROM schema_migrations WHERE name = 'fleet_store'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn integrity_check(&self) -> Result<String, StoreError> {
        self.connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(StoreError::from)
    }

    fn ensure_column(&self, table: &str, column: &str, statement: &str) -> Result<(), StoreError> {
        if !self.has_column(table, column)? {
            self.connection.execute(statement, [])?;
        }
        Ok(())
    }

    pub fn has_column(&self, table: &str, column: &str) -> Result<bool, StoreError> {
        let mut statement = self
            .connection
            .prepare(&format!("PRAGMA table_info({table})"))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get(1)?;
            if name == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn insert_admin_token_hash(&self, token_hash: &str) -> Result<(), StoreError> {
        self.insert_admin_token_hash_with_identity(token_hash, "bootstrap-admin", "owner")
    }

    pub fn insert_admin_token_hash_with_identity(
        &self,
        token_hash: &str,
        actor_id: &str,
        role: &str,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO admin_tokens (id, token_hash, actor_id, role, created_at)
             VALUES (1, ?1, ?2, ?3, unixepoch())
             ON CONFLICT(id) DO NOTHING",
            params![token_hash, actor_id, role],
        )?;
        Ok(())
    }

    pub fn admin_token_exists(&self) -> Result<bool, StoreError> {
        Ok(self
            .connection
            .prepare("SELECT 1 FROM admin_tokens WHERE id = 1")?
            .exists([])?)
    }

    pub fn verify_admin_token_hash(&self, token_hash: &str) -> Result<bool, StoreError> {
        Ok(self
            .connection
            .prepare("SELECT 1 FROM admin_tokens WHERE id = 1 AND token_hash = ?1")?
            .exists(params![token_hash])?)
    }

    pub fn find_admin_token_record(
        &self,
        token_hash: &str,
    ) -> Result<Option<AppAdminTokenRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT actor_id, role
                 FROM admin_tokens
                 WHERE id = 1 AND token_hash = ?1",
                params![token_hash],
                |row| {
                    Ok(AppAdminTokenRecord {
                        actor_id: row.get(0)?,
                        role: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn insert_enrollment_token_hash(
        &self,
        id: &str,
        token_hash: &str,
        default_labels: &str,
        expires_at: SystemTime,
        max_uses: u32,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO enrollment_tokens (
                id, token_hash, default_labels, expires_at, max_uses, used_count
             ) VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            params![
                id,
                token_hash,
                default_labels,
                system_time_to_unix_secs(expires_at),
                max_uses,
            ],
        )?;
        Ok(())
    }

    pub fn list_enrollment_tokens(&self) -> Result<Vec<EnrollmentTokenRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, default_labels, expires_at, max_uses, used_count, revoked_at
             FROM enrollment_tokens
             ORDER BY created_at DESC",
        )?;
        let mut rows = statement.query([])?;
        let mut records = Vec::new();
        while let Some(row) = rows.next()? {
            records.push(EnrollmentTokenRecord {
                id: row.get(0)?,
                default_labels: row.get(1)?,
                expires_at: unix_secs_to_system_time(row.get(2)?),
                max_uses: row.get::<_, i64>(3)?.max(0) as u32,
                used_count: row.get::<_, i64>(4)?.max(0) as u32,
                revoked: row.get::<_, Option<i64>>(5)?.is_some(),
            });
        }
        Ok(records)
    }

    pub fn revoke_enrollment_token(&self, id: &str) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE enrollment_tokens
             SET revoked_at = unixepoch()
             WHERE id = ?1 AND revoked_at IS NULL",
            params![id],
        )?;
        Ok(changed > 0)
    }

    pub fn consume_enrollment_token_hash(
        &self,
        token_hash: &str,
        now: SystemTime,
    ) -> Result<EnrollmentTokenRecord, StoreError> {
        let record = self
            .connection
            .query_row(
                "SELECT id, default_labels, expires_at, max_uses, used_count, revoked_at
                 FROM enrollment_tokens
                 WHERE token_hash = ?1",
                params![token_hash],
                |row| {
                    Ok(EnrollmentTokenRecord {
                        id: row.get(0)?,
                        default_labels: row.get(1)?,
                        expires_at: unix_secs_to_system_time(row.get(2)?),
                        max_uses: row.get::<_, i64>(3)?.max(0) as u32,
                        used_count: row.get::<_, i64>(4)?.max(0) as u32,
                        revoked: row.get::<_, Option<i64>>(5)?.is_some(),
                    })
                },
            )
            .optional()?
            .ok_or(StoreError::NotFound)?;

        if record.revoked {
            return Err(StoreError::Domain("enrollment token is revoked".to_owned()));
        }
        if now >= record.expires_at {
            return Err(StoreError::Domain("enrollment token is expired".to_owned()));
        }
        if record.used_count >= record.max_uses {
            return Err(StoreError::Domain(
                "enrollment token max uses exceeded".to_owned(),
            ));
        }

        self.connection.execute(
            "UPDATE enrollment_tokens
             SET used_count = used_count + 1
             WHERE id = ?1",
            params![record.id],
        )?;

        Ok(record)
    }

    pub fn save_agent(&self, agent: Agent) -> Result<(), StoreError> {
        self.insert_agent(&agent)
    }

    pub fn agent_count(&self) -> Result<usize, StoreError> {
        let count: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM agents", [], |row| row.get(0))?;
        Ok(count.max(0) as usize)
    }

    pub fn list_agents(&self) -> Result<Vec<Agent>, StoreError> {
        <Self as AgentRepository>::list(self)
    }

    pub fn find_agent_by_id(&self, agent_id: &str) -> Result<Option<Agent>, StoreError> {
        let agent_id = AgentId::new(agent_id).map_err(StoreError::from)?;
        <Self as AgentRepository>::find_by_id(self, &agent_id)
    }

    pub fn update_agent_labels(
        &self,
        agent_id: &str,
        labels: &[AgentLabel],
    ) -> Result<bool, StoreError> {
        let labels = encode_labels(labels);
        let changed = self.connection.execute(
            "UPDATE agents
             SET labels = ?2, updated_at = unixepoch()
             WHERE id = ?1",
            params![agent_id, labels],
        )?;
        Ok(changed > 0)
    }

    pub fn revoke_agent_key(&self, agent_id: &str) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE agents
             SET status = 'disabled', updated_at = unixepoch()
             WHERE id = ?1",
            params![agent_id],
        )?;
        Ok(changed > 0)
    }

    pub fn insert_facts_snapshot(
        &self,
        agent_id: &str,
        body: &str,
        collected_at: SystemTime,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO facts_snapshots (agent_id, body, collected_at)
             VALUES (?1, ?2, ?3)",
            params![agent_id, body, system_time_to_unix_secs(collected_at)],
        )?;
        Ok(())
    }

    pub fn latest_facts_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<FactsSnapshotRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT agent_id, body, collected_at
                 FROM facts_snapshots
                 WHERE agent_id = ?1
                 ORDER BY collected_at DESC, id DESC
                 LIMIT 1",
                params![agent_id],
                |row| {
                    Ok(FactsSnapshotRecord {
                        agent_id: row.get(0)?,
                        body: row.get(1)?,
                        collected_at: unix_secs_to_system_time(row.get(2)?),
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn list_facts_snapshots(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<FactsSnapshotPageRecord>, StoreError> {
        let limit = limit.clamp(1, 501) as i64;
        if let Some(before) = before {
            let before_secs = system_time_to_unix_secs(before.occurred_at);
            let mut statement = self.connection.prepare(
                "SELECT id, agent_id, body, collected_at
                 FROM facts_snapshots
                 WHERE agent_id = ?1
                   AND (collected_at < ?2 OR (collected_at = ?2 AND id < ?3))
                 ORDER BY collected_at DESC, id DESC
                 LIMIT ?4",
            )?;
            return statement
                .query_map(
                    params![agent_id, before_secs, before.row_id, limit],
                    row_to_facts_snapshot_page_record,
                )?
                .collect::<Result<Vec<_>, _>>()
                .map_err(StoreError::from);
        }

        let mut statement = self.connection.prepare(
            "SELECT id, agent_id, body, collected_at
             FROM facts_snapshots
             WHERE agent_id = ?1
             ORDER BY collected_at DESC, id DESC
             LIMIT ?2",
        )?;
        statement
            .query_map(params![agent_id, limit], row_to_facts_snapshot_page_record)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn insert_metrics_snapshot(
        &self,
        agent_id: &str,
        body: &str,
        collected_at: SystemTime,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO metrics_snapshots (agent_id, body, collected_at)
             VALUES (?1, ?2, ?3)",
            params![agent_id, body, system_time_to_unix_secs(collected_at)],
        )?;
        Ok(())
    }

    pub fn latest_metrics_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<MetricsSnapshotRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT agent_id, body, collected_at
                 FROM metrics_snapshots
                 WHERE agent_id = ?1
                 ORDER BY collected_at DESC, id DESC
                 LIMIT 1",
                params![agent_id],
                |row| {
                    Ok(MetricsSnapshotRecord {
                        agent_id: row.get(0)?,
                        body: row.get(1)?,
                        collected_at: unix_secs_to_system_time(row.get(2)?),
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn list_metrics_snapshots(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<MetricsSnapshotPageRecord>, StoreError> {
        let limit = limit.clamp(1, 501) as i64;
        if let Some(before) = before {
            let before_secs = system_time_to_unix_secs(before.occurred_at);
            let mut statement = self.connection.prepare(
                "SELECT id, agent_id, body, collected_at
                 FROM metrics_snapshots
                 WHERE agent_id = ?1
                   AND (collected_at < ?2 OR (collected_at = ?2 AND id < ?3))
                 ORDER BY collected_at DESC, id DESC
                 LIMIT ?4",
            )?;
            return statement
                .query_map(
                    params![agent_id, before_secs, before.row_id, limit],
                    row_to_metrics_snapshot_page_record,
                )?
                .collect::<Result<Vec<_>, _>>()
                .map_err(StoreError::from);
        }

        let mut statement = self.connection.prepare(
            "SELECT id, agent_id, body, collected_at
             FROM metrics_snapshots
             WHERE agent_id = ?1
             ORDER BY collected_at DESC, id DESC
             LIMIT ?2",
        )?;
        statement
            .query_map(
                params![agent_id, limit],
                row_to_metrics_snapshot_page_record,
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn insert_drift_report(
        &self,
        agent_id: &str,
        report: &DriftReport,
        checked_at: SystemTime,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO drift_reports (
                agent_id, policy_name, status, severity, expected, actual, checked_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                agent_id,
                report.policy_name.as_str(),
                drift_status_to_str(&report.status),
                drift_severity_to_str(report.severity),
                report.expected.as_str(),
                report.actual.as_str(),
                system_time_to_unix_secs(checked_at),
            ],
        )?;
        Ok(())
    }

    pub fn latest_drift_report(
        &self,
        agent_id: &str,
    ) -> Result<Option<DriftReportRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT agent_id, policy_name, status, expected, actual, checked_at
                    , severity, acknowledged_at, acknowledged_by, resolved_at, resolution_job_id
                 FROM drift_reports
                 WHERE agent_id = ?1
                 ORDER BY checked_at DESC, id DESC
                 LIMIT 1",
                params![agent_id],
                |row| {
                    Ok(DriftReportRecord {
                        agent_id: row.get(0)?,
                        report: DriftReport {
                            policy_name: row.get(1)?,
                            status: parse_drift_status(&row.get::<_, String>(2)?),
                            severity: parse_drift_severity(&row.get::<_, String>(6)?),
                            acknowledgement: row_to_drift_acknowledgement(row, 7, 8, 9, 10)?,
                            expected: row.get(3)?,
                            actual: row.get(4)?,
                        },
                        checked_at: unix_secs_to_system_time(row.get(5)?),
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn list_drift_reports(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<DriftReportPageRecord>, StoreError> {
        let limit = limit.clamp(1, 501) as i64;
        if let Some(before) = before {
            let before_secs = system_time_to_unix_secs(before.occurred_at);
            let mut statement = self.connection.prepare(
                "SELECT id, agent_id, policy_name, status, expected, actual, checked_at
                    , severity, acknowledged_at, acknowledged_by, resolved_at, resolution_job_id
                 FROM drift_reports
                 WHERE agent_id = ?1
                   AND (checked_at < ?2 OR (checked_at = ?2 AND id < ?3))
                 ORDER BY checked_at DESC, id DESC
                 LIMIT ?4",
            )?;
            return statement
                .query_map(
                    params![agent_id, before_secs, before.row_id, limit],
                    row_to_drift_report_page_record,
                )?
                .collect::<Result<Vec<_>, _>>()
                .map_err(StoreError::from);
        }

        let mut statement = self.connection.prepare(
            "SELECT id, agent_id, policy_name, status, expected, actual, checked_at
                , severity, acknowledged_at, acknowledged_by, resolved_at, resolution_job_id
             FROM drift_reports
             WHERE agent_id = ?1
             ORDER BY checked_at DESC, id DESC
             LIMIT ?2",
        )?;
        statement
            .query_map(params![agent_id, limit], row_to_drift_report_page_record)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn save_policy_source(
        &self,
        policy_id: &str,
        name: &str,
        version: u32,
        source: &str,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO policies (id, name, version, source, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, unixepoch(), unixepoch())
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                version = excluded.version,
                source = excluded.source,
                updated_at = excluded.updated_at",
            params![policy_id, name, version as i64, source],
        )?;
        Ok(())
    }

    pub fn list_policies(&self) -> Result<Vec<PolicyRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, name, version, source, created_at, updated_at
             FROM policies
             ORDER BY id",
        )?;
        statement
            .query_map([], row_to_policy_record)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn find_policy(&self, policy_id: &str) -> Result<Option<PolicyRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT id, name, version, source, created_at, updated_at
                 FROM policies
                 WHERE id = ?1",
                params![policy_id],
                row_to_policy_record,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn assign_policy_to_agent(
        &self,
        policy_id: &str,
        agent_id: &str,
        assigned_at: SystemTime,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO policy_assignments (policy_id, agent_id, assigned_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(policy_id, agent_id) DO UPDATE SET
                assigned_at = excluded.assigned_at",
            params![policy_id, agent_id, system_time_to_unix_secs(assigned_at)],
        )?;
        Ok(())
    }

    pub fn policies_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<PolicyAssignmentRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT policy_id, agent_id, assigned_at
             FROM policy_assignments
             WHERE agent_id = ?1
             ORDER BY policy_id",
        )?;
        statement
            .query_map(params![agent_id], row_to_policy_assignment_record)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn assigned_policy_ids_for_agent(&self, agent_id: &str) -> Result<Vec<String>, StoreError> {
        Ok(self
            .policies_for_agent(agent_id)?
            .into_iter()
            .map(|record| record.policy_id)
            .collect())
    }

    pub fn upsert_policy_schedule(
        &self,
        policy_id: &str,
        agent_id: &str,
        interval: Duration,
        next_due_at: SystemTime,
    ) -> Result<(), StoreError> {
        if interval.is_zero() {
            return Err(StoreError::Domain(
                "policy schedule interval must be positive".to_owned(),
            ));
        }
        self.connection.execute(
            "INSERT INTO policy_drift_schedules (
                policy_id, agent_id, interval_seconds, next_due_at, last_checked_at
             ) VALUES (?1, ?2, ?3, ?4, NULL)
             ON CONFLICT(policy_id, agent_id) DO UPDATE SET
                interval_seconds = excluded.interval_seconds,
                next_due_at = excluded.next_due_at",
            params![
                policy_id,
                agent_id,
                interval.as_secs().max(1) as i64,
                system_time_to_unix_secs(next_due_at),
            ],
        )?;
        Ok(())
    }

    pub fn due_scheduled_drift_checks(
        &self,
        now: SystemTime,
        limit: usize,
    ) -> Result<Vec<ScheduledDriftRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT policy_id, agent_id, interval_seconds, next_due_at, last_checked_at
             FROM policy_drift_schedules
             WHERE next_due_at <= ?1
             ORDER BY next_due_at ASC
             LIMIT ?2",
        )?;
        statement
            .query_map(
                params![system_time_to_unix_secs(now), limit.clamp(1, 500) as i64],
                row_to_scheduled_drift_record,
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn record_scheduled_drift_check(
        &self,
        policy_id: &str,
        agent_id: &str,
        checked_at: SystemTime,
    ) -> Result<(), StoreError> {
        let changed = self.connection.execute(
            "UPDATE policy_drift_schedules
             SET last_checked_at = ?3,
                 next_due_at = ?3 + interval_seconds
             WHERE policy_id = ?1 AND agent_id = ?2",
            params![policy_id, agent_id, system_time_to_unix_secs(checked_at),],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    pub fn acknowledge_latest_drift_report(
        &self,
        agent_id: &str,
        policy_name: &str,
        actor: &str,
        acknowledged_at: SystemTime,
    ) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE drift_reports
             SET acknowledged_at = ?3, acknowledged_by = ?4
             WHERE id = (
                SELECT id FROM drift_reports
                WHERE agent_id = ?1 AND policy_name = ?2
                ORDER BY checked_at DESC, id DESC
                LIMIT 1
             )",
            params![
                agent_id,
                policy_name,
                system_time_to_unix_secs(acknowledged_at),
                actor,
            ],
        )?;
        Ok(changed > 0)
    }

    pub fn mark_latest_drift_resolved(
        &self,
        agent_id: &str,
        policy_name: &str,
        job_id: &str,
        resolved_at: SystemTime,
    ) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE drift_reports
             SET resolved_at = ?3, resolution_job_id = ?4
             WHERE id = (
                SELECT id FROM drift_reports
                WHERE agent_id = ?1 AND policy_name = ?2
                ORDER BY checked_at DESC, id DESC
                LIMIT 1
             )",
            params![
                agent_id,
                policy_name,
                system_time_to_unix_secs(resolved_at),
                job_id,
            ],
        )?;
        Ok(changed > 0)
    }

    pub fn insert_agent_log_chunk(
        &self,
        agent_id: &str,
        line: &str,
        collected_at: SystemTime,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO agent_log_chunks (agent_id, line, collected_at)
             VALUES (?1, ?2, ?3)",
            params![agent_id, line, system_time_to_unix_secs(collected_at)],
        )?;
        Ok(())
    }

    pub fn list_agent_log_chunks(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<AgentLogChunkRecord>, StoreError> {
        let limit = limit.clamp(1, 500) as i64;
        let mut statement = self.connection.prepare(
            "SELECT agent_id, line, collected_at
             FROM agent_log_chunks
             WHERE agent_id = ?1
             ORDER BY collected_at DESC, id DESC
             LIMIT ?2",
        )?;
        statement
            .query_map(params![agent_id, limit], |row| {
                Ok(AgentLogChunkRecord {
                    agent_id: row.get(0)?,
                    line: row.get(1)?,
                    collected_at: unix_secs_to_system_time(row.get(2)?),
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn list_agent_log_chunks_page(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AgentLogChunkPageRecord>, StoreError> {
        let limit = limit.clamp(1, 501) as i64;
        if let Some(before) = before {
            let before_secs = system_time_to_unix_secs(before.occurred_at);
            let mut statement = self.connection.prepare(
                "SELECT id, agent_id, line, collected_at
                 FROM agent_log_chunks
                 WHERE agent_id = ?1
                   AND (collected_at < ?2 OR (collected_at = ?2 AND id < ?3))
                 ORDER BY collected_at DESC, id DESC
                 LIMIT ?4",
            )?;
            return statement
                .query_map(
                    params![agent_id, before_secs, before.row_id, limit],
                    row_to_agent_log_chunk_page_record,
                )?
                .collect::<Result<Vec<_>, _>>()
                .map_err(StoreError::from);
        }

        let mut statement = self.connection.prepare(
            "SELECT id, agent_id, line, collected_at
             FROM agent_log_chunks
             WHERE agent_id = ?1
             ORDER BY collected_at DESC, id DESC
             LIMIT ?2",
        )?;
        statement
            .query_map(params![agent_id, limit], row_to_agent_log_chunk_page_record)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn cleanup_retention(
        &self,
        cutoff: SystemTime,
        dry_run: bool,
    ) -> Result<RetentionCleanupSummary, StoreError> {
        let cutoff = system_time_to_unix_secs(cutoff);
        let summary = RetentionCleanupSummary {
            job_output_chunks: self.count_before("job_output_chunks", "created_at", cutoff)?,
            facts_snapshots: self.count_before("facts_snapshots", "collected_at", cutoff)?,
            metrics_snapshots: self.count_before("metrics_snapshots", "collected_at", cutoff)?,
            agent_log_chunks: self.count_before("agent_log_chunks", "collected_at", cutoff)?,
        };
        if dry_run {
            return Ok(summary);
        }
        self.delete_before("job_output_chunks", "created_at", cutoff)?;
        self.delete_before("facts_snapshots", "collected_at", cutoff)?;
        self.delete_before("metrics_snapshots", "collected_at", cutoff)?;
        self.delete_before("agent_log_chunks", "collected_at", cutoff)?;
        Ok(summary)
    }

    fn count_before(&self, table: &str, column: &str, cutoff: i64) -> Result<usize, StoreError> {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE {column} < ?1");
        let count: i64 = self
            .connection
            .query_row(&sql, params![cutoff], |row| row.get(0))?;
        Ok(count.max(0) as usize)
    }

    fn delete_before(&self, table: &str, column: &str, cutoff: i64) -> Result<usize, StoreError> {
        let sql = format!("DELETE FROM {table} WHERE {column} < ?1");
        self.connection
            .execute(&sql, params![cutoff])
            .map_err(StoreError::from)
    }

    pub fn write_audit_event(&self, event: AuditEvent) -> Result<(), StoreError> {
        self.insert_audit(&event)
    }

    pub fn audit_count_by_category(&self, category: AuditCategory) -> Result<usize, StoreError> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM audit_events WHERE category = ?1",
            params![category.as_str()],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as usize)
    }

    pub fn list_audit_events_by_category(
        &self,
        category: AuditCategory,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, StoreError> {
        self.query_audit(Some(category), limit)
    }

    pub fn list_audit_events(&self, limit: usize) -> Result<Vec<AuditEvent>, StoreError> {
        self.query_audit(None, limit)
    }

    pub fn find_agent_fingerprint(&self, agent_id: &str) -> Result<Option<String>, StoreError> {
        self.connection
            .query_row(
                "SELECT fingerprint FROM agents WHERE id = ?1",
                params![agent_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn find_agent_identity(
        &self,
        agent_id: &str,
    ) -> Result<Option<(String, String)>, StoreError> {
        self.connection
            .query_row(
                "SELECT public_key, fingerprint FROM agents WHERE id = ?1 AND status != 'disabled'",
                params![agent_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn mark_agent_online(&self, agent_id: &str, at: SystemTime) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE agents
             SET status = 'online', last_seen_at = ?2, updated_at = unixepoch()
             WHERE id = ?1 AND status != 'disabled'",
            params![agent_id, system_time_to_unix_secs(at)],
        )?;
        Ok(changed > 0)
    }

    pub fn mark_agent_degraded(&self, agent_id: &str, at: SystemTime) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE agents
             SET status = 'degraded', last_seen_at = ?2, updated_at = unixepoch()
             WHERE id = ?1 AND status != 'disabled'",
            params![agent_id, system_time_to_unix_secs(at)],
        )?;
        Ok(changed > 0)
    }

    pub fn mark_stale_agents_offline(
        &self,
        cutoff: SystemTime,
        now: SystemTime,
    ) -> Result<usize, StoreError> {
        let changed = self.connection.execute(
            "UPDATE agents
             SET status = 'offline', updated_at = ?2
             WHERE status IN ('online', 'busy', 'degraded')
               AND last_seen_at IS NOT NULL
               AND last_seen_at < ?1",
            params![
                system_time_to_unix_secs(cutoff),
                system_time_to_unix_secs(now),
            ],
        )?;
        Ok(changed)
    }

    pub fn save_job_record(&self, job: &Job) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO jobs (id, status, risk, approval_requirement, timeout_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                job.id().as_str(),
                job_status_to_str(job.status()),
                task_risk_to_str(job.risk()),
                approval_requirement_to_str(job.approval_requirement()),
                job.timeout().as_millis() as i64,
            ],
        )?;
        Ok(())
    }

    pub fn save_command_job_record(&self, job: &Job, task: &CommandTask) -> Result<(), StoreError> {
        let args = serde_json::to_string(task.args())
            .map_err(|error| StoreError::Domain(error.to_string()))?;
        self.connection.execute(
            "INSERT INTO jobs (
                id, status, risk, approval_requirement, timeout_ms,
                command_program, command_args_json, command_max_output_bytes
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                job.id().as_str(),
                job_status_to_str(job.status()),
                task_risk_to_str(job.risk()),
                approval_requirement_to_str(job.approval_requirement()),
                job.timeout().as_millis() as i64,
                task.program(),
                args,
                task.max_output_bytes() as i64,
            ],
        )?;
        Ok(())
    }

    pub fn save_drift_check_job_record(
        &self,
        job: &Job,
        task: &DriftCheckTask,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO jobs (
                id, status, risk, approval_requirement, timeout_ms,
                drift_policy_document
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                job.id().as_str(),
                job_status_to_str(job.status()),
                task_risk_to_str(job.risk()),
                approval_requirement_to_str(job.approval_requirement()),
                job.timeout().as_millis() as i64,
                task.policy_document(),
            ],
        )?;
        Ok(())
    }

    pub fn save_runbook_job_record(
        &self,
        job: &Job,
        task: &RunbookExecutionTask,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO jobs (
                id, status, risk, approval_requirement, timeout_ms,
                runbook_document
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                job.id().as_str(),
                job_status_to_str(job.status()),
                task_risk_to_str(job.risk()),
                approval_requirement_to_str(job.approval_requirement()),
                job.timeout().as_millis() as i64,
                task.runbook_document(),
            ],
        )?;
        Ok(())
    }

    pub fn save_task_assignment_record(&self, envelope: &TaskEnvelope) -> Result<(), StoreError> {
        let signature = envelope
            .signature
            .as_ref()
            .ok_or_else(|| StoreError::Domain("task assignment must be signed".to_owned()))?;
        self.connection.execute(
            "INSERT INTO task_assignments (
                id, job_id, agent_id, nonce, payload_hash, signature, issued_at, expires_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                envelope.task_id.as_str(),
                envelope.job_id.as_str(),
                envelope.target_agent_id.as_str(),
                envelope.nonce.as_str(),
                envelope.payload_hash.as_str(),
                signature.as_str(),
                system_time_to_unix_secs(envelope.issued_at),
                system_time_to_unix_secs(envelope.expires_at.as_system_time()),
            ],
        )?;
        self.connection.execute(
            "INSERT INTO job_targets (
                job_id, agent_id, status, agent_display_name, agent_status_snapshot, labels_snapshot
             )
             SELECT
                ?1,
                a.id,
                a.status,
                a.name,
                a.status,
                a.labels
             FROM agents a
             WHERE a.id = ?2
             ON CONFLICT(job_id, agent_id) DO NOTHING",
            params![envelope.job_id.as_str(), envelope.target_agent_id.as_str()],
        )?;
        Ok(())
    }

    pub fn update_job_selector_snapshot(
        &self,
        job_id: &str,
        selector_kind: &str,
        selector_source: &str,
    ) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE jobs
             SET selector_kind = ?2, selector_source = ?3
             WHERE id = ?1",
            params![job_id, selector_kind, selector_source],
        )?;
        Ok(changed > 0)
    }

    pub fn update_job_strategy(
        &self,
        job_id: &str,
        concurrency: u32,
        max_failures: Option<u32>,
    ) -> Result<bool, StoreError> {
        let concurrency = concurrency.max(1);
        let changed = self.connection.execute(
            "UPDATE jobs
             SET strategy_concurrency = ?2, strategy_max_failures = ?3
             WHERE id = ?1",
            params![job_id, concurrency as i64, max_failures.map(i64::from)],
        )?;
        Ok(changed > 0)
    }

    pub fn job_strategy(&self, job_id: &str) -> Result<Option<JobStrategyRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT strategy_concurrency, strategy_max_failures
                 FROM jobs
                 WHERE id = ?1",
                params![job_id],
                |row| {
                    let concurrency = row.get::<_, i64>(0)?.max(1) as u32;
                    let max_failures = row
                        .get::<_, Option<i64>>(1)?
                        .and_then(|value| u32::try_from(value).ok())
                        .filter(|value| *value > 0);
                    Ok(JobStrategyRecord {
                        concurrency,
                        max_failures,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn job_dispatch_gate(
        &self,
        job_id: &str,
    ) -> Result<Option<JobDispatchGateRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT
                    j.strategy_concurrency,
                    j.strategy_max_failures,
                    COALESCE(SUM(CASE WHEN ta.status IN ('dispatched', 'accepted', 'started') THEN 1 ELSE 0 END), 0) AS active_count,
                    COALESCE(SUM(CASE WHEN ta.status IN ('failed', 'rejected', 'expired') THEN 1 ELSE 0 END), 0) AS failure_count
                 FROM jobs j
                 LEFT JOIN task_assignments ta ON ta.job_id = j.id
                 WHERE j.id = ?1
                 GROUP BY j.id",
                params![job_id],
                |row| {
                    let concurrency = row.get::<_, i64>(0)?.max(1) as u32;
                    let max_failures = row
                        .get::<_, Option<i64>>(1)?
                        .and_then(|value| u32::try_from(value).ok())
                        .filter(|value| *value > 0);
                    Ok(JobDispatchGateRecord {
                        concurrency,
                        max_failures,
                        active_count: row.get::<_, i64>(2)?.max(0) as usize,
                        failure_count: row.get::<_, i64>(3)?.max(0) as usize,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn update_task_assignment_status(
        &self,
        task_id: &str,
        status: AssignmentStatus,
        occurred_at: SystemTime,
        last_error: Option<&str>,
    ) -> Result<bool, StoreError> {
        let status_value = assignment_status_to_str(status);
        let occurred_at = system_time_to_unix_secs(occurred_at);
        let changed = match status {
            AssignmentStatus::Dispatched => self.connection.execute(
                "UPDATE task_assignments
                 SET status = ?2, dispatched_at = ?3
                 WHERE id = ?1",
                params![task_id, status_value, occurred_at],
            )?,
            AssignmentStatus::Accepted => self.connection.execute(
                "UPDATE task_assignments
                 SET status = ?2, accepted_at = ?3
                 WHERE id = ?1",
                params![task_id, status_value, occurred_at],
            )?,
            AssignmentStatus::Started => self.connection.execute(
                "UPDATE task_assignments
                 SET status = ?2, started_at = ?3
                 WHERE id = ?1",
                params![task_id, status_value, occurred_at],
            )?,
            AssignmentStatus::Succeeded
            | AssignmentStatus::Failed
            | AssignmentStatus::Rejected
            | AssignmentStatus::Canceled
            | AssignmentStatus::Expired => self.connection.execute(
                "UPDATE task_assignments
                 SET status = ?2, completed_at = ?3, last_error = COALESCE(?4, last_error)
                 WHERE id = ?1",
                params![task_id, status_value, occurred_at, last_error],
            )?,
            AssignmentStatus::Queued => self.connection.execute(
                "UPDATE task_assignments
                 SET status = ?2, last_error = COALESCE(?3, last_error)
                 WHERE id = ?1",
                params![task_id, status_value, last_error],
            )?,
        };
        Ok(changed > 0)
    }

    pub fn update_active_task_assignment_status(
        &self,
        task_id: &str,
        status: AssignmentStatus,
        occurred_at: SystemTime,
        last_error: Option<&str>,
    ) -> Result<bool, StoreError> {
        let Some(current_status) = self.find_task_assignment_status(task_id)? else {
            return Ok(false);
        };
        if assignment_status_value_is_terminal(&current_status) {
            return Ok(false);
        }
        self.update_task_assignment_status(task_id, status, occurred_at, last_error)
    }

    pub fn find_task_assignment_status(&self, task_id: &str) -> Result<Option<String>, StoreError> {
        self.connection
            .query_row(
                "SELECT status FROM task_assignments WHERE id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn find_task_assignment_state_for_job(
        &self,
        job_id: &str,
    ) -> Result<Option<TaskAssignmentStateRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT job_id, id, agent_id, status
                 FROM task_assignments
                 WHERE job_id = ?1
                 ORDER BY created_at, id
                 LIMIT 1",
                params![job_id],
                |row| {
                    Ok(TaskAssignmentStateRecord {
                        job_id: row.get(0)?,
                        task_id: row.get(1)?,
                        agent_id: row.get(2)?,
                        status: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn list_task_assignment_summaries_for_job(
        &self,
        job_id: &str,
    ) -> Result<Vec<TaskAssignmentSummaryRecord>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT job_id, id, agent_id, status, last_error
             FROM task_assignments
             WHERE job_id = ?1
             ORDER BY created_at, id",
        )?;
        statement
            .query_map(params![job_id], |row| {
                Ok(TaskAssignmentSummaryRecord {
                    job_id: row.get(0)?,
                    task_id: row.get(1)?,
                    agent_id: row.get(2)?,
                    status: row.get(3)?,
                    last_error: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn recompute_job_status_from_assignments(
        &self,
        job_id: &str,
    ) -> Result<Option<JobStatus>, StoreError> {
        let strategy = self.job_strategy(job_id)?;
        let statuses = self
            .list_task_assignment_summaries_for_job(job_id)?
            .into_iter()
            .map(|assignment| {
                AssignmentStatus::parse(&assignment.status).ok_or_else(|| {
                    StoreError::Domain(format!(
                        "invalid task assignment status: {}",
                        assignment.status
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if statuses.is_empty() && strategy.is_none() {
            return Ok(None);
        }
        let status = aggregate_job_status(
            &statuses,
            strategy.and_then(|strategy| strategy.max_failures),
        );
        self.update_job_status(job_id, status)?;
        Ok(Some(status))
    }

    pub fn cancel_queued_assignments_after_max_failures(
        &self,
        job_id: &str,
        occurred_at: SystemTime,
        reason: &str,
    ) -> Result<usize, StoreError> {
        let Some(gate) = self.job_dispatch_gate(job_id)? else {
            return Ok(0);
        };
        if !matches!(gate.max_failures, Some(limit) if limit > 0 && gate.failure_count >= limit as usize)
        {
            return Ok(0);
        }
        let changed = self.connection.execute(
            "UPDATE task_assignments
             SET status = 'canceled', completed_at = ?2, last_error = ?3
             WHERE job_id = ?1
               AND status = 'queued'",
            params![job_id, system_time_to_unix_secs(occurred_at), reason],
        )?;
        Ok(changed)
    }

    pub fn list_pending_command_assignments_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<PendingCommandAssignment>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT
                ta.job_id, ta.id, ta.agent_id, ta.nonce, ta.payload_hash,
                ta.signature, ta.issued_at, ta.expires_at,
                j.command_program, j.command_args_json, j.timeout_ms
             FROM task_assignments ta
             JOIN jobs j ON j.id = ta.job_id
             WHERE ta.agent_id = ?1
               AND ta.status = 'queued'
               AND j.status IN ('queued', 'running')
               AND j.command_program IS NOT NULL
             ORDER BY ta.created_at, ta.id",
        )?;
        let rows = statement
            .query_map(params![agent_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, i64>(10)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(
                |(
                    job_id,
                    task_id,
                    target_agent_id,
                    nonce,
                    payload_hash,
                    signature,
                    issued_at,
                    expires_at,
                    command_program,
                    command_args_json,
                    timeout_ms,
                )| {
                    let command_args = parse_command_args(&command_args_json)?;
                    Ok(PendingCommandAssignment {
                        envelope: TaskEnvelope {
                            job_id: JobId::new(job_id)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            task_id: TaskId::new(task_id)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            target_agent_id: AgentId::new(target_agent_id)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            issued_at: unix_secs_to_system_time(issued_at),
                            expires_at: TaskExpiry::new(unix_secs_to_system_time(expires_at)),
                            nonce: TaskNonce::new(nonce)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            payload_hash,
                            signature: Some(
                                TaskSignature::new(signature)
                                    .map_err(|error| StoreError::Domain(error.to_string()))?,
                            ),
                        },
                        command: CommandTask::new(
                            command_program,
                            command_args,
                            Duration::from_millis(timeout_ms as u64),
                        )
                        .map_err(|error| StoreError::Domain(error.to_string()))?,
                    })
                },
            )
            .collect()
    }

    pub fn list_pending_drift_check_assignments_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<PendingDriftCheckAssignment>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT
                ta.job_id, ta.id, ta.agent_id, ta.nonce, ta.payload_hash,
                ta.signature, ta.issued_at, ta.expires_at,
                j.drift_policy_document, j.timeout_ms
             FROM task_assignments ta
             JOIN jobs j ON j.id = ta.job_id
             WHERE ta.agent_id = ?1
               AND ta.status = 'queued'
               AND j.status IN ('queued', 'running')
               AND j.drift_policy_document IS NOT NULL
             ORDER BY ta.created_at, ta.id",
        )?;
        let rows = statement
            .query_map(params![agent_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(
                |(
                    job_id,
                    task_id,
                    target_agent_id,
                    nonce,
                    payload_hash,
                    signature,
                    issued_at,
                    expires_at,
                    policy_document,
                    timeout_ms,
                )| {
                    Ok(PendingDriftCheckAssignment {
                        envelope: TaskEnvelope {
                            job_id: JobId::new(job_id)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            task_id: TaskId::new(task_id)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            target_agent_id: AgentId::new(target_agent_id)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            issued_at: unix_secs_to_system_time(issued_at),
                            expires_at: TaskExpiry::new(unix_secs_to_system_time(expires_at)),
                            nonce: TaskNonce::new(nonce)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            payload_hash,
                            signature: Some(
                                TaskSignature::new(signature)
                                    .map_err(|error| StoreError::Domain(error.to_string()))?,
                            ),
                        },
                        drift_check: DriftCheckTask::new(
                            policy_document,
                            Duration::from_millis(timeout_ms as u64),
                        )
                        .map_err(|error| StoreError::Domain(error.to_string()))?,
                    })
                },
            )
            .collect()
    }

    pub fn list_pending_runbook_assignments_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<PendingRunbookAssignment>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT
                ta.job_id, ta.id, ta.agent_id, ta.nonce, ta.payload_hash,
                ta.signature, ta.issued_at, ta.expires_at,
                j.runbook_document, j.timeout_ms
             FROM task_assignments ta
             JOIN jobs j ON j.id = ta.job_id
             WHERE ta.agent_id = ?1
               AND ta.status = 'queued'
               AND j.status IN ('queued', 'running')
               AND j.runbook_document IS NOT NULL
             ORDER BY ta.created_at, ta.id",
        )?;
        let rows = statement
            .query_map(params![agent_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(
                |(
                    job_id,
                    task_id,
                    target_agent_id,
                    nonce,
                    payload_hash,
                    signature,
                    issued_at,
                    expires_at,
                    runbook_document,
                    timeout_ms,
                )| {
                    Ok(PendingRunbookAssignment {
                        envelope: TaskEnvelope {
                            job_id: JobId::new(job_id)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            task_id: TaskId::new(task_id)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            target_agent_id: AgentId::new(target_agent_id)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            issued_at: unix_secs_to_system_time(issued_at),
                            expires_at: TaskExpiry::new(unix_secs_to_system_time(expires_at)),
                            nonce: TaskNonce::new(nonce)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                            payload_hash,
                            signature: Some(
                                TaskSignature::new(signature)
                                    .map_err(|error| StoreError::Domain(error.to_string()))?,
                            ),
                        },
                        runbook: RunbookExecutionTask::new(
                            runbook_document,
                            Duration::from_millis(timeout_ms as u64),
                        )
                        .map_err(|error| StoreError::Domain(error.to_string()))?,
                    })
                },
            )
            .collect()
    }

    pub fn list_pending_dispatch_assignments(
        &self,
        agent_id: Option<&AgentId>,
        job_id: Option<&JobId>,
        limit: usize,
    ) -> Result<Vec<AppPendingTaskAssignment>, StoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let agent_id = agent_id.map(AgentId::as_str);
        let job_id = job_id.map(JobId::as_str);
        let limit = limit.min(100) as i64;
        let mut statement = self.connection.prepare(
            "SELECT
                ta.job_id, ta.id, ta.agent_id, ta.nonce, ta.payload_hash,
                ta.signature, ta.issued_at, ta.expires_at,
                j.command_program, j.command_args_json, j.drift_policy_document,
                j.runbook_document, j.timeout_ms
             FROM task_assignments ta
             JOIN jobs j ON j.id = ta.job_id
             WHERE ta.status = 'queued'
               AND j.status IN ('queued', 'running')
               AND (?1 IS NULL OR ta.agent_id = ?1)
               AND (?2 IS NULL OR ta.job_id = ?2)
               AND (
                    j.command_program IS NOT NULL
                 OR j.drift_policy_document IS NOT NULL
                 OR j.runbook_document IS NOT NULL
               )
             ORDER BY ta.created_at, ta.id
             LIMIT ?3",
        )?;
        let rows = statement
            .query_map(params![agent_id, job_id, limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, i64>(12)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        rows.into_iter()
            .map(
                |(
                    job_id,
                    task_id,
                    target_agent_id,
                    nonce,
                    payload_hash,
                    signature,
                    issued_at,
                    expires_at,
                    command_program,
                    command_args_json,
                    drift_policy_document,
                    runbook_document,
                    timeout_ms,
                )| {
                    let envelope = TaskEnvelope {
                        job_id: JobId::new(job_id)
                            .map_err(|error| StoreError::Domain(error.to_string()))?,
                        task_id: TaskId::new(task_id)
                            .map_err(|error| StoreError::Domain(error.to_string()))?,
                        target_agent_id: AgentId::new(target_agent_id)
                            .map_err(|error| StoreError::Domain(error.to_string()))?,
                        issued_at: unix_secs_to_system_time(issued_at),
                        expires_at: TaskExpiry::new(unix_secs_to_system_time(expires_at)),
                        nonce: TaskNonce::new(nonce)
                            .map_err(|error| StoreError::Domain(error.to_string()))?,
                        payload_hash,
                        signature: Some(
                            TaskSignature::new(signature)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                        ),
                    };
                    let timeout = Duration::from_millis(timeout_ms as u64);
                    let task = if let Some(program) = command_program {
                        TaskKind::Command(
                            CommandTask::new(
                                program,
                                parse_command_args(&command_args_json)?,
                                timeout,
                            )
                            .map_err(|error| StoreError::Domain(error.to_string()))?,
                        )
                    } else if let Some(policy_document) = drift_policy_document {
                        TaskKind::DriftCheck(
                            DriftCheckTask::new(policy_document, timeout)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                        )
                    } else if let Some(runbook_document) = runbook_document {
                        TaskKind::RunbookExecution(
                            RunbookExecutionTask::new(runbook_document, timeout)
                                .map_err(|error| StoreError::Domain(error.to_string()))?,
                        )
                    } else {
                        return Err(StoreError::Domain(
                            "pending assignment has no task payload".to_owned(),
                        ));
                    };

                    Ok(AppPendingTaskAssignment { envelope, task })
                },
            )
            .collect()
    }

    pub fn update_job_status(&self, job_id: &str, status: JobStatus) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE jobs SET status = ?2 WHERE id = ?1",
            params![job_id, job_status_to_str(status)],
        )?;
        Ok(changed > 0)
    }

    pub fn find_job_status_value(&self, job_id: &str) -> Result<Option<String>, StoreError> {
        self.connection
            .query_row(
                "SELECT status FROM jobs WHERE id = ?1",
                params![job_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn insert_approval_request(
        &self,
        request: AppApprovalRequestRecord,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO approval_requests (
                id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                request.id,
                request.job_id,
                request.requester,
                request.approver,
                request.reason,
                request.status,
                system_time_to_unix_secs(request.expires_at),
                system_time_to_unix_secs(request.created_at),
                request.decided_at.map(system_time_to_unix_secs),
            ],
        )?;
        Ok(())
    }

    pub fn find_approval_request(
        &self,
        approval_id: &str,
    ) -> Result<Option<AppApprovalRequestRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
                 FROM approval_requests
                 WHERE id = ?1",
                params![approval_id],
                row_to_app_approval_request_record,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn find_pending_approval_for_job(
        &self,
        job_id: &str,
    ) -> Result<Option<AppApprovalRequestRecord>, StoreError> {
        self.connection
            .query_row(
                "SELECT id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
                 FROM approval_requests
                 WHERE job_id = ?1 AND status = 'pending'
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
                params![job_id],
                row_to_app_approval_request_record,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn list_approval_requests(
        &self,
        status: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AppApprovalRequestRecord>, StoreError> {
        let limit = limit.clamp(1, 500) as i64;
        let mut statement = self.connection.prepare(
            "SELECT id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
             FROM approval_requests
             WHERE (?1 IS NULL OR status = ?1)
             ORDER BY created_at DESC, id DESC
             LIMIT ?2",
        )?;
        let rows = statement
            .query_map(params![status, limit], row_to_app_approval_request_record)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn update_approval_request(
        &self,
        request: AppApprovalRequestRecord,
    ) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE approval_requests
             SET requester = ?2,
                 approver = ?3,
                 reason = ?4,
                 status = ?5,
                 expires_at = ?6,
                 created_at = ?7,
                 decided_at = ?8
             WHERE id = ?1",
            params![
                request.id,
                request.requester,
                request.approver,
                request.reason,
                request.status,
                system_time_to_unix_secs(request.expires_at),
                system_time_to_unix_secs(request.created_at),
                request.decided_at.map(system_time_to_unix_secs),
            ],
        )?;
        Ok(changed > 0)
    }

    pub fn list_job_summaries(&self, limit: usize) -> Result<Vec<JobSummaryRecord>, StoreError> {
        self.list_job_summaries_filtered(None, limit)
    }

    pub fn find_job_summary(&self, job_id: &str) -> Result<Option<JobSummaryRecord>, StoreError> {
        Ok(self
            .list_job_summaries_filtered(Some(job_id), 1)?
            .into_iter()
            .next())
    }

    fn list_job_summaries_filtered(
        &self,
        job_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<JobSummaryRecord>, StoreError> {
        let limit = limit.min(100) as i64;
        let mut statement = self.connection.prepare(
            "SELECT
                j.id,
                j.status,
                j.risk,
                j.command_program,
                j.command_args_json,
                j.created_at,
                j.selector_kind,
                j.selector_source,
                j.strategy_concurrency,
                j.strategy_max_failures,
                COUNT(ta.id) AS target_count,
                COALESCE(GROUP_CONCAT(
                    COALESCE(jt.agent_id, ta.agent_id, '') || char(30) ||
                    COALESCE(NULLIF(jt.agent_display_name, ''), a.name, jt.agent_id, ta.agent_id, '') || char(30) ||
                    COALESCE(NULLIF(jt.agent_status_snapshot, ''), jt.status, a.status, 'unknown') || char(30) ||
                    COALESCE(jt.labels_snapshot, '') || char(30) ||
                    COALESCE(ta.id, '') || char(30) ||
                    COALESCE(ta.status, '') || char(30) ||
                    COALESCE(ta.last_error, ''),
                    char(31)
                ), '') AS target_agents,
                MAX(ta.expires_at) AS expires_at
             FROM jobs j
             LEFT JOIN task_assignments ta ON ta.job_id = j.id
             LEFT JOIN job_targets jt ON jt.job_id = j.id AND jt.agent_id = ta.agent_id
             LEFT JOIN agents a ON a.id = ta.agent_id
             WHERE (?1 IS NULL OR j.id = ?1)
             GROUP BY j.id
             ORDER BY j.created_at DESC, j.id DESC
             LIMIT ?2",
        )?;
        let rows = statement
            .query_map(params![job_id, limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, Option<i64>>(12)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(
                |(
                    id,
                    status,
                    risk,
                    command_program,
                    command_args_json,
                    created_at,
                    selector_kind,
                    selector_source,
                    strategy_concurrency,
                    strategy_max_failures,
                    target_count,
                    target_agents,
                    expires_at,
                )| {
                    Ok(JobSummaryRecord {
                        id,
                        status,
                        risk,
                        command_program,
                        command_args: parse_command_args(&command_args_json)?,
                        selector_kind,
                        selector_source,
                        strategy_concurrency: strategy_concurrency.max(1) as u32,
                        strategy_max_failures: strategy_max_failures
                            .and_then(|value| u32::try_from(value).ok())
                            .filter(|value| *value > 0),
                        target_count: target_count.max(0) as usize,
                        target_agents: parse_job_target_summaries(&target_agents),
                        created_at: unix_secs_to_system_time(created_at),
                        expires_at: expires_at.map(unix_secs_to_system_time),
                    })
                },
            )
            .collect()
    }

    pub fn append_job_output_chunk_record(&self, chunk: &JobOutputChunk) -> Result<(), StoreError> {
        let stream = output_stream_to_str(chunk.stream);
        let result = self.connection.execute(
            "INSERT INTO job_output_chunks (
                job_id, agent_id, stream, chunk_index, body
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                chunk.job_id.as_str(),
                chunk.agent_id.as_str(),
                stream,
                chunk.sequence as i64,
                chunk.body.as_str(),
            ],
        );

        match result {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(error, Some(message)))
                if error.code == ErrorCode::ConstraintViolation =>
            {
                let existing_body = self
                    .connection
                    .query_row(
                        "SELECT body
                         FROM job_output_chunks
                         WHERE job_id = ?1
                           AND agent_id = ?2
                           AND stream = ?3
                           AND chunk_index = ?4",
                        params![
                            chunk.job_id.as_str(),
                            chunk.agent_id.as_str(),
                            stream,
                            chunk.sequence as i64,
                        ],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?;
                if existing_body.as_deref() == Some(chunk.body.as_str()) {
                    Ok(())
                } else {
                    Err(StoreError::ConstraintViolation(message))
                }
            }
            Err(error) => Err(StoreError::from(error)),
        }
    }

    pub fn list_job_output_chunks(
        &self,
        job_id: &str,
        agent_id: &str,
    ) -> Result<Vec<JobOutputChunk>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT job_id, agent_id, stream, chunk_index, body
             FROM job_output_chunks
             WHERE job_id = ?1 AND agent_id = ?2
             ORDER BY chunk_index",
        )?;
        let mut rows = statement.query(params![job_id, agent_id])?;
        let mut chunks = Vec::new();
        while let Some(row) = rows.next()? {
            chunks.push(JobOutputChunk {
                job_id: row.get(0)?,
                agent_id: row.get(1)?,
                stream: parse_output_stream(&row.get::<_, String>(2)?),
                sequence: row.get::<_, i64>(3)?.max(0) as u64,
                body: row.get(4)?,
            });
        }
        Ok(chunks)
    }

    pub fn list_job_output_chunks_for_job(
        &self,
        job_id: &str,
    ) -> Result<Vec<JobOutputChunk>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT job_id, agent_id, stream, chunk_index, body
             FROM job_output_chunks
             WHERE job_id = ?1
             ORDER BY agent_id, chunk_index, stream",
        )?;
        let mut rows = statement.query(params![job_id])?;
        let mut chunks = Vec::new();
        while let Some(row) = rows.next()? {
            chunks.push(JobOutputChunk {
                job_id: row.get(0)?,
                agent_id: row.get(1)?,
                stream: parse_output_stream(&row.get::<_, String>(2)?),
                sequence: row.get::<_, i64>(3)?.max(0) as u64,
                body: row.get(4)?,
            });
        }
        Ok(chunks)
    }

    fn insert_agent(&self, agent: &Agent) -> Result<(), StoreError> {
        let status = status_to_str(agent.status());
        let labels = encode_labels(agent.labels());
        let last_seen_at = agent.last_seen_at().map(system_time_to_unix_secs);
        let pinned_controller = agent
            .pinned_controller()
            .map(ControllerPublicKey::as_str)
            .unwrap_or_default();

        match self.connection.execute(
            "INSERT INTO agents (
                id, name, public_key, fingerprint, labels, status, last_seen_at, pinned_controller
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                agent.id().as_str(),
                agent.name().as_str(),
                agent.identity().public_key.as_str(),
                agent.identity().fingerprint.as_str(),
                labels,
                status,
                last_seen_at,
                pinned_controller,
            ],
        ) {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == ErrorCode::ConstraintViolation =>
            {
                Err(StoreError::DuplicateAgent)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn row_to_agent(row: StoredAgentRow) -> Result<Agent, StoreError> {
        let labels = decode_labels(&row.labels)?;
        let pinned_controller = if row.pinned_controller.is_empty() {
            None
        } else {
            Some(ControllerPublicKey::new(row.pinned_controller)?)
        };

        Ok(Agent::restore(
            AgentId::new(row.id)?,
            AgentName::new(row.name)?,
            AgentIdentity {
                public_key: AgentPublicKey::new(row.public_key)?,
                fingerprint: AgentFingerprint::new(row.fingerprint)?,
            },
            labels,
            parse_status(&row.status),
            row.last_seen_at.map(unix_secs_to_system_time),
            pinned_controller,
        ))
    }

    fn insert_audit(&self, event: &AuditEvent) -> Result<(), StoreError> {
        let (value_kind, value_text) = encode_audit_value(&event.value);
        self.connection.execute(
            "INSERT INTO audit_events (
                category, action, actor, target, value_kind, value_text, occurred_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                event.category.as_str(),
                event.action,
                event.actor.as_str(),
                event.target.as_str(),
                value_kind,
                value_text,
                system_time_to_unix_secs(event.occurred_at),
            ],
        )?;
        Ok(())
    }

    fn query_audit(
        &self,
        category: Option<AuditCategory>,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, StoreError> {
        let limit = limit.clamp(1, 500) as i64;
        let mut events = Vec::new();

        if let Some(category) = category {
            let mut statement = self.connection.prepare(
                "SELECT category, action, actor, target, value_kind, value_text, occurred_at
                 FROM audit_events
                 WHERE category = ?1
                 ORDER BY id DESC
                 LIMIT ?2",
            )?;
            let mut rows = statement.query(params![category.as_str(), limit])?;
            while let Some(row) = rows.next()? {
                events.push(row_to_audit(row)?);
            }
        } else {
            let mut statement = self.connection.prepare(
                "SELECT category, action, actor, target, value_kind, value_text, occurred_at
                 FROM audit_events
                 ORDER BY id DESC
                 LIMIT ?1",
            )?;
            let mut rows = statement.query(params![limit])?;
            while let Some(row) = rows.next()? {
                events.push(row_to_audit(row)?);
            }
        }

        Ok(events)
    }
}

impl AgentRepository for SqliteStore {
    type Error = StoreError;

    fn save(&mut self, agent: Agent) -> Result<(), Self::Error> {
        self.insert_agent(&agent)
    }

    fn find_by_id(&self, id: &AgentId) -> Result<Option<Agent>, Self::Error> {
        let row = self
            .connection
            .query_row(
                "SELECT id, name, public_key, fingerprint, labels, status, last_seen_at, pinned_controller
                 FROM agents
                 WHERE id = ?1",
                params![id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()?;

        row.map(
            |(id, name, public_key, fingerprint, labels, status, last_seen_at, pinned)| {
                Self::row_to_agent(StoredAgentRow {
                    id,
                    name,
                    public_key,
                    fingerprint,
                    labels,
                    status,
                    last_seen_at,
                    pinned_controller: pinned,
                })
            },
        )
        .transpose()
    }

    fn list(&self) -> Result<Vec<Agent>, Self::Error> {
        let mut statement = self.connection.prepare(
            "SELECT id, name, public_key, fingerprint, labels, status, last_seen_at, pinned_controller
             FROM agents
             ORDER BY name",
        )?;
        let mut rows = statement.query([])?;
        let mut agents = Vec::new();
        while let Some(row) = rows.next()? {
            agents.push(Self::row_to_agent(StoredAgentRow {
                id: row.get(0)?,
                name: row.get(1)?,
                public_key: row.get(2)?,
                fingerprint: row.get(3)?,
                labels: row.get(4)?,
                status: row.get(5)?,
                last_seen_at: row.get(6)?,
                pinned_controller: row.get(7)?,
            })?);
        }
        Ok(agents)
    }
}

struct StoredAgentRow {
    id: String,
    name: String,
    public_key: String,
    fingerprint: String,
    labels: String,
    status: String,
    last_seen_at: Option<i64>,
    pinned_controller: String,
}

impl AuditWriter for SqliteStore {
    type Error = StoreError;

    fn write(&mut self, event: AuditEvent) -> Result<(), Self::Error> {
        self.insert_audit(&event)
    }
}

impl AuditRepository for SqliteStore {
    fn list(&self, limit: usize) -> Result<Vec<AuditEvent>, Self::Error> {
        self.query_audit(None, limit)
    }

    fn list_by_category(
        &self,
        category: AuditCategory,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, Self::Error> {
        self.query_audit(Some(category), limit)
    }
}

impl AdminTokenRepository for SqliteStore {
    type Error = StoreError;

    fn admin_token_exists(&self) -> Result<bool, Self::Error> {
        SqliteStore::admin_token_exists(self)
    }

    fn insert_admin_token_hash(&mut self, token_hash: &str) -> Result<(), Self::Error> {
        SqliteStore::insert_admin_token_hash(self, token_hash)
    }

    fn verify_admin_token_hash(&self, token_hash: &str) -> Result<bool, Self::Error> {
        SqliteStore::verify_admin_token_hash(self, token_hash)
    }

    fn find_admin_token_record(
        &self,
        token_hash: &str,
    ) -> Result<Option<AppAdminTokenRecord>, Self::Error> {
        SqliteStore::find_admin_token_record(self, token_hash)
    }
}

impl AgentIdentityRepository for SqliteStore {
    type Error = StoreError;

    fn find_agent_identity(
        &self,
        agent_id: &str,
    ) -> Result<Option<AppAgentIdentityRecord>, Self::Error> {
        Ok(
            SqliteStore::find_agent_identity(self, agent_id)?.map(|(public_key, fingerprint)| {
                AppAgentIdentityRecord {
                    public_key,
                    fingerprint,
                }
            }),
        )
    }
}

impl ControllerIdentityRepository for SqliteStore {
    type Error = StoreError;

    fn save_controller_identity_metadata(
        &mut self,
        metadata: ControllerIdentityMetadata,
    ) -> Result<(), Self::Error> {
        self.connection.execute(
            "INSERT INTO controller_identity (
                id, public_key, public_fingerprint, private_key_path, created_at
             ) VALUES (1, ?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                public_key = excluded.public_key,
                public_fingerprint = excluded.public_fingerprint,
                private_key_path = excluded.private_key_path,
                created_at = excluded.created_at",
            params![
                metadata.public_key,
                metadata.public_fingerprint,
                metadata.private_key_path,
                system_time_to_unix_secs(metadata.created_at),
            ],
        )?;
        Ok(())
    }

    fn controller_identity_metadata(
        &self,
    ) -> Result<Option<ControllerIdentityMetadata>, Self::Error> {
        self.connection
            .query_row(
                "SELECT public_key, public_fingerprint, private_key_path, created_at
                 FROM controller_identity
                 WHERE id = 1",
                [],
                |row| {
                    Ok(ControllerIdentityMetadata {
                        public_key: row.get(0)?,
                        public_fingerprint: row.get(1)?,
                        private_key_path: row.get(2)?,
                        created_at: unix_secs_to_system_time(row.get(3)?),
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }
}

impl EnrollmentTokenRepository for SqliteStore {
    type Error = StoreError;

    fn insert_enrollment_token_hash(
        &mut self,
        id: &str,
        token_hash: &str,
        default_labels: &str,
        expires_at: SystemTime,
        max_uses: u32,
    ) -> Result<(), Self::Error> {
        SqliteStore::insert_enrollment_token_hash(
            self,
            id,
            token_hash,
            default_labels,
            expires_at,
            max_uses,
        )
    }

    fn list_enrollment_tokens(&self) -> Result<Vec<AppEnrollmentTokenRecord>, Self::Error> {
        Ok(SqliteStore::list_enrollment_tokens(self)?
            .into_iter()
            .map(|record| AppEnrollmentTokenRecord {
                id: record.id,
                default_labels: record.default_labels,
                expires_at: record.expires_at,
                max_uses: record.max_uses,
                used_count: record.used_count,
                revoked: record.revoked,
            })
            .collect())
    }

    fn revoke_enrollment_token(&mut self, id: &str) -> Result<bool, Self::Error> {
        SqliteStore::revoke_enrollment_token(self, id)
    }

    fn consume_enrollment_token_hash(
        &mut self,
        token_hash: &str,
        now: SystemTime,
    ) -> Result<AppEnrollmentTokenRecord, Self::Error> {
        let record = SqliteStore::consume_enrollment_token_hash(self, token_hash, now)?;
        Ok(AppEnrollmentTokenRecord {
            id: record.id,
            default_labels: record.default_labels,
            expires_at: record.expires_at,
            max_uses: record.max_uses,
            used_count: record.used_count,
            revoked: record.revoked,
        })
    }
}

impl ApprovalRepository for SqliteStore {
    type Error = StoreError;

    fn insert_approval_request(
        &mut self,
        request: AppApprovalRequestRecord,
    ) -> Result<(), Self::Error> {
        self.connection.execute(
            "INSERT INTO approval_requests (
                id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                request.id,
                request.job_id,
                request.requester,
                request.approver,
                request.reason,
                request.status,
                system_time_to_unix_secs(request.expires_at),
                system_time_to_unix_secs(request.created_at),
                request.decided_at.map(system_time_to_unix_secs),
            ],
        )?;
        Ok(())
    }

    fn find_approval_request(
        &self,
        approval_id: &str,
    ) -> Result<Option<AppApprovalRequestRecord>, Self::Error> {
        self.connection
            .query_row(
                "SELECT id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
                 FROM approval_requests
                 WHERE id = ?1",
                params![approval_id],
                row_to_app_approval_request_record,
            )
            .optional()
            .map_err(StoreError::from)
    }

    fn find_pending_approval_for_job(
        &self,
        job_id: &str,
    ) -> Result<Option<AppApprovalRequestRecord>, Self::Error> {
        self.connection
            .query_row(
                "SELECT id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
                 FROM approval_requests
                 WHERE job_id = ?1 AND status = 'pending'
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1",
                params![job_id],
                row_to_app_approval_request_record,
            )
            .optional()
            .map_err(StoreError::from)
    }

    fn list_approval_requests(
        &self,
        status: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AppApprovalRequestRecord>, Self::Error> {
        let limit = limit.clamp(1, 500) as i64;
        let mut statement = self.connection.prepare(
            "SELECT id, job_id, requester, approver, reason, status, expires_at, created_at, decided_at
             FROM approval_requests
             WHERE (?1 IS NULL OR status = ?1)
             ORDER BY created_at DESC, id DESC
             LIMIT ?2",
        )?;
        let rows = statement
            .query_map(params![status, limit], row_to_app_approval_request_record)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn update_approval_request(
        &mut self,
        request: AppApprovalRequestRecord,
    ) -> Result<bool, Self::Error> {
        let changed = self.connection.execute(
            "UPDATE approval_requests
             SET requester = ?2,
                 approver = ?3,
                 reason = ?4,
                 status = ?5,
                 expires_at = ?6,
                 created_at = ?7,
                 decided_at = ?8
             WHERE id = ?1",
            params![
                request.id,
                request.requester,
                request.approver,
                request.reason,
                request.status,
                system_time_to_unix_secs(request.expires_at),
                system_time_to_unix_secs(request.created_at),
                request.decided_at.map(system_time_to_unix_secs),
            ],
        )?;
        Ok(changed > 0)
    }

    fn update_job_status_for_approval(
        &mut self,
        job_id: &str,
        status: JobStatus,
    ) -> Result<bool, Self::Error> {
        self.update_job_status(job_id, status)
    }
}

impl JobRepository for SqliteStore {
    type Error = StoreError;

    fn save(&mut self, job: Job) -> Result<(), Self::Error> {
        self.save_job_record(&job)
    }
}

impl TaskAssignmentRepository for SqliteStore {
    type Error = StoreError;

    fn save_assignment(&mut self, envelope: TaskEnvelope) -> Result<(), Self::Error> {
        self.save_task_assignment_record(&envelope)
    }
}

impl DispatchAssignmentRepository for SqliteStore {
    type Error = StoreError;

    fn list_pending_assignments(
        &self,
        agent_id: Option<&AgentId>,
        job_id: Option<&JobId>,
        limit: usize,
    ) -> Result<Vec<AppPendingTaskAssignment>, Self::Error> {
        self.list_pending_dispatch_assignments(agent_id, job_id, limit)
    }

    fn find_dispatch_agent(&self, agent_id: &AgentId) -> Result<Option<Agent>, Self::Error> {
        self.find_agent_by_id(agent_id.as_str())
    }

    fn dispatch_gate(&self, job_id: &JobId) -> Result<AppJobDispatchGate, Self::Error> {
        let gate = self
            .job_dispatch_gate(job_id.as_str())?
            .unwrap_or(JobDispatchGateRecord {
                concurrency: 1,
                max_failures: None,
                active_count: 0,
                failure_count: 0,
            });
        Ok(AppJobDispatchGate {
            concurrency: gate.concurrency as usize,
            max_failures: gate.max_failures,
            active_count: gate.active_count,
            failure_count: gate.failure_count,
        })
    }

    fn mark_assignment_dispatched(
        &mut self,
        task_id: &TaskId,
        now: SystemTime,
    ) -> Result<(), Self::Error> {
        self.update_task_assignment_status(
            task_id.as_str(),
            AssignmentStatus::Dispatched,
            now,
            None,
        )?;
        Ok(())
    }

    fn mark_job_running(&mut self, job_id: &JobId, _now: SystemTime) -> Result<(), Self::Error> {
        self.update_job_status(job_id.as_str(), JobStatus::Running)?;
        Ok(())
    }

    fn mark_job_expired(&mut self, job_id: &JobId, _now: SystemTime) -> Result<(), Self::Error> {
        self.update_job_status(job_id.as_str(), JobStatus::Expired)?;
        Ok(())
    }
}

impl CommandJobRepository for SqliteStore {
    fn save_command_job(
        &mut self,
        job: Job,
        task: &CommandTask,
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.save_command_job_record(&job, task)
    }
}

impl DriftCheckJobRepository for SqliteStore {
    fn save_drift_check_job(
        &mut self,
        job: Job,
        task: &DriftCheckTask,
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.save_drift_check_job_record(&job, task)
    }
}

impl RunbookJobRepository for SqliteStore {
    fn save_runbook_job(
        &mut self,
        job: Job,
        task: &RunbookExecutionTask,
    ) -> Result<(), <Self as TaskAssignmentRepository>::Error> {
        self.save_runbook_job_record(&job, task)
    }
}

impl JobOutputRepository for SqliteStore {
    type Error = StoreError;

    fn append_output_chunk(&mut self, chunk: JobOutputChunk) -> Result<(), Self::Error> {
        self.append_job_output_chunk_record(&chunk)
    }

    fn list_output_chunks(
        &self,
        job_id: &str,
        agent_id: &str,
    ) -> Result<Vec<JobOutputChunk>, Self::Error> {
        self.list_job_output_chunks(job_id, agent_id)
    }

    fn list_output_chunks_for_job(&self, job_id: &str) -> Result<Vec<JobOutputChunk>, Self::Error> {
        self.list_job_output_chunks_for_job(job_id)
    }
}

impl JobQueryRepository for SqliteStore {
    type Error = StoreError;

    fn list_job_summaries(&self, limit: usize) -> Result<Vec<AppJobSummaryRecord>, Self::Error> {
        Ok(SqliteStore::list_job_summaries(self, limit)?
            .into_iter()
            .map(|record| AppJobSummaryRecord {
                id: record.id,
                status: record.status,
                risk: record.risk,
                command_program: record.command_program,
                command_args: record.command_args,
                selector_kind: record.selector_kind,
                selector_source: record.selector_source,
                strategy_concurrency: record.strategy_concurrency,
                strategy_max_failures: record.strategy_max_failures,
                target_count: record.target_count,
                target_agents: record
                    .target_agents
                    .into_iter()
                    .map(|target| AppJobTargetSummaryRecord {
                        agent_id: target.agent_id,
                        agent_name: target.agent_name,
                        status: target.status,
                        labels: target.labels,
                        task_id: target.task_id,
                        assignment_status: target.assignment_status,
                        last_error: target.last_error,
                    })
                    .collect(),
                created_at: record.created_at,
                expires_at: record.expires_at,
            })
            .collect())
    }

    fn find_job_summary(&self, job_id: &str) -> Result<Option<AppJobSummaryRecord>, Self::Error> {
        Ok(
            SqliteStore::find_job_summary(self, job_id)?.map(|record| AppJobSummaryRecord {
                id: record.id,
                status: record.status,
                risk: record.risk,
                command_program: record.command_program,
                command_args: record.command_args,
                selector_kind: record.selector_kind,
                selector_source: record.selector_source,
                strategy_concurrency: record.strategy_concurrency,
                strategy_max_failures: record.strategy_max_failures,
                target_count: record.target_count,
                target_agents: record
                    .target_agents
                    .into_iter()
                    .map(|target| AppJobTargetSummaryRecord {
                        agent_id: target.agent_id,
                        agent_name: target.agent_name,
                        status: target.status,
                        labels: target.labels,
                        task_id: target.task_id,
                        assignment_status: target.assignment_status,
                        last_error: target.last_error,
                    })
                    .collect(),
                created_at: record.created_at,
                expires_at: record.expires_at,
            }),
        )
    }
}

impl FactsRepository for SqliteStore {
    type Error = StoreError;

    fn insert_facts_snapshot(
        &mut self,
        agent_id: &str,
        body: &str,
        collected_at: SystemTime,
    ) -> Result<(), Self::Error> {
        SqliteStore::insert_facts_snapshot(self, agent_id, body, collected_at)
    }

    fn latest_facts_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<AppFactsSnapshotRecord>, Self::Error> {
        Ok(
            SqliteStore::latest_facts_snapshot(self, agent_id)?.map(|record| {
                AppFactsSnapshotRecord {
                    agent_id: record.agent_id,
                    body: record.body,
                    collected_at: record.collected_at,
                }
            }),
        )
    }

    fn list_facts_snapshots(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AppFactsSnapshotPageRecord>, Self::Error> {
        Ok(
            SqliteStore::list_facts_snapshots(self, agent_id, limit, before)?
                .into_iter()
                .map(|record| AppFactsSnapshotPageRecord {
                    agent_id: record.agent_id,
                    body: record.body,
                    collected_at: record.collected_at,
                    cursor: record.cursor,
                })
                .collect(),
        )
    }
}

impl MetricsRepository for SqliteStore {
    type Error = StoreError;

    fn insert_metrics_snapshot(
        &mut self,
        agent_id: &str,
        body: &str,
        collected_at: SystemTime,
    ) -> Result<(), Self::Error> {
        SqliteStore::insert_metrics_snapshot(self, agent_id, body, collected_at)
    }

    fn latest_metrics_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<Option<AppMetricsSnapshotRecord>, Self::Error> {
        Ok(
            SqliteStore::latest_metrics_snapshot(self, agent_id)?.map(|record| {
                AppMetricsSnapshotRecord {
                    agent_id: record.agent_id,
                    body: record.body,
                    collected_at: record.collected_at,
                }
            }),
        )
    }

    fn list_metrics_snapshots(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AppMetricsSnapshotPageRecord>, Self::Error> {
        Ok(
            SqliteStore::list_metrics_snapshots(self, agent_id, limit, before)?
                .into_iter()
                .map(|record| AppMetricsSnapshotPageRecord {
                    agent_id: record.agent_id,
                    body: record.body,
                    collected_at: record.collected_at,
                    cursor: record.cursor,
                })
                .collect(),
        )
    }
}

impl AgentLogRepository for SqliteStore {
    type Error = StoreError;

    fn list_agent_log_chunks(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AppAgentLogChunkPageRecord>, Self::Error> {
        Ok(
            SqliteStore::list_agent_log_chunks_page(self, agent_id, limit, before)?
                .into_iter()
                .map(|record| AppAgentLogChunkPageRecord {
                    agent_id: record.agent_id,
                    line: record.line,
                    collected_at: record.collected_at,
                    cursor: record.cursor,
                })
                .collect(),
        )
    }
}

impl DriftRepository for SqliteStore {
    type Error = StoreError;

    fn insert_drift_report(
        &mut self,
        agent_id: &str,
        report: &DriftReport,
        checked_at: SystemTime,
    ) -> Result<(), Self::Error> {
        SqliteStore::insert_drift_report(self, agent_id, report, checked_at)
    }

    fn latest_drift_report(
        &self,
        agent_id: &str,
    ) -> Result<Option<AppDriftReportRecord>, Self::Error> {
        Ok(
            SqliteStore::latest_drift_report(self, agent_id)?.map(|record| AppDriftReportRecord {
                agent_id: record.agent_id,
                report: record.report,
                checked_at: record.checked_at,
            }),
        )
    }

    fn list_drift_reports(
        &self,
        agent_id: &str,
        limit: usize,
        before: Option<SnapshotPageCursor>,
    ) -> Result<Vec<AppDriftReportPageRecord>, Self::Error> {
        Ok(
            SqliteStore::list_drift_reports(self, agent_id, limit, before)?
                .into_iter()
                .map(|record| AppDriftReportPageRecord {
                    agent_id: record.agent_id,
                    report: record.report,
                    checked_at: record.checked_at,
                    cursor: record.cursor,
                })
                .collect(),
        )
    }
}

impl PolicyRepository for SqliteStore {
    type Error = StoreError;

    fn save_policy_source(
        &mut self,
        policy_id: &str,
        name: &str,
        version: u32,
        source: &str,
    ) -> Result<(), Self::Error> {
        SqliteStore::save_policy_source(self, policy_id, name, version, source)
    }

    fn list_policies(&self) -> Result<Vec<AppPolicyRecord>, Self::Error> {
        Ok(SqliteStore::list_policies(self)?
            .into_iter()
            .map(|record| AppPolicyRecord {
                id: record.id,
                name: record.name,
                version: record.version,
                source: record.source,
                created_at: record.created_at,
                updated_at: record.updated_at,
            })
            .collect())
    }

    fn find_policy(&self, policy_id: &str) -> Result<Option<AppPolicyRecord>, Self::Error> {
        Ok(
            SqliteStore::find_policy(self, policy_id)?.map(|record| AppPolicyRecord {
                id: record.id,
                name: record.name,
                version: record.version,
                source: record.source,
                created_at: record.created_at,
                updated_at: record.updated_at,
            }),
        )
    }

    fn assign_policy_to_agent(
        &mut self,
        policy_id: &str,
        agent_id: &str,
        assigned_at: SystemTime,
    ) -> Result<(), Self::Error> {
        SqliteStore::assign_policy_to_agent(self, policy_id, agent_id, assigned_at)
    }

    fn policies_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<AppPolicyAssignmentRecord>, Self::Error> {
        Ok(SqliteStore::policies_for_agent(self, agent_id)?
            .into_iter()
            .map(|record| AppPolicyAssignmentRecord {
                policy_id: record.policy_id,
                agent_id: record.agent_id,
                assigned_at: record.assigned_at,
            })
            .collect())
    }

    fn upsert_policy_schedule(
        &mut self,
        policy_id: &str,
        agent_id: &str,
        interval: Duration,
        next_due_at: SystemTime,
    ) -> Result<(), Self::Error> {
        SqliteStore::upsert_policy_schedule(self, policy_id, agent_id, interval, next_due_at)
    }

    fn due_scheduled_drift_checks(
        &self,
        now: SystemTime,
        limit: usize,
    ) -> Result<Vec<AppScheduledDriftRecord>, Self::Error> {
        Ok(SqliteStore::due_scheduled_drift_checks(self, now, limit)?
            .into_iter()
            .map(|record| AppScheduledDriftRecord {
                policy_id: record.policy_id,
                agent_id: record.agent_id,
                interval_seconds: record.interval_seconds,
                next_due_at: record.next_due_at,
                last_checked_at: record.last_checked_at,
            })
            .collect())
    }

    fn record_scheduled_drift_check(
        &mut self,
        policy_id: &str,
        agent_id: &str,
        checked_at: SystemTime,
    ) -> Result<(), Self::Error> {
        SqliteStore::record_scheduled_drift_check(self, policy_id, agent_id, checked_at)
    }

    fn acknowledge_latest_drift_report(
        &mut self,
        agent_id: &str,
        policy_name: &str,
        actor: &str,
        acknowledged_at: SystemTime,
    ) -> Result<bool, Self::Error> {
        SqliteStore::acknowledge_latest_drift_report(
            self,
            agent_id,
            policy_name,
            actor,
            acknowledged_at,
        )
    }

    fn mark_latest_drift_resolved(
        &mut self,
        agent_id: &str,
        policy_name: &str,
        job_id: &str,
        resolved_at: SystemTime,
    ) -> Result<bool, Self::Error> {
        SqliteStore::mark_latest_drift_resolved(self, agent_id, policy_name, job_id, resolved_at)
    }
}

fn row_to_facts_snapshot_page_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<FactsSnapshotPageRecord> {
    let id = row.get(0)?;
    let collected_at = unix_secs_to_system_time(row.get(3)?);
    Ok(FactsSnapshotPageRecord {
        agent_id: row.get(1)?,
        body: row.get(2)?,
        collected_at,
        cursor: SnapshotPageCursor {
            occurred_at: collected_at,
            row_id: id,
        },
    })
}

fn row_to_metrics_snapshot_page_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<MetricsSnapshotPageRecord> {
    let id = row.get(0)?;
    let collected_at = unix_secs_to_system_time(row.get(3)?);
    Ok(MetricsSnapshotPageRecord {
        agent_id: row.get(1)?,
        body: row.get(2)?,
        collected_at,
        cursor: SnapshotPageCursor {
            occurred_at: collected_at,
            row_id: id,
        },
    })
}

fn row_to_agent_log_chunk_page_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<AgentLogChunkPageRecord> {
    let id = row.get(0)?;
    let collected_at = unix_secs_to_system_time(row.get(3)?);
    Ok(AgentLogChunkPageRecord {
        agent_id: row.get(1)?,
        line: row.get(2)?,
        collected_at,
        cursor: SnapshotPageCursor {
            occurred_at: collected_at,
            row_id: id,
        },
    })
}

fn row_to_drift_report_page_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<DriftReportPageRecord> {
    let id = row.get(0)?;
    let checked_at = unix_secs_to_system_time(row.get(6)?);
    Ok(DriftReportPageRecord {
        agent_id: row.get(1)?,
        report: DriftReport {
            policy_name: row.get(2)?,
            status: parse_drift_status(&row.get::<_, String>(3)?),
            severity: parse_drift_severity(&row.get::<_, String>(7)?),
            acknowledgement: row_to_drift_acknowledgement(row, 8, 9, 10, 11)?,
            expected: row.get(4)?,
            actual: row.get(5)?,
        },
        checked_at,
        cursor: SnapshotPageCursor {
            occurred_at: checked_at,
            row_id: id,
        },
    })
}

fn row_to_policy_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<PolicyRecord> {
    Ok(PolicyRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        version: row.get::<_, i64>(2)?.max(0) as u32,
        source: row.get(3)?,
        created_at: unix_secs_to_system_time(row.get(4)?),
        updated_at: unix_secs_to_system_time(row.get(5)?),
    })
}

fn row_to_policy_assignment_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<PolicyAssignmentRecord> {
    Ok(PolicyAssignmentRecord {
        policy_id: row.get(0)?,
        agent_id: row.get(1)?,
        assigned_at: unix_secs_to_system_time(row.get(2)?),
    })
}

fn row_to_scheduled_drift_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ScheduledDriftRecord> {
    Ok(ScheduledDriftRecord {
        policy_id: row.get(0)?,
        agent_id: row.get(1)?,
        interval_seconds: row.get::<_, i64>(2)?.max(0) as u64,
        next_due_at: unix_secs_to_system_time(row.get(3)?),
        last_checked_at: row.get::<_, Option<i64>>(4)?.map(unix_secs_to_system_time),
    })
}

fn row_to_app_approval_request_record(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<AppApprovalRequestRecord> {
    Ok(AppApprovalRequestRecord {
        id: row.get(0)?,
        job_id: row.get(1)?,
        requester: row.get(2)?,
        approver: row.get(3)?,
        reason: row.get(4)?,
        status: row.get(5)?,
        expires_at: unix_secs_to_system_time(row.get(6)?),
        created_at: unix_secs_to_system_time(row.get(7)?),
        decided_at: row.get::<_, Option<i64>>(8)?.map(unix_secs_to_system_time),
    })
}

fn row_to_audit(row: &rusqlite::Row<'_>) -> Result<AuditEvent, StoreError> {
    let category: String = row.get(0)?;
    let action: String = row.get(1)?;
    let actor: String = row.get(2)?;
    let target: String = row.get(3)?;
    let value_kind: String = row.get(4)?;
    let value_text: String = row.get(5)?;
    let occurred_at: i64 = row.get(6)?;

    let category = AuditCategory::parse(&category)
        .ok_or_else(|| StoreError::Domain(format!("unknown audit category: {category}")))?;

    Ok(AuditEvent {
        category,
        action,
        actor: AuditActor::new(actor),
        target: AuditTarget::new(target),
        value: decode_audit_value(&value_kind, &value_text),
        occurred_at: unix_secs_to_system_time(occurred_at),
    })
}

fn status_to_str(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Pending => "pending",
        AgentStatus::Online => "online",
        AgentStatus::Busy => "busy",
        AgentStatus::Degraded => "degraded",
        AgentStatus::Offline => "offline",
        AgentStatus::Disabled => "disabled",
    }
}

fn parse_status(value: &str) -> AgentStatus {
    match value {
        "online" => AgentStatus::Online,
        "busy" => AgentStatus::Busy,
        "degraded" => AgentStatus::Degraded,
        "offline" => AgentStatus::Offline,
        "disabled" => AgentStatus::Disabled,
        _ => AgentStatus::Pending,
    }
}

fn job_status_to_str(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Draft => "draft",
        JobStatus::PendingApproval => "pending_approval",
        JobStatus::Queued => "queued",
        JobStatus::Running => "running",
        JobStatus::PartialSuccess => "partial_success",
        JobStatus::Success => "success",
        JobStatus::Failed => "failed",
        JobStatus::Canceled => "canceled",
        JobStatus::Expired => "expired",
    }
}

fn assignment_status_to_str(status: AssignmentStatus) -> &'static str {
    status.as_str()
}

fn assignment_status_value_is_terminal(value: &str) -> bool {
    matches!(
        value,
        "succeeded" | "failed" | "rejected" | "canceled" | "expired"
    )
}

fn task_risk_to_str(risk: fleet_domain::TaskRisk) -> &'static str {
    match risk {
        fleet_domain::TaskRisk::Low => "low",
        fleet_domain::TaskRisk::Medium => "medium",
        fleet_domain::TaskRisk::High => "high",
    }
}

fn approval_requirement_to_str(requirement: fleet_domain::ApprovalRequirement) -> &'static str {
    match requirement {
        fleet_domain::ApprovalRequirement::NotRequired => "not_required",
        fleet_domain::ApprovalRequirement::AdminConfirmation => "admin_confirmation",
        fleet_domain::ApprovalRequirement::ManualApproval => "manual_approval",
    }
}

fn output_stream_to_str(stream: JobOutputStream) -> &'static str {
    match stream {
        JobOutputStream::Stdout => "stdout",
        JobOutputStream::Stderr => "stderr",
    }
}

fn parse_output_stream(value: &str) -> JobOutputStream {
    match value {
        "stderr" => JobOutputStream::Stderr,
        _ => JobOutputStream::Stdout,
    }
}

fn drift_status_to_str(status: &DriftStatus) -> &'static str {
    match status {
        DriftStatus::Compliant => "compliant",
        DriftStatus::Drifted => "drifted",
        DriftStatus::Unknown => "unknown",
    }
}

fn parse_drift_status(value: &str) -> DriftStatus {
    match value {
        "compliant" => DriftStatus::Compliant,
        "drifted" => DriftStatus::Drifted,
        _ => DriftStatus::Unknown,
    }
}

fn drift_severity_to_str(severity: DriftSeverity) -> &'static str {
    match severity {
        DriftSeverity::None => "none",
        DriftSeverity::Warning => "warning",
        DriftSeverity::Critical => "critical",
        DriftSeverity::Unknown => "unknown",
    }
}

fn parse_drift_severity(value: &str) -> DriftSeverity {
    match value {
        "none" => DriftSeverity::None,
        "warning" => DriftSeverity::Warning,
        "critical" => DriftSeverity::Critical,
        _ => DriftSeverity::Unknown,
    }
}

fn row_to_drift_acknowledgement(
    row: &rusqlite::Row<'_>,
    acknowledged_at_index: usize,
    acknowledged_by_index: usize,
    resolved_at_index: usize,
    resolution_job_id_index: usize,
) -> rusqlite::Result<DriftAcknowledgement> {
    let resolved_at = row.get::<_, Option<i64>>(resolved_at_index)?;
    let resolution_job_id = row.get::<_, Option<String>>(resolution_job_id_index)?;
    if let (Some(resolved_at), Some(job_id)) = (resolved_at, resolution_job_id) {
        return Ok(DriftAcknowledgement::Resolved {
            job_id,
            at: unix_secs_to_system_time(resolved_at),
        });
    }
    let acknowledged_at = row.get::<_, Option<i64>>(acknowledged_at_index)?;
    let acknowledged_by = row.get::<_, Option<String>>(acknowledged_by_index)?;
    if let (Some(acknowledged_at), Some(by)) = (acknowledged_at, acknowledged_by) {
        return Ok(DriftAcknowledgement::Acknowledged {
            by,
            at: unix_secs_to_system_time(acknowledged_at),
        });
    }
    Ok(DriftAcknowledgement::Open)
}

fn parse_command_args(value: &str) -> Result<Vec<String>, StoreError> {
    serde_json::from_str(value).map_err(|error| StoreError::Domain(error.to_string()))
}

fn parse_job_target_summaries(value: &str) -> Vec<JobTargetSummaryRecord> {
    if value.is_empty() {
        return Vec::new();
    }
    value
        .split('\u{1f}')
        .filter_map(|item| {
            let mut fields = item.split('\u{1e}');
            let agent_id = fields.next()?;
            let agent_name = fields.next().unwrap_or(agent_id);
            let status = fields.next().unwrap_or("unknown");
            let labels_snapshot = fields.next().unwrap_or("");
            let task_id = fields.next().filter(|value| !value.is_empty());
            let assignment_status = fields.next().filter(|value| !value.is_empty());
            let last_error = fields.next().unwrap_or("");
            if agent_id.is_empty() {
                return None;
            }
            Some(JobTargetSummaryRecord {
                agent_id: agent_id.to_owned(),
                agent_name: agent_name.to_owned(),
                status: status.to_owned(),
                labels: decode_labels(labels_snapshot)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|label| (label.key().to_owned(), label.value().to_owned()))
                    .collect(),
                task_id: task_id.map(str::to_owned),
                assignment_status: assignment_status.map(str::to_owned),
                last_error: last_error.to_owned(),
            })
        })
        .collect()
}

fn encode_labels(labels: &[AgentLabel]) -> String {
    labels
        .iter()
        .map(|label| format!("{}={}", label.key(), label.value()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn decode_labels(value: &str) -> Result<Vec<AgentLabel>, StoreError> {
    value
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| StoreError::Domain(format!("invalid label storage: {line}")))?;
            AgentLabel::new(key, value).map_err(StoreError::from)
        })
        .collect()
}

fn encode_audit_value(value: &AuditValue) -> (&'static str, String) {
    match value {
        AuditValue::Plain(value) => ("plain", value.clone()),
        AuditValue::SecretRef(value) => ("secret_ref", value.clone()),
        AuditValue::Redacted => ("redacted", String::new()),
    }
}

fn decode_audit_value(kind: &str, value: &str) -> AuditValue {
    match kind {
        "plain" => AuditValue::Plain(value.to_owned()),
        "secret_ref" => AuditValue::SecretRef(value.to_owned()),
        _ => AuditValue::Redacted,
    }
}

fn system_time_to_unix_secs(value: SystemTime) -> i64 {
    value
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn unix_secs_to_system_time(value: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(value.max(0) as u64)
}

pub fn schema_sql() -> &'static str {
    SCHEMA_SQL
}

pub fn store_layer_ready() -> bool {
    fleet_application::application_layer_name() == fleet_domain::DOMAIN_LAYER
}

const SCHEMA_SQL: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_migrations (
    name TEXT PRIMARY KEY,
    version INTEGER NOT NULL,
    applied_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS controller_identity (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    public_key TEXT NOT NULL,
    public_fingerprint TEXT NOT NULL,
    private_key_path TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS admin_tokens (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    token_hash TEXT NOT NULL,
    actor_id TEXT NOT NULL DEFAULT 'bootstrap-admin',
    role TEXT NOT NULL DEFAULT 'owner',
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS agents (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    public_key TEXT NOT NULL,
    fingerprint TEXT NOT NULL UNIQUE,
    labels TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL,
    last_seen_at INTEGER,
    pinned_controller TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS agent_identities (
    agent_id TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
    public_key TEXT NOT NULL,
    fingerprint TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS enrollment_tokens (
    id TEXT PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    default_labels TEXT NOT NULL DEFAULT '',
    expires_at INTEGER NOT NULL,
    max_uses INTEGER NOT NULL,
    used_count INTEGER NOT NULL DEFAULT 0,
    revoked_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS jobs (
    id TEXT PRIMARY KEY,
    status TEXT NOT NULL,
    risk TEXT NOT NULL,
    approval_requirement TEXT NOT NULL,
    timeout_ms INTEGER NOT NULL,
    command_program TEXT,
    command_args_json TEXT NOT NULL DEFAULT '[]',
    command_max_output_bytes INTEGER NOT NULL DEFAULT 1048576,
    drift_policy_document TEXT,
    runbook_document TEXT,
    selector_kind TEXT NOT NULL DEFAULT 'explicit_ids',
    selector_source TEXT NOT NULL DEFAULT '',
    strategy_concurrency INTEGER NOT NULL DEFAULT 1,
    strategy_max_failures INTEGER,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS job_targets (
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    agent_display_name TEXT NOT NULL DEFAULT '',
    agent_status_snapshot TEXT NOT NULL DEFAULT 'unknown',
    labels_snapshot TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (job_id, agent_id)
);

CREATE TABLE IF NOT EXISTS task_assignments (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'queued',
    nonce TEXT NOT NULL UNIQUE,
    payload_hash TEXT NOT NULL,
    signature TEXT NOT NULL,
    issued_at INTEGER NOT NULL DEFAULT (unixepoch()),
    expires_at INTEGER NOT NULL,
    dispatched_at INTEGER,
    accepted_at INTEGER,
    started_at INTEGER,
    completed_at INTEGER,
    last_error TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS job_output_chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    stream TEXT NOT NULL,
    chunk_index INTEGER NOT NULL,
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(job_id, agent_id, stream, chunk_index)
);

CREATE TABLE IF NOT EXISTS approval_decisions (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    actor TEXT NOT NULL,
    decision TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS approval_requests (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    requester TEXT NOT NULL,
    approver TEXT,
    reason TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    decided_at INTEGER
);

CREATE TABLE IF NOT EXISTS audit_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    category TEXT NOT NULL,
    action TEXT NOT NULL,
    actor TEXT NOT NULL,
    target TEXT NOT NULL,
    value_kind TEXT NOT NULL,
    value_text TEXT NOT NULL DEFAULT '',
    occurred_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS facts_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    body TEXT NOT NULL,
    collected_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS metrics_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    body TEXT NOT NULL,
    collected_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS drift_reports (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    policy_name TEXT NOT NULL,
    status TEXT NOT NULL,
    severity TEXT NOT NULL DEFAULT 'unknown',
    expected TEXT NOT NULL,
    actual TEXT NOT NULL,
    checked_at INTEGER NOT NULL,
    acknowledged_at INTEGER,
    acknowledged_by TEXT,
    resolved_at INTEGER,
    resolution_job_id TEXT
);

CREATE TABLE IF NOT EXISTS policies (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    version INTEGER NOT NULL,
    source TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS policy_assignments (
    policy_id TEXT NOT NULL REFERENCES policies(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    assigned_at INTEGER NOT NULL,
    PRIMARY KEY (policy_id, agent_id)
);

CREATE TABLE IF NOT EXISTS policy_drift_schedules (
    policy_id TEXT NOT NULL REFERENCES policies(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    interval_seconds INTEGER NOT NULL,
    next_due_at INTEGER NOT NULL,
    last_checked_at INTEGER,
    PRIMARY KEY (policy_id, agent_id)
);

CREATE TABLE IF NOT EXISTS agent_log_chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    line TEXT NOT NULL,
    collected_at INTEGER NOT NULL
);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn agent() -> Agent {
        agent_with_id("a1", "web-01", "0123456789abcdef")
    }

    fn agent_with_id(id: &str, name: &str, fingerprint: &str) -> Agent {
        let mut agent = Agent::new(
            AgentId::new(id).unwrap(),
            AgentName::new(name).unwrap(),
            AgentIdentity {
                public_key: AgentPublicKey::new("pk").unwrap(),
                fingerprint: AgentFingerprint::new(fingerprint).unwrap(),
            },
        );
        agent.set_labels(vec![AgentLabel::new("role", "web").unwrap()]);
        agent.pin_controller(ControllerPublicKey::new("controller-pk").unwrap());
        agent
    }

    #[test]
    fn memory_repo_stores_and_finds_agent() {
        let mut repo = MemoryAgentRepository::default();
        repo.save(agent()).unwrap();
        assert!(
            repo.find_by_id(&AgentId::new("a1").unwrap())
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn memory_repo_rejects_duplicate_agent() {
        let mut repo = MemoryAgentRepository::default();
        repo.save(agent()).unwrap();
        assert_eq!(repo.save(agent()), Err(StoreError::DuplicateAgent));
    }

    #[test]
    fn migration_is_repeatable() {
        let store = SqliteStore::in_memory().unwrap();
        store.migrate().unwrap();
        store.migrate().unwrap();
        assert_eq!(
            store.current_schema_version().unwrap(),
            Some(CURRENT_SCHEMA_VERSION)
        );
    }

    #[test]
    fn empty_database_initialization_records_schema_version() {
        let store = SqliteStore::in_memory().unwrap();

        assert_eq!(
            store.current_schema_version().unwrap(),
            Some(CURRENT_SCHEMA_VERSION)
        );
        assert!(store.has_column("schema_migrations", "version").unwrap());
        assert!(store.has_column("agents", "pinned_controller").unwrap());
        assert!(store.has_column("task_assignments", "nonce").unwrap());
        assert!(store.has_column("task_assignments", "status").unwrap());
        assert!(store.has_column("task_assignments", "accepted_at").unwrap());
        assert!(store.has_column("admin_tokens", "actor_id").unwrap());
        assert!(store.has_column("admin_tokens", "role").unwrap());
    }

    #[test]
    fn migration_from_previous_jobs_schema_adds_columns_without_losing_rows() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                r#"
                CREATE TABLE jobs (
                    id TEXT PRIMARY KEY,
                    status TEXT NOT NULL,
                    risk TEXT NOT NULL,
                    approval_requirement TEXT NOT NULL,
                    timeout_ms INTEGER NOT NULL,
                    command_program TEXT,
                    command_args_json TEXT NOT NULL DEFAULT '[]',
                    command_max_output_bytes INTEGER NOT NULL DEFAULT 1048576,
                    created_at INTEGER NOT NULL DEFAULT (unixepoch())
                );
                INSERT INTO jobs (
                    id, status, risk, approval_requirement, timeout_ms,
                    command_program, command_args_json, command_max_output_bytes
                ) VALUES (
                    'legacy-job', 'queued', 'high', 'admin_confirmation', 30000,
                    'uptime', '[]', 1048576
                );
                "#,
            )
            .unwrap();

        let store = SqliteStore { connection };
        store.migrate().unwrap();

        assert!(store.has_column("jobs", "drift_policy_document").unwrap());
        assert!(store.has_column("jobs", "runbook_document").unwrap());
        assert_eq!(
            store.current_schema_version().unwrap(),
            Some(CURRENT_SCHEMA_VERSION)
        );
        let job_count: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE id = 'legacy-job'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(job_count, 1);
    }

    #[test]
    fn schema_does_not_store_raw_enrollment_token() {
        let store = SqliteStore::in_memory().unwrap();
        assert!(store.has_column("enrollment_tokens", "token_hash").unwrap());
        assert!(!store.has_column("enrollment_tokens", "token").unwrap());
        assert!(!store.has_column("enrollment_tokens", "raw_token").unwrap());
    }

    #[test]
    fn schema_contains_mvp_command_and_inventory_columns() {
        let store = SqliteStore::in_memory().unwrap();

        assert!(store.has_column("jobs", "command_program").unwrap());
        assert!(store.has_column("jobs", "command_args_json").unwrap());
        assert!(store.has_column("jobs", "timeout_ms").unwrap());
        assert!(store.has_column("jobs", "drift_policy_document").unwrap());
        assert!(store.has_column("jobs", "runbook_document").unwrap());
        assert!(store.has_column("jobs", "selector_kind").unwrap());
        assert!(store.has_column("jobs", "selector_source").unwrap());
        assert!(store.has_column("jobs", "strategy_concurrency").unwrap());
        assert!(store.has_column("jobs", "strategy_max_failures").unwrap());
        assert!(
            store
                .has_column("job_targets", "agent_display_name")
                .unwrap()
        );
        assert!(
            store
                .has_column("job_targets", "agent_status_snapshot")
                .unwrap()
        );
        assert!(store.has_column("job_targets", "labels_snapshot").unwrap());
        assert!(store.has_column("task_assignments", "issued_at").unwrap());
        assert!(
            store
                .has_column("job_output_chunks", "chunk_index")
                .unwrap()
        );
        assert!(store.has_column("facts_snapshots", "body").unwrap());
        assert!(store.has_column("facts_snapshots", "collected_at").unwrap());
        assert!(store.has_column("agent_log_chunks", "line").unwrap());
        assert!(
            store
                .has_column("agent_log_chunks", "collected_at")
                .unwrap()
        );
        assert!(store.has_column("agents", "labels").unwrap());
        assert!(store.has_column("agents", "status").unwrap());
    }

    #[test]
    fn sqlite_store_implements_application_repository_contracts() {
        let mut store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();

        let identity = <SqliteStore as AgentIdentityRepository>::find_agent_identity(&store, "a1")
            .unwrap()
            .unwrap();
        assert_eq!(identity.fingerprint, "0123456789abcdef");

        <SqliteStore as ControllerIdentityRepository>::save_controller_identity_metadata(
            &mut store,
            ControllerIdentityMetadata {
                public_key: "controller-pk".to_owned(),
                public_fingerprint: "controller-fp".to_owned(),
                private_key_path: "/var/lib/sponzey/controller_private.key".to_owned(),
                created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            },
        )
        .unwrap();
        assert_eq!(
            <SqliteStore as ControllerIdentityRepository>::controller_identity_metadata(&store)
                .unwrap()
                .unwrap()
                .public_fingerprint,
            "controller-fp"
        );

        <SqliteStore as AdminTokenRepository>::insert_admin_token_hash(
            &mut store,
            "admin-hash-contract",
        )
        .unwrap();
        assert!(
            <SqliteStore as AdminTokenRepository>::verify_admin_token_hash(
                &store,
                "admin-hash-contract"
            )
            .unwrap()
        );
        assert_eq!(
            <SqliteStore as AdminTokenRepository>::find_admin_token_record(
                &store,
                "admin-hash-contract"
            )
            .unwrap(),
            Some(AppAdminTokenRecord {
                actor_id: "bootstrap-admin".to_owned(),
                role: "owner".to_owned(),
            })
        );

        <SqliteStore as EnrollmentTokenRepository>::insert_enrollment_token_hash(
            &mut store,
            "et-contract",
            "hash-contract",
            "role=web",
            SystemTime::UNIX_EPOCH + Duration::from_secs(60),
            1,
        )
        .unwrap();
        assert_eq!(
            <SqliteStore as EnrollmentTokenRepository>::list_enrollment_tokens(&store)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            <SqliteStore as EnrollmentTokenRepository>::consume_enrollment_token_hash(
                &mut store,
                "hash-contract",
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap()
            .default_labels,
            "role=web"
        );

        let job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-contract").unwrap(),
            fleet_domain::TaskRisk::Low,
            fleet_domain::ApprovalRequirement::NotRequired,
            Duration::from_secs(30),
        );
        store.save_job_record(&job).unwrap();
        <SqliteStore as ApprovalRepository>::insert_approval_request(
            &mut store,
            AppApprovalRequestRecord {
                id: "approval-contract".to_owned(),
                job_id: "job-contract".to_owned(),
                requester: "operator".to_owned(),
                approver: None,
                reason: "test".to_owned(),
                status: "pending".to_owned(),
                expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(60),
                created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(2),
                decided_at: None,
            },
        )
        .unwrap();
        assert_eq!(
            <SqliteStore as ApprovalRepository>::list_approval_requests(
                &store,
                Some("pending"),
                10
            )
            .unwrap()[0]
                .job_id,
            "job-contract"
        );

        <SqliteStore as FactsRepository>::insert_facts_snapshot(
            &mut store,
            "a1",
            "{\"os\":\"linux\"}",
            SystemTime::UNIX_EPOCH + Duration::from_secs(3),
        )
        .unwrap();
        assert!(
            <SqliteStore as FactsRepository>::latest_facts_snapshot(&store, "a1")
                .unwrap()
                .is_some()
        );

        <SqliteStore as MetricsRepository>::insert_metrics_snapshot(
            &mut store,
            "a1",
            "{\"cpu\":{\"logical_count\":2}}",
            SystemTime::UNIX_EPOCH + Duration::from_secs(4),
        )
        .unwrap();
        assert!(
            <SqliteStore as MetricsRepository>::latest_metrics_snapshot(&store, "a1")
                .unwrap()
                .is_some()
        );

        <SqliteStore as DriftRepository>::insert_drift_report(
            &mut store,
            "a1",
            &DriftReport {
                policy_name: "contract".to_owned(),
                status: DriftStatus::Compliant,
                severity: DriftSeverity::None,
                acknowledgement: DriftAcknowledgement::Open,
                expected: "expected".to_owned(),
                actual: "actual".to_owned(),
            },
            SystemTime::UNIX_EPOCH + Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(
            <SqliteStore as DriftRepository>::latest_drift_report(&store, "a1")
                .unwrap()
                .unwrap()
                .report
                .policy_name,
            "contract"
        );
    }

    #[test]
    fn task_assignment_nonce_is_unique() {
        let store = SqliteStore::in_memory().unwrap();
        let result = store.connection.execute(
            "INSERT INTO task_assignments (
                id, job_id, agent_id, nonce, payload_hash, signature, expires_at
             ) VALUES ('t1', 'missing-job', 'missing-agent', 'nonce-1', 'hash', 'sig', 1)",
            [],
        );
        assert!(result.is_err());

        let unique_index_exists = store
            .connection
            .prepare("SELECT 1 FROM pragma_index_list('task_assignments') WHERE [unique] = 1")
            .unwrap()
            .exists([])
            .unwrap();
        assert!(unique_index_exists);
    }

    #[test]
    fn sqlite_repo_stores_and_finds_agent() {
        let mut store = SqliteStore::in_memory().unwrap();
        AgentRepository::save(&mut store, agent()).unwrap();

        let found = store
            .find_by_id(&AgentId::new("a1").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(found.name().as_str(), "web-01");
        assert_eq!(found.labels()[0].key(), "role");
    }

    #[test]
    fn sqlite_repo_returns_none_for_missing_records() {
        let store = SqliteStore::in_memory().unwrap();

        assert!(store.find_agent_by_id("missing-agent").unwrap().is_none());
        assert!(
            store
                .latest_facts_snapshot("missing-agent")
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .find_job_status_value("missing-job")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn sqlite_repo_rejects_duplicate_agent() {
        let mut store = SqliteStore::in_memory().unwrap();
        AgentRepository::save(&mut store, agent()).unwrap();
        assert_eq!(
            AgentRepository::save(&mut store, agent()),
            Err(StoreError::DuplicateAgent)
        );
    }

    #[test]
    fn updates_agent_labels() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let labels = vec![
            AgentLabel::new("role", "api").unwrap(),
            AgentLabel::new("env", "prod").unwrap(),
        ];

        assert!(store.update_agent_labels("a1", &labels).unwrap());
        let agent = store.find_agent_by_id("a1").unwrap().unwrap();

        assert_eq!(agent.labels()[0].value(), "api");
        assert_eq!(agent.labels()[1].key(), "env");
    }

    #[test]
    fn revoked_agent_key_disables_agent_identity() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        assert!(store.find_agent_identity("a1").unwrap().is_some());

        assert!(store.revoke_agent_key("a1").unwrap());
        assert!(store.find_agent_identity("a1").unwrap().is_none());
        assert!(
            !store
                .mark_agent_online("a1", SystemTime::UNIX_EPOCH + Duration::from_secs(5))
                .unwrap()
        );
        let agent = store.find_agent_by_id("a1").unwrap().unwrap();

        assert_eq!(agent.status(), AgentStatus::Disabled);
    }

    #[test]
    fn stores_latest_facts_snapshot() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .insert_facts_snapshot(
                "a1",
                "{\"os\":\"linux\"}",
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();
        store
            .insert_facts_snapshot(
                "a1",
                "{\"os\":\"macos\"}",
                SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            )
            .unwrap();

        let snapshot = store.latest_facts_snapshot("a1").unwrap().unwrap();

        assert_eq!(snapshot.agent_id, "a1");
        assert_eq!(snapshot.body, "{\"os\":\"macos\"}");
    }

    #[test]
    fn stores_agent_log_chunks() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .insert_agent_log_chunk(
                "a1",
                "level=info event=agent_heartbeat_completed",
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();
        store
            .insert_agent_log_chunk(
                "a1",
                "level=info event=agent_heartbeat_completed sequence=2",
                SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            )
            .unwrap();

        let chunks = store.list_agent_log_chunks("a1", 10).unwrap();

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].agent_id, "a1");
        assert!(chunks[0].line.contains("sequence=2"));
        assert_eq!(
            chunks[0].collected_at,
            SystemTime::UNIX_EPOCH + Duration::from_secs(2)
        );
    }

    #[test]
    fn pages_agent_log_chunks_before_cursor() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        for (seconds, line) in [
            (1, "level=info event=agent_log_uploaded sequence=1"),
            (2, "level=info event=agent_log_uploaded sequence=2"),
            (3, "level=info event=agent_log_uploaded sequence=3"),
        ] {
            store
                .insert_agent_log_chunk(
                    "a1",
                    line,
                    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds),
                )
                .unwrap();
        }

        let first_page = store.list_agent_log_chunks_page("a1", 2, None).unwrap();
        assert_eq!(
            first_page
                .iter()
                .map(|record| record.line.as_str())
                .collect::<Vec<_>>(),
            vec![
                "level=info event=agent_log_uploaded sequence=3",
                "level=info event=agent_log_uploaded sequence=2"
            ]
        );

        let second_page = store
            .list_agent_log_chunks_page("a1", 2, Some(first_page[1].cursor))
            .unwrap();
        assert_eq!(second_page.len(), 1);
        assert!(second_page[0].line.contains("sequence=1"));
    }

    #[test]
    fn pages_facts_snapshots_with_stable_cursor() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        for body in ["{\"seq\":1}", "{\"seq\":2}", "{\"seq\":3}"] {
            store
                .insert_facts_snapshot("a1", body, SystemTime::UNIX_EPOCH + Duration::from_secs(1))
                .unwrap();
        }

        let first_page = store.list_facts_snapshots("a1", 2, None).unwrap();
        assert_eq!(
            first_page
                .iter()
                .map(|record| record.body.as_str())
                .collect::<Vec<_>>(),
            vec!["{\"seq\":3}", "{\"seq\":2}"]
        );

        let second_page = store
            .list_facts_snapshots("a1", 2, Some(first_page[1].cursor))
            .unwrap();
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].body, "{\"seq\":1}");
    }

    #[test]
    fn stores_latest_metrics_snapshot() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .insert_metrics_snapshot(
                "a1",
                "{\"cpu\":{\"logical_count\":2}}",
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();
        store
            .insert_metrics_snapshot(
                "a1",
                "{\"cpu\":{\"logical_count\":4}}",
                SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            )
            .unwrap();

        let snapshot = store.latest_metrics_snapshot("a1").unwrap().unwrap();

        assert_eq!(snapshot.agent_id, "a1");
        assert_eq!(snapshot.body, "{\"cpu\":{\"logical_count\":4}}");
    }

    #[test]
    fn pages_metrics_snapshots_before_cursor() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        for (seconds, body) in [
            (1, "{\"cpu\":{\"logical_count\":1}}"),
            (2, "{\"cpu\":{\"logical_count\":2}}"),
            (3, "{\"cpu\":{\"logical_count\":3}}"),
        ] {
            store
                .insert_metrics_snapshot(
                    "a1",
                    body,
                    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds),
                )
                .unwrap();
        }

        let first_page = store.list_metrics_snapshots("a1", 2, None).unwrap();
        assert_eq!(
            first_page
                .iter()
                .map(|record| record.body.as_str())
                .collect::<Vec<_>>(),
            vec![
                "{\"cpu\":{\"logical_count\":3}}",
                "{\"cpu\":{\"logical_count\":2}}"
            ]
        );

        let second_page = store
            .list_metrics_snapshots("a1", 2, Some(first_page[1].cursor))
            .unwrap();
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].body, "{\"cpu\":{\"logical_count\":1}}");
    }

    #[test]
    fn stores_latest_drift_report() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .insert_drift_report(
                "a1",
                &DriftReport {
                    policy_name: "nginx-running".to_owned(),
                    status: DriftStatus::Unknown,
                    severity: DriftSeverity::Unknown,
                    acknowledgement: DriftAcknowledgement::Open,
                    expected: "service nginx running".to_owned(),
                    actual: "unknown".to_owned(),
                },
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();
        store
            .insert_drift_report(
                "a1",
                &DriftReport {
                    policy_name: "nginx-running".to_owned(),
                    status: DriftStatus::Drifted,
                    severity: DriftSeverity::Warning,
                    acknowledgement: DriftAcknowledgement::Open,
                    expected: "service nginx running".to_owned(),
                    actual: "stopped".to_owned(),
                },
                SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            )
            .unwrap();

        let record = store.latest_drift_report("a1").unwrap().unwrap();

        assert_eq!(record.agent_id, "a1");
        assert_eq!(record.report.status, DriftStatus::Drifted);
        assert_eq!(record.report.actual, "stopped");
    }

    #[test]
    fn pages_drift_reports_before_cursor() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        for (seconds, status, actual) in [
            (1, DriftStatus::Unknown, "unknown"),
            (2, DriftStatus::Compliant, "running"),
            (3, DriftStatus::Drifted, "stopped"),
        ] {
            store
                .insert_drift_report(
                    "a1",
                    &DriftReport {
                        policy_name: "nginx-running".to_owned(),
                        severity: DriftSeverity::for_status(status.clone()),
                        acknowledgement: DriftAcknowledgement::Open,
                        status,
                        expected: "service nginx running".to_owned(),
                        actual: actual.to_owned(),
                    },
                    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds),
                )
                .unwrap();
        }

        let first_page = store.list_drift_reports("a1", 2, None).unwrap();
        assert_eq!(
            first_page
                .iter()
                .map(|record| record.report.actual.as_str())
                .collect::<Vec<_>>(),
            vec!["stopped", "running"]
        );

        let second_page = store
            .list_drift_reports("a1", 2, Some(first_page[1].cursor))
            .unwrap();
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].report.actual, "unknown");
    }

    #[test]
    fn stores_policy_assignment_schedule_and_drift_acknowledgement_state() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .save_policy_source("policy-1", "nginx-running", 1, "kind: Policy")
            .unwrap();

        assert_eq!(store.list_policies().unwrap().len(), 1);
        assert!(store.find_policy("policy-1").unwrap().is_some());

        store
            .assign_policy_to_agent(
                "policy-1",
                "a1",
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(
            store.assigned_policy_ids_for_agent("a1").unwrap(),
            vec!["policy-1".to_owned()]
        );

        store
            .upsert_policy_schedule(
                "policy-1",
                "a1",
                Duration::from_secs(300),
                SystemTime::UNIX_EPOCH + Duration::from_secs(300),
            )
            .unwrap();
        assert!(
            store
                .due_scheduled_drift_checks(SystemTime::UNIX_EPOCH + Duration::from_secs(299), 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .due_scheduled_drift_checks(SystemTime::UNIX_EPOCH + Duration::from_secs(300), 10)
                .unwrap()
                .len(),
            1
        );
        store
            .record_scheduled_drift_check(
                "policy-1",
                "a1",
                SystemTime::UNIX_EPOCH + Duration::from_secs(300),
            )
            .unwrap();
        assert!(
            store
                .due_scheduled_drift_checks(SystemTime::UNIX_EPOCH + Duration::from_secs(599), 10)
                .unwrap()
                .is_empty()
        );

        store
            .insert_drift_report(
                "a1",
                &DriftReport {
                    policy_name: "nginx-running".to_owned(),
                    status: DriftStatus::Drifted,
                    severity: DriftSeverity::Warning,
                    acknowledgement: DriftAcknowledgement::Open,
                    expected: "service nginx running".to_owned(),
                    actual: "stopped".to_owned(),
                },
                SystemTime::UNIX_EPOCH + Duration::from_secs(301),
            )
            .unwrap();

        assert!(
            store
                .acknowledge_latest_drift_report(
                    "a1",
                    "nginx-running",
                    "admin",
                    SystemTime::UNIX_EPOCH + Duration::from_secs(302),
                )
                .unwrap()
        );
        let acknowledged = store.latest_drift_report("a1").unwrap().unwrap();
        assert!(matches!(
            acknowledged.report.acknowledgement,
            DriftAcknowledgement::Acknowledged { ref by, .. } if by == "admin"
        ));

        assert!(
            store
                .mark_latest_drift_resolved(
                    "a1",
                    "nginx-running",
                    "job-remediate",
                    SystemTime::UNIX_EPOCH + Duration::from_secs(303),
                )
                .unwrap()
        );
        let resolved = store.latest_drift_report("a1").unwrap().unwrap();
        assert!(matches!(
            resolved.report.acknowledgement,
            DriftAcknowledgement::Resolved { ref job_id, .. } if job_id == "job-remediate"
        ));
    }

    #[test]
    fn retention_cleanup_dry_run_does_not_delete() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        seed_retention_rows(&store);

        let summary = store
            .cleanup_retention(SystemTime::UNIX_EPOCH + Duration::from_secs(100), true)
            .unwrap();

        assert_eq!(
            summary,
            RetentionCleanupSummary {
                job_output_chunks: 1,
                facts_snapshots: 1,
                metrics_snapshots: 1,
                agent_log_chunks: 1,
            }
        );
        assert_eq!(row_count(&store, "job_output_chunks"), 2);
        assert_eq!(row_count(&store, "facts_snapshots"), 2);
        assert_eq!(row_count(&store, "metrics_snapshots"), 2);
        assert_eq!(row_count(&store, "agent_log_chunks"), 2);
    }

    #[test]
    fn retention_cleanup_deletes_only_old_rows() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        seed_retention_rows(&store);

        let summary = store
            .cleanup_retention(SystemTime::UNIX_EPOCH + Duration::from_secs(100), false)
            .unwrap();

        assert_eq!(summary.total(), 4);
        assert_eq!(row_count(&store, "job_output_chunks"), 1);
        assert_eq!(row_count(&store, "facts_snapshots"), 1);
        assert_eq!(row_count(&store, "metrics_snapshots"), 1);
        assert_eq!(row_count(&store, "agent_log_chunks"), 1);
    }

    #[test]
    fn retention_cleanup_does_not_delete_audit_events() {
        let mut store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        seed_retention_rows(&store);
        store
            .write(AuditEvent::security("retention_guard", "store"))
            .unwrap();

        store
            .cleanup_retention(SystemTime::UNIX_EPOCH + Duration::from_secs(100), false)
            .unwrap();

        assert_eq!(AuditRepository::list(&store, 10).unwrap().len(), 1);
        assert_eq!(row_count(&store, "audit_events"), 1);
    }

    #[test]
    fn stores_job_and_task_assignment() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-1").unwrap(),
            fleet_domain::TaskRisk::High,
            fleet_domain::ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        store.save_job_record(&job).unwrap();

        store
            .save_task_assignment_record(&task_envelope("nonce-1", "task-1"))
            .unwrap();

        let assignment_exists = store
            .connection
            .prepare("SELECT 1 FROM task_assignments WHERE id = 'task-1'")
            .unwrap()
            .exists([])
            .unwrap();
        assert!(assignment_exists);
        assert_eq!(
            store.find_task_assignment_status("task-1").unwrap(),
            Some("queued".to_owned())
        );
    }

    #[test]
    fn task_assignment_status_transitions_are_persisted() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let mut job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-1").unwrap(),
            fleet_domain::TaskRisk::High,
            fleet_domain::ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        job.queue(true).unwrap();
        let command = CommandTask::new("uptime", Vec::new(), Duration::from_secs(30)).unwrap();
        store.save_command_job_record(&job, &command).unwrap();
        store
            .save_task_assignment_record(&task_envelope("nonce-1", "task-1"))
            .unwrap();

        store
            .update_task_assignment_status(
                "task-1",
                AssignmentStatus::Dispatched,
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
                None,
            )
            .unwrap();

        assert_eq!(
            store.find_task_assignment_status("task-1").unwrap(),
            Some("dispatched".to_owned())
        );
        assert!(
            store
                .list_pending_dispatch_assignments(
                    Some(&AgentId::new("a1").unwrap()),
                    Some(&fleet_domain::JobId::new("job-1").unwrap()),
                    10,
                )
                .unwrap()
                .is_empty()
        );

        store
            .update_task_assignment_status(
                "task-1",
                AssignmentStatus::Accepted,
                SystemTime::UNIX_EPOCH + Duration::from_secs(2),
                None,
            )
            .unwrap();
        store
            .update_task_assignment_status(
                "task-1",
                AssignmentStatus::Started,
                SystemTime::UNIX_EPOCH + Duration::from_secs(3),
                None,
            )
            .unwrap();
        store
            .update_task_assignment_status(
                "task-1",
                AssignmentStatus::Failed,
                SystemTime::UNIX_EPOCH + Duration::from_secs(4),
                Some("exit_code=1"),
            )
            .unwrap();

        assert_eq!(
            store.find_task_assignment_status("task-1").unwrap(),
            Some("failed".to_owned())
        );
    }

    #[test]
    fn active_assignment_update_does_not_override_terminal_status() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let mut job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-1").unwrap(),
            fleet_domain::TaskRisk::High,
            fleet_domain::ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        job.queue(true).unwrap();
        let command = CommandTask::new("uptime", Vec::new(), Duration::from_secs(30)).unwrap();
        store.save_command_job_record(&job, &command).unwrap();
        store
            .save_task_assignment_record(&task_envelope("nonce-1", "task-1"))
            .unwrap();

        store
            .update_task_assignment_status(
                "task-1",
                AssignmentStatus::Canceled,
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
                Some("operator requested cancel"),
            )
            .unwrap();
        let changed = store
            .update_active_task_assignment_status(
                "task-1",
                AssignmentStatus::Succeeded,
                SystemTime::UNIX_EPOCH + Duration::from_secs(2),
                Some("exit_code=0"),
            )
            .unwrap();
        let state = store
            .find_task_assignment_state_for_job("job-1")
            .unwrap()
            .unwrap();

        assert!(!changed);
        assert_eq!(state.task_id, "task-1");
        assert_eq!(state.agent_id, "a1");
        assert_eq!(state.status, "canceled");
    }

    #[test]
    fn pending_command_assignments_include_command_payload() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let mut job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-1").unwrap(),
            fleet_domain::TaskRisk::High,
            fleet_domain::ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        job.queue(true).unwrap();
        let command =
            CommandTask::new("echo", vec!["hello".to_owned()], Duration::from_secs(30)).unwrap();
        store.save_command_job_record(&job, &command).unwrap();
        store
            .save_task_assignment_record(&task_envelope("nonce-1", "task-1"))
            .unwrap();

        let assignments = store
            .list_pending_command_assignments_for_agent("a1")
            .unwrap();

        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].command.program(), "echo");
        assert_eq!(assignments[0].command.args(), ["hello"]);
        assert_eq!(assignments[0].envelope.task_id.as_str(), "task-1");
    }

    #[test]
    fn pending_runbook_assignments_include_runbook_payload() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let mut job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-1").unwrap(),
            fleet_domain::TaskRisk::High,
            fleet_domain::ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        job.queue(true).unwrap();
        let runbook = RunbookExecutionTask::new(
            "apiVersion: fleet.sponzey.dev/v1alpha1\nkind: Runbook",
            Duration::from_secs(30),
        )
        .unwrap();
        store.save_runbook_job_record(&job, &runbook).unwrap();
        store
            .save_task_assignment_record(&task_envelope("nonce-runbook", "task-runbook"))
            .unwrap();

        let assignments = store
            .list_pending_runbook_assignments_for_agent("a1")
            .unwrap();

        assert_eq!(assignments.len(), 1);
        assert!(
            assignments[0]
                .runbook
                .runbook_document()
                .contains("kind: Runbook")
        );
        assert_eq!(assignments[0].envelope.task_id.as_str(), "task-runbook");
    }

    #[test]
    fn pending_dispatch_assignments_are_fifo_across_task_types() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let mut command_job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-command").unwrap(),
            fleet_domain::TaskRisk::High,
            fleet_domain::ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        command_job.queue(true).unwrap();
        let command =
            CommandTask::new("echo", vec!["hello".to_owned()], Duration::from_secs(30)).unwrap();
        store
            .save_command_job_record(&command_job, &command)
            .unwrap();
        store
            .save_task_assignment_record(&task_envelope_for_job(
                "job-command",
                "a1",
                "nonce-command",
                "task-command",
            ))
            .unwrap();
        let mut runbook_job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-runbook").unwrap(),
            fleet_domain::TaskRisk::High,
            fleet_domain::ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        runbook_job.queue(true).unwrap();
        let runbook = RunbookExecutionTask::new("kind: Runbook", Duration::from_secs(30)).unwrap();
        store
            .save_runbook_job_record(&runbook_job, &runbook)
            .unwrap();
        store
            .save_task_assignment_record(&task_envelope_for_job(
                "job-runbook",
                "a1",
                "nonce-runbook-fifo",
                "task-runbook-fifo",
            ))
            .unwrap();
        store
            .connection
            .execute(
                "UPDATE task_assignments
                 SET created_at = CASE id
                   WHEN 'task-runbook-fifo' THEN 1
                   WHEN 'task-command' THEN 2
                   ELSE created_at
                 END",
                [],
            )
            .unwrap();

        let assignments = store
            .list_pending_dispatch_assignments(Some(&AgentId::new("a1").unwrap()), None, 10)
            .unwrap();

        assert_eq!(assignments.len(), 2);
        assert_eq!(
            assignments[0].envelope.task_id.as_str(),
            "task-runbook-fifo"
        );
        assert!(matches!(assignments[0].task, TaskKind::RunbookExecution(_)));
        assert_eq!(assignments[1].envelope.task_id.as_str(), "task-command");
        assert!(matches!(assignments[1].task, TaskKind::Command(_)));
    }

    #[test]
    fn pending_dispatch_assignments_filter_by_agent_and_job() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .save_agent(agent_with_id("a2", "web-02", "1123456789abcdef"))
            .unwrap();
        for (job_id, agent_id, nonce, task_id) in [
            ("job-1", "a1", "nonce-1", "task-1"),
            ("job-2", "a2", "nonce-2", "task-2"),
        ] {
            let mut job = fleet_domain::Job::new(
                fleet_domain::JobId::new(job_id).unwrap(),
                fleet_domain::TaskRisk::High,
                fleet_domain::ApprovalRequirement::AdminConfirmation,
                Duration::from_secs(30),
            );
            job.queue(true).unwrap();
            let command = CommandTask::new("uptime", Vec::new(), Duration::from_secs(30)).unwrap();
            store.save_command_job_record(&job, &command).unwrap();
            store
                .save_task_assignment_record(&task_envelope_for_job(
                    job_id, agent_id, nonce, task_id,
                ))
                .unwrap();
        }

        let assignments = store
            .list_pending_dispatch_assignments(
                Some(&AgentId::new("a2").unwrap()),
                Some(&fleet_domain::JobId::new("job-2").unwrap()),
                10,
            )
            .unwrap();
        let mismatched = store
            .list_pending_dispatch_assignments(
                Some(&AgentId::new("a2").unwrap()),
                Some(&fleet_domain::JobId::new("job-1").unwrap()),
                10,
            )
            .unwrap();

        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].envelope.task_id.as_str(), "task-2");
        assert!(mismatched.is_empty());
    }

    #[test]
    fn lists_recent_job_summaries() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let mut job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-1").unwrap(),
            fleet_domain::TaskRisk::High,
            fleet_domain::ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        job.queue(true).unwrap();
        let command =
            CommandTask::new("uptime", vec!["-a".to_owned()], Duration::from_secs(30)).unwrap();
        store.save_command_job_record(&job, &command).unwrap();
        store
            .save_task_assignment_record(&task_envelope("nonce-1", "task-1"))
            .unwrap();

        let summaries = store.list_job_summaries(10).unwrap();

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, "job-1");
        assert_eq!(summaries[0].status, "queued");
        assert_eq!(summaries[0].command_program.as_deref(), Some("uptime"));
        assert_eq!(summaries[0].command_args, vec!["-a"]);
        assert_eq!(summaries[0].target_count, 1);
        assert_eq!(summaries[0].selector_kind, "explicit_ids");
        assert_eq!(summaries[0].strategy_concurrency, 1);
        assert_eq!(summaries[0].strategy_max_failures, None);
        assert_eq!(summaries[0].target_agents[0].agent_id, "a1");
        assert_eq!(summaries[0].target_agents[0].agent_name, "web-01");
        assert_eq!(summaries[0].target_agents[0].status, "pending");
        assert_eq!(
            summaries[0].target_agents[0].task_id.as_deref(),
            Some("task-1")
        );
        assert_eq!(
            summaries[0].target_agents[0].assignment_status.as_deref(),
            Some("queued")
        );
        assert_eq!(
            summaries[0].target_agents[0].labels,
            vec![("role".to_owned(), "web".to_owned())]
        );
    }

    #[test]
    fn job_target_snapshot_survives_later_agent_label_and_status_changes() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        let mut job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-1").unwrap(),
            fleet_domain::TaskRisk::High,
            fleet_domain::ApprovalRequirement::AdminConfirmation,
            Duration::from_secs(30),
        );
        job.queue(true).unwrap();
        let command = CommandTask::new("uptime", Vec::new(), Duration::from_secs(30)).unwrap();
        store.save_command_job_record(&job, &command).unwrap();
        store
            .update_job_selector_snapshot("job-1", "selector", "label:role=web")
            .unwrap();
        store.update_job_strategy("job-1", 2, Some(1)).unwrap();
        store
            .save_task_assignment_record(&task_envelope("nonce-1", "task-1"))
            .unwrap();

        store
            .update_agent_labels("a1", &[AgentLabel::new("role", "db").unwrap()])
            .unwrap();
        store.revoke_agent_key("a1").unwrap();

        let summary = store.find_job_summary("job-1").unwrap().unwrap();

        assert_eq!(summary.selector_kind, "selector");
        assert_eq!(summary.selector_source, "label:role=web");
        assert_eq!(summary.strategy_concurrency, 2);
        assert_eq!(summary.strategy_max_failures, Some(1));
        assert_eq!(summary.target_agents[0].status, "pending");
        assert_eq!(
            summary.target_agents[0].labels,
            vec![("role".to_owned(), "web".to_owned())]
        );
    }

    #[test]
    fn job_output_chunks_are_stored_in_order() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .save_job_record(&fleet_domain::Job::new(
                fleet_domain::JobId::new("job-1").unwrap(),
                fleet_domain::TaskRisk::Low,
                fleet_domain::ApprovalRequirement::NotRequired,
                Duration::from_secs(30),
            ))
            .unwrap();
        store
            .append_job_output_chunk_record(&JobOutputChunk {
                job_id: "job-1".to_owned(),
                agent_id: "a1".to_owned(),
                stream: JobOutputStream::Stdout,
                sequence: 1,
                body: "second".to_owned(),
            })
            .unwrap();
        store
            .append_job_output_chunk_record(&JobOutputChunk {
                job_id: "job-1".to_owned(),
                agent_id: "a1".to_owned(),
                stream: JobOutputStream::Stdout,
                sequence: 0,
                body: "first".to_owned(),
            })
            .unwrap();

        let chunks = store.list_job_output_chunks("job-1", "a1").unwrap();

        assert_eq!(chunks[0].body, "first");
        assert_eq!(chunks[1].body, "second");
    }

    #[test]
    fn duplicate_job_output_chunks_with_same_body_are_idempotent() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .save_job_record(&fleet_domain::Job::new(
                fleet_domain::JobId::new("job-1").unwrap(),
                fleet_domain::TaskRisk::Low,
                fleet_domain::ApprovalRequirement::NotRequired,
                Duration::from_secs(30),
            ))
            .unwrap();
        let chunk = JobOutputChunk {
            job_id: "job-1".to_owned(),
            agent_id: "a1".to_owned(),
            stream: JobOutputStream::Stdout,
            sequence: 0,
            body: "first".to_owned(),
        };
        store.append_job_output_chunk_record(&chunk).unwrap();

        store.append_job_output_chunk_record(&chunk).unwrap();

        let chunks = store.list_job_output_chunks("job-1", "a1").unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].body, "first");
    }

    #[test]
    fn duplicate_job_output_chunks_with_different_body_are_constraint_violation() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .save_job_record(&fleet_domain::Job::new(
                fleet_domain::JobId::new("job-1").unwrap(),
                fleet_domain::TaskRisk::Low,
                fleet_domain::ApprovalRequirement::NotRequired,
                Duration::from_secs(30),
            ))
            .unwrap();
        let chunk = JobOutputChunk {
            job_id: "job-1".to_owned(),
            agent_id: "a1".to_owned(),
            stream: JobOutputStream::Stdout,
            sequence: 0,
            body: "first".to_owned(),
        };
        store.append_job_output_chunk_record(&chunk).unwrap();
        let conflicting = JobOutputChunk {
            body: "changed".to_owned(),
            ..chunk
        };

        assert!(matches!(
            store.append_job_output_chunk_record(&conflicting),
            Err(StoreError::ConstraintViolation(_))
        ));
    }

    #[test]
    fn consumes_valid_enrollment_token_once() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_enrollment_token_hash(
                "et-1",
                "hash-1",
                "role=web",
                SystemTime::UNIX_EPOCH + Duration::from_secs(60),
                1,
            )
            .unwrap();

        let record = store
            .consume_enrollment_token_hash("hash-1", SystemTime::UNIX_EPOCH)
            .unwrap();

        assert_eq!(record.default_labels, "role=web");
        assert_eq!(
            store.consume_enrollment_token_hash("hash-1", SystemTime::UNIX_EPOCH),
            Err(StoreError::Domain(
                "enrollment token max uses exceeded".to_owned()
            ))
        );
    }

    fn task_envelope(nonce: &str, task_id: &str) -> TaskEnvelope {
        task_envelope_for_job("job-1", "a1", nonce, task_id)
    }

    fn task_envelope_for_job(
        job_id: &str,
        agent_id: &str,
        nonce: &str,
        task_id: &str,
    ) -> TaskEnvelope {
        TaskEnvelope {
            job_id: fleet_domain::JobId::new(job_id).unwrap(),
            task_id: fleet_domain::TaskId::new(task_id).unwrap(),
            target_agent_id: AgentId::new(agent_id).unwrap(),
            issued_at: SystemTime::UNIX_EPOCH,
            expires_at: fleet_domain::TaskExpiry::new(
                SystemTime::UNIX_EPOCH + Duration::from_secs(60),
            ),
            nonce: fleet_domain::TaskNonce::new(nonce).unwrap(),
            payload_hash: "hash".to_owned(),
            signature: Some(fleet_domain::TaskSignature::new("sig").unwrap()),
        }
    }

    fn seed_retention_rows(store: &SqliteStore) {
        let job = fleet_domain::Job::new(
            fleet_domain::JobId::new("job-1").unwrap(),
            fleet_domain::TaskRisk::Low,
            fleet_domain::ApprovalRequirement::NotRequired,
            Duration::from_secs(30),
        );
        store.save_job_record(&job).unwrap();
        store
            .append_job_output_chunk_record(&JobOutputChunk {
                job_id: "job-1".to_owned(),
                agent_id: "a1".to_owned(),
                stream: JobOutputStream::Stdout,
                sequence: 0,
                body: "old".to_owned(),
            })
            .unwrap();
        store
            .append_job_output_chunk_record(&JobOutputChunk {
                job_id: "job-1".to_owned(),
                agent_id: "a1".to_owned(),
                stream: JobOutputStream::Stdout,
                sequence: 1,
                body: "recent".to_owned(),
            })
            .unwrap();
        store
            .connection
            .execute(
                "UPDATE job_output_chunks SET created_at = CASE chunk_index WHEN 0 THEN 1 ELSE 200 END",
                [],
            )
            .unwrap();
        store
            .insert_facts_snapshot(
                "a1",
                "{\"old\":true}",
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();
        store
            .insert_facts_snapshot(
                "a1",
                "{\"recent\":true}",
                SystemTime::UNIX_EPOCH + Duration::from_secs(200),
            )
            .unwrap();
        store
            .insert_metrics_snapshot(
                "a1",
                "{\"old\":true}",
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();
        store
            .insert_metrics_snapshot(
                "a1",
                "{\"recent\":true}",
                SystemTime::UNIX_EPOCH + Duration::from_secs(200),
            )
            .unwrap();
        store
            .insert_agent_log_chunk(
                "a1",
                "level=info event=old_agent_log",
                SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            )
            .unwrap();
        store
            .insert_agent_log_chunk(
                "a1",
                "level=info event=recent_agent_log",
                SystemTime::UNIX_EPOCH + Duration::from_secs(200),
            )
            .unwrap();
    }

    fn row_count(store: &SqliteStore, table: &str) -> usize {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        let count: i64 = store
            .connection
            .query_row(&sql, [], |row| row.get(0))
            .unwrap();
        count as usize
    }

    #[test]
    fn rejects_expired_enrollment_token() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_enrollment_token_hash("et-1", "hash-1", "", SystemTime::UNIX_EPOCH, 1)
            .unwrap();

        assert_eq!(
            store.consume_enrollment_token_hash("hash-1", SystemTime::UNIX_EPOCH),
            Err(StoreError::Domain("enrollment token is expired".to_owned()))
        );
    }

    #[test]
    fn rejects_revoked_enrollment_token() {
        let store = SqliteStore::in_memory().unwrap();
        store
            .insert_enrollment_token_hash(
                "et-1",
                "hash-1",
                "",
                SystemTime::UNIX_EPOCH + Duration::from_secs(60),
                1,
            )
            .unwrap();
        assert!(store.revoke_enrollment_token("et-1").unwrap());

        assert_eq!(
            store.consume_enrollment_token_hash("hash-1", SystemTime::UNIX_EPOCH),
            Err(StoreError::Domain("enrollment token is revoked".to_owned()))
        );
    }

    #[test]
    fn marks_agent_online() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();

        assert!(
            store
                .mark_agent_online("a1", SystemTime::UNIX_EPOCH + Duration::from_secs(5))
                .unwrap()
        );

        let found = store
            .find_by_id(&AgentId::new("a1").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(found.status(), AgentStatus::Online);
        assert_eq!(
            store.find_agent_fingerprint("a1").unwrap().as_deref(),
            Some("0123456789abcdef")
        );
    }

    #[test]
    fn marks_agent_degraded_without_touching_disabled_agents() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();

        assert!(
            store
                .mark_agent_degraded("a1", SystemTime::UNIX_EPOCH + Duration::from_secs(5))
                .unwrap()
        );
        let found = store.find_agent_by_id("a1").unwrap().unwrap();
        assert_eq!(found.status(), AgentStatus::Degraded);

        let disabled_store = SqliteStore::in_memory().unwrap();
        let mut disabled = agent();
        disabled.disable();
        disabled_store.save_agent(disabled).unwrap();
        assert!(
            !disabled_store
                .mark_agent_degraded("a1", SystemTime::UNIX_EPOCH + Duration::from_secs(10))
                .unwrap()
        );
        let found = disabled_store.find_agent_by_id("a1").unwrap().unwrap();
        assert_eq!(found.status(), AgentStatus::Disabled);
    }

    #[test]
    fn stale_online_agents_transition_offline() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .mark_agent_online("a1", SystemTime::UNIX_EPOCH + Duration::from_secs(10))
            .unwrap();

        let changed = store
            .mark_stale_agents_offline(
                SystemTime::UNIX_EPOCH + Duration::from_secs(20),
                SystemTime::UNIX_EPOCH + Duration::from_secs(30),
            )
            .unwrap();

        let found = store.find_agent_by_id("a1").unwrap().unwrap();
        assert_eq!(changed, 1);
        assert_eq!(found.status(), AgentStatus::Offline);
    }

    #[test]
    fn stale_degraded_agents_transition_offline() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .mark_agent_degraded("a1", SystemTime::UNIX_EPOCH + Duration::from_secs(10))
            .unwrap();

        let changed = store
            .mark_stale_agents_offline(
                SystemTime::UNIX_EPOCH + Duration::from_secs(20),
                SystemTime::UNIX_EPOCH + Duration::from_secs(30),
            )
            .unwrap();

        let found = store.find_agent_by_id("a1").unwrap().unwrap();
        assert_eq!(changed, 1);
        assert_eq!(found.status(), AgentStatus::Offline);
    }

    #[test]
    fn recent_online_agents_remain_online_during_offline_sweep() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        store
            .mark_agent_online("a1", SystemTime::UNIX_EPOCH + Duration::from_secs(30))
            .unwrap();

        let changed = store
            .mark_stale_agents_offline(
                SystemTime::UNIX_EPOCH + Duration::from_secs(20),
                SystemTime::UNIX_EPOCH + Duration::from_secs(40),
            )
            .unwrap();

        let found = store.find_agent_by_id("a1").unwrap().unwrap();
        assert_eq!(changed, 0);
        assert_eq!(found.status(), AgentStatus::Online);
    }

    #[test]
    fn audit_repository_is_append_only_and_queryable() {
        let mut store = SqliteStore::in_memory().unwrap();
        store
            .write(AuditEvent::security("invalid_signature", "agent-1"))
            .unwrap();
        store
            .write(AuditEvent {
                category: AuditCategory::Agent,
                action: "online".to_owned(),
                actor: AuditActor::new("system"),
                target: AuditTarget::new("agent-1"),
                value: AuditValue::Plain("status=online".to_owned()),
                occurred_at: SystemTime::UNIX_EPOCH,
            })
            .unwrap();

        assert_eq!(AuditRepository::list(&store, 10).unwrap().len(), 2);
        let security = store.list_by_category(AuditCategory::Security, 10).unwrap();
        assert_eq!(security.len(), 1);
        assert!(!security[0].contains_secret_plaintext());
        assert_eq!(
            store
                .audit_count_by_category(AuditCategory::Security)
                .unwrap(),
            1
        );
    }
}
