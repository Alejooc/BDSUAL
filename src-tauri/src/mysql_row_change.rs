//! Cambios localizados de una fila MySQL con precondiciones y compensación cifrada.

use crate::{history, AppError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{mysql::MySqlRow, MySqlPool, Row, ValueRef};
use std::io::Read;
use zeroize::Zeroizing;

const MAX_IDENTIFIER_CHARS: usize = 64;
const MAX_VALUE_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PrimaryKeyValue {
    pub column: String,
    pub value: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MysqlRowUpdatePlan {
    pub version: u8,
    pub connection_id: String,
    pub database_name: String,
    pub table_name: String,
    pub server_uuid: String,
    pub server_version: String,
    pub primary_key: Vec<PrimaryKeyValue>,
    pub column_name: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    #[serde(default)]
    pub compensates_revision_id: Option<String>,
    #[serde(default)]
    operation: MysqlRowOperation,
    #[serde(default)]
    values: Vec<RowColumnValue>,
    row_sha256: String,
    column_names: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum MysqlRowOperation {
    #[default]
    Update,
    Insert,
    Delete,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RowColumnValue {
    pub column: String,
    pub value: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MysqlRowUpdatePreview {
    pub revision: Option<history::RevisionSummary>,
    pub primary_key: Vec<PrimaryKeyValue>,
    pub column_name: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    #[serde(default)]
    pub operation: String,
    #[serde(default)]
    pub values: Vec<RowColumnValue>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreparedMysqlRowUpdate {
    pub revision: history::RevisionSummary,
    pub primary_key: Vec<PrimaryKeyValue>,
    pub column_name: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    #[serde(default)]
    pub operation: String,
    #[serde(default)]
    pub values: Vec<RowColumnValue>,
}

pub(crate) async fn prepare(
    pool: &MySqlPool,
    connection_id: &str,
    database_name: &str,
    table_name: &str,
    primary_key: Vec<PrimaryKeyValue>,
    column_name: &str,
    new_value: Option<String>,
) -> Result<(MysqlRowUpdatePlan, MysqlRowUpdatePreview), AppError> {
    validate_target(
        connection_id,
        database_name,
        table_name,
        &primary_key,
        column_name,
    )?;
    if new_value
        .as_ref()
        .is_some_and(|value| value.len() > MAX_VALUE_BYTES)
    {
        return Err(invalid_change());
    }

    let (server_version, server_uuid): (String, String) =
        sqlx::query_as("SELECT CAST(VERSION() AS CHAR), CAST(@@server_uuid AS CHAR)")
            .fetch_one(pool)
            .await
            .map_err(|_| target_unavailable())?;
    if server_version.to_ascii_lowercase().contains("mariadb") {
        return Err(AppError::history(
            "ENGINE_NOT_SUPPORTED",
            "La edición protegida inicial solo admite MySQL.",
        ));
    }
    let table: Option<(String, String)> = sqlx::query_as(
        "SELECT CAST(TABLE_TYPE AS CHAR), CAST(COALESCE(ENGINE, '') AS CHAR) FROM information_schema.TABLES
         WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?",
    )
    .bind(database_name)
    .bind(table_name)
    .fetch_optional(pool)
    .await
    .map_err(|_| target_unavailable())?;
    if table.as_ref().map(|(kind, _)| kind.as_str()) != Some("BASE TABLE")
        || table.as_ref().map(|(_, engine)| engine.as_str()) != Some("InnoDB")
    {
        return Err(unsupported_table());
    }

    let columns: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT CAST(COLUMN_NAME AS CHAR), CAST(LOWER(DATA_TYPE) AS CHAR), CAST(IS_NULLABLE AS CHAR), CAST(EXTRA AS CHAR)
         FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?
         ORDER BY ORDINAL_POSITION",
    )
    .bind(database_name)
    .bind(table_name)
    .fetch_all(pool)
    .await
    .map_err(|_| target_unavailable())?;
    if columns.is_empty()
        || columns.len() > 4096
        || columns
            .iter()
            .any(|(_, kind, _, _)| !supported_scalar_type(kind))
    {
        return Err(unsupported_table());
    }
    let column_names: Vec<String> = columns.iter().map(|(name, _, _, _)| name.clone()).collect();
    let key_columns: Vec<String> = sqlx::query_scalar(
        "SELECT CAST(COLUMN_NAME AS CHAR) FROM information_schema.KEY_COLUMN_USAGE
         WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND CONSTRAINT_NAME = 'PRIMARY'
         ORDER BY ORDINAL_POSITION",
    )
    .bind(database_name)
    .bind(table_name)
    .fetch_all(pool)
    .await
    .map_err(|_| target_unavailable())?;
    if key_columns.is_empty()
        || key_columns.len() != primary_key.len()
        || key_columns
            .iter()
            .zip(&primary_key)
            .any(|(expected, actual)| expected != &actual.column || actual.value.is_none())
    {
        return Err(AppError::history(
            "ROW_IDENTITY_CHANGED",
            "La clave primaria de la fila no coincide con la tabla actual.",
        ));
    }
    let target_column = columns.iter().find(|(name, _, _, _)| name == column_name);
    let Some((_, kind, nullable, extra)) = target_column else {
        return Err(AppError::history(
            "COLUMN_NOT_FOUND",
            "La columna ya no existe en la tabla.",
        ));
    };
    if key_columns.iter().any(|name| name == column_name)
        || extra.to_ascii_lowercase().contains("generated")
        || extra.to_ascii_lowercase().contains("on update")
        || (!nullable.eq_ignore_ascii_case("YES") && new_value.is_none())
    {
        return Err(unsupported_table());
    }
    let trigger_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.TRIGGERS
         WHERE TRIGGER_SCHEMA = ? AND EVENT_OBJECT_TABLE = ?",
    )
    .bind(database_name)
    .bind(table_name)
    .fetch_one(pool)
    .await
    .map_err(|_| target_unavailable())?;
    if trigger_count != 0 {
        return Err(AppError::history(
            "ROW_EFFECTS_UNSUPPORTED",
            "La tabla tiene triggers; esta edición localizada no puede acotar sus efectos.",
        ));
    }

    let row = fetch_row(
        pool,
        database_name,
        table_name,
        &column_names,
        &primary_key,
        false,
    )
    .await?
    .ok_or_else(|| AppError::history("ROW_NOT_FOUND", "La fila ya no existe."))?;
    let old_value = read_text_cell(
        &row,
        column_names
            .iter()
            .position(|name| name == column_name)
            .unwrap(),
    )?;
    if old_value == new_value {
        return Err(AppError::history(
            "NO_ROW_CHANGE",
            "El valor propuesto es igual al valor actual.",
        ));
    }
    let row_sha256 = hash_row(&row)?;
    let plan = MysqlRowUpdatePlan {
        version: 1,
        connection_id: connection_id.to_owned(),
        database_name: database_name.to_owned(),
        table_name: table_name.to_owned(),
        server_uuid,
        server_version,
        primary_key: primary_key.clone(),
        column_name: column_name.to_owned(),
        old_value: old_value.clone(),
        new_value: new_value.clone(),
        compensates_revision_id: None,
        operation: MysqlRowOperation::Update,
        values: Vec::new(),
        row_sha256,
        column_names,
    };
    let preview = MysqlRowUpdatePreview {
        revision: None,
        primary_key,
        column_name: column_name.to_owned(),
        old_value,
        new_value,
        operation: "update".into(),
        values: Vec::new(),
    };
    let _ = kind;
    Ok((plan, preview))
}

#[tauri::command]
pub(crate) async fn prepare_mysql_row_update(
    state: tauri::State<'_, crate::AppState>,
    connection_id: String,
    database_name: String,
    table_name: String,
    primary_key: Vec<PrimaryKeyValue>,
    column_name: String,
    new_value: Option<String>,
) -> Result<PreparedMysqlRowUpdate, AppError> {
    let storage_pool = state.pool()?;
    let active = state
        .active
        .0
        .lock()
        .await
        .get(&connection_id)
        .cloned()
        .ok_or_else(|| {
            AppError::history(
                "CONNECTION_CLOSED",
                "Abre la conexión MySQL antes de preparar la edición.",
            )
        })?;
    let (plan, preview) = prepare(
        &active,
        &connection_id,
        &database_name,
        &table_name,
        primary_key,
        &column_name,
        new_value,
    )
    .await?;
    persist_prepared_plan(
        storage_pool,
        &state.artifacts_directory,
        plan,
        preview,
        "Actualizar fila",
    )
    .await
}

#[tauri::command]
pub(crate) async fn prepare_mysql_row_insert(
    state: tauri::State<'_, crate::AppState>,
    connection_id: String,
    database_name: String,
    table_name: String,
    values: Vec<RowColumnValue>,
) -> Result<PreparedMysqlRowUpdate, AppError> {
    let storage_pool = state.pool()?;
    let active = state
        .active
        .0
        .lock()
        .await
        .get(&connection_id)
        .cloned()
        .ok_or_else(|| {
            AppError::history(
                "CONNECTION_CLOSED",
                "Abre la conexión MySQL antes de preparar la fila.",
            )
        })?;
    let (server_version, server_uuid): (String, String) =
        sqlx::query_as("SELECT CAST(VERSION() AS CHAR), CAST(@@server_uuid AS CHAR)")
            .fetch_one(&active)
            .await
            .map_err(|_| target_unavailable())?;
    if server_version.to_ascii_lowercase().contains("mariadb") {
        return Err(AppError::history(
            "ENGINE_NOT_SUPPORTED",
            "La inserción protegida inicial solo admite MySQL.",
        ));
    }
    if !valid_identifier(&database_name)
        || !valid_identifier(&table_name)
        || values.is_empty()
        || values.len() > 4096
        || values.iter().any(|v| {
            !valid_identifier(&v.column)
                || v.value.as_ref().is_some_and(|s| s.len() > MAX_VALUE_BYTES)
        })
    {
        return Err(invalid_change());
    }
    let table: Option<(String, String)> = sqlx::query_as("SELECT CAST(TABLE_TYPE AS CHAR), CAST(COALESCE(ENGINE, '') AS CHAR) FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?")
        .bind(&database_name).bind(&table_name).fetch_optional(&active).await.map_err(|_| target_unavailable())?;
    if table.as_ref().map(|v| (v.0.as_str(), v.1.as_str())) != Some(("BASE TABLE", "InnoDB")) {
        return Err(unsupported_table());
    }
    let columns: Vec<(String, String, String, String)> = sqlx::query_as("SELECT CAST(COLUMN_NAME AS CHAR), CAST(LOWER(DATA_TYPE) AS CHAR), CAST(EXTRA AS CHAR), CAST(IS_NULLABLE AS CHAR) FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION")
        .bind(&database_name).bind(&table_name).fetch_all(&active).await.map_err(|_| target_unavailable())?;
    if columns.is_empty()
        || columns.iter().any(|(_, kind, extra, _)| {
            !supported_scalar_type(kind)
                || extra.to_ascii_lowercase().contains("generated")
                || extra.to_ascii_lowercase().contains("on update")
        })
    {
        return Err(unsupported_table());
    }
    let mut names = std::collections::HashSet::new();
    if values.len() != columns.len()
        || values.iter().any(|v| {
            !names.insert(v.column.as_str())
                || !columns.iter().any(|c| c.0 == v.column)
                || (v.value.is_none()
                    && columns
                        .iter()
                        .find(|c| c.0 == v.column)
                        .is_some_and(|c| c.3 != "YES"))
        })
    {
        return Err(invalid_change());
    }
    let pk: Vec<String> = sqlx::query_scalar("SELECT CAST(COLUMN_NAME AS CHAR) FROM information_schema.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND CONSTRAINT_NAME = 'PRIMARY' ORDER BY ORDINAL_POSITION")
        .bind(&database_name).bind(&table_name).fetch_all(&active).await.map_err(|_| target_unavailable())?;
    if pk.is_empty()
        || pk.len() > 16
        || pk.iter().any(|name| {
            !names.contains(name.as_str())
                || values
                    .iter()
                    .find(|v| &v.column == name)
                    .is_none_or(|v| v.value.is_none())
        })
    {
        return Err(AppError::history(
            "ROW_IDENTITY_CHANGED",
            "Incluye todos los valores de la clave primaria para identificar la fila.",
        ));
    }
    validate_no_row_side_effects(&active, &database_name, &table_name).await?;
    let primary_key: Vec<PrimaryKeyValue> = pk
        .iter()
        .map(|column| PrimaryKeyValue {
            column: column.clone(),
            value: values
                .iter()
                .find(|v| &v.column == column)
                .and_then(|v| v.value.clone()),
        })
        .collect();
    if fetch_row(
        &active,
        &database_name,
        &table_name,
        &columns.iter().map(|c| c.0.clone()).collect::<Vec<_>>(),
        &primary_key,
        false,
    )
    .await?
    .is_some()
    {
        return Err(AppError::history(
            "ROW_CONFLICT",
            "Ya existe una fila con esa clave primaria.",
        ));
    }
    let column_names = columns.iter().map(|c| c.0.clone()).collect();
    let plan = MysqlRowUpdatePlan {
        version: 1,
        connection_id: connection_id.clone(),
        database_name: database_name.clone(),
        table_name,
        server_uuid,
        server_version,
        primary_key: primary_key.clone(),
        column_name: pk[0].clone(),
        old_value: None,
        new_value: None,
        compensates_revision_id: None,
        operation: MysqlRowOperation::Insert,
        values: values.clone(),
        row_sha256: hex(&Sha256::digest(b"DBSUAL mysql row absent v1\0")),
        column_names,
    };
    let preview = MysqlRowUpdatePreview {
        revision: None,
        primary_key,
        column_name: String::new(),
        old_value: None,
        new_value: None,
        operation: "insert".into(),
        values,
    };
    persist_prepared_plan(
        storage_pool,
        &state.artifacts_directory,
        plan,
        preview,
        "Insertar fila",
    )
    .await
}

async fn persist_prepared_plan(
    storage_pool: &sqlx::SqlitePool,
    artifacts_directory: &std::path::Path,
    plan: MysqlRowUpdatePlan,
    preview: MysqlRowUpdatePreview,
    message: &str,
) -> Result<PreparedMysqlRowUpdate, AppError> {
    let project = history::create_project(
        storage_pool,
        &plan.connection_id,
        &plan.database_name,
        history::DatabaseEngine::Mysql,
        Some(&plan.server_version),
    )
    .await?;
    let envelope: Option<crate::vault::RecoveryEnvelope> =
        crate::storage::load(storage_pool, crate::VAULT_ENVELOPE_KEY).await?;
    let envelope = envelope.ok_or_else(|| {
        AppError::history(
            "VAULT_NOT_CONFIGURED",
            "Configura el depósito de recuperación antes de preparar cambios.",
        )
    })?;
    let vault_id = crate::local_vault_id(storage_pool).await?.ok_or_else(|| {
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
    let master_key =
        tauri::async_runtime::spawn_blocking(move || crate::vault::load_master_key(vault_id))
            .await
            .map_err(|_| vault_key_error())?
            .map_err(|_| vault_key_error())?;
    let plaintext = Zeroizing::new(serde_json::to_vec(&plan).map_err(|_| {
        AppError::history(
            "PLAN_ENCRYPTION_FAILED",
            "No se pudo preparar el plan protegido.",
        )
    })?);
    let plan_hash = hex(&Sha256::digest(&*plaintext));
    let encryption_directory = artifacts_directory.to_path_buf();
    let encrypted_plaintext = Zeroizing::new(plaintext.to_vec());
    let artifact = tauri::async_runtime::spawn_blocking(move || {
        crate::vault::encrypt_artifact_bytes_to_file(
            &encryption_directory,
            &encrypted_plaintext,
            vault_id,
            &master_key,
            &envelope,
        )
    })
    .await
    .map_err(|_| plan_encryption_error())?
    .map_err(|_| plan_encryption_error())?;
    let artifact_path =
        artifacts_directory.join(format!("{}.dbsual-artifact", artifact.artifact_id));
    let artifact_path_for_hash = artifact_path.clone();
    let (encrypted_bytes, ciphertext_sha256) =
        tauri::async_runtime::spawn_blocking(move || hash_artifact_file(&artifact_path_for_hash))
            .await
            .map_err(|_| AppError::storage())??;

    let revision = history::create_draft(
        storage_pool,
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
            let _ = std::fs::remove_file(artifact_path);
            return Err(error);
        }
    };
    if let Err(error) = async {
        history::record_row_recovery_artifact(
            storage_pool,
            &project.id,
            &revision.id,
            artifact.artifact_id,
            &ciphertext_sha256,
            encrypted_bytes,
        )
        .await?;
        history::mark_row_recovery_artifact_verified(
            storage_pool,
            &revision.id,
            artifact.artifact_id,
            &ciphertext_sha256,
        )
        .await?;
        Ok::<(), AppError>(())
    }
    .await
    {
        let _ = std::fs::remove_file(artifact_path);
        return Err(error);
    }
    let revision = history::list_revisions(storage_pool, &project.id)
        .await?
        .into_iter()
        .find(|entry| entry.id == revision.id)
        .ok_or_else(AppError::storage)?;
    Ok(PreparedMysqlRowUpdate {
        revision,
        primary_key: preview.primary_key,
        column_name: preview.column_name,
        old_value: preview.old_value,
        new_value: preview.new_value,
        operation: preview.operation,
        values: preview.values,
    })
}

/// Prepara una revisión compensatoria nueva; nunca escribe el valor anterior directamente.
#[tauri::command]
pub(crate) async fn prepare_mysql_row_revert(
    state: tauri::State<'_, crate::AppState>,
    revision_id: String,
) -> Result<PreparedMysqlRowUpdate, AppError> {
    if uuid::Uuid::parse_str(&revision_id).is_err() {
        return Err(AppError::history(
            "INVALID_HISTORY_REVISION",
            "La revisión del historial no es válida.",
        ));
    }
    let storage_pool = state.pool()?;
    let source: Option<(String, String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT p.connection_id, p.database_name, p.engine, r.status, r.recovery_state,
                r.message, r.artifact_id
         FROM history_revisions r JOIN history_projects p ON p.id = r.project_id
         WHERE r.id = ?",
    )
    .bind(&revision_id)
    .fetch_optional(storage_pool)
    .await
    .map_err(|_| AppError::storage())?;
    let Some((connection_id, database_name, engine, status, recovery_state, message, artifact_id)) =
        source
    else {
        return Err(AppError::history(
            "HISTORY_REVISION_NOT_FOUND",
            "La revisión del historial ya no está disponible.",
        ));
    };
    if status != "applied"
        || recovery_state != "verified"
        || engine != "mysql"
        || !matches!(
            message.as_str(),
            "Actualizar fila"
                | "Revertir edición de fila"
                | "Insertar fila"
                | "Revertir inserción de fila"
        )
    {
        return Err(AppError::history(
            "ROW_REVERT_UNAVAILABLE",
            "Solo se puede preparar una reversión para una edición MySQL aplicada y recuperable.",
        ));
    }
    let source_artifact = history::get_row_recovery_artifact(storage_pool, &revision_id)
        .await?
        .ok_or_else(|| {
            AppError::history(
                "RECOVERY_ARTIFACT_NOT_FOUND",
                "Falta el artefacto cifrado asociado a la edición aplicada.",
            )
        })?;
    if source_artifact.kind != history::RowRecoveryArtifactKind::RowCompensation
        || source_artifact.verification_state != history::RowRecoveryArtifactState::Verified
        || source_artifact.artifact_id.to_string() != artifact_id
    {
        return Err(AppError::history(
            "RECOVERY_ARTIFACT_MISMATCH",
            "El artefacto cifrado no corresponde a la edición aplicada.",
        ));
    }

    // La función autentica y compara el hash del artefacto antes de devolver su contenido.
    let source_plaintext =
        Zeroizing::new(crate::read_history_plan(state.clone(), revision_id.clone()).await?);
    let source_plan: MysqlRowUpdatePlan =
        serde_json::from_str(&source_plaintext).map_err(|_| {
            AppError::history(
                "HISTORY_ARTIFACT_UNAVAILABLE",
                "El plan cifrado de la edición aplicada no es válido.",
            )
        })?;
    if source_plan.version != 1
        || source_plan.connection_id != connection_id
        || source_plan.database_name != database_name
        || source_plan.primary_key.is_empty()
        || source_plan
            .compensates_revision_id
            .as_ref()
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_err() || id == &revision_id)
    {
        return Err(AppError::history(
            "HISTORY_ARTIFACT_MISMATCH",
            "El plan cifrado no coincide con la edición aplicada.",
        ));
    }

    let active = state
        .active
        .0
        .lock()
        .await
        .get(&connection_id)
        .cloned()
        .ok_or_else(|| {
            AppError::history(
                "CONNECTION_CLOSED",
                "Abre la conexión MySQL antes de preparar la reversión.",
            )
        })?;
    if message == "Insertar fila" {
        if source_plan.operation != MysqlRowOperation::Insert {
            return Err(AppError::history(
                "HISTORY_ARTIFACT_MISMATCH",
                "La revisión no contiene una inserción.",
            ));
        }
        let (plan, preview) = prepare_insert_revert(&active, &source_plan, &revision_id).await?;
        return persist_prepared_plan(
            storage_pool,
            &state.artifacts_directory,
            plan,
            preview,
            "Revertir inserción de fila",
        )
        .await;
    }
    let (mut plan, preview) = prepare(
        &active,
        &connection_id,
        &database_name,
        &source_plan.table_name,
        source_plan.primary_key.clone(),
        &source_plan.column_name,
        source_plan.old_value.clone(),
    )
    .await?;
    if plan.server_uuid != source_plan.server_uuid {
        return Err(AppError::history(
            "ROW_TARGET_CHANGED",
            "El servidor MySQL es distinto al que recibió la edición original.",
        ));
    }
    if preview.old_value != source_plan.new_value {
        return Err(AppError::history(
            "ROW_REVERT_CONFLICT",
            "La fila cambió después de la edición aplicada. Inspecciónala antes de preparar una reversión.",
        ));
    }
    plan.compensates_revision_id = Some(revision_id);
    persist_prepared_plan(
        storage_pool,
        &state.artifacts_directory,
        plan,
        preview,
        "Revertir edición de fila",
    )
    .await
}

async fn prepare_insert_revert(
    pool: &MySqlPool,
    source: &MysqlRowUpdatePlan,
    source_revision_id: &str,
) -> Result<(MysqlRowUpdatePlan, MysqlRowUpdatePreview), AppError> {
    let (version, uuid): (String, String) =
        sqlx::query_as("SELECT CAST(VERSION() AS CHAR), CAST(@@server_uuid AS CHAR)")
            .fetch_one(pool)
            .await
            .map_err(|_| target_unavailable())?;
    if version != source.server_version || uuid != source.server_uuid {
        return Err(AppError::history(
            "ROW_TARGET_CHANGED",
            "El servidor MySQL es distinto al de la inserción original.",
        ));
    }
    let mut tx = pool.begin().await.map_err(|_| target_unavailable())?;
    let mut plan = source.clone();
    plan.operation = MysqlRowOperation::Delete;
    plan.compensates_revision_id = Some(source_revision_id.to_owned());
    validate_live_table(&mut *tx, &plan).await?;
    validate_no_row_side_effects(&mut *tx, &plan.database_name, &plan.table_name).await?;
    let row = fetch_row(
        &mut *tx,
        &plan.database_name,
        &plan.table_name,
        &plan.column_names,
        &plan.primary_key,
        true,
    )
    .await?
    .ok_or_else(|| {
        AppError::history(
            "ROW_REVERT_CONFLICT",
            "La fila insertada ya no existe; no se preparó ninguna eliminación.",
        )
    })?;
    // Todas las columnas se capturaron explícitamente antes de insertar; comparar la fila completa detecta cambios externos.
    let text_row = fetch_row_text(
        pool,
        &plan.database_name,
        &plan.table_name,
        &plan.column_names,
        &plan.primary_key,
    )
    .await?
    .ok_or_else(|| {
        AppError::history(
            "ROW_REVERT_CONFLICT",
            "La fila insertada dejó de estar disponible.",
        )
    })?;
    for value in &source.values {
        let index = source
            .column_names
            .iter()
            .position(|column| column == &value.column)
            .ok_or_else(invalid_change)?;
        if read_text_cell(&text_row, index)? != value.value {
            return Err(AppError::history(
                "ROW_REVERT_CONFLICT",
                "La fila insertada cambió desde su aplicación; inspecciónala antes de revertir.",
            ));
        }
    }
    let row_sha256 = hash_row(&row)?;
    let _ = tx.rollback().await;
    plan.values.clear();
    plan.row_sha256 = row_sha256;
    let preview = MysqlRowUpdatePreview {
        revision: None,
        primary_key: plan.primary_key.clone(),
        column_name: String::new(),
        old_value: None,
        new_value: None,
        operation: "delete".into(),
        values: source.values.clone(),
    };
    Ok((plan, preview))
}

#[tauri::command]
pub(crate) async fn apply_mysql_row_update(
    state: tauri::State<'_, crate::AppState>,
    revision_id: String,
) -> Result<(), AppError> {
    if uuid::Uuid::parse_str(&revision_id).is_err() {
        return Err(AppError::history(
            "INVALID_HISTORY_REVISION",
            "La revisión del historial no es válida.",
        ));
    }
    let storage_pool = state.pool()?;
    let revision: Option<(String, String, String, String, String, String, String)> =
        sqlx::query_as(
            "SELECT p.connection_id, p.database_name, p.engine, r.status, r.recovery_state,
                    r.message, r.artifact_id
             FROM history_revisions r JOIN history_projects p ON p.id = r.project_id
             WHERE r.id = ?",
        )
        .bind(&revision_id)
        .fetch_optional(storage_pool)
        .await
        .map_err(|_| AppError::storage())?;
    let (connection_id, database_name, engine, status, recovery_state, message, plan_artifact_id) =
        revision.ok_or_else(|| {
            AppError::history(
                "HISTORY_REVISION_NOT_FOUND",
                "La revisión ya no está disponible.",
            )
        })?;
    if !matches!(
        message.as_str(),
        "Actualizar fila"
            | "Revertir edición de fila"
            | "Insertar fila"
            | "Revertir inserción de fila"
            | "Importar CSV"
            | "Revertir importación CSV"
    ) || status != "confirmed"
        || recovery_state != "verified"
        || engine != "mysql"
    {
        return Err(AppError::history(
            "REVISION_NOT_APPLICABLE",
            "La revisión no está confirmada o no corresponde a una edición MySQL recuperable.",
        ));
    }
    let recovery = history::get_row_recovery_artifact(storage_pool, &revision_id)
        .await?
        .ok_or_else(|| {
            AppError::history(
                "RECOVERY_ARTIFACT_NOT_FOUND",
                "Falta la compensación cifrada requerida por esta revisión.",
            )
        })?;
    if recovery.kind != history::RowRecoveryArtifactKind::RowCompensation
        || recovery.verification_state != history::RowRecoveryArtifactState::Verified
        || recovery.artifact_id != plan_artifact_id
    {
        return Err(AppError::history(
            "RECOVERY_ARTIFACT_MISMATCH",
            "La compensación cifrada no coincide con la revisión confirmada.",
        ));
    }
    let artifact_path = state
        .artifacts_directory
        .join(format!("{}.dbsual-artifact", recovery.artifact_id));
    let artifact_path_for_hash = artifact_path.clone();
    let (encrypted_bytes, ciphertext_sha256) =
        tauri::async_runtime::spawn_blocking(move || hash_artifact_file(&artifact_path_for_hash))
            .await
            .map_err(|_| AppError::storage())??;
    if encrypted_bytes != recovery.encrypted_bytes as u64
        || ciphertext_sha256 != recovery.ciphertext_sha256
    {
        return Err(AppError::history(
            "RECOVERY_ARTIFACT_MISMATCH",
            "El artefacto cifrado cambió después de preparar la revisión.",
        ));
    }
    let encrypted_plan = crate::read_history_plan(state.clone(), revision_id.clone()).await?;
    if matches!(
        message.as_str(),
        "Importar CSV" | "Revertir importación CSV"
    ) {
        let active = state
            .active
            .0
            .lock()
            .await
            .get(&connection_id)
            .cloned()
            .ok_or_else(|| {
                AppError::history(
                    "CONNECTION_CLOSED",
                    "Abre la conexión MySQL antes de aplicar la revisión.",
                )
            })?;
        return crate::mysql_csv_batch::apply_confirmed(
            &active,
            storage_pool,
            &revision_id,
            &connection_id,
            encrypted_plan.as_bytes(),
            &message,
        )
        .await;
    }
    let plan: MysqlRowUpdatePlan = serde_json::from_str(&encrypted_plan).map_err(|_| {
        AppError::history(
            "HISTORY_ARTIFACT_UNAVAILABLE",
            "El plan cifrado de edición no es válido.",
        )
    })?;
    if plan.connection_id != connection_id || plan.database_name != database_name {
        return Err(AppError::history(
            "HISTORY_ARTIFACT_MISMATCH",
            "El destino del plan no coincide con el proyecto del historial.",
        ));
    }
    let kind_mismatch = match message.as_str() {
        "Actualizar fila" => {
            plan.operation != MysqlRowOperation::Update || plan.compensates_revision_id.is_some()
        }
        "Revertir edición de fila" => {
            plan.operation != MysqlRowOperation::Update
                || plan
                    .compensates_revision_id
                    .as_deref()
                    .is_none_or(|id| uuid::Uuid::parse_str(id).is_err())
        }
        "Insertar fila" => {
            plan.operation != MysqlRowOperation::Insert || plan.compensates_revision_id.is_some()
        }
        "Revertir inserción de fila" => {
            plan.operation != MysqlRowOperation::Delete
                || plan
                    .compensates_revision_id
                    .as_deref()
                    .is_none_or(|id| uuid::Uuid::parse_str(id).is_err())
        }
        _ => true,
    };
    if kind_mismatch {
        return Err(AppError::history(
            "HISTORY_ARTIFACT_MISMATCH",
            "El tipo de revisión no coincide con el plan cifrado.",
        ));
    }
    let active = state
        .active
        .0
        .lock()
        .await
        .get(&connection_id)
        .cloned()
        .ok_or_else(|| {
            AppError::history(
                "CONNECTION_CLOSED",
                "Abre la conexión MySQL antes de aplicar la revisión.",
            )
        })?;
    apply(&active, storage_pool, &revision_id, &connection_id, &plan).await
}

fn hash_artifact_file(path: &std::path::Path) -> Result<(u64, String), AppError> {
    let metadata = std::fs::metadata(path).map_err(|_| AppError::storage())?;
    if metadata.len() == 0 || metadata.len() > 2 * 1024 * 1024 {
        return Err(AppError::storage());
    }
    let file = std::fs::File::open(path).map_err(|_| AppError::storage())?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(metadata.len() as usize));
    file.take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| AppError::storage())?;
    if bytes.len() as u64 != metadata.len() {
        return Err(AppError::storage());
    }
    Ok((metadata.len(), hex(&Sha256::digest(&bytes))))
}

fn vault_key_error() -> AppError {
    AppError::history(
        "VAULT_KEYRING_UNAVAILABLE",
        "Windows no pudo abrir la clave del depósito.",
    )
}

fn plan_encryption_error() -> AppError {
    AppError::history(
        "PLAN_ENCRYPTION_FAILED",
        "No se pudo guardar la edición cifrada.",
    )
}

pub(crate) async fn apply(
    pool: &MySqlPool,
    storage: &sqlx::SqlitePool,
    revision_id: &str,
    connection_id: &str,
    plan: &MysqlRowUpdatePlan,
) -> Result<(), AppError> {
    if plan.version != 1
        || plan.connection_id != connection_id
        || plan.column_names.is_empty()
        || plan.column_names.len() > 4096
        || plan.primary_key.is_empty()
        || plan.primary_key.len() > plan.column_names.len()
        || plan.row_sha256.len() != 64
        || !plan.row_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || plan
            .new_value
            .as_ref()
            .is_some_and(|value| value.len() > MAX_VALUE_BYTES)
        || validate_target(
            connection_id,
            &plan.database_name,
            &plan.table_name,
            &plan.primary_key,
            &plan.column_name,
        )
        .is_err()
    {
        return Err(invalid_change());
    }
    let (server_version, server_uuid): (String, String) =
        sqlx::query_as("SELECT CAST(VERSION() AS CHAR), CAST(@@server_uuid AS CHAR)")
            .fetch_one(pool)
            .await
            .map_err(|_| target_unavailable())?;
    if server_uuid != plan.server_uuid || server_version != plan.server_version {
        return Err(AppError::history(
            "TARGET_CHANGED",
            "El servidor cambió desde la revisión. Prepara el cambio de nuevo.",
        ));
    }
    let recovery = history::get_row_recovery_artifact(storage, revision_id)
        .await?
        .ok_or_else(|| {
            AppError::history(
                "RECOVERY_ARTIFACT_NOT_FOUND",
                "Falta la compensación cifrada requerida por esta revisión.",
            )
        })?;
    if recovery.kind != history::RowRecoveryArtifactKind::RowCompensation
        || recovery.verification_state != history::RowRecoveryArtifactState::Verified
    {
        return Err(AppError::history(
            "RECOVERY_NOT_VERIFIED",
            "La imagen anterior de la fila no está verificada.",
        ));
    }

    if plan.operation == MysqlRowOperation::Delete {
        if plan
            .compensates_revision_id
            .as_deref()
            .is_none_or(|id| uuid::Uuid::parse_str(id).is_err())
            || !plan.values.is_empty()
        {
            return Err(invalid_change());
        }
        return apply_delete(pool, storage, revision_id, plan).await;
    }
    if plan.operation == MysqlRowOperation::Insert {
        if plan.values.len() != plan.column_names.len()
            || plan.compensates_revision_id.is_some()
            || plan.values.iter().any(|value| {
                !plan.column_names.contains(&value.column)
                    || value
                        .value
                        .as_ref()
                        .is_some_and(|v| v.len() > MAX_VALUE_BYTES)
            })
        {
            return Err(invalid_change());
        }
        return apply_insert(pool, storage, revision_id, plan).await;
    }

    let mut transaction = pool.begin().await.map_err(|_| target_unavailable())?;
    let locked = fetch_row(
        &mut *transaction,
        &plan.database_name,
        &plan.table_name,
        &plan.column_names,
        &plan.primary_key,
        true,
    )
    .await?;
    let Some(current) = locked else {
        return Err(AppError::history(
            "ROW_CONFLICT",
            "La fila ya no existe; revisa el cambio.",
        ));
    };
    if hash_row(&current)? != plan.row_sha256 {
        return Err(AppError::history(
            "ROW_CONFLICT",
            "La fila cambió desde que preparaste la revisión. Vuelve a revisarla.",
        ));
    }
    validate_live_table(&mut *transaction, plan).await?;
    history::begin_application(storage, revision_id).await?;

    let update = update_row(&mut *transaction, plan).await;
    let affected = match update {
        Ok(affected) => affected,
        Err(_) => {
            if transaction.rollback().await.is_ok() {
                history::finish_application(
                    storage,
                    revision_id,
                    history::RevisionStatus::Failed,
                    "row_update_rejected",
                )
                .await?;
                return Err(AppError::history(
                    "ROW_UPDATE_FAILED",
                    "MySQL rechazó la actualización; la transacción se revirtió.",
                ));
            }
            history::finish_application(
                storage,
                revision_id,
                history::RevisionStatus::Uncertain,
                "row_update_uncertain",
            )
            .await?;
            return Err(AppError::history("APPLICATION_UNCERTAIN", "No se pudo confirmar el resultado de la actualización. Vuelve a consultar la fila antes de reintentar."));
        }
    };
    if affected != 1 {
        let rollback = transaction.rollback().await;
        history::finish_application(
            storage,
            revision_id,
            if rollback.is_ok() {
                history::RevisionStatus::Failed
            } else {
                history::RevisionStatus::Uncertain
            },
            if rollback.is_ok() {
                "row_identity_mismatch"
            } else {
                "row_rollback_uncertain"
            },
        )
        .await?;
        if rollback.is_err() {
            return Err(AppError::history("APPLICATION_UNCERTAIN", "No se pudo verificar la reversión de la actualización. Consulta la fila antes de reintentar."));
        }
        return Err(AppError::history(
            "ROW_CONFLICT",
            "La actualización no afectó exactamente una fila.",
        ));
    }
    let updated = fetch_row(
        &mut *transaction,
        &plan.database_name,
        &plan.table_name,
        &plan.column_names,
        &plan.primary_key,
        false,
    )
    .await;
    let updated = match updated {
        Ok(updated) => updated,
        Err(error) => {
            let rollback = transaction.rollback().await;
            history::finish_application(
                storage,
                revision_id,
                if rollback.is_ok() {
                    history::RevisionStatus::Failed
                } else {
                    history::RevisionStatus::Uncertain
                },
                if rollback.is_ok() {
                    "row_verification_failed"
                } else {
                    "row_rollback_uncertain"
                },
            )
            .await?;
            if rollback.is_err() {
                return Err(AppError::history("APPLICATION_UNCERTAIN", "No se pudo verificar la reversión de la actualización. Consulta la fila antes de reintentar."));
            }
            return Err(error);
        }
    };
    let column_index = plan
        .column_names
        .iter()
        .position(|name| name == &plan.column_name)
        .ok_or_else(invalid_change)?;
    if updated
        .as_ref()
        .and_then(|row| read_text_cell(row, column_index).ok())
        != Some(plan.new_value.clone())
    {
        let rollback = transaction.rollback().await;
        history::finish_application(
            storage,
            revision_id,
            if rollback.is_ok() {
                history::RevisionStatus::Failed
            } else {
                history::RevisionStatus::Uncertain
            },
            if rollback.is_ok() {
                "row_update_unverified"
            } else {
                "row_rollback_uncertain"
            },
        )
        .await?;
        if rollback.is_err() {
            return Err(AppError::history("APPLICATION_UNCERTAIN", "No se pudo verificar la reversión de la actualización. Consulta la fila antes de reintentar."));
        }
        return Err(AppError::history(
            "ROW_UPDATE_UNVERIFIED",
            "No se pudo verificar el valor escrito; la transacción se revirtió.",
        ));
    }
    if transaction.commit().await.is_err() {
        history::finish_application(
            storage,
            revision_id,
            history::RevisionStatus::Uncertain,
            "row_commit_uncertain",
        )
        .await?;
        return Err(AppError::history(
            "APPLICATION_UNCERTAIN",
            "Se perdió la confirmación final. Consulta la fila antes de repetir el cambio.",
        ));
    }
    history::finish_application(
        storage,
        revision_id,
        history::RevisionStatus::Applied,
        "row_updated",
    )
    .await
}

async fn validate_no_row_side_effects<'a, E>(
    executor: E,
    database: &str,
    table: &str,
) -> Result<(), AppError>
where
    E: sqlx::Executor<'a, Database = sqlx::MySql>,
{
    let effects: i64 = sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA = ? AND EVENT_OBJECT_TABLE = ?) + (SELECT COUNT(*) FROM information_schema.KEY_COLUMN_USAGE WHERE REFERENCED_TABLE_SCHEMA = ? AND REFERENCED_TABLE_NAME = ?)")
        .bind(database).bind(table).bind(database).bind(table).fetch_one(executor).await.map_err(|_| target_unavailable())?;
    if effects != 0 {
        return Err(AppError::history("ROW_EFFECTS_UNSUPPORTED", "La tabla tiene triggers o claves foráneas entrantes; no se puede garantizar una compensación localizada."));
    }
    Ok(())
}

async fn apply_delete(
    pool: &MySqlPool,
    storage: &sqlx::SqlitePool,
    revision_id: &str,
    plan: &MysqlRowUpdatePlan,
) -> Result<(), AppError> {
    let mut tx = pool.begin().await.map_err(|_| target_unavailable())?;
    validate_live_table(&mut *tx, plan).await?;
    validate_no_row_side_effects(&mut *tx, &plan.database_name, &plan.table_name).await?;
    let row = fetch_row(
        &mut *tx,
        &plan.database_name,
        &plan.table_name,
        &plan.column_names,
        &plan.primary_key,
        true,
    )
    .await?
    .ok_or_else(|| AppError::history("ROW_CONFLICT", "La fila insertada ya no existe."))?;
    if hash_row(&row)? != plan.row_sha256 {
        return Err(AppError::history(
            "ROW_CONFLICT",
            "La fila cambió desde que preparaste la compensación.",
        ));
    }
    history::begin_application(storage, revision_id).await?;
    let predicate = plan
        .primary_key
        .iter()
        .map(|key| format!("{} = ?", quote_identifier(&key.column)))
        .collect::<Vec<_>>()
        .join(" AND ");
    let statement = format!(
        "DELETE FROM {}.{} WHERE {predicate}",
        quote_identifier(&plan.database_name),
        quote_identifier(&plan.table_name)
    );
    let mut query = sqlx::query(&statement);
    for key in &plan.primary_key {
        query = query.bind(key.value.as_deref());
    }
    let deleted = query
        .execute(&mut *tx)
        .await
        .map(|result| result.rows_affected())
        .unwrap_or(0);
    if deleted != 1 {
        let rollback = tx.rollback().await;
        history::finish_application(
            storage,
            revision_id,
            if rollback.is_ok() {
                history::RevisionStatus::Failed
            } else {
                history::RevisionStatus::Uncertain
            },
            if rollback.is_ok() {
                "row_delete_conflict"
            } else {
                "row_delete_uncertain"
            },
        )
        .await?;
        return Err(AppError::history(
            if rollback.is_ok() {
                "ROW_CONFLICT"
            } else {
                "APPLICATION_UNCERTAIN"
            },
            if rollback.is_ok() {
                "La compensación no eliminó exactamente la fila prevista."
            } else {
                "No se pudo confirmar la reversión de la compensación."
            },
        ));
    }
    if tx.commit().await.is_err() {
        history::finish_application(
            storage,
            revision_id,
            history::RevisionStatus::Uncertain,
            "row_delete_commit_uncertain",
        )
        .await?;
        return Err(AppError::history(
            "APPLICATION_UNCERTAIN",
            "Se perdió la confirmación. Consulta la tabla antes de reintentar.",
        ));
    }
    history::finish_application(
        storage,
        revision_id,
        history::RevisionStatus::Applied,
        "row_insert_compensated",
    )
    .await
}

async fn apply_insert(
    pool: &MySqlPool,
    storage: &sqlx::SqlitePool,
    revision_id: &str,
    plan: &MysqlRowUpdatePlan,
) -> Result<(), AppError> {
    let (version, uuid): (String, String) =
        sqlx::query_as("SELECT CAST(VERSION() AS CHAR), CAST(@@server_uuid AS CHAR)")
            .fetch_one(pool)
            .await
            .map_err(|_| target_unavailable())?;
    if version != plan.server_version || uuid != plan.server_uuid {
        return Err(AppError::history(
            "TARGET_CHANGED",
            "El servidor cambió desde la revisión. Prepara la fila de nuevo.",
        ));
    }
    let mut tx = pool.begin().await.map_err(|_| target_unavailable())?;
    validate_live_table(&mut *tx, plan).await?;
    validate_no_row_side_effects(&mut *tx, &plan.database_name, &plan.table_name).await?;
    if fetch_row(
        &mut *tx,
        &plan.database_name,
        &plan.table_name,
        &plan.column_names,
        &plan.primary_key,
        true,
    )
    .await?
    .is_some()
    {
        return Err(AppError::history(
            "ROW_CONFLICT",
            "Ya existe una fila con esa clave primaria. Revisa la inserción.",
        ));
    }
    history::begin_application(storage, revision_id).await?;
    let columns = plan
        .values
        .iter()
        .map(|v| quote_identifier(&v.column))
        .collect::<Vec<_>>()
        .join(", ");
    let placeholders = vec!["?"; plan.values.len()].join(", ");
    let statement = format!(
        "INSERT INTO {}.{} ({columns}) VALUES ({placeholders})",
        quote_identifier(&plan.database_name),
        quote_identifier(&plan.table_name)
    );
    let mut query = sqlx::query(&statement);
    for value in &plan.values {
        query = query.bind(value.value.as_deref());
    }
    let inserted = match query.execute(&mut *tx).await {
        Ok(result) if result.rows_affected() == 1 => true,
        _ => false,
    };
    if !inserted {
        let rollback = tx.rollback().await;
        history::finish_application(
            storage,
            revision_id,
            if rollback.is_ok() {
                history::RevisionStatus::Failed
            } else {
                history::RevisionStatus::Uncertain
            },
            if rollback.is_ok() {
                "row_insert_rejected"
            } else {
                "row_insert_uncertain"
            },
        )
        .await?;
        return Err(AppError::history(
            if rollback.is_ok() {
                "ROW_INSERT_FAILED"
            } else {
                "APPLICATION_UNCERTAIN"
            },
            if rollback.is_ok() {
                "MySQL rechazó la inserción; la transacción se revirtió."
            } else {
                "No se pudo confirmar la reversión de la inserción. Consulta la tabla antes de reintentar."
            },
        ));
    }
    let text_row = fetch_row_text(
        &mut *tx,
        &plan.database_name,
        &plan.table_name,
        &plan.column_names,
        &plan.primary_key,
    )
    .await?;
    let matches = text_row.as_ref().is_some_and(|row| {
        plan.values.iter().all(|value| {
            plan.column_names
                .iter()
                .position(|name| name == &value.column)
                .and_then(|index| read_text_cell(row, index).ok())
                == Some(value.value.clone())
        })
    });
    if !matches {
        let rollback = tx.rollback().await;
        history::finish_application(
            storage,
            revision_id,
            if rollback.is_ok() {
                history::RevisionStatus::Failed
            } else {
                history::RevisionStatus::Uncertain
            },
            if rollback.is_ok() {
                "row_insert_unverified"
            } else {
                "row_insert_uncertain"
            },
        )
        .await?;
        return Err(AppError::history(
            if rollback.is_ok() {
                "ROW_INSERT_UNVERIFIED"
            } else {
                "APPLICATION_UNCERTAIN"
            },
            if rollback.is_ok() {
                "No se pudo verificar la fila insertada; la transacción se revirtió."
            } else {
                "No se pudo verificar el estado de la inserción. Consulta la tabla antes de reintentar."
            },
        ));
    }
    if tx.commit().await.is_err() {
        history::finish_application(
            storage,
            revision_id,
            history::RevisionStatus::Uncertain,
            "row_insert_commit_uncertain",
        )
        .await?;
        return Err(AppError::history(
            "APPLICATION_UNCERTAIN",
            "Se perdió la confirmación final. Consulta la fila antes de repetir el cambio.",
        ));
    }
    history::finish_application(
        storage,
        revision_id,
        history::RevisionStatus::Applied,
        "row_inserted",
    )
    .await
}

async fn validate_live_table(
    executor: &mut sqlx::MySqlConnection,
    plan: &MysqlRowUpdatePlan,
) -> Result<(), AppError> {
    let table: Option<(String, String)> = sqlx::query_as(
        "SELECT CAST(TABLE_TYPE AS CHAR), CAST(COALESCE(ENGINE, '') AS CHAR) FROM information_schema.TABLES
         WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?",
    )
    .bind(&plan.database_name)
    .bind(&plan.table_name)
    .fetch_optional(&mut *executor)
    .await
    .map_err(|_| target_unavailable())?;
    if table.as_ref().map(|(kind, _)| kind.as_str()) != Some("BASE TABLE")
        || table.as_ref().map(|(_, engine)| engine.as_str()) != Some("InnoDB")
    {
        return Err(unsupported_table());
    }
    let columns: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT CAST(COLUMN_NAME AS CHAR), CAST(LOWER(DATA_TYPE) AS CHAR), CAST(EXTRA AS CHAR) FROM information_schema.COLUMNS
         WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
    )
    .bind(&plan.database_name)
    .bind(&plan.table_name)
    .fetch_all(&mut *executor)
    .await
    .map_err(|_| target_unavailable())?;
    if columns
        .iter()
        .map(|(name, _, _)| name)
        .cloned()
        .collect::<Vec<_>>()
        != plan.column_names
        || columns.iter().any(|(_, kind, extra)| {
            !supported_scalar_type(kind)
                || extra.to_ascii_lowercase().contains("generated")
                || extra.to_ascii_lowercase().contains("on update")
        })
    {
        return Err(AppError::history(
            "ROW_SCHEMA_CHANGED",
            "La estructura de la tabla cambió desde que preparaste la revisión.",
        ));
    }
    let primary_key: Vec<String> = sqlx::query_scalar(
        "SELECT CAST(COLUMN_NAME AS CHAR) FROM information_schema.KEY_COLUMN_USAGE
         WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND CONSTRAINT_NAME = 'PRIMARY'
         ORDER BY ORDINAL_POSITION",
    )
    .bind(&plan.database_name)
    .bind(&plan.table_name)
    .fetch_all(&mut *executor)
    .await
    .map_err(|_| target_unavailable())?;
    if primary_key
        != plan
            .primary_key
            .iter()
            .map(|key| key.column.clone())
            .collect::<Vec<_>>()
    {
        return Err(AppError::history(
            "ROW_SCHEMA_CHANGED",
            "La clave primaria de la tabla cambió desde que preparaste la revisión.",
        ));
    }
    let trigger_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.TRIGGERS
         WHERE TRIGGER_SCHEMA = ? AND EVENT_OBJECT_TABLE = ?",
    )
    .bind(&plan.database_name)
    .bind(&plan.table_name)
    .fetch_one(&mut *executor)
    .await
    .map_err(|_| target_unavailable())?;
    if trigger_count != 0 {
        return Err(AppError::history(
            "ROW_EFFECTS_UNSUPPORTED",
            "La tabla tiene triggers; esta revisión ya no puede aplicarse con seguridad.",
        ));
    }
    Ok(())
}

async fn update_row<'a, E>(executor: E, plan: &MysqlRowUpdatePlan) -> Result<u64, sqlx::Error>
where
    E: sqlx::Executor<'a, Database = sqlx::MySql>,
{
    let assignments = format!("{} = ?", quote_identifier(&plan.column_name));
    let predicates = plan
        .primary_key
        .iter()
        .map(|key| format!("{} = ?", quote_identifier(&key.column)))
        .collect::<Vec<_>>()
        .join(" AND ");
    let query = format!(
        "UPDATE {}.{} SET {assignments} WHERE {predicates}",
        quote_identifier(&plan.database_name),
        quote_identifier(&plan.table_name)
    );
    let mut query = sqlx::query(&query).bind(plan.new_value.as_deref());
    for key in &plan.primary_key {
        query = query.bind(key.value.as_deref());
    }
    Ok(query.execute(executor).await?.rows_affected())
}

async fn fetch_row<'a, E>(
    executor: E,
    database: &str,
    table: &str,
    columns: &[String],
    key: &[PrimaryKeyValue],
    for_update: bool,
) -> Result<Option<MySqlRow>, AppError>
where
    E: sqlx::Executor<'a, Database = sqlx::MySql>,
{
    let projection = columns
        .iter()
        .map(|name| quote_identifier(name))
        .collect::<Vec<_>>()
        .join(", ");
    let predicates = key
        .iter()
        .map(|item| format!("{} = ?", quote_identifier(&item.column)))
        .collect::<Vec<_>>()
        .join(" AND ");
    let suffix = if for_update { " FOR UPDATE" } else { "" };
    let statement = format!(
        "SELECT {projection} FROM {}.{} WHERE {predicates}{suffix}",
        quote_identifier(database),
        quote_identifier(table)
    );
    let mut query = sqlx::query(&statement);
    for item in key {
        query = query.bind(item.value.as_deref());
    }
    query
        .fetch_optional(executor)
        .await
        .map_err(|_| target_unavailable())
}

async fn fetch_row_text<'a, E>(
    executor: E,
    database: &str,
    table: &str,
    columns: &[String],
    key: &[PrimaryKeyValue],
) -> Result<Option<MySqlRow>, AppError>
where
    E: sqlx::Executor<'a, Database = sqlx::MySql>,
{
    let projection = columns
        .iter()
        .map(|name| format!("CAST({} AS CHAR)", quote_identifier(name)))
        .collect::<Vec<_>>()
        .join(", ");
    let predicates = key
        .iter()
        .map(|item| format!("{} = ?", quote_identifier(&item.column)))
        .collect::<Vec<_>>()
        .join(" AND ");
    let statement = format!(
        "SELECT {projection} FROM {}.{} WHERE {predicates}",
        quote_identifier(database),
        quote_identifier(table)
    );
    let mut query = sqlx::query(&statement);
    for item in key {
        query = query.bind(item.value.as_deref());
    }
    query
        .fetch_optional(executor)
        .await
        .map_err(|_| target_unavailable())
}

fn hash_row(row: &MySqlRow) -> Result<String, AppError> {
    let mut hasher = Sha256::new();
    hasher.update(b"DBSUAL mysql row precondition v1\0");
    for index in 0..row.len() {
        let raw = row.try_get_raw(index).map_err(|_| target_unavailable())?;
        if raw.is_null() {
            hasher.update([0]);
        } else {
            let bytes = raw.as_bytes().map_err(|_| unsupported_table())?;
            hasher.update([1]);
            hasher.update((bytes.len() as u64).to_be_bytes());
            hasher.update(bytes);
        }
    }
    Ok(hex(&hasher.finalize()))
}

fn read_text_cell(row: &MySqlRow, index: usize) -> Result<Option<String>, AppError> {
    let raw = row.try_get_raw(index).map_err(|_| target_unavailable())?;
    if raw.is_null() {
        return Ok(None);
    }
    let bytes = raw.as_bytes().map_err(|_| unsupported_table())?;
    String::from_utf8(bytes.to_vec())
        .map(Some)
        .map_err(|_| unsupported_table())
}

fn validate_target(
    connection_id: &str,
    database: &str,
    table: &str,
    key: &[PrimaryKeyValue],
    column: &str,
) -> Result<(), AppError> {
    if connection_id.is_empty()
        || connection_id.len() > 128
        || key.is_empty()
        || key.len() > 16
        || key.iter().any(|item| {
            item.value
                .as_ref()
                .is_some_and(|value| value.len() > MAX_VALUE_BYTES)
        })
        || !valid_identifier(database)
        || !valid_identifier(table)
        || !valid_identifier(column)
    {
        return Err(invalid_change());
    }
    let mut seen = std::collections::HashSet::new();
    if key
        .iter()
        .any(|item| !valid_identifier(&item.column) || !seen.insert(item.column.as_str()))
    {
        return Err(invalid_change());
    }
    Ok(())
}

pub(crate) fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= MAX_IDENTIFIER_CHARS
        && !value.chars().any(char::is_control)
}

pub(crate) fn supported_scalar_type(kind: &str) -> bool {
    matches!(
        kind,
        "char"
            | "varchar"
            | "tinytext"
            | "text"
            | "mediumtext"
            | "longtext"
            | "tinyint"
            | "smallint"
            | "mediumint"
            | "int"
            | "integer"
            | "bigint"
            | "decimal"
            | "numeric"
            | "float"
            | "double"
            | "real"
            | "date"
            | "time"
            | "year"
            | "timestamp"
            | "datetime"
            | "enum"
            | "set"
    )
}

pub(crate) fn quote_identifier(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn invalid_change() -> AppError {
    AppError::history(
        "INVALID_ROW_CHANGE",
        "La edición de fila no cumple las reglas admitidas.",
    )
}
fn unsupported_table() -> AppError {
    AppError::history(
        "ROW_TABLE_UNSUPPORTED",
        "La tabla o el tipo de dato no admite esta edición protegida.",
    )
}
fn target_unavailable() -> AppError {
    AppError::history(
        "HISTORY_TARGET_UNAVAILABLE",
        "No se pudo comprobar el estado actual de la tabla.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use std::panic::{resume_unwind, AssertUnwindSafe};

    struct TestDirectory(std::path::PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "dbsual-row-update-{}",
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

    async fn create_verified_revision(
        storage: &sqlx::SqlitePool,
        connection_id: &str,
        database: &str,
        message: &str,
    ) -> history::RevisionSummary {
        let project = history::create_project(
            storage,
            connection_id,
            database,
            history::DatabaseEngine::Mysql,
            Some("8.4 test"),
        )
        .await
        .expect("create history project");
        let artifact_id = uuid::Uuid::new_v4();
        let revision = history::create_draft(
            storage,
            &project.id,
            message,
            &"a".repeat(64),
            artifact_id,
            1,
            history::RecoveryState::Pending,
        )
        .await
        .expect("create pending row revision");
        let ciphertext_sha256 = "b".repeat(64);
        history::record_row_recovery_artifact(
            storage,
            &project.id,
            &revision.id,
            artifact_id,
            &ciphertext_sha256,
            128,
        )
        .await
        .expect("record captured compensation");
        history::mark_row_recovery_artifact_verified(
            storage,
            &revision.id,
            artifact_id,
            &ciphertext_sha256,
        )
        .await
        .expect("verify compensation metadata");
        history::confirm_draft(storage, &revision.id)
            .await
            .expect("confirm revision");
        revision
    }

    async fn create_artifact_revision(
        storage: &sqlx::SqlitePool,
        artifact_directory: &std::path::Path,
        vault: &crate::vault::NewVault,
        connection_id: &str,
        database: &str,
        message: &str,
        plan: &MysqlRowUpdatePlan,
    ) -> history::RevisionSummary {
        let project = history::create_project(
            storage,
            connection_id,
            database,
            history::DatabaseEngine::Mysql,
            Some(&plan.server_version),
        )
        .await
        .expect("create history project");
        let plaintext = serde_json::to_vec(plan).expect("serialize prepared plan");
        let plan_hash = hex(&Sha256::digest(&plaintext));
        let artifact = crate::vault::encrypt_artifact_bytes_to_file(
            artifact_directory,
            &plaintext,
            vault.vault_id,
            &vault.master_key,
            &vault.recovery_envelope,
        )
        .expect("encrypt plan as a real artifact");
        let artifact_path =
            artifact_directory.join(format!("{}.dbsual-artifact", artifact.artifact_id));
        let ciphertext = std::fs::read(&artifact_path).expect("read ciphertext for metadata");
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
        .await
        .expect("create row revision");
        history::record_row_recovery_artifact(
            storage,
            &project.id,
            &revision.id,
            artifact.artifact_id,
            &ciphertext_hash,
            ciphertext.len() as u64,
        )
        .await
        .expect("record captured compensation");
        history::mark_row_recovery_artifact_verified(
            storage,
            &revision.id,
            artifact.artifact_id,
            &ciphertext_hash,
        )
        .await
        .expect("verify compensation metadata");
        history::confirm_draft(storage, &revision.id)
            .await
            .expect("confirm revision");
        revision
    }

    fn insert_fixture_plan(
        template: &MysqlRowUpdatePlan,
        table: &str,
        id: &str,
        value: &str,
    ) -> MysqlRowUpdatePlan {
        let mut plan = template.clone();
        plan.table_name = table.to_owned();
        plan.primary_key = vec![PrimaryKeyValue {
            column: "id".into(),
            value: Some(id.to_owned()),
        }];
        plan.column_name = "id".into();
        plan.old_value = None;
        plan.new_value = None;
        plan.compensates_revision_id = None;
        plan.operation = MysqlRowOperation::Insert;
        plan.values = vec![
            RowColumnValue {
                column: "id".into(),
                value: Some(id.to_owned()),
            },
            RowColumnValue {
                column: "value".into(),
                value: Some(value.to_owned()),
            },
        ];
        plan.row_sha256 = hex(&Sha256::digest(b"DBSUAL mysql row absent v1\0"));
        plan.column_names = vec!["id".into(), "value".into()];
        plan
    }

    async fn create_authenticated_insert_revision(
        storage: &sqlx::SqlitePool,
        artifact_directory: &std::path::Path,
        vault: &crate::vault::NewVault,
        connection_id: &str,
        database: &str,
        plan: &MysqlRowUpdatePlan,
    ) -> (history::RevisionSummary, MysqlRowUpdatePlan) {
        let revision = create_artifact_revision(
            storage,
            artifact_directory,
            vault,
            connection_id,
            database,
            "Insertar fila",
            plan,
        )
        .await;
        let plaintext = crate::read_history_plan_from(
            storage,
            artifact_directory,
            revision.id.clone(),
            vault.vault_id,
        )
        .await
        .expect("authenticate inserted-row history artifact");
        let plan = serde_json::from_str(&plaintext).expect("decode inserted-row artifact");
        (revision, plan)
    }

    fn example_plan() -> MysqlRowUpdatePlan {
        MysqlRowUpdatePlan {
            version: 1,
            connection_id: "connection".into(),
            database_name: "app".into(),
            table_name: "users".into(),
            server_uuid: "server".into(),
            server_version: "8.4.11".into(),
            primary_key: vec![PrimaryKeyValue {
                column: "id".into(),
                value: Some("7".into()),
            }],
            column_name: "name".into(),
            old_value: Some("old".into()),
            new_value: Some("new".into()),
            compensates_revision_id: Some("source-revision".into()),
            operation: MysqlRowOperation::Update,
            values: Vec::new(),
            row_sha256: "a".repeat(64),
            column_names: vec!["id".into(), "name".into()],
        }
    }

    #[test]
    fn compensating_plan_keeps_source_revision_inside_the_encrypted_payload() {
        let serialized = serde_json::to_value(example_plan()).unwrap();
        assert_eq!(serialized["compensatesRevisionId"], "source-revision");

        let mut legacy = serialized;
        legacy
            .as_object_mut()
            .unwrap()
            .remove("compensatesRevisionId");
        let legacy: MysqlRowUpdatePlan = serde_json::from_value(legacy).unwrap();
        assert!(legacy.compensates_revision_id.is_none());
    }

    #[tokio::test]
    #[ignore = "requiere DBSUAL_MYSQL_TEST_PASSWORD y MySQL 8.0/8.4 desechable"]
    async fn mysql_row_update_applies_verified_revision_and_rejects_external_conflict() {
        let pool = crate::connections::connect_mysql_test_from_env()
            .await
            .expect("connect to disposable MySQL server");
        let database = format!("dbsual_row_{}", uuid::Uuid::new_v4().simple());
        let connection_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(&format!("CREATE DATABASE {}", quote_identifier(&database)))
            .execute(&pool)
            .await
            .expect("create disposable database");
        let directory = TestDirectory::new();
        let storage = crate::storage::open_at(directory.0.join("history.sqlite"))
            .await
            .expect("open isolated history");
        let artifact_directory = directory.0.join("artifacts");
        let vault = crate::vault::create_vault().expect("create isolated test vault");
        crate::vault::store_master_key(vault.vault_id, &vault.master_key)
            .expect("store test key using the Windows keyring");
        let _vault_key_cleanup = TestVaultKey(vault.vault_id);
        crate::storage::save(&storage, crate::VAULT_ID_KEY, &vault.vault_id)
            .await
            .expect("persist test vault ID");
        crate::storage::save(
            &storage,
            crate::VAULT_ENVELOPE_KEY,
            &vault.recovery_envelope,
        )
        .await
        .expect("persist test recovery envelope");

        let result = AssertUnwindSafe(async {
            sqlx::query(&format!(
                "CREATE TABLE {}.items (id INT PRIMARY KEY, value VARCHAR(100) NULL) ENGINE=InnoDB",
                quote_identifier(&database)
            ))
            .execute(&pool)
            .await
            .expect("create InnoDB fixture");
            sqlx::query(&format!(
                "INSERT INTO {}.items VALUES (1, 'before'), (2, 'before-conflict')",
                quote_identifier(&database)
            ))
            .execute(&pool)
            .await
            .expect("insert fixture rows");

            let (plan, preview) = prepare(
                &pool,
                &connection_id,
                &database,
                "items",
                vec![PrimaryKeyValue {
                    column: "id".into(),
                    value: Some("1".into()),
                }],
                "value",
                Some("after".into()),
            )
            .await
            .expect("prepare without mutation");
            assert_eq!(preview.old_value.as_deref(), Some("before"));
            let revision = create_artifact_revision(
                &storage,
                &artifact_directory,
                &vault,
                &connection_id,
                &database,
                "Actualizar fila",
                &plan,
            )
            .await;
            let authenticated = crate::read_history_plan_from(
                &storage,
                &artifact_directory,
                revision.id.clone(),
                vault.vault_id,
            )
            .await
            .expect("authenticate the real encrypted artifact through the command's core reader");
            let authenticated: MysqlRowUpdatePlan =
                serde_json::from_str(&authenticated).expect("decode authenticated row plan");
            apply(
                &pool,
                &storage,
                &revision.id,
                &connection_id,
                &authenticated,
            )
            .await
            .expect("apply confirmed revision");
            let applied: (String,) = sqlx::query_as(&format!(
                "SELECT value FROM {}.items WHERE id=1",
                quote_identifier(&database)
            ))
            .fetch_one(&pool)
            .await
            .expect("read applied value");
            assert_eq!(applied.0, "after");

            let (mut revert_plan, revert_preview) = prepare(
                &pool,
                &connection_id,
                &database,
                "items",
                vec![PrimaryKeyValue {
                    column: "id".into(),
                    value: Some("1".into()),
                }],
                "value",
                Some("before".into()),
            )
            .await
            .expect("prepare a compensating revision from the preimage");
            assert_eq!(revert_preview.old_value.as_deref(), Some("after"));
            assert_eq!(revert_preview.new_value.as_deref(), Some("before"));
            revert_plan.compensates_revision_id = Some(revision.id.clone());
            let revert_revision = create_verified_revision(
                &storage,
                &connection_id,
                &database,
                "Revertir edición de fila",
            )
            .await;
            apply(
                &pool,
                &storage,
                &revert_revision.id,
                &connection_id,
                &revert_plan,
            )
            .await
            .expect("apply the confirmed compensating revision");
            let restored: (String,) = sqlx::query_as(&format!(
                "SELECT value FROM {}.items WHERE id=1",
                quote_identifier(&database)
            ))
            .fetch_one(&pool)
            .await
            .expect("read compensated value");
            assert_eq!(restored.0, "before");

            let (server_version, server_uuid): (String, String) =
                sqlx::query_as("SELECT CAST(VERSION() AS CHAR), CAST(@@server_uuid AS CHAR)")
                    .fetch_one(&pool)
                    .await
                    .expect("identify MySQL target");
            let insert_plan = MysqlRowUpdatePlan {
                version: 1,
                connection_id: connection_id.clone(),
                database_name: database.clone(),
                table_name: "items".into(),
                server_uuid,
                server_version,
                primary_key: vec![PrimaryKeyValue {
                    column: "id".into(),
                    value: Some("3".into()),
                }],
                column_name: "id".into(),
                old_value: None,
                new_value: None,
                compensates_revision_id: None,
                operation: MysqlRowOperation::Insert,
                values: vec![
                    RowColumnValue {
                        column: "id".into(),
                        value: Some("3".into()),
                    },
                    RowColumnValue {
                        column: "value".into(),
                        value: Some("inserted".into()),
                    },
                ],
                row_sha256: hex(&Sha256::digest(b"DBSUAL mysql row absent v1\0")),
                column_names: vec!["id".into(), "value".into()],
            };
            let insert_revision = create_artifact_revision(&storage, &artifact_directory, &vault, &connection_id, &database, "Insertar fila", &insert_plan).await;
            let insert_plaintext = crate::read_history_plan_from(&storage, &artifact_directory, insert_revision.id.clone(), vault.vault_id).await.expect("authenticate insert artifact");
            let insert_plan: MysqlRowUpdatePlan = serde_json::from_str(&insert_plaintext).expect("decode authenticated insert plan");
            let before_insert: Option<(String,)> = sqlx::query_as(&format!(
                "SELECT value FROM {}.items WHERE id=3",
                quote_identifier(&database)
            ))
            .fetch_optional(&pool)
            .await
            .expect("check pre-insert state");
            assert!(
                before_insert.is_none(),
                "preparing/confirming leaves the database unchanged"
            );
            apply(
                &pool,
                &storage,
                &insert_revision.id,
                &connection_id,
                &insert_plan,
            )
            .await
            .expect("apply confirmed insertion");
            let inserted: (String,) = sqlx::query_as(&format!(
                "SELECT value FROM {}.items WHERE id=3",
                quote_identifier(&database)
            ))
            .fetch_one(&pool)
            .await
            .expect("read inserted row");
            assert_eq!(inserted.0, "inserted");
            sqlx::query(&format!("UPDATE {}.items SET value='edited-externally' WHERE id=3", quote_identifier(&database))).execute(&pool).await.expect("change inserted row externally");
            let conflict = prepare_insert_revert(&pool, &insert_plan, &insert_revision.id).await.expect_err("external edits must block insert compensation");
            assert_eq!(conflict.code, "ROW_REVERT_CONFLICT");
            let preserved_after_conflict: (String,) = sqlx::query_as(&format!("SELECT value FROM {}.items WHERE id=3", quote_identifier(&database))).fetch_one(&pool).await.expect("confirm conflict did not delete the row");
            assert_eq!(preserved_after_conflict.0, "edited-externally");
            sqlx::query(&format!("UPDATE {}.items SET value='inserted' WHERE id=3", quote_identifier(&database))).execute(&pool).await.expect("restore row for successful compensation");
            let (delete_plan, delete_preview) =
                prepare_insert_revert(&pool, &insert_plan, &insert_revision.id)
                    .await
                    .expect("prepare compensating delete revision");
            assert_eq!(delete_preview.operation, "delete");
            let delete_revision = create_artifact_revision(&storage, &artifact_directory, &vault, &connection_id, &database, "Revertir inserción de fila", &delete_plan).await;
            let delete_plaintext = crate::read_history_plan_from(&storage, &artifact_directory, delete_revision.id.clone(), vault.vault_id).await.expect("authenticate delete artifact");
            let delete_plan: MysqlRowUpdatePlan = serde_json::from_str(&delete_plaintext).expect("decode authenticated delete plan");
            apply(
                &pool,
                &storage,
                &delete_revision.id,
                &connection_id,
                &delete_plan,
            )
            .await
            .expect("apply confirmed compensating delete");
            let after_delete: Option<(String,)> = sqlx::query_as(&format!(
                "SELECT value FROM {}.items WHERE id=3",
                quote_identifier(&database)
            ))
            .fetch_optional(&pool)
            .await
            .expect("verify compensating deletion");
            assert!(after_delete.is_none());

            // A post-insert external update must stop the compensating delete and preserve that update.
            let (external_insert_revision, external_insert_plan) = create_authenticated_insert_revision(&storage, &artifact_directory, &vault, &connection_id, &database, &insert_fixture_plan(&insert_plan, "items", "4", "original")).await;
            apply(&pool, &storage, &external_insert_revision.id, &connection_id, &external_insert_plan).await.expect("insert conflict fixture");
            sqlx::query(&format!("UPDATE {}.items SET value='external-edit' WHERE id=4", quote_identifier(&database))).execute(&pool).await.expect("edit inserted row outside DBSUAL");
            let conflict = prepare_insert_revert(&pool, &external_insert_plan, &external_insert_revision.id).await.expect_err("compensation must reject externally changed row");
            assert_eq!(conflict.code, "ROW_REVERT_CONFLICT");
            let preserved_external: (String,) = sqlx::query_as(&format!("SELECT value FROM {}.items WHERE id=4", quote_identifier(&database))).fetch_one(&pool).await.expect("read externally changed row");
            assert_eq!(preserved_external.0, "external-edit", "failed compensation must not delete or overwrite the external change");

            // Triggers make both a new insert and its later localized compensation unsafe.
            sqlx::query(&format!("CREATE TABLE {}.guard_trigger (id INT PRIMARY KEY, value VARCHAR(100) NOT NULL) ENGINE=InnoDB", quote_identifier(&database))).execute(&pool).await.expect("create trigger fixture");
            let trigger_source = insert_fixture_plan(&insert_plan, "guard_trigger", "1", "source");
            let (trigger_source_revision, trigger_source_plan) = create_authenticated_insert_revision(&storage, &artifact_directory, &vault, &connection_id, &database, &trigger_source).await;
            apply(&pool, &storage, &trigger_source_revision.id, &connection_id, &trigger_source_plan).await.expect("insert trigger compensation source before trigger exists");
            sqlx::raw_sql(&format!("CREATE TRIGGER {}.guard_trigger_bi BEFORE INSERT ON {}.guard_trigger FOR EACH ROW SET @dbsual_row_test_guard = 1", quote_identifier(&database), quote_identifier(&database))).execute(&pool).await.expect("create real trigger");
            let trigger_revert = prepare_insert_revert(&pool, &trigger_source_plan, &trigger_source_revision.id).await.expect_err("trigger must block localized compensation");
            assert_eq!(trigger_revert.code, "ROW_EFFECTS_UNSUPPORTED");
            let trigger_insert = insert_fixture_plan(&insert_plan, "guard_trigger", "2", "blocked");
            let (trigger_attempt_revision, trigger_attempt_plan) = create_authenticated_insert_revision(&storage, &artifact_directory, &vault, &connection_id, &database, &trigger_insert).await;
            let trigger_error = apply(&pool, &storage, &trigger_attempt_revision.id, &connection_id, &trigger_attempt_plan).await.expect_err("trigger must block protected insert");
            assert_eq!(trigger_error.code, "ROW_EFFECTS_UNSUPPORTED");
            let trigger_rows: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {}.guard_trigger WHERE id=2", quote_identifier(&database))).fetch_one(&pool).await.expect("check trigger rejection left no row");
            assert_eq!(trigger_rows, 0);
            let trigger_source_rows: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {}.guard_trigger WHERE id=1", quote_identifier(&database))).fetch_one(&pool).await.expect("check trigger-blocked compensation retained source row");
            assert_eq!(trigger_source_rows, 1);

            // An incoming ON DELETE CASCADE FK blocks insertion and compensation for the referenced table.
            sqlx::query(&format!("CREATE TABLE {}.guard_parent (id INT PRIMARY KEY, value VARCHAR(100) NOT NULL) ENGINE=InnoDB", quote_identifier(&database))).execute(&pool).await.expect("create FK parent fixture");
            let fk_source = insert_fixture_plan(&insert_plan, "guard_parent", "1", "parent");
            let (fk_source_revision, fk_source_plan) = create_authenticated_insert_revision(&storage, &artifact_directory, &vault, &connection_id, &database, &fk_source).await;
            apply(&pool, &storage, &fk_source_revision.id, &connection_id, &fk_source_plan).await.expect("insert FK compensation source before incoming FK exists");
            sqlx::query(&format!("CREATE TABLE {}.guard_child (id INT PRIMARY KEY, parent_id INT NOT NULL, CONSTRAINT guard_child_parent FOREIGN KEY (parent_id) REFERENCES {}.guard_parent(id) ON DELETE CASCADE) ENGINE=InnoDB", quote_identifier(&database), quote_identifier(&database))).execute(&pool).await.expect("create real incoming cascading foreign key");
            sqlx::query(&format!("INSERT INTO {}.guard_child VALUES (1, 1)", quote_identifier(&database))).execute(&pool).await.expect("create referencing row");
            let fk_revert = prepare_insert_revert(&pool, &fk_source_plan, &fk_source_revision.id).await.expect_err("incoming FK must block localized compensation");
            assert_eq!(fk_revert.code, "ROW_EFFECTS_UNSUPPORTED");
            let fk_insert = insert_fixture_plan(&insert_plan, "guard_parent", "2", "blocked");
            let (fk_attempt_revision, fk_attempt_plan) = create_authenticated_insert_revision(&storage, &artifact_directory, &vault, &connection_id, &database, &fk_insert).await;
            let fk_error = apply(&pool, &storage, &fk_attempt_revision.id, &connection_id, &fk_attempt_plan).await.expect_err("incoming FK must block protected insert");
            assert_eq!(fk_error.code, "ROW_EFFECTS_UNSUPPORTED");
            let fk_parent_rows: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {}.guard_parent WHERE id IN (1,2)", quote_identifier(&database))).fetch_one(&pool).await.expect("check FK rejection left source but no attempted row");
            assert_eq!(fk_parent_rows, 1);
            let fk_child_rows: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {}.guard_child WHERE parent_id=1", quote_identifier(&database))).fetch_one(&pool).await.expect("check referencing child remains");
            assert_eq!(fk_child_rows, 1);

            let (conflict_plan, _) = prepare(
                &pool,
                &connection_id,
                &database,
                "items",
                vec![PrimaryKeyValue {
                    column: "id".into(),
                    value: Some("2".into()),
                }],
                "value",
                Some("proposed".into()),
            )
            .await
            .expect("prepare conflict fixture");
            let conflict_revision =
                create_verified_revision(&storage, &connection_id, &database, "Actualizar fila")
                    .await;
            sqlx::query(&format!(
                "UPDATE {}.items SET value='external' WHERE id=2",
                quote_identifier(&database)
            ))
            .execute(&pool)
            .await
            .expect("simulate external writer");
            let error = apply(
                &pool,
                &storage,
                &conflict_revision.id,
                &connection_id,
                &conflict_plan,
            )
            .await
            .expect_err("external conflict must block the write");
            assert_eq!(error.code, "ROW_CONFLICT");
            let preserved: (String,) = sqlx::query_as(&format!(
                "SELECT value FROM {}.items WHERE id=2",
                quote_identifier(&database)
            ))
            .fetch_one(&pool)
            .await
            .expect("read conflicting value");
            assert_eq!(preserved.0, "external");
        })
        .catch_unwind()
        .await;

        let cleanup = sqlx::query(&format!(
            "DROP DATABASE IF EXISTS {}",
            quote_identifier(&database)
        ))
        .execute(&pool)
        .await;
        pool.close().await;
        cleanup.expect("drop disposable database");
        if let Err(payload) = result {
            resume_unwind(payload);
        }
    }
}
