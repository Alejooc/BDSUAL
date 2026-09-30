mod connections;
#[allow(dead_code)] // Validador aislado hasta conectar el flujo de importación.
mod csv_import;
pub mod history;
pub mod mysql_backup;
mod mysql_database_ops;
mod mysql_row_change;
pub mod postgres;
mod postgres_sql;
mod postgres_table;
mod sql_editor;
mod sqlite_adapter;
mod ssh;
mod storage;
pub mod vault;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::io::Read;
use tauri::Manager;
use zeroize::{Zeroize, Zeroizing};

const CONTRACT_VERSION: u8 = 1;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    code: &'static str,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    public_key: Option<String>,
}

impl AppError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            fingerprint: None,
            public_key: None,
        }
    }

    fn ssh_host_key(fingerprint: String, public_key: Option<String>, changed: bool) -> Self {
        Self {
            code: if changed {
                "SSH_HOST_KEY_CHANGED"
            } else {
                "SSH_HOST_KEY_UNKNOWN"
            },
            message: if changed {
                "La clave SSH del servidor cambió; la conexión fue bloqueada."
            } else {
                "La clave SSH del servidor requiere aprobación explícita."
            },
            fingerprint: Some(fingerprint),
            public_key,
        }
    }
    fn storage() -> Self {
        Self::new(
            "STORAGE_UNAVAILABLE",
            "No se pudo abrir el almacenamiento local de DBSUAL.",
        )
    }

    fn incompatible_storage() -> Self {
        Self::new(
            "STORAGE_VERSION_NEWER",
            "Los datos locales pertenecen a una versión más reciente de DBSUAL.",
        )
    }

    fn invalid_preferences() -> Self {
        Self::new(
            "INVALID_PREFERENCES",
            "La disposición recibida no es válida.",
        )
    }

    fn invalid_session() -> Self {
        Self::new("INVALID_SESSION", "La sesión recibida no es válida.")
    }

    fn vault(code: &'static str, message: &'static str) -> Self {
        Self::new(code, message)
    }

    fn history(code: &'static str, message: &'static str) -> Self {
        Self::new(code, message)
    }
}

struct AppState {
    storage: Result<SqlitePool, AppError>,
    active: connections::ActiveConnections,
    active_postgres:
        tokio::sync::Mutex<std::collections::HashMap<String, postgres::PostgresConnection>>,
    active_sqlite: tokio::sync::Mutex<std::collections::HashMap<String, SqlitePool>>,
    active_read_queries: std::sync::Arc<sql_editor::ActiveReadQueries>,
    tunnels: connections::ActiveTunnels,
    ssh_known_hosts_path: std::path::PathBuf,
    vault_setup_lock: tokio::sync::Mutex<()>,
    pending_vault: std::sync::Mutex<Option<vault::NewVault>>,
}

impl AppState {
    fn pool(&self) -> Result<&SqlitePool, AppError> {
        self.storage.as_ref().map_err(Clone::clone)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VaultSetupResponse {
    recovery_phrase: String,
}

impl Drop for VaultSetupResponse {
    fn drop(&mut self) {
        self.recovery_phrase.zeroize();
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VaultStatus {
    configured: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryRestoreSummary {
    database_name: String,
    tables_restored: u64,
    rows_restored: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestoreRevisionPlan {
    version: u8,
    recovery_point_id: String,
    target_database: String,
    artifact_id: String,
    ciphertext_sha256: String,
    server_version: String,
    server_identity: String,
}

impl RestoreRevisionPlan {
    fn is_valid(&self) -> bool {
        self.version == 1
            && uuid::Uuid::parse_str(&self.recovery_point_id).is_ok()
            && uuid::Uuid::parse_str(&self.artifact_id).is_ok()
            && valid_mysql_database_name(&self.target_database)
            && !self.server_version.is_empty()
            && self.server_version.len() <= 128
            && !self.server_version.chars().any(char::is_control)
            && !self.server_identity.is_empty()
            && self.server_identity.len() <= 512
            && !self.server_identity.chars().any(char::is_control)
            && self.ciphertext_sha256.len() == 64
            && self
                .ciphertext_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
    }
}

const VAULT_ENVELOPE_KEY: &str = "artifact_vault_envelope_v1";
const VAULT_ID_KEY: &str = "artifact_vault_id_v1";

async fn local_vault_id(pool: &SqlitePool) -> Result<Option<uuid::Uuid>, AppError> {
    let envelope: Option<vault::RecoveryEnvelope> = storage::load(pool, VAULT_ENVELOPE_KEY).await?;
    if let Some(envelope) = envelope {
        return Ok(Some(envelope.vault_id()));
    }
    storage::load(pool, VAULT_ID_KEY).await
}

#[tauri::command]
async fn list_history_projects(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<history::HistoryProject>, AppError> {
    history::list_projects(state.pool()?).await
}

#[tauri::command]
async fn list_history_recovery_points(
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<Vec<history::RecoveryPoint>, AppError> {
    history::list_recovery_points(state.pool()?, &project_id).await
}

#[tauri::command]
async fn capture_recovery_point(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    connection_id: String,
    database_name: String,
) -> Result<history::RecoveryPoint, AppError> {
    if uuid::Uuid::parse_str(&connection_id).is_err() || !valid_mysql_database_name(&database_name)
    {
        return Err(AppError::history(
            "INVALID_BACKUP_TARGET",
            "El destino del respaldo no es válido.",
        ));
    }
    let pool = state.pool()?;
    let saved_engine: Option<(String,)> =
        sqlx::query_as("SELECT engine FROM connections WHERE id = ?")
            .bind(&connection_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| AppError::storage())?;
    let engine = saved_engine
        .map(|(engine,)| engine)
        .filter(|engine| matches!(engine.as_str(), "mysql" | "mariadb"))
        .ok_or_else(|| {
            AppError::history(
                "BACKUP_ENGINE_UNSUPPORTED",
                "La captura de puntos de recuperación está disponible para MySQL y MariaDB.",
            )
        })?;
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
                "Abre la conexión antes de capturar el respaldo.",
            )
        })?;
    let server_version: String = sqlx::query_scalar("SELECT VERSION()")
        .fetch_one(&active)
        .await
        .map_err(|_| {
            AppError::history(
                "BACKUP_TARGET_UNAVAILABLE",
                "No se pudo verificar el servidor de destino.",
            )
        })?;
    let is_mariadb = server_version.to_ascii_lowercase().contains("mariadb");
    if (engine == "mysql" && is_mariadb) || (engine == "mariadb" && !is_mariadb) {
        return Err(AppError::history(
            "BACKUP_ENGINE_MISMATCH",
            "El motor guardado no coincide con el servidor conectado.",
        ));
    }
    if engine == "mariadb" && !mysql_backup::supports_mariadb_backup_stage(&server_version) {
        return Err(AppError::history(
            "BACKUP_VERSION_UNSUPPORTED",
            "La captura protegida requiere MariaDB 10.6, 10.11 o 11.4.",
        ));
    }
    let database_exists: Option<(String,)> = sqlx::query_as(
        "SELECT CAST(SCHEMA_NAME AS CHAR) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
    )
    .bind(&database_name)
    .fetch_optional(&active)
    .await
    .map_err(|_| {
        AppError::history(
            "BACKUP_TARGET_UNAVAILABLE",
            "No se pudo comprobar la base de destino.",
        )
    })?;
    if database_exists.is_none() {
        return Err(AppError::history(
            "BACKUP_TARGET_NOT_FOUND",
            "La base ya no aparece en el servidor conectado.",
        ));
    }
    let project = history::create_project(
        pool,
        &connection_id,
        &database_name,
        if engine == "mariadb" {
            history::DatabaseEngine::Mariadb
        } else {
            history::DatabaseEngine::Mysql
        },
        Some(&server_version),
    )
    .await?;
    let envelope: Option<vault::RecoveryEnvelope> = storage::load(pool, VAULT_ENVELOPE_KEY).await?;
    let envelope = envelope.ok_or_else(|| {
        AppError::history(
            "VAULT_NOT_CONFIGURED",
            "Configura el depósito cifrado antes de capturar un respaldo.",
        )
    })?;
    let vault_id = local_vault_id(pool).await?.ok_or_else(|| {
        AppError::history("VAULT_NOT_CONFIGURED", "No se encontró el depósito local.")
    })?;
    if envelope.vault_id() != vault_id {
        return Err(AppError::history(
            "VAULT_ID_MISMATCH",
            "La clave y la frase local no pertenecen al mismo depósito.",
        ));
    }
    let master_key = tauri::async_runtime::spawn_blocking(move || vault::load_master_key(vault_id))
        .await
        .map_err(|_| AppError::history("VAULT_KEYRING_UNAVAILABLE", "No se pudo abrir la clave."))?
        .map_err(|_| {
            AppError::history("VAULT_KEYRING_UNAVAILABLE", "No se pudo abrir la clave.")
        })?;
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|_| AppError::storage())?
        .join("artifacts");
    let provider_engine = engine.clone();
    let artifact = tauri::async_runtime::spawn_blocking(move || {
        if provider_engine == "mariadb" {
            tauri::async_runtime::block_on(mysql_backup::encrypt_mariadb_table_recovery_snapshot(
                &active,
                &database_name,
                directory,
                master_key,
                envelope,
            ))
        } else {
            tauri::async_runtime::block_on(mysql_backup::encrypt_mysql_table_recovery_snapshot(
                &active,
                &database_name,
                directory,
                master_key,
                envelope,
            ))
        }
    })
    .await
    .map_err(|_| {
        AppError::history(
            "BACKUP_CAPTURE_FAILED",
            "No se pudo iniciar el trabajo de captura.",
        )
    })?
    .map_err(|_| {
        AppError::history(
            "BACKUP_CAPTURE_FAILED",
            "No se pudo completar la captura. Revisa permisos, versión y objetos de la base.",
        )
    })?;
    match history::create_recovery_point(
        pool,
        &project.id,
        if engine == "mariadb" {
            history::DatabaseEngine::Mariadb
        } else {
            history::DatabaseEngine::Mysql
        },
        Some(&server_version),
        artifact.artifact.artifact_id,
        &artifact.ciphertext_sha256,
        artifact.encrypted_bytes,
        artifact.artifact.plaintext_bytes,
        "visible_tables_views_triggers",
        "captured",
    )
    .await
    {
        Ok(point) => Ok(point),
        Err(error) => {
            if let Ok(path) = app.path().app_data_dir() {
                let _ = std::fs::remove_file(
                    path.join("artifacts")
                        .join(format!("{}.dbsual-artifact", artifact.artifact.artifact_id)),
                );
            }
            Err(error)
        }
    }
}

#[tauri::command]
async fn prepare_recovery_restore(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    recovery_point_id: String,
    target_database: String,
) -> Result<history::RevisionSummary, AppError> {
    if uuid::Uuid::parse_str(&recovery_point_id).is_err()
        || !valid_mysql_database_name(&target_database)
    {
        return Err(AppError::history(
            "INVALID_RESTORE_TARGET",
            "El respaldo o el nombre de la base destino no es válido.",
        ));
    }
    let pool = state.pool()?;
    let point: Option<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT r.project_id, r.engine, r.artifact_id, r.ciphertext_sha256, r.verification_state
         FROM recovery_points r WHERE r.id = ?",
    )
    .bind(&recovery_point_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| AppError::storage())?;
    let (project_id, engine, artifact_id, ciphertext_sha256, verification) =
        point.ok_or_else(|| {
            AppError::history(
                "RECOVERY_POINT_NOT_FOUND",
                "No se encontró el punto de recuperación.",
            )
        })?;
    if !matches!(engine.as_str(), "mysql" | "mariadb") || verification != "captured" {
        return Err(AppError::history(
            "RECOVERY_POINT_UNSUPPORTED",
            "El punto no tiene un estado compatible con su proveedor de restauración.",
        ));
    }
    let project: Option<(String, String)> =
        sqlx::query_as("SELECT connection_id, engine FROM history_projects WHERE id = ?")
            .bind(&project_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| AppError::storage())?;
    let (connection_id, project_engine) = project.ok_or_else(|| {
        AppError::history(
            "HISTORY_PROJECT_NOT_FOUND",
            "El proyecto del historial ya no está disponible.",
        )
    })?;
    if project_engine != engine {
        return Err(AppError::history(
            "BACKUP_ENGINE_MISMATCH",
            "El motor del punto no coincide con su proyecto.",
        ));
    }
    let artifact_uuid = uuid::Uuid::parse_str(&artifact_id).map_err(|_| {
        AppError::history(
            "RECOVERY_ARTIFACT_UNAVAILABLE",
            "El artefacto del respaldo no es válido.",
        )
    })?;
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
                "Abre la conexión antes de preparar la restauración.",
            )
        })?;
    let server_version: String = sqlx::query_scalar("SELECT VERSION()")
        .fetch_one(&active)
        .await
        .map_err(|_| {
            AppError::history(
                "RESTORE_TARGET_UNAVAILABLE",
                "No se pudo verificar el servidor.",
            )
        })?;
    let server_is_mariadb = server_version.to_ascii_lowercase().contains("mariadb");
    if (engine == "mysql" && server_is_mariadb) || (engine == "mariadb" && !server_is_mariadb) {
        return Err(AppError::history(
            "BACKUP_ENGINE_MISMATCH",
            "El motor de la conexión no coincide con el punto.",
        ));
    }
    let server_identity = if engine == "mysql" {
        sqlx::query_scalar::<_, String>("SELECT @@server_uuid")
            .fetch_one(&active)
            .await
            .map_err(|_| {
                AppError::history(
                    "RESTORE_TARGET_UNAVAILABLE",
                    "No se pudo identificar el servidor MySQL.",
                )
            })?
    } else {
        let (hostname, port, server_id): (String, String, String) = sqlx::query_as(
            "SELECT CAST(@@hostname AS CHAR), CAST(@@port AS CHAR), CAST(@@server_id AS CHAR)",
        )
        .fetch_one(&active)
        .await
        .map_err(|_| {
            AppError::history(
                "RESTORE_TARGET_UNAVAILABLE",
                "No se pudo identificar el servidor MariaDB.",
            )
        })?;
        format!("{hostname}:{port}:{server_id}")
    };
    let existing: Option<(String,)> = sqlx::query_as(
        "SELECT CAST(SCHEMA_NAME AS CHAR) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
    )
    .bind(&target_database)
    .fetch_optional(&active)
    .await
    .map_err(|_| {
        AppError::history(
            "RESTORE_TARGET_UNAVAILABLE",
            "No se pudo verificar el nombre destino.",
        )
    })?;
    if existing.is_some() {
        return Err(AppError::history(
            "RESTORE_TARGET_EXISTS",
            "La base destino ya existe. Elige un nombre nuevo; DBSUAL no la sobrescribirá.",
        ));
    }

    let envelope: Option<vault::RecoveryEnvelope> = storage::load(pool, VAULT_ENVELOPE_KEY).await?;
    let envelope = envelope.ok_or_else(|| {
        AppError::history(
            "VAULT_NOT_CONFIGURED",
            "Configura el depósito cifrado antes de preparar la restauración.",
        )
    })?;
    let vault_id = local_vault_id(pool).await?.ok_or_else(|| {
        AppError::history("VAULT_NOT_CONFIGURED", "No se encontró el depósito local.")
    })?;
    if envelope.vault_id() != vault_id {
        return Err(AppError::history(
            "VAULT_ID_MISMATCH",
            "La clave y la frase local no pertenecen al mismo depósito.",
        ));
    }
    let master_key = tauri::async_runtime::spawn_blocking(move || vault::load_master_key(vault_id))
        .await
        .map_err(|_| AppError::history("VAULT_KEYRING_UNAVAILABLE", "No se pudo abrir la clave."))?
        .map_err(|_| {
            AppError::history("VAULT_KEYRING_UNAVAILABLE", "No se pudo abrir la clave.")
        })?;
    let plan = RestoreRevisionPlan {
        version: 1,
        recovery_point_id,
        target_database: target_database.clone(),
        artifact_id: artifact_uuid.to_string(),
        ciphertext_sha256: ciphertext_sha256.clone(),
        server_version,
        server_identity,
    };
    let plaintext = Zeroizing::new(serde_json::to_vec(&plan).map_err(|_| AppError::storage())?);
    let plan_hash = Sha256::digest(&*plaintext)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|_| AppError::storage())?
        .join("artifacts");
    let encrypted = tauri::async_runtime::spawn_blocking(move || {
        vault::encrypt_artifact_bytes_to_file(
            &directory,
            &plaintext,
            vault_id,
            &master_key,
            &envelope,
        )
    })
    .await
    .map_err(|_| {
        AppError::history(
            "PLAN_ENCRYPTION_FAILED",
            "No se pudo proteger la revisión de restauración.",
        )
    })?
    .map_err(|_| {
        AppError::history(
            "PLAN_ENCRYPTION_FAILED",
            "No se pudo proteger la revisión de restauración.",
        )
    })?;
    match history::create_draft(
        pool,
        &project_id,
        &format!("Restaurar punto en {target_database}"),
        &plan_hash,
        encrypted.artifact_id,
        1,
        history::RecoveryState::NotRequired,
    )
    .await
    {
        Ok(revision) => Ok(revision),
        Err(error) => {
            if let Ok(path) = app.path().app_data_dir() {
                let _ = std::fs::remove_file(
                    path.join("artifacts")
                        .join(format!("{}.dbsual-artifact", encrypted.artifact_id)),
                );
            }
            Err(error)
        }
    }
}

#[tauri::command]
async fn restore_recovery_point(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    revision_id: String,
) -> Result<RecoveryRestoreSummary, AppError> {
    if uuid::Uuid::parse_str(&revision_id).is_err() {
        return Err(AppError::history(
            "INVALID_HISTORY_REVISION",
            "La revisión del historial no es válida.",
        ));
    }
    let pool = state.pool()?;
    let revision: Option<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT project_id, message, status, recovery_state, plan_sha256
         FROM history_revisions WHERE id = ?",
    )
    .bind(&revision_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| AppError::storage())?;
    let (revision_project_id, message, status, recovery_state, expected_plan_hash) = revision
        .ok_or_else(|| {
            AppError::history(
                "HISTORY_REVISION_NOT_FOUND",
                "La revisión ya no está disponible.",
            )
        })?;
    if !message.starts_with("Restaurar punto en ")
        || status != "confirmed"
        || recovery_state != "not_required"
    {
        return Err(AppError::history(
            "REVISION_NOT_APPLICABLE",
            "La restauración requiere una revisión confirmada y compatible del historial.",
        ));
    }
    let encrypted_plan = read_history_plan(state.clone(), app.clone(), revision_id.clone()).await?;
    let plan: RestoreRevisionPlan = serde_json::from_str(&encrypted_plan).map_err(|_| {
        AppError::history(
            "HISTORY_ARTIFACT_UNAVAILABLE",
            "El plan cifrado de restauración no es válido.",
        )
    })?;
    if !plan.is_valid() || message != format!("Restaurar punto en {}", plan.target_database) {
        return Err(AppError::history(
            "HISTORY_ARTIFACT_UNAVAILABLE",
            "El plan cifrado de restauración no es válido.",
        ));
    }
    let plan_hash = Sha256::digest(encrypted_plan.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if plan_hash != expected_plan_hash {
        return Err(AppError::history(
            "HISTORY_ARTIFACT_UNAVAILABLE",
            "El plan cifrado no coincide con el hash confirmado.",
        ));
    }
    let recovery_point_id = plan.recovery_point_id.clone();
    let target_database = plan.target_database.clone();
    let point: Option<(
        String,
        String,
        String,
        String,
        Option<String>,
        String,
        String,
        i64,
        i64,
        String,
        String,
    )> = sqlx::query_as(
        "SELECT r.project_id, r.engine, p.connection_id, p.engine, r.server_version, r.artifact_id, r.ciphertext_sha256,
                    r.encrypted_bytes, r.plaintext_bytes, r.coverage, r.verification_state
             FROM recovery_points r JOIN history_projects p ON p.id = r.project_id
             WHERE r.id = ?",
    )
    .bind(&recovery_point_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| AppError::storage())?;
    let (
        point_project_id,
        point_engine,
        connection_id,
        engine,
        recovery_server_version,
        artifact_id,
        hash,
        encrypted_bytes,
        _plaintext_bytes,
        coverage,
        verification,
    ) = point.ok_or_else(|| {
        AppError::history(
            "RECOVERY_POINT_NOT_FOUND",
            "No se encontró el punto de recuperación.",
        )
    })?;
    if point_project_id != revision_project_id
        || point_engine != engine
        || artifact_id != plan.artifact_id
        || hash != plan.ciphertext_sha256
    {
        return Err(AppError::history(
            "RECOVERY_POINT_CHANGED",
            "El punto ya no coincide con la revisión confirmada.",
        ));
    }
    if !matches!(engine.as_str(), "mysql" | "mariadb")
        || !matches!(
            coverage.as_str(),
            "visible_tables_only" | "visible_tables_and_views" | "visible_tables_views_triggers"
        )
        || verification != "captured"
    {
        return Err(AppError::history(
            "RECOVERY_POINT_UNSUPPORTED",
            "Este punto no tiene una cobertura compatible con su proveedor de restauración.",
        ));
    }
    let artifact_id = uuid::Uuid::parse_str(&artifact_id).map_err(|_| {
        AppError::history(
            "RECOVERY_ARTIFACT_UNAVAILABLE",
            "El artefacto del respaldo no es válido.",
        )
    })?;
    let saved_engine: Option<(String,)> =
        sqlx::query_as("SELECT engine FROM connections WHERE id = ?")
            .bind(&connection_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| AppError::storage())?;
    if saved_engine
        .as_ref()
        .is_none_or(|(saved_engine,)| saved_engine != &engine)
    {
        return Err(AppError::history(
            "BACKUP_ENGINE_UNSUPPORTED",
            "La restauración requiere una conexión del mismo motor que creó el punto.",
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
            AppError::history("CONNECTION_CLOSED", "Abre la conexión antes de restaurar.")
        })?;
    let server_version: String = sqlx::query_scalar("SELECT VERSION()")
        .fetch_one(&active)
        .await
        .map_err(|_| {
            AppError::history(
                "RESTORE_TARGET_UNAVAILABLE",
                "No se pudo verificar el servidor.",
            )
        })?;
    let server_is_mariadb = server_version.to_ascii_lowercase().contains("mariadb");
    if (engine == "mysql" && server_is_mariadb) || (engine == "mariadb" && !server_is_mariadb) {
        return Err(AppError::history(
            "BACKUP_ENGINE_MISMATCH",
            "El motor de la conexión no coincide con el punto de recuperación.",
        ));
    }
    let server_identity = if engine == "mysql" {
        sqlx::query_scalar::<_, String>("SELECT @@server_uuid")
            .fetch_one(&active)
            .await
            .map_err(|_| {
                AppError::history(
                    "RESTORE_TARGET_UNAVAILABLE",
                    "No se pudo identificar el servidor MySQL.",
                )
            })?
    } else {
        let (hostname, port, server_id): (String, String, String) = sqlx::query_as(
            "SELECT CAST(@@hostname AS CHAR), CAST(@@port AS CHAR), CAST(@@server_id AS CHAR)",
        )
        .fetch_one(&active)
        .await
        .map_err(|_| {
            AppError::history(
                "RESTORE_TARGET_UNAVAILABLE",
                "No se pudo identificar el servidor MariaDB.",
            )
        })?;
        format!("{hostname}:{port}:{server_id}")
    };
    if plan.server_version != server_version || plan.server_identity != server_identity {
        return Err(AppError::history(
            "TARGET_CHANGED",
            "El servidor difiere del destino revisado. Prepara una restauración nueva.",
        ));
    }
    if engine == "mariadb"
        && (!mysql_backup::supports_mariadb_backup_stage(&server_version)
            || recovery_server_version
                .as_deref()
                .and_then(mysql_backup::mariadb_backup_branch)
                != mysql_backup::mariadb_backup_branch(&server_version))
    {
        return Err(AppError::history(
            "BACKUP_VERSION_UNSUPPORTED",
            "La restauración MariaDB requiere la misma rama compatible que creó el punto.",
        ));
    }
    let existing: Option<(String,)> = sqlx::query_as(
        "SELECT CAST(SCHEMA_NAME AS CHAR) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
    )
    .bind(&target_database)
    .fetch_optional(&active)
    .await
    .map_err(|_| {
        AppError::history(
            "RESTORE_TARGET_UNAVAILABLE",
            "No se pudo verificar el nombre destino.",
        )
    })?;
    if existing.is_some() {
        return Err(AppError::history(
            "RESTORE_TARGET_EXISTS",
            "La base destino ya existe. Elige un nombre nuevo; DBSUAL no la sobrescribirá.",
        ));
    }
    let vault_id = local_vault_id(pool).await?.ok_or_else(|| {
        AppError::history(
            "VAULT_KEYRING_UNAVAILABLE",
            "No se encontró la clave local del respaldo.",
        )
    })?;
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|_| AppError::storage())?
        .join("artifacts");
    let artifact_path = directory.join(format!("{artifact_id}.dbsual-artifact"));
    let expected_bytes = u64::try_from(encrypted_bytes).map_err(|_| {
        AppError::history(
            "RECOVERY_ARTIFACT_UNAVAILABLE",
            "El tamaño del respaldo no es válido.",
        )
    })?;
    let expected_hash = hash.clone();
    let expected_target = target_database.clone();
    let restore_engine = engine.clone();
    history::begin_application(pool, &revision_id).await?;
    let restore_pool = active.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        if restore_engine == "mariadb" {
            tauri::async_runtime::block_on(mysql_backup::restore_mariadb_encrypted_table_snapshot(
                &restore_pool,
                &expected_target,
                artifact_path,
                vault_id,
                artifact_id,
                &expected_hash,
                expected_bytes,
            ))
        } else {
            tauri::async_runtime::block_on(mysql_backup::restore_mysql_encrypted_table_snapshot(
                &restore_pool,
                &expected_target,
                artifact_path,
                vault_id,
                artifact_id,
                &expected_hash,
                expected_bytes,
            ))
        }
    })
    .await;
    let restored = match result {
        Ok(Ok(restored)) => restored,
        Ok(Err(_)) => {
            let remains: Result<Option<(String,)>, sqlx::Error> = sqlx::query_as(
                "SELECT CAST(SCHEMA_NAME AS CHAR) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
            )
            .bind(&target_database)
            .fetch_optional(&active)
            .await;
            let (status, code) = match remains {
                Ok(None) => (history::RevisionStatus::Failed, "restore_failed_clean"),
                _ => (
                    history::RevisionStatus::Uncertain,
                    "restore_result_uncertain",
                ),
            };
            history::finish_application(pool, &revision_id, status, code).await?;
            return Err(AppError::history(
                if status == history::RevisionStatus::Failed {
                    "RESTORE_FAILED"
                } else {
                    "APPLICATION_UNCERTAIN"
                },
                if status == history::RevisionStatus::Failed {
                    "La restauración falló y la base provisional fue retirada."
                } else {
                    "La restauración terminó sin poder confirmar el estado de la base destino. No la repitas hasta revisarla."
                },
            ));
        }
        Err(_) => {
            history::finish_application(
                pool,
                &revision_id,
                history::RevisionStatus::Uncertain,
                "restore_worker_uncertain",
            )
            .await?;
            return Err(AppError::history("APPLICATION_UNCERTAIN", "No se pudo confirmar el resultado de la restauración. Revisa el destino antes de reintentar."));
        }
    };
    let exists: Result<Option<(String,)>, sqlx::Error> = sqlx::query_as(
        "SELECT CAST(SCHEMA_NAME AS CHAR) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
    )
    .bind(&target_database)
    .fetch_optional(&active)
    .await;
    match exists {
        Ok(Some(_)) => {
            history::finish_application(
                pool,
                &revision_id,
                history::RevisionStatus::Applied,
                "restore_verified",
            )
            .await?
        }
        _ => {
            history::finish_application(
                pool,
                &revision_id,
                history::RevisionStatus::Uncertain,
                "restore_unverified",
            )
            .await?;
            return Err(AppError::history(
                "APPLICATION_UNCERTAIN",
                "La restauración terminó, pero no se pudo verificar la base destino.",
            ));
        }
    }
    Ok(RecoveryRestoreSummary {
        database_name: target_database,
        tables_restored: restored.0,
        rows_restored: restored.1,
    })
}

#[tauri::command]
async fn inspect_mysql_backup_catalog(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    database_name: String,
) -> Result<mysql_backup::MysqlBackupInspection, AppError> {
    if uuid::Uuid::parse_str(&connection_id).is_err()
        || database_name.trim().is_empty()
        || database_name.len() > 256
        || database_name.chars().any(char::is_control)
    {
        return Err(AppError::history(
            "INVALID_BACKUP_TARGET",
            "El destino del respaldo no es válido.",
        ));
    }
    let pool = state.pool()?;
    let engine: Option<(String,)> = sqlx::query_as("SELECT engine FROM connections WHERE id = ?")
        .bind(&connection_id)
        .fetch_optional(pool)
        .await
        .map_err(|_| AppError::storage())?;
    if engine.as_ref().is_none_or(|(engine,)| engine != "mysql") {
        return Err(AppError::history(
            "HISTORY_CONNECTION_NOT_FOUND",
            "No se encontró una conexión MySQL guardada.",
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
                "Abre la conexión antes de inspeccionar el catálogo.",
            )
        })?;
    let inventory = mysql_backup::inspect_mysql_database(&active, database_name.trim())
        .await
        .map_err(|_| {
            AppError::history(
                "BACKUP_CATALOG_UNAVAILABLE",
                "No se pudo consultar el catálogo de objetos de la base.",
            )
        })?;
    if inventory
        .server_version
        .to_ascii_lowercase()
        .contains("mariadb")
    {
        return Err(AppError::history(
            "BACKUP_ENGINE_MISMATCH",
            "El inventario MySQL no se aplica a un servidor MariaDB.",
        ));
    }
    let definitions = mysql_backup::read_mysql_definitions(&active, &inventory)
        .await
        .map_err(|_| {
            AppError::history(
                "BACKUP_DEFINITION_UNAVAILABLE",
                "No se pudieron leer todas las definiciones de la base.",
            )
        })?;
    Ok(mysql_backup::MysqlBackupInspection {
        inventory,
        definitions,
    })
}

#[tauri::command]
async fn list_history_revisions(
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<Vec<history::RevisionSummary>, AppError> {
    if uuid::Uuid::parse_str(&project_id).is_err() {
        return Err(AppError::history(
            "INVALID_HISTORY_PROJECT",
            "El proyecto del historial no es válido.",
        ));
    }
    history::list_revisions(state.pool()?, &project_id).await
}

#[tauri::command]
async fn confirm_history_revision(
    state: tauri::State<'_, AppState>,
    revision_id: String,
) -> Result<(), AppError> {
    if uuid::Uuid::parse_str(&revision_id).is_err() {
        return Err(AppError::history(
            "INVALID_HISTORY_REVISION",
            "La revisión del historial no es válida.",
        ));
    }
    history::confirm_draft(state.pool()?, &revision_id).await
}

#[tauri::command]
async fn read_history_plan(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    revision_id: String,
) -> Result<String, AppError> {
    if uuid::Uuid::parse_str(&revision_id).is_err() {
        return Err(AppError::history(
            "INVALID_HISTORY_REVISION",
            "La revisión del historial no es válida.",
        ));
    }
    let artifact: Option<(String, String)> =
        sqlx::query_as("SELECT artifact_id, plan_sha256 FROM history_revisions WHERE id = ?")
            .bind(&revision_id)
            .fetch_optional(state.pool()?)
            .await
            .map_err(|_| AppError::storage())?;
    let (artifact_id, expected_hash) = artifact
        .and_then(|(id, hash)| {
            uuid::Uuid::parse_str(&id)
                .ok()
                .map(|artifact_id| (artifact_id, hash))
        })
        .ok_or_else(|| {
            AppError::history(
                "HISTORY_ARTIFACT_UNAVAILABLE",
                "El plan cifrado de esta revisión no está disponible.",
            )
        })?;
    let vault_id = local_vault_id(state.pool()?).await?.ok_or_else(|| {
        AppError::history(
            "VAULT_KEYRING_UNAVAILABLE",
            "No se pudo abrir la clave del depósito.",
        )
    })?;
    let path = app
        .path()
        .app_data_dir()
        .map_err(|_| AppError::storage())?
        .join("artifacts")
        .join(format!("{artifact_id}.dbsual-artifact"));
    tauri::async_runtime::spawn_blocking(move || {
        let metadata = std::fs::metadata(&path).map_err(|_| vault::VaultError::IoFailure)?;
        if metadata.len() > 256 * 1024 {
            return Err(vault::VaultError::ArtifactTooLarge);
        }
        let file = std::fs::File::open(path).map_err(|_| vault::VaultError::IoFailure)?;
        let mut encrypted = Vec::with_capacity(metadata.len() as usize);
        file.take(256 * 1024 + 1)
            .read_to_end(&mut encrypted)
            .map_err(|_| vault::VaultError::IoFailure)?;
        if encrypted.len() > 256 * 1024 {
            return Err(vault::VaultError::ArtifactTooLarge);
        }
        let mut input = std::io::Cursor::new(encrypted);
        let mut plaintext = Zeroizing::new(Vec::new());
        let info = vault::decrypt_artifact_from_keyring(&mut input, &mut *plaintext)?;
        if info.vault_id != vault_id || info.artifact_id != artifact_id {
            return Err(vault::VaultError::InvalidArtifact);
        }
        if plaintext.len() > 64 * 1024 {
            return Err(vault::VaultError::ArtifactTooLarge);
        }
        let actual_hash = Sha256::digest(&*plaintext)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if actual_hash != expected_hash {
            return Err(vault::VaultError::InvalidArtifact);
        }
        String::from_utf8(plaintext.to_vec()).map_err(|_| vault::VaultError::InvalidArtifact)
    })
    .await
    .map_err(|_| {
        AppError::history(
            "HISTORY_ARTIFACT_UNAVAILABLE",
            "No se pudo abrir el plan cifrado.",
        )
    })?
    .map_err(|_| {
        AppError::history(
            "HISTORY_ARTIFACT_UNAVAILABLE",
            "No se pudo abrir el plan cifrado.",
        )
    })
}

#[tauri::command]
async fn create_history_project(
    state: tauri::State<'_, AppState>,
    connection_id: String,
    database_name: String,
) -> Result<history::HistoryProject, AppError> {
    let pool = state.pool()?;
    let saved_connection: Option<(String,)> =
        sqlx::query_as("SELECT id FROM connections WHERE id = ?")
            .bind(&connection_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| AppError::storage())?;
    if saved_connection.is_none() {
        return Err(AppError::history(
            "HISTORY_CONNECTION_NOT_FOUND",
            "Guarda primero la conexión de destino.",
        ));
    }
    let active_pool = state
        .active
        .0
        .lock()
        .await
        .get(&connection_id)
        .cloned()
        .ok_or_else(|| {
            AppError::history(
                "CONNECTION_CLOSED",
                "Abre la conexión antes de iniciar su historial.",
            )
        })?;
    let server_version: String = sqlx::query_scalar("SELECT VERSION()")
        .fetch_one(&active_pool)
        .await
        .map_err(|_| {
            AppError::history(
                "HISTORY_TARGET_UNAVAILABLE",
                "No se pudo verificar el servidor de destino.",
            )
        })?;
    let databases: Vec<String> = sqlx::query_scalar("SHOW DATABASES")
        .fetch_all(&active_pool)
        .await
        .map_err(|_| {
            AppError::history(
                "HISTORY_TARGET_UNAVAILABLE",
                "No se pudo verificar la base de destino.",
            )
        })?;
    if !databases.iter().any(|name| name == &database_name) {
        return Err(AppError::history(
            "HISTORY_TARGET_NOT_FOUND",
            "La base ya no aparece en el servidor conectado.",
        ));
    }
    let engine = if server_version.to_ascii_lowercase().contains("mariadb") {
        history::DatabaseEngine::Mariadb
    } else {
        history::DatabaseEngine::Mysql
    };
    history::create_project(
        pool,
        &connection_id,
        &database_name,
        engine,
        Some(&server_version),
    )
    .await
}

#[tauri::command]
async fn prepare_sql_draft(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    connection_id: String,
    database_name: String,
    sql: String,
) -> Result<history::RevisionSummary, AppError> {
    let sql = Zeroizing::new(sql);
    if !sql_editor::is_single_change_plan(&sql) {
        return Err(AppError::history(
            "INVALID_CHANGE_PLAN",
            "Escribe una instrucción SQL modificadora de hasta 64 KiB para preparar el cambio.",
        ));
    }
    let pool = state.pool()?;
    let saved_connection: Option<(String,)> =
        sqlx::query_as("SELECT id FROM connections WHERE id = ?")
            .bind(&connection_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| AppError::storage())?;
    if saved_connection.is_none() {
        return Err(AppError::history(
            "HISTORY_CONNECTION_NOT_FOUND",
            "Guarda primero la conexión de destino.",
        ));
    }
    let active_pool = state
        .active
        .0
        .lock()
        .await
        .get(&connection_id)
        .cloned()
        .ok_or_else(|| {
            AppError::history(
                "CONNECTION_CLOSED",
                "Abre la conexión antes de preparar el cambio.",
            )
        })?;
    let server_version: String = sqlx::query_scalar("SELECT VERSION()")
        .fetch_one(&active_pool)
        .await
        .map_err(|_| {
            AppError::history(
                "HISTORY_TARGET_UNAVAILABLE",
                "No se pudo verificar el servidor de destino.",
            )
        })?;
    let databases: Vec<String> = sqlx::query_scalar("SHOW DATABASES")
        .fetch_all(&active_pool)
        .await
        .map_err(|_| {
            AppError::history(
                "HISTORY_TARGET_UNAVAILABLE",
                "No se pudo verificar la base de destino.",
            )
        })?;
    if !databases.iter().any(|name| name == &database_name) {
        return Err(AppError::history(
            "HISTORY_TARGET_NOT_FOUND",
            "La base ya no aparece en el servidor conectado.",
        ));
    }
    let engine = if server_version.to_ascii_lowercase().contains("mariadb") {
        history::DatabaseEngine::Mariadb
    } else {
        history::DatabaseEngine::Mysql
    };
    let project = history::create_project(
        pool,
        &connection_id,
        &database_name,
        engine,
        Some(&server_version),
    )
    .await?;
    let envelope: Option<vault::RecoveryEnvelope> = storage::load(pool, VAULT_ENVELOPE_KEY).await?;
    let envelope = envelope.ok_or_else(|| {
        AppError::history(
            "VAULT_NOT_CONFIGURED",
            "Configura una frase de recuperación antes de preparar cambios.",
        )
    })?;
    let vault_id = local_vault_id(pool).await?.ok_or_else(|| {
        AppError::history(
            "VAULT_NOT_CONFIGURED",
            "Configura el depósito de respaldos antes de preparar cambios.",
        )
    })?;
    if envelope.vault_id() != vault_id {
        return Err(AppError::history(
            "VAULT_ID_MISMATCH",
            "La clave y la frase local no pertenecen al mismo depósito.",
        ));
    }
    let artifact_directory = app
        .path()
        .app_data_dir()
        .map_err(|_| AppError::storage())?
        .join("artifacts");
    let master_key = tauri::async_runtime::spawn_blocking(move || vault::load_master_key(vault_id))
        .await
        .map_err(|_| {
            AppError::history(
                "VAULT_KEYRING_UNAVAILABLE",
                "Windows no pudo abrir la clave del depósito.",
            )
        })?
        .map_err(|_| {
            AppError::history(
                "VAULT_KEYRING_UNAVAILABLE",
                "Windows no pudo abrir la clave del depósito.",
            )
        })?;
    let plan_hash = Sha256::digest(sql.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let sql = Zeroizing::new(sql);
    let encrypted = tauri::async_runtime::spawn_blocking(move || {
        vault::encrypt_artifact_bytes_to_file(
            &artifact_directory,
            sql.as_bytes(),
            vault_id,
            &master_key,
            &envelope,
        )
    })
    .await
    .map_err(|_| {
        AppError::history(
            "PLAN_ENCRYPTION_FAILED",
            "No se pudo guardar el plan de forma protegida.",
        )
    })?
    .map_err(|_| {
        AppError::history(
            "PLAN_ENCRYPTION_FAILED",
            "No se pudo guardar el plan de forma protegida.",
        )
    })?;
    let revision = history::create_draft(
        pool,
        &project.id,
        "SQL preparado",
        &plan_hash,
        encrypted.artifact_id,
        1,
        history::RecoveryState::Pending,
    )
    .await;
    if revision.is_err() {
        if let Ok(directory) = app.path().app_data_dir() {
            let _ = std::fs::remove_file(
                directory
                    .join("artifacts")
                    .join(format!("{}.dbsual-artifact", encrypted.artifact_id)),
            );
        }
    }
    revision
}

fn valid_mysql_database_name(name: &str) -> bool {
    mysql_database_ops::valid_database_name(name)
}

#[cfg(test)]
fn mysql_create_plan(name: &str, server_uuid: &str, server_version: &str) -> String {
    mysql_database_ops::create_plan(name, server_uuid, server_version)
}

#[tauri::command]
async fn prepare_create_database(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    connection_id: String,
    database_name: String,
) -> Result<history::RevisionSummary, AppError> {
    if uuid::Uuid::parse_str(&connection_id).is_err() || !valid_mysql_database_name(&database_name)
    {
        return Err(AppError::history(
            "INVALID_DATABASE_NAME",
            "El nombre debe tener hasta 64 caracteres ASCII: letras, números, _ o $.",
        ));
    }
    let pool = state.pool()?;
    let saved: Option<(String,)> = sqlx::query_as("SELECT engine FROM connections WHERE id = ?")
        .bind(&connection_id)
        .fetch_optional(pool)
        .await
        .map_err(|_| AppError::storage())?;
    if saved.as_ref().is_none_or(|(engine,)| engine != "mysql") {
        return Err(AppError::history(
            "HISTORY_CONNECTION_NOT_FOUND",
            "No se encontró una conexión MySQL guardada.",
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
                "Abre la conexión antes de preparar la creación.",
            )
        })?;
    let server_version: String = sqlx::query_scalar("SELECT VERSION()")
        .fetch_one(&active)
        .await
        .map_err(|_| {
            AppError::history(
                "HISTORY_TARGET_UNAVAILABLE",
                "No se pudo identificar el servidor MySQL.",
            )
        })?;
    if server_version.to_ascii_lowercase().contains("mariadb") {
        return Err(AppError::history(
            "ENGINE_NOT_SUPPORTED",
            "La creación mediante historial aún no está habilitada para MariaDB.",
        ));
    }
    let existing: Option<(String,)> = sqlx::query_as(
        "SELECT CAST(SCHEMA_NAME AS CHAR) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
    )
    .bind(&database_name)
    .fetch_optional(&active)
    .await
    .map_err(|_| {
        AppError::history(
            "HISTORY_TARGET_UNAVAILABLE",
            "No se pudo comprobar si el nombre ya existe.",
        )
    })?;
    if existing.is_some() {
        return Err(AppError::history(
            "DATABASE_ALREADY_EXISTS",
            "Ya existe una base de datos con ese nombre.",
        ));
    }
    let envelope: Option<vault::RecoveryEnvelope> = storage::load(pool, VAULT_ENVELOPE_KEY).await?;
    let envelope = envelope.ok_or_else(|| {
        AppError::history(
            "VAULT_NOT_CONFIGURED",
            "Configura el depósito local antes de preparar cambios.",
        )
    })?;
    let vault_id = local_vault_id(pool).await?.ok_or_else(|| {
        AppError::history(
            "VAULT_NOT_CONFIGURED",
            "Configura el depósito local antes de preparar cambios.",
        )
    })?;
    if envelope.vault_id() != vault_id {
        return Err(AppError::history(
            "VAULT_ID_MISMATCH",
            "La clave y la frase local no pertenecen al mismo depósito.",
        ));
    }
    let master_key = tauri::async_runtime::spawn_blocking(move || vault::load_master_key(vault_id))
        .await
        .map_err(|_| {
            AppError::history(
                "VAULT_KEYRING_UNAVAILABLE",
                "Windows no pudo abrir la clave del depósito.",
            )
        })?
        .map_err(|_| {
            AppError::history(
                "VAULT_KEYRING_UNAVAILABLE",
                "Windows no pudo abrir la clave del depósito.",
            )
        })?;
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|_| AppError::storage())?
        .join("artifacts");
    mysql_database_ops::prepare_create_database_revision(
        pool,
        &active,
        &connection_id,
        &database_name,
        directory,
        vault_id,
        master_key,
        envelope,
    )
    .await
}

#[tauri::command]
async fn apply_create_database(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    revision_id: String,
) -> Result<(), AppError> {
    if uuid::Uuid::parse_str(&revision_id).is_err() {
        return Err(AppError::history(
            "INVALID_HISTORY_REVISION",
            "La revisión no es válida.",
        ));
    }
    let pool = state.pool()?;
    let row: Option<(String, String, String, String, String, String)> = sqlx::query_as("SELECT p.connection_id, p.database_name, p.engine, r.status, r.recovery_state, r.message FROM history_revisions r JOIN history_projects p ON p.id = r.project_id WHERE r.id = ?")
        .bind(&revision_id).fetch_optional(pool).await.map_err(|_| AppError::storage())?;
    let (connection_id, database_name, engine, status, recovery, message) =
        row.ok_or_else(|| {
            AppError::history(
                "HISTORY_REVISION_NOT_FOUND",
                "La revisión ya no está disponible.",
            )
        })?;
    if message != "Crear base de datos"
        || status != "confirmed"
        || recovery != "not_required"
        || engine != "mysql"
        || !valid_mysql_database_name(&database_name)
    {
        return Err(AppError::history(
            "REVISION_NOT_APPLICABLE",
            "La revisión no está confirmada o no corresponde a una creación MySQL compatible.",
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
                "Abre la conexión de destino antes de aplicar.",
            )
        })?;
    let plan = read_history_plan(state.clone(), app, revision_id.clone()).await?;
    mysql_database_ops::apply_create_database_revision(
        pool,
        &active,
        &revision_id,
        &database_name,
        &plan,
    )
    .await
}

#[tauri::command]
async fn get_vault_status(state: tauri::State<'_, AppState>) -> Result<VaultStatus, AppError> {
    let vault_id = local_vault_id(state.pool()?).await?;
    let configured = if let Some(vault_id) = vault_id {
        tauri::async_runtime::spawn_blocking(move || vault::load_master_key(vault_id).is_ok())
            .await
            .unwrap_or(false)
    } else {
        false
    };
    Ok(VaultStatus { configured })
}

#[tauri::command]
async fn begin_vault_setup(
    state: tauri::State<'_, AppState>,
) -> Result<VaultSetupResponse, AppError> {
    let _setup = state.vault_setup_lock.lock().await;
    let pool = state.pool()?;
    if local_vault_id(pool).await?.is_some() {
        return Err(AppError::vault(
            "VAULT_ALREADY_CONFIGURED",
            "El depósito de respaldos ya está configurado.",
        ));
    }
    {
        let pending = state.pending_vault.lock().map_err(|_| {
            AppError::vault("VAULT_UNAVAILABLE", "No se pudo preparar el depósito.")
        })?;
        if let Some(vault) = pending.as_ref() {
            return Ok(VaultSetupResponse {
                recovery_phrase: vault.recovery_phrase.clone(),
            });
        }
    }
    let new_vault = tauri::async_runtime::spawn_blocking(vault::create_vault)
        .await
        .map_err(|_| AppError::vault("VAULT_UNAVAILABLE", "No se pudo preparar el depósito."))?
        .map_err(|_| AppError::vault("VAULT_UNAVAILABLE", "No se pudo preparar el depósito."))?;
    let response = VaultSetupResponse {
        recovery_phrase: new_vault.recovery_phrase.clone(),
    };
    *state
        .pending_vault
        .lock()
        .map_err(|_| AppError::vault("VAULT_UNAVAILABLE", "No se pudo preparar el depósito."))? =
        Some(new_vault);
    Ok(response)
}

#[tauri::command]
async fn confirm_vault_setup(
    state: tauri::State<'_, AppState>,
    recovery_phrase: String,
) -> Result<VaultStatus, AppError> {
    let _setup = state.vault_setup_lock.lock().await;
    let pool = state.pool()?;
    let (vault_id, envelope) = {
        let pending = state.pending_vault.lock().map_err(|_| {
            AppError::vault("VAULT_UNAVAILABLE", "No se pudo confirmar el depósito.")
        })?;
        let pending = pending.as_ref().ok_or_else(|| {
            AppError::vault(
                "VAULT_SETUP_EXPIRED",
                "Inicia de nuevo la configuración del depósito.",
            )
        })?;
        (pending.vault_id, pending.recovery_envelope.clone())
    };
    if local_vault_id(pool)
        .await?
        .is_some_and(|existing_id| existing_id != vault_id)
    {
        return Err(AppError::vault(
            "VAULT_ID_MISMATCH",
            "La clave preparada pertenece a otro depósito local.",
        ));
    }
    let phrase_for_kdf = Zeroizing::new(recovery_phrase);
    let envelope_for_kdf = envelope.clone();
    let master_key = tauri::async_runtime::spawn_blocking(move || {
        vault::recover_master_key(&envelope_for_kdf, &phrase_for_kdf)
    })
    .await
    .map_err(|_| {
        AppError::vault(
            "VAULT_SETUP_INVALID",
            "La frase no coincide. Revísala e inténtalo de nuevo.",
        )
    })?
    .map_err(|_| {
        AppError::vault(
            "VAULT_SETUP_INVALID",
            "La frase no coincide. Revísala e inténtalo de nuevo.",
        )
    })?;
    let key_for_store = master_key;
    tauri::async_runtime::spawn_blocking(move || vault::store_master_key(vault_id, &key_for_store))
        .await
        .map_err(|_| {
            AppError::vault(
                "VAULT_KEYRING_UNAVAILABLE",
                "Windows no pudo guardar la clave del depósito.",
            )
        })?
        .map_err(|_| {
            AppError::vault(
                "VAULT_KEYRING_UNAVAILABLE",
                "Windows no pudo guardar la clave del depósito.",
            )
        })?;
    if let Err(error) = storage::save(pool, VAULT_ENVELOPE_KEY, &envelope).await {
        let _ = storage::remove(pool, VAULT_ENVELOPE_KEY).await;
        let _ =
            tauri::async_runtime::spawn_blocking(move || vault::remove_master_key(vault_id)).await;
        return Err(error);
    }
    if let Err(error) = storage::save(pool, VAULT_ID_KEY, &vault_id).await {
        let _ = storage::remove(pool, VAULT_ENVELOPE_KEY).await;
        let _ =
            tauri::async_runtime::spawn_blocking(move || vault::remove_master_key(vault_id)).await;
        return Err(error);
    }
    *state
        .pending_vault
        .lock()
        .map_err(|_| AppError::vault("VAULT_UNAVAILABLE", "No se pudo finalizar el depósito."))? =
        None;
    Ok(VaultStatus { configured: true })
}

#[tauri::command]
async fn cancel_vault_setup(state: tauri::State<'_, AppState>) -> Result<(), AppError> {
    let _setup = state.vault_setup_lock.lock().await;
    *state.pending_vault.lock().map_err(|_| {
        AppError::vault("VAULT_UNAVAILABLE", "No se pudo cancelar la configuración.")
    })? = None;
    Ok(())
}

#[tauri::command]
async fn export_recovery_key_file(
    state: tauri::State<'_, AppState>,
    path: String,
    password: String,
) -> Result<(), AppError> {
    if path.is_empty() || path.len() > 32_767 {
        return Err(AppError::vault(
            "INVALID_RECOVERY_FILE_PATH",
            "La ruta seleccionada no es válida.",
        ));
    }
    let path = std::path::PathBuf::from(path);
    if !path.is_absolute() {
        return Err(AppError::vault(
            "INVALID_RECOVERY_FILE_PATH",
            "Selecciona una ruta absoluta para el archivo de recuperación.",
        ));
    }
    let vault_id = local_vault_id(state.pool()?).await?.ok_or_else(|| {
        AppError::vault(
            "VAULT_NOT_CONFIGURED",
            "Configura primero el depósito de respaldos.",
        )
    })?;
    let password = Zeroizing::new(password);
    tauri::async_runtime::spawn_blocking(move || {
        let master_key = vault::load_master_key(vault_id)?;
        vault::export_key_file_to_path(&path, vault_id, &master_key, &password)
    })
    .await
    .map_err(|_| {
        AppError::vault(
            "RECOVERY_FILE_EXPORT_FAILED",
            "No se pudo crear el archivo de recuperación.",
        )
    })?
    .map_err(|error| match error {
        vault::VaultError::WeakKeyFilePassword => AppError::vault(
            "WEAK_RECOVERY_FILE_PASSWORD",
            "Usa una contraseña de al menos 12 caracteres.",
        ),
        vault::VaultError::KeyringUnavailable => AppError::vault(
            "VAULT_KEYRING_UNAVAILABLE",
            "Windows no encontró la clave del depósito.",
        ),
        _ => AppError::vault(
            "RECOVERY_FILE_EXPORT_FAILED",
            "No se pudo crear el archivo de recuperación en la ruta seleccionada.",
        ),
    })
}

#[tauri::command]
async fn import_recovery_key_file(
    state: tauri::State<'_, AppState>,
    path: String,
    password: String,
) -> Result<VaultSetupResponse, AppError> {
    if path.is_empty() || path.len() > 32_767 {
        return Err(AppError::vault(
            "INVALID_RECOVERY_FILE_PATH",
            "La ruta seleccionada no es válida.",
        ));
    }
    let path = std::path::PathBuf::from(path);
    if !path.is_absolute() {
        return Err(AppError::vault(
            "INVALID_RECOVERY_FILE_PATH",
            "Selecciona una ruta absoluta para el archivo de recuperación.",
        ));
    }
    let _setup = state.vault_setup_lock.lock().await;
    if state
        .pending_vault
        .lock()
        .map_err(|_| AppError::vault("VAULT_UNAVAILABLE", "No se pudo abrir el depósito."))?
        .is_some()
    {
        return Err(AppError::vault(
            "VAULT_SETUP_IN_PROGRESS",
            "Confirma o cancela la configuración de frase antes de importar otra clave.",
        ));
    }
    let pool = state.pool()?;
    let existing_id = local_vault_id(pool).await?;
    let password = Zeroizing::new(password);
    let imported = tauri::async_runtime::spawn_blocking(move || {
        let mut file = std::fs::File::open(path).map_err(|_| vault::VaultError::InvalidEnvelope)?;
        let mut bytes = Vec::new();
        file.by_ref()
            .take(4097)
            .read_to_end(&mut bytes)
            .map_err(|_| vault::VaultError::InvalidEnvelope)?;
        vault::import_key_file(&bytes, &password)
    })
    .await
    .map_err(|_| {
        AppError::vault(
            "RECOVERY_FILE_IMPORT_FAILED",
            "No se pudo abrir el archivo de recuperación.",
        )
    })?
    .map_err(|_| {
        AppError::vault(
            "RECOVERY_FILE_IMPORT_FAILED",
            "El archivo o la contraseña no son válidos.",
        )
    })?;
    let (vault_id, master_key) = imported;
    if existing_id.is_some_and(|existing| existing != vault_id) {
        return Err(AppError::vault(
            "VAULT_ID_MISMATCH",
            "El archivo pertenece a otro depósito y no se reemplazó la clave local.",
        ));
    }
    if existing_id.is_some() {
        let already_available =
            tauri::async_runtime::spawn_blocking(move || vault::load_master_key(vault_id).is_ok())
                .await
                .unwrap_or(false);
        if already_available {
            return Err(AppError::vault(
                "VAULT_ALREADY_CONFIGURED",
                "La clave de este depósito ya está disponible en Windows.",
            ));
        }
    }
    let pending = tauri::async_runtime::spawn_blocking(move || {
        vault::prepare_recovered_vault(vault_id, master_key)
    })
    .await
    .map_err(|_| {
        AppError::vault(
            "RECOVERY_FILE_IMPORT_FAILED",
            "No se pudo preparar la recuperación del depósito.",
        )
    })?;
    let pending = pending.map_err(|_| {
        AppError::vault(
            "RECOVERY_FILE_IMPORT_FAILED",
            "No se pudo preparar la recuperación del depósito.",
        )
    })?;
    let response = VaultSetupResponse {
        recovery_phrase: pending.recovery_phrase.clone(),
    };
    *state
        .pending_vault
        .lock()
        .map_err(|_| AppError::vault("VAULT_UNAVAILABLE", "No se pudo abrir el depósito."))? =
        Some(pending);
    Ok(response)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiPreferences {
    version: u8,
    section: Section,
    sidebar_width: u8,
    sidebar_collapsed: bool,
    bottom_height: u8,
    bottom_collapsed: bool,
    #[serde(default)]
    theme: ThemePreference,
    #[serde(default)]
    font_scale: FontScalePreference,
    #[serde(default = "default_sql_font_size")]
    sql_font_size: u8,
}

fn default_sql_font_size() -> u8 {
    14
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum ThemePreference {
    Dark,
    Light,
    Contrast,
}
impl Default for ThemePreference {
    fn default() -> Self {
        Self::Dark
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum FontScalePreference {
    Compact,
    Default,
    Large,
}
impl Default for FontScalePreference {
    fn default() -> Self {
        Self::Default
    }
}

impl Default for UiPreferences {
    fn default() -> Self {
        Self {
            version: CONTRACT_VERSION,
            section: Section::Explorer,
            sidebar_width: 24,
            sidebar_collapsed: false,
            bottom_height: 28,
            bottom_collapsed: false,
            theme: ThemePreference::Dark,
            font_scale: FontScalePreference::Default,
            sql_font_size: 14,
        }
    }
}

impl UiPreferences {
    fn is_valid(&self) -> bool {
        self.version == CONTRACT_VERSION
            && (15..=45).contains(&self.sidebar_width)
            && (15..=60).contains(&self.bottom_height)
            && (12..=20).contains(&self.sql_font_size)
    }

    fn normalized(mut self) -> Self {
        if self.version != CONTRACT_VERSION {
            return Self::default();
        }
        self.sidebar_width = self.sidebar_width.clamp(15, 45);
        self.bottom_height = self.bottom_height.clamp(15, 60);
        self.sql_font_size = self.sql_font_size.clamp(12, 20);
        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Section {
    Explorer,
    Changes,
    History,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Session {
    version: u8,
    tabs: Vec<SessionTab>,
    active_tab: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionTab {
    id: String,
    kind: String,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            version: CONTRACT_VERSION,
            tabs: vec![SessionTab {
                id: "welcome".into(),
                kind: "welcome".into(),
            }],
            active_tab: "welcome".into(),
        }
    }
}

impl Session {
    fn is_valid(&self) -> bool {
        if self.version != CONTRACT_VERSION || self.tabs.len() > 2 {
            return false;
        }
        let welcome = SessionTab {
            id: "welcome".into(),
            kind: "welcome".into(),
        };
        let query = SessionTab {
            id: "query".into(),
            kind: "query".into(),
        };
        let known_and_unique = self.tabs.iter().all(|tab| tab == &welcome || tab == &query)
            && self.tabs.iter().enumerate().all(|(index, tab)| {
                self.tabs[..index]
                    .iter()
                    .all(|previous| previous.id != tab.id)
            });
        known_and_unique
            && if self.tabs.is_empty() {
                self.active_tab == "none"
            } else {
                self.tabs.iter().any(|tab| tab.id == self.active_tab)
            }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapResponse {
    contract_version: u8,
    storage_status: &'static str,
    preferences: UiPreferences,
    session: Session,
}

#[tauri::command]
async fn bootstrap(state: tauri::State<'_, AppState>) -> Result<BootstrapResponse, AppError> {
    let pool = state.pool()?;
    let preferences = storage::load::<UiPreferences>(pool, "ui_preferences")
        .await?
        .unwrap_or_default()
        .normalized();
    let session = storage::load::<Session>(pool, "ui_session")
        .await?
        .filter(Session::is_valid)
        .unwrap_or_default();
    Ok(BootstrapResponse {
        contract_version: CONTRACT_VERSION,
        storage_status: "ready",
        preferences,
        session,
    })
}

#[tauri::command]
async fn save_ui_preferences(
    state: tauri::State<'_, AppState>,
    preferences: UiPreferences,
) -> Result<UiPreferences, AppError> {
    if !preferences.is_valid() {
        return Err(AppError::invalid_preferences());
    }
    storage::save(state.pool()?, "ui_preferences", &preferences).await?;
    Ok(preferences)
}

#[tauri::command]
async fn save_session(
    state: tauri::State<'_, AppState>,
    session: Session,
) -> Result<Session, AppError> {
    if !session.is_valid() {
        return Err(AppError::invalid_session());
    }
    storage::save(state.pool()?, "ui_session", &session).await?;
    Ok(session)
}

#[tauri::command]
async fn reset_ui_preferences(
    state: tauri::State<'_, AppState>,
) -> Result<UiPreferences, AppError> {
    let preferences = UiPreferences::default();
    storage::save(state.pool()?, "ui_preferences", &preferences).await?;
    Ok(preferences)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let storage = tauri::async_runtime::block_on(async {
                let pool = storage::open(app.handle()).await?;
                history::reconcile_interrupted_applications(&pool).await?;
                Ok::<_, AppError>(pool)
            });
            let ssh_known_hosts_path = app.path().app_data_dir()?.join("ssh_known_hosts");
            app.manage(AppState {
                storage,
                active: connections::ActiveConnections::default(),
                active_postgres: tokio::sync::Mutex::new(std::collections::HashMap::new()),
                active_sqlite: tokio::sync::Mutex::new(std::collections::HashMap::new()),
                active_read_queries: std::sync::Arc::default(),
                tunnels: connections::ActiveTunnels::default(),
                ssh_known_hosts_path,
                vault_setup_lock: tokio::sync::Mutex::new(()),
                pending_vault: std::sync::Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            list_history_projects,
            list_history_recovery_points,
            capture_recovery_point,
            prepare_recovery_restore,
            restore_recovery_point,
            inspect_mysql_backup_catalog,
            list_history_revisions,
            confirm_history_revision,
            read_history_plan,
            create_history_project,
            prepare_sql_draft,
            prepare_create_database,
            apply_create_database,
            mysql_row_change::prepare_mysql_row_update,
            mysql_row_change::prepare_mysql_row_insert,
            mysql_row_change::prepare_mysql_row_revert,
            mysql_row_change::apply_mysql_row_update,
            get_vault_status,
            begin_vault_setup,
            confirm_vault_setup,
            cancel_vault_setup,
            export_recovery_key_file,
            import_recovery_key_file,
            save_ui_preferences,
            save_session,
            reset_ui_preferences,
            connections::list_connections,
            connections::save_connection,
            connections::approve_ssh_host_key,
            connections::test_connection,
            connections::open_connection,
            connections::disconnect_connection,
            connections::remove_connection,
            connections::list_databases,
            connections::list_server_processes,
            connections::list_server_users,
            connections::list_server_variables,
            connections::list_database_objects,
            connections::list_columns,
            connections::get_table_structure,
            sqlite_adapter::execute_sqlite_read_query,
            sql_editor::execute_read_query,
            postgres_sql::execute_postgres_read_query,
            postgres_table::read_postgres_table_page,
            sqlite_adapter::read_sqlite_table_page,
            sql_editor::read_table_page,
            sql_editor::cancel_read_query,
            sql_editor::export_table_csv
        ])
        .run(tauri::generate_context!())
        .expect("No se pudo iniciar DBSUAL");
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use std::panic::{resume_unwind, AssertUnwindSafe};

    struct TestDirectory(std::path::PathBuf);

    #[test]
    fn recovery_restore_plan_is_bound_to_a_valid_destination_and_point() {
        let plan = RestoreRevisionPlan {
            version: 1,
            recovery_point_id: "123e4567-e89b-12d3-a456-426614174000".into(),
            target_database: "catalog_restore".into(),
            artifact_id: "123e4567-e89b-12d3-a456-426614174001".into(),
            ciphertext_sha256: "a".repeat(64),
            server_version: "8.4.11".into(),
            server_identity: "server-uuid".into(),
        };
        assert!(plan.is_valid());
        let mut changed = plan;
        changed.version = 2;
        assert!(!changed.is_valid());
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct TestVaultKey(Option<uuid::Uuid>);

    impl TestVaultKey {
        fn remove(&mut self) {
            if let Some(vault_id) = self.0.take() {
                let _ = vault::remove_master_key(vault_id);
            }
        }
    }

    impl Drop for TestVaultKey {
        fn drop(&mut self) {
            self.remove();
        }
    }

    #[test]
    fn mysql_create_plan_quotes_only_restricted_identifiers_and_binds_server() {
        assert!(valid_mysql_database_name("app_2026$db"));
        assert!(!valid_mysql_database_name("`; DROP DATABASE prod; --"));
        assert!(!valid_mysql_database_name(""));
        assert!(!valid_mysql_database_name(&"a".repeat(65)));
        let plan = mysql_create_plan("app_db", "server-uuid", "8.4.11");
        assert!(plan.contains("server_uuid=server-uuid version_sha256="));
        assert!(plan.ends_with("CREATE DATABASE `app_db`;"));
    }

    #[tokio::test]
    #[ignore = "requiere DBSUAL_MYSQL_TEST_PASSWORD y un MySQL desechable con permiso CREATE DATABASE"]
    async fn mysql_create_database_revision_requires_confirmation_and_reconciles_conflicts() {
        let active = connections::connect_mysql_test_from_env()
            .await
            .expect("connect to disposable MySQL server through the application connection path");
        let normal_database = format!("dbsual_create_{}", uuid::Uuid::new_v4().simple());
        let conflict_database = format!("dbsual_conflict_{}", uuid::Uuid::new_v4().simple());
        let data_directory = std::env::temp_dir().join(format!(
            "dbsual-create-flow-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _test_directory = TestDirectory(data_directory.clone());
        let artifacts_directory = data_directory.join("artifacts");
        std::fs::create_dir_all(&artifacts_directory).expect("create isolated test directory");
        let local = storage::open_at(data_directory.join("history.sqlite"))
            .await
            .expect("open isolated history storage");
        let vault = vault::create_vault().expect("create disposable vault");
        let vault_id = vault.vault_id;
        let mut vault_key_cleanup = TestVaultKey(Some(vault_id));
        vault::store_master_key(vault_id, &vault.master_key).expect("store disposable vault key");
        storage::save(&local, VAULT_ID_KEY, &vault_id)
            .await
            .expect("persist disposable vault identity");
        storage::save(&local, VAULT_ENVELOPE_KEY, &vault.recovery_envelope)
            .await
            .expect("persist disposable vault envelope");
        let (test_master_key, test_envelope) = vault.artifact_material();
        let connection_id = uuid::Uuid::new_v4().to_string();

        let body_result = AssertUnwindSafe(async {
        let revision = mysql_database_ops::prepare_create_database_revision(
            &local,
            &active,
            &connection_id,
            &normal_database,
            artifacts_directory.clone(),
            vault_id,
            test_master_key,
            test_envelope,
        )
        .await
        .expect("prepare encrypted database creation revision");
        assert_eq!(revision.status, "draft");
        let present: Option<(String,)> = sqlx::query_as(
            "SELECT CAST(SCHEMA_NAME AS CHAR) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
        )
        .bind(&normal_database)
        .fetch_optional(&active)
        .await
        .expect("check destination after prepare");
        assert!(present.is_none(), "preparing must not create the database");
        let artifact_id = uuid::Uuid::parse_str(&revision.artifact_id).unwrap();
        let artifact_path = artifacts_directory.join(format!("{artifact_id}.dbsual-artifact"));
        let mut artifact =
            std::fs::File::open(&artifact_path).expect("open encrypted plan artifact");
        let mut plaintext = Vec::new();
        let info = vault::decrypt_artifact_from_keyring(&mut artifact, &mut plaintext)
            .expect("authenticate and decrypt prepared plan");
        assert_eq!(info.vault_id, vault_id);
        assert_eq!(info.artifact_id, artifact_id);
        let plan = String::from_utf8(plaintext).expect("plan is UTF-8");
        let (server_version, server_uuid): (String, String) =
            sqlx::query_as("SELECT VERSION(), @@server_uuid")
                .fetch_one(&active)
                .await
                .expect("read target identity");
        assert_eq!(
            plan,
            mysql_database_ops::create_plan(&normal_database, &server_uuid, &server_version)
        );

        history::confirm_draft(&local, &revision.id)
            .await
            .expect("confirm database creation");
        let present_after_confirmation: Option<(String,)> = sqlx::query_as(
            "SELECT CAST(SCHEMA_NAME AS CHAR) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
        )
        .bind(&normal_database)
        .fetch_optional(&active)
        .await
        .expect("check destination after confirmation");
        assert!(
            present_after_confirmation.is_none(),
            "confirmation must not execute SQL"
        );
        let apply_result = mysql_database_ops::apply_create_database_revision(
            &local,
            &active,
            &revision.id,
            &normal_database,
            &plan,
        )
        .await;
        if let Err(error) = apply_result {
            let observed: Result<Option<(String,)>, sqlx::Error> = sqlx::query_as(
                "SELECT CAST(SCHEMA_NAME AS CHAR) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
            )
            .bind(&normal_database)
            .fetch_optional(&active)
            .await;
            panic!("apply confirmed creation: {error:?}; observed destination: {observed:?}");
        }
        let final_status: (String,) =
            sqlx::query_as("SELECT status FROM history_revisions WHERE id = ?")
                .bind(&revision.id)
                .fetch_one(&local)
                .await
                .expect("read applied history state");
        assert_eq!(final_status.0, "applied");
        let applied_event: Option<(String, String)> = sqlx::query_as(
            "SELECT event_type, detail_code FROM history_events WHERE revision_id = ? AND to_status = 'applied'",
        )
        .bind(&revision.id)
        .fetch_optional(&local)
        .await
        .expect("read application history event");
        assert_eq!(
            applied_event,
            Some(("status_changed".into(), "database_created".into()))
        );
        let present_after_apply: Option<(String,)> = sqlx::query_as(
            "SELECT CAST(SCHEMA_NAME AS CHAR) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
        )
        .bind(&normal_database)
        .fetch_optional(&active)
        .await
        .expect("verify created database");
        assert!(present_after_apply.is_some());
        let repeated_apply = mysql_database_ops::apply_create_database_revision(
            &local,
            &active,
            &revision.id,
            &normal_database,
            &plan,
        )
        .await;
        assert!(
            repeated_apply.is_err(),
            "an applied revision cannot run twice"
        );

        let conflict_revision = mysql_database_ops::prepare_create_database_revision(
            &local,
            &active,
            &connection_id,
            &conflict_database,
            artifacts_directory.clone(),
            vault_id,
            vault::load_master_key(vault_id).expect("reload disposable key"),
            storage::load(&local, VAULT_ENVELOPE_KEY)
                .await
                .expect("load vault envelope")
                .expect("vault envelope exists"),
        )
        .await
        .expect("prepare conflict fixture revision");
        history::confirm_draft(&local, &conflict_revision.id)
            .await
            .expect("confirm conflict fixture revision");
        sqlx::query(&format!("CREATE DATABASE `{conflict_database}`"))
            .execute(&active)
            .await
            .expect("simulate a database created outside DBSUAL");
        sqlx::query(&format!(
            "CREATE TABLE `{conflict_database}`.sentinel (id INT PRIMARY KEY)"
        ))
        .execute(&active)
        .await
        .expect("add data that a conflicting application must preserve");
        sqlx::query(&format!(
            "INSERT INTO `{conflict_database}`.sentinel (id) VALUES (7)"
        ))
        .execute(&active)
        .await
        .expect("write sentinel row");
        let conflict_plan: (String,) =
            sqlx::query_as("SELECT artifact_id FROM history_revisions WHERE id = ?")
                .bind(&conflict_revision.id)
                .fetch_one(&local)
                .await
                .expect("read conflict artifact id");
        let mut conflict_artifact = std::fs::File::open(
            artifacts_directory.join(format!("{}.dbsual-artifact", conflict_plan.0)),
        )
        .expect("open conflict artifact");
        let mut conflict_plaintext = Vec::new();
        vault::decrypt_artifact_from_keyring(&mut conflict_artifact, &mut conflict_plaintext)
            .expect("authenticate conflict artifact");
        let conflict_plan = String::from_utf8(conflict_plaintext).expect("plan is UTF-8");
        let conflict = mysql_database_ops::apply_create_database_revision(
            &local,
            &active,
            &conflict_revision.id,
            &conflict_database,
            &conflict_plan,
        )
        .await
        .expect_err("external database creation must stop application");
        assert_eq!(conflict.code, "TARGET_CONFLICT");
        let conflict_status: (String,) =
            sqlx::query_as("SELECT status FROM history_revisions WHERE id = ?")
                .bind(&conflict_revision.id)
                .fetch_one(&local)
                .await
                .expect("read conflict history state");
        assert_eq!(conflict_status.0, "confirmed");
        let sentinel: Option<(String,)> = sqlx::query_as(&format!(
            "SELECT CAST(SCHEMA_NAME AS CHAR) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = '{}'",
            conflict_database
        ))
        .fetch_optional(&active)
        .await
        .expect("verify external destination remains");
        assert!(sentinel.is_some());
        let sentinel_rows: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM `{conflict_database}`.sentinel WHERE id = 7"
        ))
        .fetch_one(&active)
        .await
        .expect("verify conflicting database data remains unchanged");
        assert_eq!(sentinel_rows, 1);

        })
        .catch_unwind()
        .await;

        let mut cleanup_errors = Vec::new();
        for database in [&normal_database, &conflict_database] {
            if let Err(error) = sqlx::query(&format!("DROP DATABASE IF EXISTS `{database}`"))
                .execute(&active)
                .await
            {
                cleanup_errors.push(format!("{database}: {error}"));
            }
        }
        vault_key_cleanup.remove();
        active.close().await;
        local.close().await;
        if let Err(error) = std::fs::remove_dir_all(&data_directory) {
            if error.kind() != std::io::ErrorKind::NotFound {
                cleanup_errors.push(format!("test directory: {error}"));
            }
        }
        if !cleanup_errors.is_empty() {
            panic!("failed to clean disposable MySQL test resources: {cleanup_errors:?}");
        }
        if let Err(panic_payload) = body_result {
            resume_unwind(panic_payload);
        }
    }

    #[test]
    fn preferences_reject_out_of_range_values() {
        let mut preferences = UiPreferences::default();
        preferences.sidebar_width = 90;
        assert!(!preferences.is_valid());
        assert_eq!(preferences.normalized().sidebar_width, 45);
    }

    #[test]
    fn session_rejects_unexpected_tabs() {
        let mut session = Session::default();
        session.tabs[0].kind = "query".into();
        assert!(!session.is_valid());
    }

    #[test]
    fn session_restores_empty_query_tab_and_active_tab() {
        let session = Session {
            version: CONTRACT_VERSION,
            tabs: vec![
                SessionTab {
                    id: "welcome".into(),
                    kind: "welcome".into(),
                },
                SessionTab {
                    id: "query".into(),
                    kind: "query".into(),
                },
            ],
            active_tab: "query".into(),
        };
        assert!(session.is_valid());
    }

    #[test]
    fn session_allows_no_work_tabs_after_the_last_tab_is_closed() {
        let session = Session {
            version: CONTRACT_VERSION,
            tabs: Vec::new(),
            active_tab: "none".into(),
        };
        assert!(session.is_valid());
    }

    #[test]
    fn session_rejects_unknown_or_duplicate_tabs() {
        let mut session = Session::default();
        session.tabs.push(SessionTab {
            id: "query".into(),
            kind: "table".into(),
        });
        assert!(!session.is_valid());
        session.tabs[1].kind = "query".into();
        session.tabs[1].id = "welcome".into();
        assert!(!session.is_valid());
    }

    #[test]
    fn imported_vault_id_is_found_without_a_local_phrase_envelope() {
        tauri::async_runtime::block_on(async {
            let path = std::env::temp_dir().join(format!(
                "dbsual-imported-vault-{}.sqlite",
                uuid::Uuid::new_v4()
            ));
            let pool = storage::open_at(path.clone()).await.unwrap();
            let vault_id = uuid::Uuid::new_v4();
            assert_eq!(local_vault_id(&pool).await.unwrap(), None);
            storage::save(&pool, VAULT_ID_KEY, &vault_id).await.unwrap();
            assert_eq!(local_vault_id(&pool).await.unwrap(), Some(vault_id));
            pool.close().await;
            drop(pool);
            let mut removal = std::fs::remove_file(&path);
            for _ in 0..20 {
                match removal {
                    Ok(()) => return,
                    Err(error) if error.raw_os_error() == Some(32) => {
                        std::thread::sleep(std::time::Duration::from_millis(25));
                        removal = std::fs::remove_file(&path);
                    }
                    Err(error) => panic!("remove temporary SQLite file: {error}"),
                }
            }
            removal.expect("Windows released the closed SQLite file lock");
        });
    }
}
