//! Importación CSV MySQL protegida: solo inserta un lote InnoDB atómico.

use crate::{history, AppError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{MySqlPool, Row};
use std::{collections::HashSet, io::Cursor};
use zeroize::Zeroizing;

const MAX_VALUE_BYTES: usize = 64 * 1024;
const MAX_PLAN_BYTES: usize = crate::csv_import::MAX_IMPORT_PLAN_BYTES;
const INSERT_MESSAGE: &str = "Importar CSV";
const REVERT_MESSAGE: &str = "Revertir importación CSV";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MysqlCsvBatchPlan {
    version: u8,
    connection_id: String,
    database_name: String,
    table_name: String,
    server_uuid: String,
    server_version: String,
    column_names: Vec<String>,
    primary_key: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
    operation: BatchOperation,
    compensates_revision_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum BatchOperation {
    Insert,
    Delete,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreparedMysqlCsvBatch {
    pub revision: history::RevisionSummary,
    pub headers: Vec<String>,
    pub row_count: usize,
    pub sample_rows: Vec<Vec<Option<String>>>,
}

#[tauri::command]
pub(crate) async fn prepare_mysql_csv_import(
    state: tauri::State<'_, crate::AppState>,
    connection_id: String,
    database_name: String,
    table_name: String,
    csv_text: String,
    delimiter: String,
    null_marker: String,
) -> Result<PreparedMysqlCsvBatch, AppError> {
    if csv_text.len() > 5 * 1024 * 1024 {
        return Err(csv_invalid());
    }
    let delimiter = match delimiter.as_str() {
        "comma" => crate::csv_import::CsvDelimiter::Comma,
        "semicolon" => crate::csv_import::CsvDelimiter::Semicolon,
        "tab" => crate::csv_import::CsvDelimiter::Tab,
        _ => return Err(csv_invalid()),
    };
    if !crate::mysql_row_change::valid_identifier(&database_name)
        || !crate::mysql_row_change::valid_identifier(&table_name)
    {
        return Err(csv_invalid());
    }
    let parsed =
        crate::csv_import::parse(Cursor::new(csv_text.as_bytes()), delimiter, &null_marker)
            .map_err(map_csv_error)?;
    if parsed.rows.len() > crate::csv_import::MAX_IMPORT_ROWS {
        return Err(csv_too_large());
    }
    let storage = state.pool()?;
    let active = active_pool(&state, &connection_id).await?;
    let (server_version, server_uuid): (String, String) =
        sqlx::query_as("SELECT CAST(VERSION() AS CHAR), CAST(@@server_uuid AS CHAR)")
            .fetch_one(&active)
            .await
            .map_err(|_| target_unavailable())?;
    if server_version.to_ascii_lowercase().contains("mariadb") {
        return Err(AppError::history(
            "ENGINE_NOT_SUPPORTED",
            "La importación CSV protegida inicial solo admite MySQL.",
        ));
    }
    let plan = build_insert_plan(
        &active,
        connection_id,
        database_name,
        table_name,
        server_uuid,
        server_version,
        parsed,
    )
    .await?;
    let bytes = crate::csv_import::enforce_plan_size(&plan).map_err(map_csv_error)?;
    persist_plan(
        &storage,
        &state.artifacts_directory,
        &plan,
        &bytes,
        INSERT_MESSAGE,
    )
    .await
}

async fn build_insert_plan(
    pool: &MySqlPool,
    connection_id: String,
    database_name: String,
    table_name: String,
    server_uuid: String,
    server_version: String,
    parsed: crate::csv_import::ParsedCsv,
) -> Result<MysqlCsvBatchPlan, AppError> {
    let table: Option<(String, String)> = sqlx::query_as(
        "SELECT CAST(TABLE_TYPE AS CHAR), CAST(COALESCE(ENGINE, '') AS CHAR)
         FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?",
    )
    .bind(&database_name)
    .bind(&table_name)
    .fetch_optional(pool)
    .await
    .map_err(|_| target_unavailable())?;
    if table.as_ref().map(|v| (v.0.as_str(), v.1.as_str())) != Some(("BASE TABLE", "InnoDB")) {
        return Err(unsupported_table());
    }
    let columns: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT CAST(COLUMN_NAME AS CHAR), CAST(LOWER(DATA_TYPE) AS CHAR),
                CAST(EXTRA AS CHAR), CAST(IS_NULLABLE AS CHAR)
         FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?
         ORDER BY ORDINAL_POSITION",
    )
    .bind(&database_name)
    .bind(&table_name)
    .fetch_all(pool)
    .await
    .map_err(|_| target_unavailable())?;
    let column_names: Vec<String> = columns.iter().map(|c| c.0.clone()).collect();
    if columns.is_empty()
        || columns.len() != parsed.headers.len()
        || columns.iter().any(|(_, kind, extra, _)| {
            !crate::mysql_row_change::supported_scalar_type(kind)
                || extra.to_ascii_lowercase().contains("generated")
                || extra.to_ascii_lowercase().contains("on update")
        })
        || parsed
            .headers
            .iter()
            .any(|name| !column_names.contains(name))
    {
        return Err(unsupported_table());
    }
    if columns.iter().any(|(name, _, _, nullable)| {
        parsed
            .headers
            .iter()
            .position(|header| header == name)
            .and_then(|index| parsed.rows.iter().find(|row| row[index].is_none()))
            .is_some()
            && nullable != "YES"
    }) {
        return Err(csv_invalid());
    }
    let primary_key: Vec<String> = sqlx::query_scalar(
        "SELECT CAST(COLUMN_NAME AS CHAR) FROM information_schema.KEY_COLUMN_USAGE
         WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND CONSTRAINT_NAME = 'PRIMARY'
         ORDER BY ORDINAL_POSITION",
    )
    .bind(&database_name)
    .bind(&table_name)
    .fetch_all(pool)
    .await
    .map_err(|_| target_unavailable())?;
    if primary_key.is_empty() || primary_key.len() > 16 {
        return Err(unsupported_table());
    }
    crate::csv_import::validate_unique_primary_keys(&parsed, &primary_key)
        .map_err(map_csv_error)?;
    validate_no_effects(pool, &database_name, &table_name).await?;

    let mut indexes = Vec::with_capacity(column_names.len());
    for column in &column_names {
        indexes.push(
            parsed
                .headers
                .iter()
                .position(|header| header == column)
                .ok_or_else(csv_invalid)?,
        );
    }
    let rows: Vec<Vec<Option<String>>> = parsed
        .rows
        .iter()
        .map(|row| indexes.iter().map(|index| row[*index].clone()).collect())
        .collect();
    if rows.iter().flatten().any(|value| {
        value
            .as_ref()
            .is_some_and(|value| value.len() > MAX_VALUE_BYTES)
    }) {
        return Err(csv_too_large());
    }
    let plan = MysqlCsvBatchPlan {
        version: 1,
        connection_id,
        database_name,
        table_name,
        server_uuid,
        server_version,
        column_names,
        primary_key,
        rows,
        operation: BatchOperation::Insert,
        compensates_revision_id: None,
    };
    validate_batch_keys(&plan)?;
    for row in &plan.rows {
        if lookup_row(pool, &plan, row, false).await?.is_some() {
            return Err(AppError::history(
                "ROW_CONFLICT",
                "El CSV contiene una clave primaria que ya existe en la tabla destino.",
            ));
        }
    }
    Ok(plan)
}

#[tauri::command]
pub(crate) async fn prepare_mysql_csv_import_revert(
    state: tauri::State<'_, crate::AppState>,
    revision_id: String,
) -> Result<PreparedMysqlCsvBatch, AppError> {
    if uuid::Uuid::parse_str(&revision_id).is_err() {
        return Err(AppError::history(
            "INVALID_HISTORY_REVISION",
            "La revisión del historial no es válida.",
        ));
    }
    let storage = state.pool()?;
    let source: Option<(String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT p.connection_id, p.database_name, p.engine, r.status, r.recovery_state, r.message
         FROM history_revisions r JOIN history_projects p ON p.id = r.project_id WHERE r.id = ?",
    )
    .bind(&revision_id)
    .fetch_optional(storage)
    .await
    .map_err(|_| AppError::storage())?;
    let Some((connection_id, database_name, engine, status, recovery, message)) = source else {
        return Err(AppError::history(
            "HISTORY_REVISION_NOT_FOUND",
            "La revisión ya no está disponible.",
        ));
    };
    if engine != "mysql"
        || status != "applied"
        || recovery != "verified"
        || message != INSERT_MESSAGE
    {
        return Err(AppError::history(
            "ROW_REVERT_UNAVAILABLE",
            "Solo se puede revertir una importación CSV MySQL aplicada y recuperable.",
        ));
    }
    let source_artifact = history::get_row_recovery_artifact(&storage, &revision_id)
        .await?
        .ok_or_else(|| {
            AppError::history(
                "RECOVERY_ARTIFACT_NOT_FOUND",
                "Falta el plan cifrado de la importación aplicada.",
            )
        })?;
    if source_artifact.kind != history::RowRecoveryArtifactKind::RowCompensation
        || source_artifact.verification_state != history::RowRecoveryArtifactState::Verified
    {
        return Err(AppError::history(
            "RECOVERY_NOT_VERIFIED",
            "El plan cifrado de la importación no está verificado.",
        ));
    }
    let active = active_pool(&state, &connection_id).await?;
    let plaintext =
        Zeroizing::new(crate::read_history_plan(state.clone(), revision_id.clone()).await?);
    let source_plan: MysqlCsvBatchPlan =
        serde_json::from_slice(plaintext.as_bytes()).map_err(|_| plan_invalid())?;
    if source_plan.version != 1
        || source_plan.operation != BatchOperation::Insert
        || source_plan.connection_id != connection_id
        || source_plan.database_name != database_name
        || source_plan.compensates_revision_id.is_some()
    {
        return Err(plan_invalid());
    }
    verify_server(&active, &source_plan).await?;
    let mut connection = active.acquire().await.map_err(|_| target_unavailable())?;
    validate_live_schema(&mut connection, &source_plan).await?;
    validate_no_effects(&active, &database_name, &source_plan.table_name).await?;
    for row in &source_plan.rows {
        if lookup_row(&active, &source_plan, row, false)
            .await?
            .as_deref()
            != Some(row.as_slice())
        {
            return Err(AppError::history(
                "ROW_REVERT_CONFLICT",
                "Una fila importada cambió o desapareció; no se preparó la reversión.",
            ));
        }
    }
    let mut plan = source_plan;
    plan.operation = BatchOperation::Delete;
    plan.compensates_revision_id = Some(revision_id);
    let bytes = crate::csv_import::enforce_plan_size(&plan).map_err(map_csv_error)?;
    persist_plan(
        &storage,
        &state.artifacts_directory,
        &plan,
        &bytes,
        REVERT_MESSAGE,
    )
    .await
}

pub(crate) async fn apply_confirmed(
    pool: &MySqlPool,
    storage: &sqlx::SqlitePool,
    revision_id: &str,
    connection_id: &str,
    plaintext: &[u8],
    message: &str,
) -> Result<(), AppError> {
    let plan: MysqlCsvBatchPlan = serde_json::from_slice(plaintext).map_err(|_| plan_invalid())?;
    if plan.version != 1
        || plan.connection_id != connection_id
        || plan.rows.is_empty()
        || !matches!(message, INSERT_MESSAGE | REVERT_MESSAGE)
        || (message == INSERT_MESSAGE
            && (plan.operation != BatchOperation::Insert || plan.compensates_revision_id.is_some()))
        || (message == REVERT_MESSAGE
            && (plan.operation != BatchOperation::Delete
                || plan
                    .compensates_revision_id
                    .as_deref()
                    .is_none_or(|id| uuid::Uuid::parse_str(id).is_err())))
    {
        return Err(plan_invalid());
    }
    validate_batch_keys(&plan)?;
    verify_server(pool, &plan).await?;
    let mut tx = pool.begin().await.map_err(|_| target_unavailable())?;
    validate_live_schema(&mut *tx, &plan).await?;
    validate_no_effects(&mut *tx, &plan.database_name, &plan.table_name).await?;
    for row in &plan.rows {
        let actual = lookup_row(&mut *tx, &plan, row, true).await?;
        match plan.operation {
            BatchOperation::Insert if actual.is_some() => return Err(row_conflict()),
            BatchOperation::Delete if actual.as_deref() != Some(row.as_slice()) => {
                return Err(revert_conflict())
            }
            _ => (),
        }
    }
    history::begin_application(storage, revision_id).await?;
    let predicate = plan
        .primary_key
        .iter()
        .map(|key| format!("{} = ?", crate::mysql_row_change::quote_identifier(key)))
        .collect::<Vec<_>>()
        .join(" AND ");
    match plan.operation {
        BatchOperation::Insert => {
            let cols = plan
                .column_names
                .iter()
                .map(|c| crate::mysql_row_change::quote_identifier(c))
                .collect::<Vec<_>>()
                .join(", ");
            let placeholders = vec!["?"; plan.column_names.len()].join(", ");
            let statement = format!(
                "INSERT INTO {}.{} ({cols}) VALUES ({placeholders})",
                crate::mysql_row_change::quote_identifier(&plan.database_name),
                crate::mysql_row_change::quote_identifier(&plan.table_name)
            );
            for row in &plan.rows {
                let mut query = sqlx::query(&statement);
                for value in row {
                    query = query.bind(value.as_deref());
                }
                if query
                    .execute(&mut *tx)
                    .await
                    .map(|r| r.rows_affected())
                    .unwrap_or(0)
                    != 1
                {
                    return fail_application(
                        tx,
                        storage,
                        revision_id,
                        "csv_batch_insert_failed",
                        "CSV_BATCH_FAILED",
                        "MySQL rechazó el lote; la transacción se revirtió.",
                    )
                    .await;
                }
                match lookup_row(&mut *tx, &plan, row, false).await {
                    Ok(actual) if actual.as_deref() == Some(row.as_slice()) => {}
                    _ => {
                        return fail_application(
                            tx,
                            storage,
                            revision_id,
                            "csv_batch_insert_unverified",
                            "CSV_BATCH_UNVERIFIED",
                            "No se pudo verificar una fila insertada; se revirtió el lote.",
                        )
                        .await;
                    }
                }
            }
        }
        BatchOperation::Delete => {
            let statement = format!(
                "DELETE FROM {}.{} WHERE {predicate}",
                crate::mysql_row_change::quote_identifier(&plan.database_name),
                crate::mysql_row_change::quote_identifier(&plan.table_name)
            );
            for row in &plan.rows {
                let mut query = sqlx::query(&statement);
                for key in &plan.primary_key {
                    let index = plan
                        .column_names
                        .iter()
                        .position(|column| column == key)
                        .ok_or_else(plan_invalid)?;
                    query = query.bind(row[index].as_deref());
                }
                if query
                    .execute(&mut *tx)
                    .await
                    .map(|r| r.rows_affected())
                    .unwrap_or(0)
                    != 1
                {
                    return fail_application(
                        tx,
                        storage,
                        revision_id,
                        "csv_batch_delete_failed",
                        "CSV_BATCH_REVERT_FAILED",
                        "La reversión no eliminó el lote completo; la transacción se revirtió.",
                    )
                    .await;
                }
                match lookup_row(&mut *tx, &plan, row, false).await {
                    Ok(None) => {}
                    _ => {
                        return fail_application(
                            tx,
                            storage,
                            revision_id,
                            "csv_batch_delete_unverified",
                            "CSV_BATCH_REVERT_UNVERIFIED",
                            "No se pudo verificar la reversión; se revirtió la transacción.",
                        )
                        .await;
                    }
                }
            }
        }
    }
    if tx.commit().await.is_err() {
        history::finish_application(
            storage,
            revision_id,
            history::RevisionStatus::Uncertain,
            "csv_batch_commit_uncertain",
        )
        .await?;
        return Err(AppError::history(
            "APPLICATION_UNCERTAIN",
            "Se perdió la confirmación del lote. Inspecciona la tabla antes de reintentar.",
        ));
    }
    history::finish_application(
        storage,
        revision_id,
        history::RevisionStatus::Applied,
        if plan.operation == BatchOperation::Insert {
            "csv_batch_inserted"
        } else {
            "csv_batch_reverted"
        },
    )
    .await
}

async fn fail_application(
    tx: sqlx::Transaction<'_, sqlx::MySql>,
    storage: &sqlx::SqlitePool,
    revision: &str,
    event: &'static str,
    code: &'static str,
    message: &'static str,
) -> Result<(), AppError> {
    let rollback = tx.rollback().await;
    history::finish_application(
        storage,
        revision,
        if rollback.is_ok() {
            history::RevisionStatus::Failed
        } else {
            history::RevisionStatus::Uncertain
        },
        if rollback.is_ok() {
            event
        } else {
            "csv_batch_rollback_uncertain"
        },
    )
    .await?;
    Err(AppError::history(
        if rollback.is_ok() {
            code
        } else {
            "APPLICATION_UNCERTAIN"
        },
        if rollback.is_ok() {
            message
        } else {
            "No se pudo confirmar el rollback del lote. Inspecciona la tabla antes de reintentar."
        },
    ))
}

async fn lookup_row<'e, E>(
    executor: E,
    plan: &MysqlCsvBatchPlan,
    expected: &[Option<String>],
    lock: bool,
) -> Result<Option<Vec<Option<String>>>, AppError>
where
    E: sqlx::Executor<'e, Database = sqlx::MySql>,
{
    let columns = plan
        .column_names
        .iter()
        .map(|c| {
            format!(
                "CAST({} AS CHAR)",
                crate::mysql_row_change::quote_identifier(c)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let predicate = plan
        .primary_key
        .iter()
        .map(|k| format!("{} = ?", crate::mysql_row_change::quote_identifier(k)))
        .collect::<Vec<_>>()
        .join(" AND ");
    let statement = format!(
        "SELECT {columns} FROM {}.{} WHERE {predicate}{}",
        crate::mysql_row_change::quote_identifier(&plan.database_name),
        crate::mysql_row_change::quote_identifier(&plan.table_name),
        if lock { " FOR UPDATE" } else { "" }
    );
    let mut query = sqlx::query(&statement);
    for key in &plan.primary_key {
        let i = plan
            .column_names
            .iter()
            .position(|c| c == key)
            .ok_or_else(plan_invalid)?;
        query = query.bind(expected[i].as_deref());
    }
    let row = query
        .fetch_optional(executor)
        .await
        .map_err(|_| target_unavailable())?;
    row.map(|row| {
        (0..plan.column_names.len())
            .map(|i| {
                row.try_get::<Option<String>, _>(i)
                    .map_err(|_| target_unavailable())
            })
            .collect()
    })
    .transpose()
}

async fn validate_live_schema(
    executor: &mut sqlx::MySqlConnection,
    plan: &MysqlCsvBatchPlan,
) -> Result<(), AppError> {
    let table: Option<(String, String)> = sqlx::query_as("SELECT CAST(TABLE_TYPE AS CHAR), CAST(COALESCE(ENGINE, '') AS CHAR) FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?").bind(&plan.database_name).bind(&plan.table_name).fetch_optional(&mut *executor).await.map_err(|_| target_unavailable())?;
    if table.as_ref().map(|x| (x.0.as_str(), x.1.as_str())) != Some(("BASE TABLE", "InnoDB")) {
        return Err(unsupported_table());
    }
    let cols: Vec<(String, String, String)> = sqlx::query_as("SELECT CAST(COLUMN_NAME AS CHAR), CAST(LOWER(DATA_TYPE) AS CHAR), CAST(EXTRA AS CHAR) FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION").bind(&plan.database_name).bind(&plan.table_name).fetch_all(&mut *executor).await.map_err(|_| target_unavailable())?;
    let pk: Vec<String> = sqlx::query_scalar("SELECT CAST(COLUMN_NAME AS CHAR) FROM information_schema.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND CONSTRAINT_NAME = 'PRIMARY' ORDER BY ORDINAL_POSITION").bind(&plan.database_name).bind(&plan.table_name).fetch_all(&mut *executor).await.map_err(|_| target_unavailable())?;
    if cols.iter().map(|x| x.0.clone()).collect::<Vec<_>>() != plan.column_names
        || pk != plan.primary_key
        || cols.iter().any(|(_, kind, extra)| {
            !crate::mysql_row_change::supported_scalar_type(kind)
                || extra.to_ascii_lowercase().contains("generated")
                || extra.to_ascii_lowercase().contains("on update")
        })
    {
        return Err(AppError::history(
            "ROW_SCHEMA_CHANGED",
            "La estructura de la tabla cambió desde que preparaste la importación.",
        ));
    }
    Ok(())
}

async fn validate_no_effects<'e, E>(
    executor: E,
    database: &str,
    table: &str,
) -> Result<(), AppError>
where
    E: sqlx::Executor<'e, Database = sqlx::MySql>,
{
    let effects: i64 = sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA = ? AND EVENT_OBJECT_TABLE = ?) + (SELECT COUNT(*) FROM information_schema.KEY_COLUMN_USAGE WHERE REFERENCED_TABLE_SCHEMA = ? AND REFERENCED_TABLE_NAME = ?)").bind(database).bind(table).bind(database).bind(table).fetch_one(executor).await.map_err(|_| target_unavailable())?;
    if effects > 0 {
        return Err(AppError::history("ROW_EFFECTS_UNSUPPORTED", "La tabla tiene triggers o claves foráneas entrantes; la importación protegida se bloqueó."));
    }
    Ok(())
}

async fn verify_server(pool: &MySqlPool, plan: &MysqlCsvBatchPlan) -> Result<(), AppError> {
    let (version, uuid): (String, String) =
        sqlx::query_as("SELECT CAST(VERSION() AS CHAR), CAST(@@server_uuid AS CHAR)")
            .fetch_one(pool)
            .await
            .map_err(|_| target_unavailable())?;
    if version != plan.server_version || uuid != plan.server_uuid {
        return Err(AppError::history(
            "TARGET_CHANGED",
            "El servidor cambió desde la revisión; prepara el lote de nuevo.",
        ));
    }
    Ok(())
}

fn validate_batch_keys(plan: &MysqlCsvBatchPlan) -> Result<(), AppError> {
    if plan.column_names.is_empty()
        || plan.primary_key.is_empty()
        || plan.rows.is_empty()
        || plan.rows.len() > crate::csv_import::MAX_IMPORT_ROWS
        || plan.rows.iter().any(|row| {
            row.len() != plan.column_names.len()
                || row.iter().flatten().any(|v| v.len() > MAX_VALUE_BYTES)
        })
    {
        return Err(plan_invalid());
    }
    let mut seen = HashSet::new();
    for row in &plan.rows {
        let key = plan
            .primary_key
            .iter()
            .map(|name| {
                plan.column_names
                    .iter()
                    .position(|c| c == name)
                    .map(|index| row[index].clone())
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(plan_invalid)?;
        if key.iter().any(Option::is_none) || !seen.insert(key) {
            return Err(plan_invalid());
        }
    }
    Ok(())
}

async fn persist_plan(
    storage: &sqlx::SqlitePool,
    artifacts_directory: &std::path::Path,
    plan: &MysqlCsvBatchPlan,
    plaintext: &[u8],
    message: &str,
) -> Result<PreparedMysqlCsvBatch, AppError> {
    if plaintext.len() > MAX_PLAN_BYTES {
        return Err(csv_too_large());
    }
    let project = history::create_project(
        storage,
        &plan.connection_id,
        &plan.database_name,
        history::DatabaseEngine::Mysql,
        Some(&plan.server_version),
    )
    .await?;
    let envelope: Option<crate::vault::RecoveryEnvelope> =
        crate::storage::load(storage, crate::VAULT_ENVELOPE_KEY).await?;
    let envelope = envelope.ok_or_else(|| {
        AppError::history(
            "VAULT_NOT_CONFIGURED",
            "Configura el depósito de recuperación antes de preparar cambios.",
        )
    })?;
    let vault_id = crate::local_vault_id(storage).await?.ok_or_else(|| {
        AppError::history(
            "VAULT_NOT_CONFIGURED",
            "Configura el depósito de recuperación antes de preparar cambios.",
        )
    })?;
    if envelope.vault_id() != vault_id {
        return Err(AppError::history(
            "VAULT_ID_MISMATCH",
            "La clave y la frase local no pertenecen al mismo depósito.",
        ));
    }
    let key = tauri::async_runtime::spawn_blocking(move || crate::vault::load_master_key(vault_id))
        .await
        .map_err(|_| vault_error())?
        .map_err(|_| vault_error())?;
    let plain = Zeroizing::new(plaintext.to_vec());
    let plan_hash = hex(&Sha256::digest(&plain));
    let directory = artifacts_directory.to_path_buf();
    let encrypted_dir = directory.clone();
    let envelope_copy = envelope.clone();
    let artifact = tauri::async_runtime::spawn_blocking(move || {
        crate::vault::encrypt_artifact_bytes_to_file(
            &encrypted_dir,
            &plain,
            vault_id,
            &key,
            &envelope_copy,
        )
    })
    .await
    .map_err(|_| vault_error())?
    .map_err(|_| vault_error())?;
    let path = directory.join(format!("{}.dbsual-artifact", artifact.artifact_id));
    let ciphertext = match std::fs::read(&path) {
        Ok(ciphertext) => ciphertext,
        Err(_) => {
            let _ = std::fs::remove_file(&path);
            return Err(AppError::storage());
        }
    };
    let ciphertext_hash = hex(&Sha256::digest(&ciphertext));
    let revision = history::create_draft(
        storage,
        &project.id,
        message,
        &plan_hash,
        artifact.artifact_id,
        1,
        history::RecoveryState::Pending,
    )
    .await;
    let revision = match revision {
        Ok(revision) => revision,
        Err(error) => {
            let _ = std::fs::remove_file(path);
            return Err(error);
        }
    };
    if let Err(error) = async {
        history::record_row_recovery_artifact(
            storage,
            &project.id,
            &revision.id,
            artifact.artifact_id,
            &ciphertext_hash,
            ciphertext.len() as u64,
        )
        .await?;
        history::mark_row_recovery_artifact_verified(
            storage,
            &revision.id,
            artifact.artifact_id,
            &ciphertext_hash,
        )
        .await?;
        Ok::<(), AppError>(())
    }
    .await
    {
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    let summary = history::list_revisions(storage, &project.id)
        .await?
        .into_iter()
        .find(|r| r.id == revision.id)
        .ok_or_else(AppError::storage)?;
    let preview = crate::csv_import::preview(&plan.column_names, &plan.rows);
    Ok(PreparedMysqlCsvBatch {
        revision: summary,
        headers: preview.headers,
        row_count: preview.row_count,
        sample_rows: preview.sample_rows,
    })
}

async fn active_pool(
    state: &tauri::State<'_, crate::AppState>,
    connection_id: &str,
) -> Result<MySqlPool, AppError> {
    state
        .active
        .0
        .lock()
        .await
        .get(connection_id)
        .cloned()
        .ok_or_else(|| {
            AppError::history(
                "CONNECTION_CLOSED",
                "Abre la conexión MySQL antes de preparar la importación.",
            )
        })
}
fn map_csv_error(error: crate::csv_import::CsvImportError) -> AppError {
    match error {
        crate::csv_import::CsvImportError::TooLarge
        | crate::csv_import::CsvImportError::TooManyRows => csv_too_large(),
        _ => csv_invalid(),
    }
}
fn csv_invalid() -> AppError {
    AppError::history(
        "CSV_IMPORT_INVALID",
        "El CSV no cumple el formato o la tabla de destino.",
    )
}
fn csv_too_large() -> AppError {
    AppError::history(
        "CSV_IMPORT_LIMIT_EXCEEDED",
        "El CSV o el plan supera el límite permitido.",
    )
}
fn row_conflict() -> AppError {
    AppError::history(
        "ROW_CONFLICT",
        "Una clave del lote ya existe; no se aplicó el lote.",
    )
}
fn revert_conflict() -> AppError {
    AppError::history(
        "ROW_REVERT_CONFLICT",
        "Una fila importada cambió o desapareció; no se aplicó la reversión.",
    )
}
fn target_unavailable() -> AppError {
    AppError::history(
        "HISTORY_TARGET_UNAVAILABLE",
        "No se pudo comprobar el estado de la tabla.",
    )
}
fn unsupported_table() -> AppError {
    AppError::history(
        "ROW_TABLE_UNSUPPORTED",
        "La tabla o sus tipos no admiten esta importación protegida.",
    )
}
fn plan_invalid() -> AppError {
    AppError::history(
        "HISTORY_ARTIFACT_UNAVAILABLE",
        "El plan cifrado de importación no es válido.",
    )
}
fn vault_error() -> AppError {
    AppError::history(
        "VAULT_KEYRING_UNAVAILABLE",
        "Windows no pudo abrir la clave del depósito.",
    )
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_plan(rows: Vec<Vec<Option<String>>>) -> MysqlCsvBatchPlan {
        MysqlCsvBatchPlan {
            version: 1,
            connection_id: "connection".into(),
            database_name: "fixture".into(),
            table_name: "items".into(),
            server_uuid: "server".into(),
            server_version: "8.4.11".into(),
            column_names: vec!["id".into(), "value".into()],
            primary_key: vec!["id".into()],
            rows,
            operation: BatchOperation::Insert,
            compensates_revision_id: None,
        }
    }

    #[test]
    fn encrypted_batch_model_rejects_duplicate_null_and_wrong_width_rows() {
        let row = vec![Some("1".into()), Some("one".into())];
        let valid = fixture_plan(vec![row.clone(), vec![Some("2".into()), None]]);
        assert!(validate_batch_keys(&valid).is_ok());
        assert!(crate::csv_import::enforce_plan_size(&valid).is_ok());

        let duplicate = fixture_plan(vec![row.clone(), row]);
        assert_eq!(
            validate_batch_keys(&duplicate).unwrap_err().code,
            "HISTORY_ARTIFACT_UNAVAILABLE"
        );
        let null_key = fixture_plan(vec![vec![None, Some("no id".into())]]);
        assert_eq!(
            validate_batch_keys(&null_key).unwrap_err().code,
            "HISTORY_ARTIFACT_UNAVAILABLE"
        );
        let ragged = fixture_plan(vec![vec![Some("1".into())]]);
        assert_eq!(
            validate_batch_keys(&ragged).unwrap_err().code,
            "HISTORY_ARTIFACT_UNAVAILABLE"
        );
    }

    struct TestDirectory(std::path::PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "dbsual-csv-batch-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir(&path).expect("create isolated test directory");
            Self(path)
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    struct TestVaultKey(uuid::Uuid);
    impl Drop for TestVaultKey {
        fn drop(&mut self) {
            let _ = crate::vault::remove_master_key(self.0);
        }
    }

    async fn create_revision(
        storage: &sqlx::SqlitePool,
        artifact_dir: &std::path::Path,
        vault: &crate::vault::NewVault,
        plan: &MysqlCsvBatchPlan,
        message: &str,
    ) -> history::RevisionSummary {
        let project = history::create_project(
            storage,
            &plan.connection_id,
            &plan.database_name,
            history::DatabaseEngine::Mysql,
            Some(&plan.server_version),
        )
        .await
        .expect("create history project");
        let plaintext = serde_json::to_vec(plan).expect("serialize plan");
        let plan_hash = hex(&Sha256::digest(&plaintext));
        let artifact = crate::vault::encrypt_artifact_bytes_to_file(
            artifact_dir,
            &plaintext,
            vault.vault_id,
            &vault.master_key,
            &vault.recovery_envelope,
        )
        .expect("encrypt plan artifact");
        let path = artifact_dir.join(format!("{}.dbsual-artifact", artifact.artifact_id));
        let ciphertext = std::fs::read(path).expect("read ciphertext for hash");
        let cipher_hash = hex(&Sha256::digest(&ciphertext));
        let revision = history::create_draft(
            storage,
            &project.id,
            message,
            &plan_hash,
            artifact.artifact_id,
            1,
            history::RecoveryState::Pending,
        )
        .await
        .expect("create draft");
        history::record_row_recovery_artifact(
            storage,
            &project.id,
            &revision.id,
            artifact.artifact_id,
            &cipher_hash,
            ciphertext.len() as u64,
        )
        .await
        .expect("record encrypted batch plan");
        history::mark_row_recovery_artifact_verified(
            storage,
            &revision.id,
            artifact.artifact_id,
            &cipher_hash,
        )
        .await
        .expect("verify plan artifact");
        history::confirm_draft(storage, &revision.id)
            .await
            .expect("confirm revision");
        revision
    }

    #[tokio::test]
    #[ignore = "requiere DBSUAL_MYSQL_TEST_PASSWORD y MySQL 8.0/8.4 desechable"]
    async fn mysql_csv_batch_applies_atomically_and_compensation_blocks_external_changes() {
        let pool = crate::connections::connect_mysql_test_from_env()
            .await
            .expect("connect disposable MySQL");
        let database = format!("dbsual_csv_{}", uuid::Uuid::new_v4().simple());
        let connection_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(&format!("CREATE DATABASE `{database}`"))
            .execute(&pool)
            .await
            .expect("create test database");
        let dir = TestDirectory::new();
        let storage = crate::storage::open_at(dir.0.join("history.sqlite"))
            .await
            .expect("open isolated history");
        let artifacts = dir.0.join("artifacts");
        std::fs::create_dir(&artifacts).expect("create artifact directory");
        let vault = crate::vault::create_vault().expect("create isolated vault");
        crate::vault::store_master_key(vault.vault_id, &vault.master_key).expect("store test key");
        let _key = TestVaultKey(vault.vault_id);
        crate::storage::save(&storage, crate::VAULT_ID_KEY, &vault.vault_id)
            .await
            .expect("save test vault id");
        crate::storage::save(
            &storage,
            crate::VAULT_ENVELOPE_KEY,
            &vault.recovery_envelope,
        )
        .await
        .expect("save test envelope");

        async {
            sqlx::query(&format!("CREATE TABLE `{database}`.items (id INT PRIMARY KEY, value VARCHAR(100) NULL) ENGINE=InnoDB")).execute(&pool).await.expect("create table");
            let (server_version, server_uuid): (String, String) = sqlx::query_as("SELECT CAST(VERSION() AS CHAR), CAST(@@server_uuid AS CHAR)").fetch_one(&pool).await.expect("identify server");
            let parsed = crate::csv_import::parse(Cursor::new(b"id,value\n1,one\n2,two\n"), crate::csv_import::CsvDelimiter::Comma, "NULL").expect("parse fixture CSV");
            let plan = build_insert_plan(&pool, connection_id.clone(), database.clone(), "items".into(), server_uuid, server_version, parsed).await.expect("prepare batch without mutation");
            let revision = create_revision(&storage, &artifacts, &vault, &plan, INSERT_MESSAGE).await;
            let authenticated = crate::read_history_plan_from(&storage, &artifacts, revision.id.clone(), vault.vault_id).await.expect("authenticate encrypted batch plan");
            sqlx::query(&format!("INSERT INTO `{database}`.items VALUES (2, 'occupied')")).execute(&pool).await.expect("simulate a key collision after preparation");
            let blocked_insert = apply_confirmed(&pool, &storage, &revision.id, &connection_id, authenticated.as_bytes(), INSERT_MESSAGE).await.expect_err("existing key must block the entire batch");
            assert_eq!(blocked_insert.code, "ROW_CONFLICT");
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM `{database}`.items")).fetch_one(&pool).await.expect("confirm no partial insert");
            assert_eq!(count, 1);
            sqlx::query(&format!("DELETE FROM `{database}`.items WHERE id=2")).execute(&pool).await.expect("remove collision fixture");
            apply_confirmed(&pool, &storage, &revision.id, &connection_id, authenticated.as_bytes(), INSERT_MESSAGE).await.expect("apply atomic batch");
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM `{database}`.items")).fetch_one(&pool).await.expect("count imported rows");
            assert_eq!(count, 2);

            let revert_id = revision.id.clone();
            let mut revert = plan.clone();
            revert.operation = BatchOperation::Delete;
            revert.compensates_revision_id = Some(revert_id);
            let revert_revision = create_revision(&storage, &artifacts, &vault, &revert, REVERT_MESSAGE).await;
            sqlx::query(&format!("UPDATE `{database}`.items SET value='external' WHERE id=2")).execute(&pool).await.expect("introduce external conflict");
            let blocked = apply_confirmed(&pool, &storage, &revert_revision.id, &connection_id, &serde_json::to_vec(&revert).unwrap(), REVERT_MESSAGE).await.expect_err("revert must block on external edit");
            assert_eq!(blocked.code, "ROW_REVERT_CONFLICT");
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM `{database}`.items")).fetch_one(&pool).await.expect("count preserved rows");
            assert_eq!(count, 2);
            sqlx::query(&format!("UPDATE `{database}`.items SET value='two' WHERE id=2")).execute(&pool).await.expect("restore original value");
            apply_confirmed(&pool, &storage, &revert_revision.id, &connection_id, &serde_json::to_vec(&revert).unwrap(), REVERT_MESSAGE).await.expect("apply reviewed batch compensation");
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM `{database}`.items")).fetch_one(&pool).await.expect("confirm compensated batch");
            assert_eq!(count, 0);
            sqlx::query(&format!("DROP DATABASE `{database}`")).execute(&pool).await.expect("drop disposable database");
        }.await;
    }
}
