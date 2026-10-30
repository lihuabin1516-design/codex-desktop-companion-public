use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{backup::Backup, Connection};

use crate::error::{map_io, persistence_error, SqlResultExt, StoreResult};

pub const STORE_SCHEMA_VERSION: i32 = 1;
pub const STORE_SCHEMA_VERSION_TEXT: &str = "coordination.store/v1";

pub(crate) fn configure_connection(connection: &Connection) -> StoreResult<()> {
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .store_result()?;
    connection
        .pragma_update(None, "synchronous", "FULL")
        .store_result()?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .store_result()?;
    connection
        .busy_timeout(Duration::from_millis(5_000))
        .store_result()?;
    connection
        .pragma_update(None, "trusted_schema", "OFF")
        .store_result()?;
    Ok(())
}

pub(crate) fn migrate(
    connection: &mut Connection,
    database_path: &Path,
    migration_backup_dir: Option<&Path>,
) -> StoreResult<()> {
    let current: i32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .store_result()?;
    if current > STORE_SCHEMA_VERSION {
        return Err(persistence_error(format!(
            "database schema version {current} is newer than supported {STORE_SCHEMA_VERSION}"
        )));
    }

    if current == 0 {
        if has_user_tables(connection)? {
            let backup_path = migration_backup_path(database_path, migration_backup_dir, 0, 1)?;
            backup_connection(connection, &backup_path)?;
        }

        let transaction = connection.transaction().store_result()?;
        transaction.execute_batch(SCHEMA_V1).store_result()?;
        transaction
            .pragma_update(None, "user_version", STORE_SCHEMA_VERSION)
            .store_result()?;
        transaction.commit().store_result()?;
    }

    verify_schema(connection)
}

pub(crate) fn backup_connection(connection: &Connection, destination: &Path) -> StoreResult<()> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(map_io)?;
    }
    let mut backup_connection = Connection::open(destination).store_result()?;
    let backup = Backup::new(connection, &mut backup_connection).store_result()?;
    backup
        .run_to_completion(128, Duration::from_millis(10), None)
        .store_result()?;
    Ok(())
}

fn has_user_tables(connection: &Connection) -> StoreResult<bool> {
    let count: i64 = connection
        .query_row(
            r#"
            SELECT COUNT(*)
            FROM sqlite_schema
            WHERE type = 'table'
              AND name NOT LIKE 'sqlite_%'
            "#,
            [],
            |row| row.get(0),
        )
        .store_result()?;
    Ok(count > 0)
}

fn migration_backup_path(
    database_path: &Path,
    backup_dir: Option<&Path>,
    from: i32,
    to: i32,
) -> StoreResult<PathBuf> {
    let directory = backup_dir
        .map(Path::to_path_buf)
        .or_else(|| database_path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&directory).map_err(map_io)?;
    let file_name = database_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("coordination.db");
    for ordinal in 0..10_000 {
        let suffix = if ordinal == 0 {
            String::new()
        } else {
            format!(".{ordinal}")
        };
        let candidate = directory.join(format!(
            "{file_name}.before-v{to}-from-v{from}.backup{suffix}"
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(persistence_error(
        "could not allocate migration backup path",
    ))
}

fn verify_schema(connection: &Connection) -> StoreResult<()> {
    for (table, columns) in REQUIRED_TABLES {
        verify_columns(connection, table, columns)?;
    }
    Ok(())
}

fn verify_columns(connection: &Connection, table: &str, required: &[&str]) -> StoreResult<()> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .store_result()?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .store_result()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .store_result()?;
    if columns.is_empty() {
        return Err(persistence_error(format!(
            "required table {table} is missing from coordination store"
        )));
    }
    for column in required {
        if !columns.iter().any(|candidate| candidate == column) {
            return Err(persistence_error(format!(
                "required column {table}.{column} is missing from coordination store"
            )));
        }
    }
    Ok(())
}

const REQUIRED_TABLES: &[(&str, &[&str])] = &[
    (
        "coordination_workflows",
        &[
            "workflow_id",
            "state",
            "revision",
            "spec_schema_version",
            "spec_json",
            "caller_id",
            "created_at_ms",
            "updated_at_ms",
            "retention_until_ms",
        ],
    ),
    (
        "coordination_workers",
        &[
            "worker_id",
            "workflow_id",
            "step_id",
            "state",
            "revision",
            "objective",
            "access",
            "created_at_ms",
            "updated_at_ms",
        ],
    ),
    (
        "coordination_external_bindings",
        &[
            "binding_id",
            "workflow_id",
            "worker_id",
            "adapter",
            "generation",
            "dispatch_marker",
            "prompt_sha256",
            "state",
            "created_at_ms",
            "updated_at_ms",
        ],
    ),
    (
        "coordination_leases",
        &[
            "lease_id",
            "canonical_resource",
            "resource_sha256",
            "mode",
            "owner_workflow_id",
            "generation",
            "acquired_at_ms",
            "heartbeat_at_ms",
            "expires_at_ms",
        ],
    ),
    (
        "coordination_events",
        &[
            "event_id",
            "workflow_id",
            "sequence",
            "revision",
            "schema_version",
            "kind_json",
            "correlation_id",
            "occurred_at_ms",
            "prev_hash",
            "event_hash",
        ],
    ),
    (
        "coordination_outbox",
        &[
            "outbox_id",
            "workflow_id",
            "state",
            "revision",
            "attempt",
            "generation",
            "command_type",
            "payload_json",
            "not_before_ms",
            "created_at_ms",
            "updated_at_ms",
        ],
    ),
    (
        "coordination_idempotency_records",
        &[
            "scope_key",
            "tool",
            "caller_id",
            "request_id",
            "response_json",
            "state",
            "created_at_ms",
            "expires_at_ms",
        ],
    ),
    (
        "coordination_results",
        &[
            "workflow_id",
            "worker_id",
            "worker_state",
            "aggregation_state",
            "summary",
            "truncated",
            "completed_at_ms",
        ],
    ),
    (
        "coordination_retention",
        &[
            "entity_kind",
            "entity_id",
            "retain_until_ms",
            "max_bytes",
            "reason",
            "created_at_ms",
            "updated_at_ms",
        ],
    ),
    (
        "coordination_store_metadata",
        &["key", "value", "updated_at_ms"],
    ),
    (
        "coordination_schema_migrations",
        &["version", "description", "applied_at_ms"],
    ),
];

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS coordination_store_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS coordination_schema_migrations (
    version INTEGER PRIMARY KEY,
    description TEXT NOT NULL,
    applied_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS coordination_workflows (
    workflow_id TEXT PRIMARY KEY,
    state TEXT NOT NULL CHECK(state IN (
        'DRAFT', 'READY', 'RUNNING', 'PAUSED', 'CANCELLING',
        'SUCCEEDED', 'PARTIAL_FAILED', 'FAILED', 'CANCELLED'
    )),
    revision INTEGER NOT NULL CHECK(revision >= 0),
    spec_schema_version TEXT NOT NULL,
    spec_json TEXT NOT NULL,
    caller_id TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    started_at_ms INTEGER,
    finished_at_ms INTEGER,
    retention_until_ms INTEGER
);

CREATE TABLE IF NOT EXISTS coordination_workers (
    worker_id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES coordination_workflows(workflow_id) ON DELETE CASCADE,
    step_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN (
        'QUEUED', 'DISPATCHING', 'DISPATCH_UNKNOWN', 'RUNNING',
        'CANCELLING', 'SUCCEEDED', 'FAILED', 'CANCELLED'
    )),
    revision INTEGER NOT NULL CHECK(revision >= 0),
    objective TEXT NOT NULL,
    access TEXT NOT NULL CHECK(access IN ('read', 'write')),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    started_at_ms INTEGER,
    finished_at_ms INTEGER,
    UNIQUE(workflow_id, step_id)
);

CREATE TABLE IF NOT EXISTS coordination_external_bindings (
    binding_id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES coordination_workflows(workflow_id) ON DELETE CASCADE,
    worker_id TEXT NOT NULL REFERENCES coordination_workers(worker_id) ON DELETE CASCADE,
    adapter TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK(generation >= 0),
    dispatch_marker TEXT NOT NULL,
    prompt_sha256 TEXT NOT NULL,
    external_task_ref TEXT,
    external_session_ref TEXT,
    state TEXT NOT NULL CHECK(state IN (
        'PENDING', 'ACTIVE', 'RESULT_UNKNOWN', 'CANCEL_REQUESTED',
        'SUCCEEDED', 'FAILED', 'CANCELLED', 'SUPERSEDED'
    )),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    last_reconciled_at_ms INTEGER,
    UNIQUE(worker_id)
);

CREATE TABLE IF NOT EXISTS coordination_leases (
    lease_id TEXT PRIMARY KEY,
    canonical_resource TEXT NOT NULL,
    resource_sha256 TEXT NOT NULL,
    mode TEXT NOT NULL CHECK(mode IN ('SHARED_READ', 'EXCLUSIVE_WRITE')),
    owner_workflow_id TEXT NOT NULL REFERENCES coordination_workflows(workflow_id) ON DELETE CASCADE,
    owner_worker_id TEXT REFERENCES coordination_workers(worker_id) ON DELETE SET NULL,
    generation INTEGER NOT NULL CHECK(generation >= 0),
    acquired_at_ms INTEGER NOT NULL,
    heartbeat_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    released_at_ms INTEGER,
    release_reason TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS coordination_exclusive_writer_lease
    ON coordination_leases(canonical_resource)
    WHERE mode = 'EXCLUSIVE_WRITE' AND released_at_ms IS NULL;

CREATE INDEX IF NOT EXISTS coordination_lease_resource
    ON coordination_leases(canonical_resource, released_at_ms, expires_at_ms);

CREATE TABLE IF NOT EXISTS coordination_events (
    event_id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES coordination_workflows(workflow_id) ON DELETE CASCADE,
    sequence INTEGER NOT NULL CHECK(sequence > 0),
    revision INTEGER NOT NULL CHECK(revision >= 0),
    schema_version TEXT NOT NULL,
    kind_json TEXT NOT NULL,
    correlation_id TEXT NOT NULL,
    occurred_at_ms INTEGER NOT NULL,
    prev_hash TEXT NOT NULL,
    event_hash TEXT NOT NULL,
    UNIQUE(workflow_id, sequence)
);

CREATE INDEX IF NOT EXISTS coordination_events_workflow_sequence
    ON coordination_events(workflow_id, sequence);

CREATE TABLE IF NOT EXISTS coordination_outbox (
    outbox_id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES coordination_workflows(workflow_id) ON DELETE CASCADE,
    worker_id TEXT REFERENCES coordination_workers(worker_id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK(state IN (
        'PENDING', 'SENDING', 'ACKNOWLEDGED',
        'RESULT_UNKNOWN', 'RETRY_WAIT', 'CANCELLED'
    )),
    revision INTEGER NOT NULL CHECK(revision >= 0),
    attempt INTEGER NOT NULL CHECK(attempt >= 0),
    generation INTEGER NOT NULL CHECK(generation >= 0),
    command_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    idempotency_scope TEXT,
    not_before_ms INTEGER NOT NULL,
    last_error TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS coordination_outbox_due
    ON coordination_outbox(state, not_before_ms, created_at_ms);

CREATE TABLE IF NOT EXISTS coordination_idempotency_records (
    scope_key TEXT PRIMARY KEY,
    tool TEXT NOT NULL,
    caller_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    response_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('COMPLETED')),
    created_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    UNIQUE(tool, caller_id, request_id)
);

CREATE INDEX IF NOT EXISTS coordination_idempotency_expires
    ON coordination_idempotency_records(expires_at_ms);

CREATE TABLE IF NOT EXISTS coordination_results (
    workflow_id TEXT NOT NULL REFERENCES coordination_workflows(workflow_id) ON DELETE CASCADE,
    worker_id TEXT NOT NULL REFERENCES coordination_workers(worker_id) ON DELETE CASCADE,
    worker_state TEXT NOT NULL CHECK(worker_state IN (
        'QUEUED', 'DISPATCHING', 'DISPATCH_UNKNOWN', 'RUNNING',
        'CANCELLING', 'SUCCEEDED', 'FAILED', 'CANCELLED'
    )),
    aggregation_state TEXT NOT NULL CHECK(aggregation_state IN (
        'complete', 'partial', 'pending', 'unavailable'
    )),
    summary TEXT NOT NULL,
    artifact_sha256 TEXT,
    truncated INTEGER NOT NULL CHECK(truncated IN (0, 1)),
    completed_at_ms INTEGER NOT NULL,
    retention_until_ms INTEGER,
    PRIMARY KEY (workflow_id, worker_id)
);

CREATE TABLE IF NOT EXISTS coordination_retention (
    entity_kind TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    retain_until_ms INTEGER,
    max_bytes INTEGER,
    reason TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (entity_kind, entity_id)
);

INSERT OR REPLACE INTO coordination_store_metadata (key, value, updated_at_ms)
VALUES ('schemaVersion', 'coordination.store/v1', strftime('%s','now') * 1000);

INSERT OR IGNORE INTO coordination_schema_migrations (version, description, applied_at_ms)
VALUES (1, 'initial coordination store schema', strftime('%s','now') * 1000);
"#;

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn migration_backup_path_is_allocated_outside_database_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("coordination.db");
        let backup = migration_backup_path(&path, Some(dir.path()), 0, 1).unwrap();
        assert_ne!(backup, path);
        assert!(backup
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("before-v1-from-v0"));
    }

    #[test]
    fn required_schema_columns_are_verified() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bad.db");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute("CREATE TABLE coordination_workflows (workflow_id TEXT)", [])
            .unwrap();
        let error = verify_schema(&connection).expect_err("missing columns");
        assert_eq!(
            error.code,
            coordination_core::ErrorCode::PersistenceUnavailable
        );
    }
}
