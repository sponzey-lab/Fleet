CREATE TABLE schema_migrations (
    name TEXT PRIMARY KEY,
    version INTEGER NOT NULL,
    applied_at INTEGER NOT NULL DEFAULT 0
);

INSERT INTO schema_migrations (name, version, applied_at)
VALUES ('fleet_store', 8, 1710000000);

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
    id,
    status,
    risk,
    approval_requirement,
    timeout_ms,
    command_program,
    command_args_json,
    command_max_output_bytes
) VALUES (
    'legacy-job',
    'queued',
    'high',
    'admin_confirmation',
    30000,
    'uptime',
    '[]',
    1048576
);
