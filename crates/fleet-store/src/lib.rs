use fleet_application::{
    AdminTokenRepository, AgentIdentityRecord as AppAgentIdentityRecord, AgentIdentityRepository,
    AgentRepository, ApprovalDecisionRecord as AppApprovalDecisionRecord, ApprovalRepository,
    AuditRepository, AuditWriter, CommandJobRepository, ControllerIdentityMetadata,
    ControllerIdentityRepository, DispatchAssignmentRepository, DriftCheckJobRepository,
    DriftReportPageRecord as AppDriftReportPageRecord, DriftReportRecord as AppDriftReportRecord,
    DriftRepository, EnrollmentTokenRecord as AppEnrollmentTokenRecord, EnrollmentTokenRepository,
    FactsRepository, FactsSnapshotPageRecord as AppFactsSnapshotPageRecord,
    FactsSnapshotRecord as AppFactsSnapshotRecord, JobOutputChunk, JobOutputRepository,
    JobOutputStream, JobQueryRepository, JobRepository, JobSummaryRecord as AppJobSummaryRecord,
    JobTargetSummaryRecord as AppJobTargetSummaryRecord, MetricsRepository,
    MetricsSnapshotPageRecord as AppMetricsSnapshotPageRecord,
    MetricsSnapshotRecord as AppMetricsSnapshotRecord,
    PendingTaskAssignment as AppPendingTaskAssignment, RunbookJobRepository, SnapshotPageCursor,
    TaskAssignmentRepository,
};
use fleet_domain::{
    Agent, AgentError, AgentFingerprint, AgentId, AgentIdentity, AgentLabel, AgentName,
    AgentPublicKey, AgentStatus, AuditActor, AuditCategory, AuditEvent, AuditTarget, AuditValue,
    CommandTask, ControllerPublicKey, DriftCheckTask, DriftReport, DriftStatus, Job, JobId,
    JobStatus, RunbookExecutionTask, TaskEnvelope, TaskExpiry, TaskId, TaskKind, TaskNonce,
    TaskSignature,
};
use rusqlite::{Connection, ErrorCode, OptionalExtension, params};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
pub struct JobSummaryRecord {
    pub id: String,
    pub status: String,
    pub risk: String,
    pub command_program: Option<String>,
    pub command_args: Vec<String>,
    pub target_count: usize,
    pub target_agents: Vec<JobTargetSummaryRecord>,
    pub created_at: SystemTime,
    pub expires_at: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobTargetSummaryRecord {
    pub agent_id: String,
    pub status: String,
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
pub struct AgentLogChunkRecord {
    pub agent_id: String,
    pub line: String,
    pub collected_at: SystemTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionCleanupSummary {
    pub job_output_chunks: usize,
    pub facts_snapshots: usize,
    pub metrics_snapshots: usize,
}

impl RetentionCleanupSummary {
    pub fn total(self) -> usize {
        self.job_output_chunks + self.facts_snapshots + self.metrics_snapshots
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
        Ok(())
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
        self.connection.execute(
            "INSERT INTO admin_tokens (id, token_hash, created_at)
             VALUES (1, ?1, unixepoch())
             ON CONFLICT(id) DO NOTHING",
            params![token_hash],
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
            "INSERT INTO drift_reports (agent_id, policy_name, status, expected, actual, checked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                agent_id,
                report.policy_name.as_str(),
                drift_status_to_str(&report.status),
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
        };
        if dry_run {
            return Ok(summary);
        }
        self.delete_before("job_output_chunks", "created_at", cutoff)?;
        self.delete_before("facts_snapshots", "collected_at", cutoff)?;
        self.delete_before("metrics_snapshots", "collected_at", cutoff)?;
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
        Ok(())
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
               AND j.status = 'queued'
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
               AND j.status = 'queued'
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
               AND j.status = 'queued'
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
             WHERE j.status = 'queued'
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
                COUNT(ta.id) AS target_count,
                COALESCE(GROUP_CONCAT(ta.agent_id || char(30) || COALESCE(a.status, 'unknown'), char(31)), '') AS target_agents,
                MAX(ta.expires_at) AS expires_at
             FROM jobs j
             LEFT JOIN task_assignments ta ON ta.job_id = j.id
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
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<i64>>(8)?,
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

    fn record_approval_decision(
        &mut self,
        decision: AppApprovalDecisionRecord,
    ) -> Result<(), Self::Error> {
        self.connection.execute(
            "INSERT INTO approval_decisions (
                id, job_id, actor, decision, reason, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                decision.id,
                decision.job_id,
                decision.actor,
                decision.decision,
                decision.reason,
                system_time_to_unix_secs(decision.created_at),
            ],
        )?;
        Ok(())
    }

    fn list_approval_decisions(
        &self,
        job_id: &str,
    ) -> Result<Vec<AppApprovalDecisionRecord>, Self::Error> {
        let mut statement = self.connection.prepare(
            "SELECT id, job_id, actor, decision, reason, created_at
             FROM approval_decisions
             WHERE job_id = ?1
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = statement
            .query_map(params![job_id], |row| {
                Ok(AppApprovalDecisionRecord {
                    id: row.get(0)?,
                    job_id: row.get(1)?,
                    actor: row.get(2)?,
                    decision: row.get(3)?,
                    reason: row.get(4)?,
                    created_at: unix_secs_to_system_time(row.get(5)?),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
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
    fn save_command_job(&mut self, job: Job, task: &CommandTask) -> Result<(), Self::Error> {
        self.save_command_job_record(&job, task)
    }
}

impl DriftCheckJobRepository for SqliteStore {
    fn save_drift_check_job(&mut self, job: Job, task: &DriftCheckTask) -> Result<(), Self::Error> {
        self.save_drift_check_job_record(&job, task)
    }
}

impl RunbookJobRepository for SqliteStore {
    fn save_runbook_job(
        &mut self,
        job: Job,
        task: &RunbookExecutionTask,
    ) -> Result<(), Self::Error> {
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
                target_count: record.target_count,
                target_agents: record
                    .target_agents
                    .into_iter()
                    .map(|target| AppJobTargetSummaryRecord {
                        agent_id: target.agent_id,
                        status: target.status,
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
                target_count: record.target_count,
                target_agents: record
                    .target_agents
                    .into_iter()
                    .map(|target| AppJobTargetSummaryRecord {
                        agent_id: target.agent_id,
                        status: target.status,
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
            let (agent_id, status) = item.split_once('\u{1e}')?;
            if agent_id.is_empty() {
                return None;
            }
            Some(JobTargetSummaryRecord {
                agent_id: agent_id.to_owned(),
                status: status.to_owned(),
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
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS job_targets (
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    PRIMARY KEY (job_id, agent_id)
);

CREATE TABLE IF NOT EXISTS task_assignments (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    nonce TEXT NOT NULL UNIQUE,
    payload_hash TEXT NOT NULL,
    signature TEXT NOT NULL,
    issued_at INTEGER NOT NULL DEFAULT (unixepoch()),
    expires_at INTEGER NOT NULL,
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
    expected TEXT NOT NULL,
    actual TEXT NOT NULL,
    checked_at INTEGER NOT NULL
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
        <SqliteStore as ApprovalRepository>::record_approval_decision(
            &mut store,
            AppApprovalDecisionRecord {
                id: "approval-contract".to_owned(),
                job_id: "job-contract".to_owned(),
                actor: "admin".to_owned(),
                decision: "approved".to_owned(),
                reason: "test".to_owned(),
                created_at: SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            },
        )
        .unwrap();
        assert_eq!(
            <SqliteStore as ApprovalRepository>::list_approval_decisions(&store, "job-contract")
                .unwrap()[0]
                .decision,
            "approved"
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
            }
        );
        assert_eq!(row_count(&store, "job_output_chunks"), 2);
        assert_eq!(row_count(&store, "facts_snapshots"), 2);
        assert_eq!(row_count(&store, "metrics_snapshots"), 2);
    }

    #[test]
    fn retention_cleanup_deletes_only_old_rows() {
        let store = SqliteStore::in_memory().unwrap();
        store.save_agent(agent()).unwrap();
        seed_retention_rows(&store);

        let summary = store
            .cleanup_retention(SystemTime::UNIX_EPOCH + Duration::from_secs(100), false)
            .unwrap();

        assert_eq!(summary.total(), 3);
        assert_eq!(row_count(&store, "job_output_chunks"), 1);
        assert_eq!(row_count(&store, "facts_snapshots"), 1);
        assert_eq!(row_count(&store, "metrics_snapshots"), 1);
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
