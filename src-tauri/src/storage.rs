use serde::{de::DeserializeOwned, Serialize};
use sqlx::{sqlite::SqliteConnectOptions, Connection, Row, SqliteConnection, SqlitePool};
use std::{fs, path::PathBuf};
use tauri::Manager;

use crate::AppError;

const SCHEMA_VERSION: i64 = 8;

pub async fn open(app: &tauri::AppHandle) -> Result<SqlitePool, AppError> {
    let data_dir = app.path().app_data_dir().map_err(|_| AppError::storage())?;
    fs::create_dir_all(&data_dir).map_err(|_| AppError::storage())?;
    open_at(data_dir.join("dbsual.sqlite")).await
}

pub(crate) async fn open_at(path: PathBuf) -> Result<SqlitePool, AppError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .map_err(|_| AppError::storage())?;
    let version: i64 = sqlx::query("PRAGMA user_version")
        .fetch_one(&mut connection)
        .await
        .map_err(|_| AppError::storage())?
        .try_get(0)
        .map_err(|_| AppError::storage())?;

    if version > SCHEMA_VERSION {
        return Err(AppError::incompatible_storage());
    }
    if version == 0 {
        let mut transaction = connection.begin().await.map_err(|_| AppError::storage())?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS local_state (\
             key TEXT PRIMARY KEY NOT NULL,\
             payload TEXT NOT NULL\
             )",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        sqlx::query("PRAGMA user_version = 1")
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
        transaction
            .commit()
            .await
            .map_err(|_| AppError::storage())?;
    }
    if version < 2 {
        let mut transaction = connection.begin().await.map_err(|_| AppError::storage())?;
        sqlx::query(
            "CREATE TABLE connections (\
             id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL COLLATE NOCASE UNIQUE, \
             engine TEXT NOT NULL, host TEXT NOT NULL, port INTEGER NOT NULL, \
             user_name TEXT NOT NULL, tls_mode TEXT NOT NULL, tls_ca_path TEXT, \
             tls_client_cert_path TEXT, tls_client_key_path TEXT)",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        sqlx::query("PRAGMA user_version = 2")
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
        transaction
            .commit()
            .await
            .map_err(|_| AppError::storage())?;
    }
    if version < 3 {
        let mut transaction = connection.begin().await.map_err(|_| AppError::storage())?;
        sqlx::query("ALTER TABLE connections ADD COLUMN ssh_enabled INTEGER NOT NULL DEFAULT 0")
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
        sqlx::query("ALTER TABLE connections ADD COLUMN ssh_host TEXT")
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
        sqlx::query("ALTER TABLE connections ADD COLUMN ssh_port INTEGER")
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
        sqlx::query("ALTER TABLE connections ADD COLUMN ssh_user TEXT")
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
        sqlx::query("ALTER TABLE connections ADD COLUMN ssh_auth_method TEXT")
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
        sqlx::query("ALTER TABLE connections ADD COLUMN ssh_key_path TEXT")
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
        sqlx::query("ALTER TABLE connections ADD COLUMN ssh_agent_pipe TEXT")
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
        sqlx::query("PRAGMA user_version = 3")
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
        transaction
            .commit()
            .await
            .map_err(|_| AppError::storage())?;
    }
    if version < 4 {
        let mut transaction = connection.begin().await.map_err(|_| AppError::storage())?;
        sqlx::query(
            "CREATE TABLE history_projects (
                id TEXT PRIMARY KEY NOT NULL,
                connection_id TEXT NOT NULL,
                database_name TEXT NOT NULL COLLATE NOCASE,
                engine TEXT NOT NULL CHECK(engine IN ('mysql','mariadb','postgresql','sqlite')),
                server_version TEXT,
                created_at_ms INTEGER NOT NULL,
                UNIQUE(connection_id, database_name)
            )",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        sqlx::query(
            "CREATE TABLE history_revisions (
                id TEXT PRIMARY KEY NOT NULL,
                project_id TEXT NOT NULL REFERENCES history_projects(id),
                revision_number INTEGER NOT NULL CHECK(revision_number > 0),
                message TEXT NOT NULL,
                status TEXT NOT NULL CHECK(status IN ('draft','confirmed','applying','applied','failed','failed_partial','uncertain','discarded')),
                recovery_state TEXT NOT NULL CHECK(recovery_state IN ('not_required','pending','verified','failed','unavailable')),
                plan_sha256 TEXT NOT NULL CHECK(length(plan_sha256) = 64),
                artifact_id TEXT NOT NULL,
                operation_count INTEGER NOT NULL CHECK(operation_count > 0),
                created_at_ms INTEGER NOT NULL,
                confirmed_at_ms INTEGER,
                UNIQUE(project_id, revision_number)
            )",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        sqlx::query(
            "CREATE TABLE history_events (
                id TEXT PRIMARY KEY NOT NULL,
                revision_id TEXT NOT NULL REFERENCES history_revisions(id),
                event_type TEXT NOT NULL,
                from_status TEXT,
                to_status TEXT,
                detail_code TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL
            )",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        sqlx::query(
            "CREATE TRIGGER history_events_no_update BEFORE UPDATE ON history_events
             BEGIN SELECT RAISE(ABORT, 'history events are immutable'); END",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        sqlx::query(
            "CREATE TRIGGER history_events_no_delete BEFORE DELETE ON history_events
             BEGIN SELECT RAISE(ABORT, 'history events are immutable'); END",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        sqlx::query("PRAGMA user_version = 4")
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
        transaction
            .commit()
            .await
            .map_err(|_| AppError::storage())?;
    }
    if version < 5 {
        let mut transaction = connection.begin().await.map_err(|_| AppError::storage())?;
        sqlx::query(
            "CREATE TABLE recovery_points (
                id TEXT PRIMARY KEY NOT NULL,
                project_id TEXT NOT NULL REFERENCES history_projects(id),
                engine TEXT NOT NULL CHECK(engine IN ('mysql','mariadb','postgresql','sqlite')),
                server_version TEXT,
                artifact_id TEXT NOT NULL UNIQUE,
                ciphertext_sha256 TEXT NOT NULL CHECK(length(ciphertext_sha256) = 64),
                encrypted_bytes INTEGER NOT NULL CHECK(encrypted_bytes > 0),
                plaintext_bytes INTEGER NOT NULL CHECK(plaintext_bytes >= 0),
                coverage TEXT NOT NULL CHECK(coverage IN ('visible_tables_only','complete')),
                verification_state TEXT NOT NULL CHECK(verification_state IN ('captured','verified','failed','unavailable')),
                protected_revision_id TEXT REFERENCES history_revisions(id),
                created_at_ms INTEGER NOT NULL
            )",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        sqlx::query(
            "CREATE INDEX recovery_points_by_project ON recovery_points(project_id, created_at_ms)",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        sqlx::query("PRAGMA user_version = 5")
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
        transaction
            .commit()
            .await
            .map_err(|_| AppError::storage())?;
    }
    if version < 6 {
        let mut transaction = connection.begin().await.map_err(|_| AppError::storage())?;
        sqlx::query(
            "CREATE TABLE recovery_points_v6 (
                id TEXT PRIMARY KEY NOT NULL,
                project_id TEXT NOT NULL REFERENCES history_projects(id),
                engine TEXT NOT NULL CHECK(engine IN ('mysql','mariadb','postgresql','sqlite')),
                server_version TEXT,
                artifact_id TEXT NOT NULL UNIQUE,
                ciphertext_sha256 TEXT NOT NULL CHECK(length(ciphertext_sha256) = 64),
                encrypted_bytes INTEGER NOT NULL CHECK(encrypted_bytes > 0),
                plaintext_bytes INTEGER NOT NULL CHECK(plaintext_bytes >= 0),
                coverage TEXT NOT NULL CHECK(coverage IN ('visible_tables_only','visible_tables_and_views','complete')),
                verification_state TEXT NOT NULL CHECK(verification_state IN ('captured','verified','failed','unavailable')),
                protected_revision_id TEXT REFERENCES history_revisions(id),
                created_at_ms INTEGER NOT NULL
            )",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        sqlx::query(
            "INSERT INTO recovery_points_v6 SELECT id, project_id, engine, server_version,
                artifact_id, ciphertext_sha256, encrypted_bytes, plaintext_bytes, coverage,
                verification_state, protected_revision_id, created_at_ms FROM recovery_points",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        sqlx::query("DROP TABLE recovery_points")
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
        sqlx::query("ALTER TABLE recovery_points_v6 RENAME TO recovery_points")
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
        sqlx::query(
            "CREATE INDEX recovery_points_by_project ON recovery_points(project_id, created_at_ms)",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        sqlx::query("PRAGMA user_version = 6")
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
        transaction
            .commit()
            .await
            .map_err(|_| AppError::storage())?;
    }
    if version < 7 {
        let mut transaction = connection.begin().await.map_err(|_| AppError::storage())?;
        sqlx::query(
            "CREATE TABLE recovery_points_v7 (
                id TEXT PRIMARY KEY NOT NULL,
                project_id TEXT NOT NULL REFERENCES history_projects(id),
                engine TEXT NOT NULL CHECK(engine IN ('mysql','mariadb','postgresql','sqlite')),
                server_version TEXT,
                artifact_id TEXT NOT NULL UNIQUE,
                ciphertext_sha256 TEXT NOT NULL CHECK(length(ciphertext_sha256) = 64),
                encrypted_bytes INTEGER NOT NULL CHECK(encrypted_bytes > 0),
                plaintext_bytes INTEGER NOT NULL CHECK(plaintext_bytes >= 0),
                coverage TEXT NOT NULL CHECK(coverage IN ('visible_tables_only','visible_tables_and_views','visible_tables_views_triggers','complete')),
                verification_state TEXT NOT NULL CHECK(verification_state IN ('captured','verified','failed','unavailable')),
                protected_revision_id TEXT REFERENCES history_revisions(id),
                created_at_ms INTEGER NOT NULL
            )",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        sqlx::query(
            "INSERT INTO recovery_points_v7 SELECT id, project_id, engine, server_version,
                artifact_id, ciphertext_sha256, encrypted_bytes, plaintext_bytes, coverage,
                verification_state, protected_revision_id, created_at_ms FROM recovery_points",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        sqlx::query("DROP TABLE recovery_points")
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
        sqlx::query("ALTER TABLE recovery_points_v7 RENAME TO recovery_points")
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
        sqlx::query(
            "CREATE INDEX recovery_points_by_project ON recovery_points(project_id, created_at_ms)",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        sqlx::query("PRAGMA user_version = 7")
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
        transaction
            .commit()
            .await
            .map_err(|_| AppError::storage())?;
    }
    if version < 8 {
        let mut transaction = connection.begin().await.map_err(|_| AppError::storage())?;
        sqlx::query(
            "CREATE UNIQUE INDEX history_revisions_id_project
             ON history_revisions(id, project_id)",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        sqlx::query(
            "CREATE TABLE row_recovery_artifacts (
                id TEXT PRIMARY KEY NOT NULL,
                project_id TEXT NOT NULL REFERENCES history_projects(id),
                revision_id TEXT NOT NULL,
                artifact_id TEXT NOT NULL UNIQUE,
                kind TEXT NOT NULL CHECK(kind = 'row_compensation'),
                ciphertext_sha256 TEXT NOT NULL CHECK(
                    length(ciphertext_sha256) = 64 AND
                    ciphertext_sha256 NOT GLOB '*[^0-9a-f]*'
                ),
                encrypted_bytes INTEGER NOT NULL CHECK(encrypted_bytes > 0),
                verification_state TEXT NOT NULL CHECK(verification_state IN ('captured','verified')),
                created_at_ms INTEGER NOT NULL,
                UNIQUE(revision_id, kind),
                FOREIGN KEY(revision_id, project_id)
                    REFERENCES history_revisions(id, project_id)
            )",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        sqlx::query(
            "CREATE INDEX row_recovery_artifacts_by_project
             ON row_recovery_artifacts(project_id, created_at_ms)",
        )
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        sqlx::query("PRAGMA user_version = 8")
            .execute(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
        transaction
            .commit()
            .await
            .map_err(|_| AppError::storage())?;
    }
    connection.close().await.map_err(|_| AppError::storage())?;
    sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|_| AppError::storage())
}

pub async fn load<T: DeserializeOwned>(
    pool: &SqlitePool,
    key: &str,
) -> Result<Option<T>, AppError> {
    let row: Option<(String,)> = sqlx::query_as("SELECT payload FROM local_state WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await
        .map_err(|_| AppError::storage())?;
    Ok(row.and_then(|(payload,)| serde_json::from_str(&payload).ok()))
}

pub async fn save<T: Serialize>(pool: &SqlitePool, key: &str, value: &T) -> Result<(), AppError> {
    let payload = serde_json::to_string(value).map_err(|_| AppError::storage())?;
    sqlx::query(
        "INSERT INTO local_state (key, payload) VALUES (?, ?) \
         ON CONFLICT(key) DO UPDATE SET payload = excluded.payload",
    )
    .bind(key)
    .bind(payload)
    .execute(pool)
    .await
    .map_err(|_| AppError::storage())?;
    Ok(())
}

pub async fn remove(pool: &SqlitePool, key: &str) -> Result<(), AppError> {
    sqlx::query("DELETE FROM local_state WHERE key = ?")
        .bind(key)
        .execute(pool)
        .await
        .map_err(|_| AppError::storage())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_and_state_survive_reopen() {
        tauri::async_runtime::block_on(async {
            let path = std::env::temp_dir().join(format!(
                "dbsual-storage-test-{}.sqlite",
                uuid::Uuid::new_v4()
            ));
            let pool = open_at(path.clone()).await.unwrap();
            save(&pool, "ui_preferences", &serde_json::json!({"version": 1}))
                .await
                .unwrap();
            pool.close().await;
            let pool = open_at(path.clone()).await.unwrap();
            let value: serde_json::Value = load(&pool, "ui_preferences").await.unwrap().unwrap();
            assert_eq!(value["version"], 1);
            let recovery_table: (String,) = sqlx::query_as(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='recovery_points'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(recovery_table.0, "recovery_points");
            let row_recovery_table: (String,) = sqlx::query_as(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='row_recovery_artifacts'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(row_recovery_table.0, "row_recovery_artifacts");
            let version: i64 = sqlx::query("PRAGMA user_version")
                .fetch_one(&pool)
                .await
                .unwrap()
                .try_get(0)
                .unwrap();
            assert_eq!(version, 8);
            let recovery_schema: (String,) = sqlx::query_as(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='recovery_points'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(recovery_schema.0.contains("visible_tables_views_triggers"));
            pool.close().await;
            drop(pool);
            let _ = fs::remove_file(path);
        });
    }

    #[test]
    fn v6_recovery_points_migrate_to_trigger_coverage_without_data_loss() {
        tauri::async_runtime::block_on(async {
            let path = std::env::temp_dir().join(format!(
                "dbsual-storage-v6-migration-{}.sqlite",
                uuid::Uuid::new_v4()
            ));
            let options = SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true);
            let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
            sqlx::query("CREATE TABLE history_projects (id TEXT PRIMARY KEY NOT NULL)")
                .execute(&mut connection)
                .await
                .unwrap();
            sqlx::query(
                "CREATE TABLE history_revisions (
                    id TEXT PRIMARY KEY NOT NULL,
                    project_id TEXT NOT NULL
                )",
            )
            .execute(&mut connection)
            .await
            .unwrap();
            sqlx::query(
                "CREATE TABLE recovery_points (
                    id TEXT PRIMARY KEY NOT NULL,
                    project_id TEXT NOT NULL REFERENCES history_projects(id),
                    engine TEXT NOT NULL CHECK(engine IN ('mysql','mariadb','postgresql','sqlite')),
                    server_version TEXT,
                    artifact_id TEXT NOT NULL UNIQUE,
                    ciphertext_sha256 TEXT NOT NULL CHECK(length(ciphertext_sha256) = 64),
                    encrypted_bytes INTEGER NOT NULL CHECK(encrypted_bytes > 0),
                    plaintext_bytes INTEGER NOT NULL CHECK(plaintext_bytes >= 0),
                    coverage TEXT NOT NULL CHECK(coverage IN ('visible_tables_only','visible_tables_and_views','complete')),
                    verification_state TEXT NOT NULL CHECK(verification_state IN ('captured','verified','failed','unavailable')),
                    protected_revision_id TEXT REFERENCES history_revisions(id),
                    created_at_ms INTEGER NOT NULL
                )",
            )
            .execute(&mut connection)
            .await
            .unwrap();
            sqlx::query("INSERT INTO history_projects VALUES ('project-v6')")
                .execute(&mut connection)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO recovery_points VALUES
                 ('point-v6','project-v6','mysql','8.4.0','artifact-v6',?,4096,2048,
                  'visible_tables_and_views','captured',NULL,12345)",
            )
            .bind("a".repeat(64))
            .execute(&mut connection)
            .await
            .unwrap();
            sqlx::query("PRAGMA user_version = 6")
                .execute(&mut connection)
                .await
                .unwrap();
            connection.close().await.unwrap();

            let pool = open_at(path.clone()).await.unwrap();
            let row: (String, String, i64, i64, String, String, i64) = sqlx::query_as(
                "SELECT project_id, engine, encrypted_bytes, plaintext_bytes, coverage,
                        verification_state, created_at_ms
                 FROM recovery_points WHERE id = 'point-v6'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(
                row,
                (
                    "project-v6".into(),
                    "mysql".into(),
                    4096,
                    2048,
                    "visible_tables_and_views".into(),
                    "captured".into(),
                    12345,
                )
            );
            let version: i64 = sqlx::query("PRAGMA user_version")
                .fetch_one(&pool)
                .await
                .unwrap()
                .try_get(0)
                .unwrap();
            assert_eq!(version, 8);
            let schema: (String,) = sqlx::query_as(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='recovery_points'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(schema.0.contains("visible_tables_views_triggers"));
            let recovery_schema: (String,) = sqlx::query_as(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='row_recovery_artifacts'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(recovery_schema.0.contains("row_compensation"));
            assert!(recovery_schema
                .0
                .contains("verification_state IN ('captured','verified')"));
            pool.close().await;
            drop(pool);
            let _ = fs::remove_file(path);
        });
    }

    #[test]
    fn newer_schema_is_rejected_without_modification() {
        tauri::async_runtime::block_on(async {
            let path = std::env::temp_dir().join(format!(
                "dbsual-newer-schema-test-{}.sqlite",
                std::process::id()
            ));
            let options = SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true);
            let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
            sqlx::query("PRAGMA user_version = 99")
                .execute(&mut connection)
                .await
                .unwrap();
            connection.close().await.unwrap();

            let error = open_at(path.clone()).await.unwrap_err();
            assert_eq!(error.code, "STORAGE_VERSION_NEWER");

            let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
            let version: i64 = sqlx::query("PRAGMA user_version")
                .fetch_one(&mut connection)
                .await
                .unwrap()
                .try_get(0)
                .unwrap();
            assert_eq!(version, 99);
            connection.close().await.unwrap();
            let _ = fs::remove_file(path);
        });
    }

    #[test]
    fn existing_preferences_migrate_without_loss() {
        tauri::async_runtime::block_on(async {
            let path = std::env::temp_dir().join(format!(
                "dbsual-v1-migration-test-{}.sqlite",
                std::process::id()
            ));
            let options = SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true);
            let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
            sqlx::query(
                "CREATE TABLE local_state (key TEXT PRIMARY KEY NOT NULL,payload TEXT NOT NULL)",
            )
            .execute(&mut connection)
            .await
            .unwrap();
            sqlx::query("INSERT INTO local_state VALUES ('ui_preferences','{\"version\":1}')")
                .execute(&mut connection)
                .await
                .unwrap();
            sqlx::query("PRAGMA user_version = 1")
                .execute(&mut connection)
                .await
                .unwrap();
            connection.close().await.unwrap();

            let pool = open_at(path.clone()).await.unwrap();
            let value: serde_json::Value = load(&pool, "ui_preferences").await.unwrap().unwrap();
            assert_eq!(value["version"], 1);
            let table: (String,) =
                sqlx::query_as("SELECT name FROM sqlite_master WHERE name='connections'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(table.0, "connections");
            pool.close().await;
            drop(pool);
            let _ = fs::remove_file(path);
        });
    }
}
