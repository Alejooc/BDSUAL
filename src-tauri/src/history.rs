use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::AppError;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseEngine {
    Mysql,
    Mariadb,
    Postgresql,
    Sqlite,
}

impl DatabaseEngine {
    fn as_db(self) -> &'static str {
        match self {
            Self::Mysql => "mysql",
            Self::Mariadb => "mariadb",
            Self::Postgresql => "postgresql",
            Self::Sqlite => "sqlite",
        }
    }
}

#[derive(Clone, Debug, FromRow, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryProject {
    pub id: String,
    pub connection_id: String,
    pub database_name: String,
    pub engine: String,
    pub server_version: Option<String>,
    pub created_at_ms: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RevisionStatus {
    Draft,
    Confirmed,
    Applying,
    Applied,
    Failed,
    FailedPartial,
    Uncertain,
    Discarded,
}

impl RevisionStatus {
    fn as_db(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Confirmed => "confirmed",
            Self::Applying => "applying",
            Self::Applied => "applied",
            Self::Failed => "failed",
            Self::FailedPartial => "failed_partial",
            Self::Uncertain => "uncertain",
            Self::Discarded => "discarded",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryState {
    NotRequired,
    Pending,
    Verified,
    Failed,
    Unavailable,
}

impl RecoveryState {
    fn as_db(self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::Pending => "pending",
            Self::Verified => "verified",
            Self::Failed => "failed",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Clone, Debug, FromRow, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionSummary {
    pub id: String,
    pub project_id: String,
    pub revision_number: i64,
    pub message: String,
    pub status: String,
    pub recovery_state: String,
    pub plan_sha256: String,
    pub artifact_id: String,
    pub operation_count: i64,
    pub created_at_ms: i64,
    pub confirmed_at_ms: Option<i64>,
}

#[derive(Clone, Debug, FromRow, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryPoint {
    pub id: String,
    pub project_id: String,
    pub engine: String,
    pub server_version: Option<String>,
    pub artifact_id: String,
    pub ciphertext_sha256: String,
    pub encrypted_bytes: i64,
    pub plaintext_bytes: i64,
    pub coverage: String,
    pub verification_state: String,
    pub protected_revision_id: Option<String>,
    pub created_at_ms: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RowRecoveryArtifactKind {
    RowCompensation,
}

impl RowRecoveryArtifactKind {
    #[allow(dead_code)] // Usado por la escritura de metadata de compensación.
    fn as_db(self) -> &'static str {
        match self {
            Self::RowCompensation => "row_compensation",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RowRecoveryArtifactState {
    Captured,
    Verified,
}

impl RowRecoveryArtifactState {
    #[allow(dead_code)] // Usado por la transición de captura a verificación.
    fn as_db(self) -> &'static str {
        match self {
            Self::Captured => "captured",
            Self::Verified => "verified",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowRecoveryArtifact {
    pub id: String,
    pub project_id: String,
    pub revision_id: String,
    pub artifact_id: String,
    pub kind: RowRecoveryArtifactKind,
    pub ciphertext_sha256: String,
    pub encrypted_bytes: i64,
    pub verification_state: RowRecoveryArtifactState,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, FromRow)]
struct StoredRowRecoveryArtifact {
    id: String,
    project_id: String,
    revision_id: String,
    artifact_id: String,
    kind: String,
    ciphertext_sha256: String,
    encrypted_bytes: i64,
    verification_state: String,
    created_at_ms: i64,
}

impl TryFrom<StoredRowRecoveryArtifact> for RowRecoveryArtifact {
    type Error = AppError;

    fn try_from(stored: StoredRowRecoveryArtifact) -> Result<Self, Self::Error> {
        let kind = match stored.kind.as_str() {
            "row_compensation" => RowRecoveryArtifactKind::RowCompensation,
            _ => return Err(AppError::storage()),
        };
        let verification_state = match stored.verification_state.as_str() {
            "captured" => RowRecoveryArtifactState::Captured,
            "verified" => RowRecoveryArtifactState::Verified,
            _ => return Err(AppError::storage()),
        };
        Ok(Self {
            id: stored.id,
            project_id: stored.project_id,
            revision_id: stored.revision_id,
            artifact_id: stored.artifact_id,
            kind,
            ciphertext_sha256: stored.ciphertext_sha256,
            encrypted_bytes: stored.encrypted_bytes,
            verification_state,
            created_at_ms: stored.created_at_ms,
        })
    }
}

pub async fn create_project(
    pool: &SqlitePool,
    connection_id: &str,
    database_name: &str,
    engine: DatabaseEngine,
    server_version: Option<&str>,
) -> Result<HistoryProject, AppError> {
    if connection_id.is_empty()
        || connection_id.len() > 128
        || database_name.trim().is_empty()
        || database_name.len() > 512
        || database_name.chars().any(char::is_control)
        || server_version
            .is_some_and(|value| value.len() > 128 || value.chars().any(char::is_control))
    {
        return Err(AppError::history(
            "INVALID_HISTORY_TARGET",
            "El destino del historial no es válido.",
        ));
    }
    let id = Uuid::new_v4().to_string();
    let now = now_ms()?;
    sqlx::query(
        "INSERT INTO history_projects(id, connection_id, database_name, engine, server_version, created_at_ms)
         VALUES(?, ?, ?, ?, ?, ?)
         ON CONFLICT(connection_id, database_name) DO UPDATE SET
             engine = excluded.engine,
             server_version = COALESCE(excluded.server_version, history_projects.server_version)",
    )
    .bind(id)
    .bind(connection_id)
    .bind(database_name.trim())
    .bind(engine.as_db())
    .bind(server_version)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|_| AppError::storage())?;
    sqlx::query_as::<_, HistoryProject>(
        "SELECT id, connection_id, database_name, engine, server_version, created_at_ms
         FROM history_projects WHERE connection_id = ? AND database_name = ?",
    )
    .bind(connection_id)
    .bind(database_name.trim())
    .fetch_one(pool)
    .await
    .map_err(|_| AppError::storage())
}

pub async fn list_projects(pool: &SqlitePool) -> Result<Vec<HistoryProject>, AppError> {
    sqlx::query_as::<_, HistoryProject>(
        "SELECT id, connection_id, database_name, engine, server_version, created_at_ms
         FROM history_projects ORDER BY created_at_ms DESC, id",
    )
    .fetch_all(pool)
    .await
    .map_err(|_| AppError::storage())
}

pub async fn list_revisions(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<Vec<RevisionSummary>, AppError> {
    sqlx::query_as::<_, RevisionSummary>(
        "SELECT id, project_id, revision_number, message, status, recovery_state,
                plan_sha256, artifact_id, operation_count, created_at_ms, confirmed_at_ms
         FROM history_revisions WHERE project_id = ? ORDER BY revision_number DESC",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await
    .map_err(|_| AppError::storage())
}

#[allow(dead_code)] // Se conecta al proveedor MySQL al cerrar la captura autenticada.
pub(crate) async fn create_recovery_point(
    pool: &SqlitePool,
    project_id: &str,
    engine: DatabaseEngine,
    server_version: Option<&str>,
    artifact_id: Uuid,
    ciphertext_sha256: &str,
    encrypted_bytes: u64,
    plaintext_bytes: u64,
    coverage: &str,
    verification_state: &str,
) -> Result<RecoveryPoint, AppError> {
    if Uuid::parse_str(project_id).is_err()
        || server_version.is_some_and(|version| {
            version.is_empty() || version.len() > 128 || version.chars().any(char::is_control)
        })
        || !is_sha256(ciphertext_sha256)
        || encrypted_bytes == 0
        || encrypted_bytes > i64::MAX as u64
        || plaintext_bytes > i64::MAX as u64
        || !matches!(
            coverage,
            "visible_tables_only"
                | "visible_tables_and_views"
                | "visible_tables_views_triggers"
                | "complete"
        )
        || !matches!(
            verification_state,
            "captured" | "verified" | "failed" | "unavailable"
        )
        || (verification_state == "verified" && coverage != "complete")
    {
        return Err(AppError::history(
            "INVALID_RECOVERY_POINT",
            "El punto de recuperación no cumple los requisitos del historial.",
        ));
    }
    let id = Uuid::new_v4().to_string();
    let created_at_ms = now_ms()?;
    sqlx::query(
        "INSERT INTO recovery_points(
            id, project_id, engine, server_version, artifact_id, ciphertext_sha256,
            encrypted_bytes, plaintext_bytes, coverage, verification_state, created_at_ms
         ) VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(project_id)
    .bind(engine.as_db())
    .bind(server_version)
    .bind(artifact_id.to_string())
    .bind(ciphertext_sha256.to_ascii_lowercase())
    .bind(encrypted_bytes as i64)
    .bind(plaintext_bytes as i64)
    .bind(coverage)
    .bind(verification_state)
    .bind(created_at_ms)
    .execute(pool)
    .await
    .map_err(|_| AppError::storage())?;
    sqlx::query_as::<_, RecoveryPoint>(
        "SELECT id, project_id, engine, server_version, artifact_id, ciphertext_sha256,
                encrypted_bytes, plaintext_bytes, coverage, verification_state,
                protected_revision_id, created_at_ms
         FROM recovery_points WHERE id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .map_err(|_| AppError::storage())
}

#[allow(dead_code)] // Se expondrá junto al comando de restauración del historial.
pub(crate) async fn list_recovery_points(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<Vec<RecoveryPoint>, AppError> {
    if Uuid::parse_str(project_id).is_err() {
        return Err(AppError::history(
            "INVALID_HISTORY_PROJECT",
            "El proyecto del historial no es válido.",
        ));
    }
    sqlx::query_as::<_, RecoveryPoint>(
        "SELECT id, project_id, engine, server_version, artifact_id, ciphertext_sha256,
                encrypted_bytes, plaintext_bytes, coverage, verification_state,
                protected_revision_id, created_at_ms
         FROM recovery_points WHERE project_id = ? ORDER BY created_at_ms DESC",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await
    .map_err(|_| AppError::storage())
}

/// Registra el ciphertext de compensación recién creado. Se registra como `captured`;
/// solo `mark_row_recovery_artifact_verified` puede marcarlo verificado después de
/// autenticar el artefacto completo con la clave del depósito.
#[allow(dead_code)] // Se conecta al flujo de preparación/aplicación de una fila.
pub(crate) async fn record_row_recovery_artifact(
    pool: &SqlitePool,
    project_id: &str,
    revision_id: &str,
    artifact_id: Uuid,
    ciphertext_sha256: &str,
    encrypted_bytes: u64,
) -> Result<RowRecoveryArtifact, AppError> {
    if Uuid::parse_str(project_id).is_err()
        || Uuid::parse_str(revision_id).is_err()
        || !is_sha256(ciphertext_sha256)
        || encrypted_bytes == 0
        || encrypted_bytes > i64::MAX as u64
    {
        return Err(AppError::history(
            "INVALID_RECOVERY_ARTIFACT",
            "Los metadatos del artefacto de recuperación no son válidos.",
        ));
    }
    let mut transaction = pool.begin().await.map_err(|_| AppError::storage())?;
    let target: Option<(String, String, String)> = sqlx::query_as(
        "SELECT p.engine, r.status, r.recovery_state FROM history_projects p
         JOIN history_revisions r ON r.project_id = p.id
         WHERE p.id = ? AND r.id = ?",
    )
    .bind(project_id)
    .bind(revision_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| AppError::storage())?;
    let Some((engine, status, recovery_state)) = target else {
        return Err(AppError::history(
            "HISTORY_REVISION_NOT_FOUND",
            "La revisión no pertenece al proyecto indicado o ya no está disponible.",
        ));
    };
    if !matches!(engine.as_str(), "mysql" | "mariadb")
        || status != "draft"
        || recovery_state != RecoveryState::Pending.as_db()
    {
        return Err(AppError::history(
            "INVALID_RECOVERY_ARTIFACT_TARGET",
            "La compensación de fila solo puede registrarse en un borrador MySQL o MariaDB.",
        ));
    }
    let id = Uuid::new_v4().to_string();
    let now = now_ms()?;
    sqlx::query(
        "INSERT INTO row_recovery_artifacts(
            id, project_id, revision_id, artifact_id, kind, ciphertext_sha256,
            encrypted_bytes, verification_state, created_at_ms
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(project_id)
    .bind(revision_id)
    .bind(artifact_id.to_string())
    .bind(RowRecoveryArtifactKind::RowCompensation.as_db())
    .bind(ciphertext_sha256.to_ascii_lowercase())
    .bind(encrypted_bytes as i64)
    .bind(RowRecoveryArtifactState::Captured.as_db())
    .bind(now)
    .execute(&mut *transaction)
    .await
    .map_err(|_| {
        AppError::history(
            "RECOVERY_ARTIFACT_ALREADY_EXISTS",
            "La revisión ya tiene un artefacto de compensación registrado.",
        )
    })?;
    transaction
        .commit()
        .await
        .map_err(|_| AppError::storage())?;
    get_row_recovery_artifact(pool, revision_id)
        .await?
        .ok_or_else(|| AppError::storage())
}

/// Marca como verificado un artefacto ya registrado. Es una transición monotónica.
#[allow(dead_code)] // Se conecta al verificador autenticado del artefacto.
pub(crate) async fn mark_row_recovery_artifact_verified(
    pool: &SqlitePool,
    revision_id: &str,
    artifact_id: Uuid,
    ciphertext_sha256: &str,
) -> Result<RowRecoveryArtifact, AppError> {
    if Uuid::parse_str(revision_id).is_err() || !is_sha256(ciphertext_sha256) {
        return Err(AppError::history(
            "INVALID_RECOVERY_ARTIFACT",
            "Los metadatos del artefacto de recuperación no son válidos.",
        ));
    }
    let normalized_hash = ciphertext_sha256.to_ascii_lowercase();
    let mut transaction = pool.begin().await.map_err(|_| AppError::storage())?;
    let current: Option<(String, String, String, String)> = sqlx::query_as(
        "SELECT a.id, a.ciphertext_sha256, a.verification_state, r.recovery_state
         FROM row_recovery_artifacts a
         JOIN history_revisions r ON r.id = a.revision_id
         WHERE a.revision_id = ? AND a.artifact_id = ? AND r.status = 'draft'",
    )
    .bind(revision_id)
    .bind(artifact_id.to_string())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| AppError::storage())?;
    let Some((id, stored_hash, state, revision_recovery_state)) = current else {
        return Err(AppError::history(
            "RECOVERY_ARTIFACT_NOT_FOUND",
            "No se encontró el artefacto de recuperación del borrador.",
        ));
    };
    if stored_hash != normalized_hash {
        return Err(AppError::history(
            "RECOVERY_ARTIFACT_MISMATCH",
            "El hash del artefacto no coincide con el registrado.",
        ));
    }
    if state == RowRecoveryArtifactState::Captured.as_db() {
        if revision_recovery_state != RecoveryState::Pending.as_db() {
            return Err(AppError::history(
                "INVALID_RECOVERY_ARTIFACT_STATE",
                "La revisión ya no espera verificar su artefacto de recuperación.",
            ));
        }
        sqlx::query(
            "UPDATE row_recovery_artifacts SET verification_state = 'verified'
             WHERE id = ? AND verification_state = 'captured'",
        )
        .bind(&id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        let revision_update = sqlx::query(
            "UPDATE history_revisions SET recovery_state = 'verified'
             WHERE id = ? AND status = 'draft' AND recovery_state = 'pending'",
        )
        .bind(revision_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?;
        if revision_update.rows_affected() != 1 {
            return Err(AppError::history(
                "INVALID_RECOVERY_ARTIFACT_STATE",
                "La revisión cambió mientras se verificaba su artefacto.",
            ));
        }
    } else if state != RowRecoveryArtifactState::Verified.as_db() {
        return Err(AppError::storage());
    } else if revision_recovery_state != RecoveryState::Verified.as_db() {
        return Err(AppError::history(
            "INVALID_RECOVERY_ARTIFACT_STATE",
            "El estado de recuperación de la revisión no coincide con su artefacto.",
        ));
    }
    transaction
        .commit()
        .await
        .map_err(|_| AppError::storage())?;
    get_row_recovery_artifact(pool, revision_id)
        .await?
        .ok_or_else(|| AppError::storage())
}

#[allow(dead_code)] // Se consulta al confirmar y aplicar una revisión de fila.
pub(crate) async fn get_row_recovery_artifact(
    pool: &SqlitePool,
    revision_id: &str,
) -> Result<Option<RowRecoveryArtifact>, AppError> {
    if Uuid::parse_str(revision_id).is_err() {
        return Err(AppError::history(
            "INVALID_HISTORY_REVISION",
            "El identificador de la revisión no es válido.",
        ));
    }
    let stored = sqlx::query_as::<_, StoredRowRecoveryArtifact>(
        "SELECT id, project_id, revision_id, artifact_id, kind, ciphertext_sha256,
                encrypted_bytes, verification_state, created_at_ms
         FROM row_recovery_artifacts WHERE revision_id = ?",
    )
    .bind(revision_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| AppError::storage())?;
    stored.map(TryInto::try_into).transpose()
}

pub async fn create_draft(
    pool: &SqlitePool,
    project_id: &str,
    message: &str,
    plan_sha256: &str,
    artifact_id: Uuid,
    operation_count: u32,
    recovery_state: RecoveryState,
) -> Result<RevisionSummary, AppError> {
    if message.trim().is_empty()
        || message.trim().len() > 200
        || message.chars().any(char::is_control)
        || !is_sha256(plan_sha256)
        || operation_count == 0
        || operation_count > 100_000
    {
        return Err(AppError::history(
            "INVALID_REVISION",
            "La revisión no cumple los requisitos del historial.",
        ));
    }
    let mut transaction = pool.begin().await.map_err(|_| AppError::storage())?;
    let project_exists: Option<(String,)> =
        sqlx::query_as("SELECT id FROM history_projects WHERE id = ?")
            .bind(project_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
    if project_exists.is_none() {
        return Err(AppError::history(
            "HISTORY_PROJECT_NOT_FOUND",
            "El proyecto del historial ya no está disponible.",
        ));
    }
    let next_number: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(revision_number), 0) + 1 FROM history_revisions WHERE project_id = ?",
    )
    .bind(project_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| AppError::storage())?;
    let revision_id = Uuid::new_v4().to_string();
    let created_at_ms = now_ms()?;
    let message = message.trim();
    sqlx::query(
        "INSERT INTO history_revisions(
            id, project_id, revision_number, message, status, recovery_state,
            plan_sha256, artifact_id, operation_count, created_at_ms
         ) VALUES(?, ?, ?, ?, 'draft', ?, ?, ?, ?, ?)",
    )
    .bind(&revision_id)
    .bind(project_id)
    .bind(next_number)
    .bind(message)
    .bind(recovery_state.as_db())
    .bind(plan_sha256.to_ascii_lowercase())
    .bind(artifact_id.to_string())
    .bind(i64::from(operation_count))
    .bind(created_at_ms)
    .execute(&mut *transaction)
    .await
    .map_err(|_| AppError::storage())?;
    insert_event(
        &mut transaction,
        &revision_id,
        "revision_created",
        None,
        Some(RevisionStatus::Draft),
        "prepared",
        created_at_ms,
    )
    .await?;
    transaction
        .commit()
        .await
        .map_err(|_| AppError::storage())?;
    sqlx::query_as::<_, RevisionSummary>(
        "SELECT id, project_id, revision_number, message, status, recovery_state,
                plan_sha256, artifact_id, operation_count, created_at_ms, confirmed_at_ms
         FROM history_revisions WHERE id = ?",
    )
    .bind(revision_id)
    .fetch_one(pool)
    .await
    .map_err(|_| AppError::storage())
}

pub async fn confirm_draft(pool: &SqlitePool, revision_id: &str) -> Result<(), AppError> {
    transition(
        pool,
        revision_id,
        RevisionStatus::Draft,
        RevisionStatus::Confirmed,
        "user_confirmed",
        true,
    )
    .await
}

pub async fn begin_application(pool: &SqlitePool, revision_id: &str) -> Result<(), AppError> {
    transition(
        pool,
        revision_id,
        RevisionStatus::Confirmed,
        RevisionStatus::Applying,
        "application_started",
        true,
    )
    .await
}

pub async fn finish_application(
    pool: &SqlitePool,
    revision_id: &str,
    final_status: RevisionStatus,
    detail_code: &str,
) -> Result<(), AppError> {
    if !matches!(
        final_status,
        RevisionStatus::Applied
            | RevisionStatus::Failed
            | RevisionStatus::FailedPartial
            | RevisionStatus::Uncertain
    ) {
        return Err(AppError::history(
            "INVALID_REVISION_TRANSITION",
            "El estado final de la revisión no es válido.",
        ));
    }
    validate_detail_code(detail_code)?;
    transition(
        pool,
        revision_id,
        RevisionStatus::Applying,
        final_status,
        detail_code,
        false,
    )
    .await
}

/// Converts work left in `applying` at process exit to a durable uncertain
/// state. It never retries the database operation because its remote outcome
/// cannot be inferred from the local history alone.
pub async fn reconcile_interrupted_applications(pool: &SqlitePool) -> Result<u64, AppError> {
    let mut transaction = pool.begin().await.map_err(|_| AppError::storage())?;
    let revisions: Vec<(String,)> = sqlx::query_as(
        "SELECT id FROM history_revisions WHERE status = 'applying' ORDER BY created_at_ms, id",
    )
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| AppError::storage())?;
    if revisions.is_empty() {
        transaction
            .commit()
            .await
            .map_err(|_| AppError::storage())?;
        return Ok(0);
    }
    let now = now_ms()?;
    for (revision_id,) in &revisions {
        let changed = sqlx::query(
            "UPDATE history_revisions SET status = 'uncertain' WHERE id = ? AND status = 'applying'",
        )
        .bind(revision_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| AppError::storage())?
        .rows_affected();
        if changed == 1 {
            insert_event(
                &mut transaction,
                revision_id,
                "status_changed",
                Some(RevisionStatus::Applying),
                Some(RevisionStatus::Uncertain),
                "interrupted_after_restart",
                now,
            )
            .await?;
        }
    }
    transaction
        .commit()
        .await
        .map_err(|_| AppError::storage())?;
    Ok(revisions.len() as u64)
}

async fn transition(
    pool: &SqlitePool,
    revision_id: &str,
    expected: RevisionStatus,
    next: RevisionStatus,
    detail_code: &str,
    require_recovery: bool,
) -> Result<(), AppError> {
    validate_detail_code(detail_code)?;
    let mut transaction = pool.begin().await.map_err(|_| AppError::storage())?;
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT status, recovery_state FROM history_revisions WHERE id = ?")
            .bind(revision_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| AppError::storage())?;
    let Some((current_status, recovery_state)) = row else {
        return Err(AppError::history(
            "HISTORY_REVISION_NOT_FOUND",
            "La revisión ya no está disponible.",
        ));
    };
    if current_status != expected.as_db() {
        return Err(AppError::history(
            "INVALID_REVISION_TRANSITION",
            "La revisión cambió de estado y debe volver a cargarse.",
        ));
    }
    if require_recovery
        && next == RevisionStatus::Applying
        && !matches!(recovery_state.as_str(), "verified" | "not_required")
    {
        return Err(AppError::history(
            "RECOVERY_NOT_VERIFIED",
            "La revisión no tiene un punto de recuperación válido.",
        ));
    }
    let now = now_ms()?;
    let confirmed_at = (next == RevisionStatus::Confirmed).then_some(now);
    sqlx::query(
        "UPDATE history_revisions SET status = ?, confirmed_at_ms = COALESCE(?, confirmed_at_ms) WHERE id = ?",
    )
    .bind(next.as_db())
    .bind(confirmed_at)
    .bind(revision_id)
    .execute(&mut *transaction)
    .await
    .map_err(|_| AppError::storage())?;
    insert_event(
        &mut transaction,
        revision_id,
        "status_changed",
        Some(expected),
        Some(next),
        detail_code,
        now,
    )
    .await?;
    transaction.commit().await.map_err(|_| AppError::storage())
}

async fn insert_event(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    revision_id: &str,
    event_type: &str,
    from_status: Option<RevisionStatus>,
    to_status: Option<RevisionStatus>,
    detail_code: &str,
    created_at_ms: i64,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO history_events(id, revision_id, event_type, from_status, to_status, detail_code, created_at_ms)
         VALUES(?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(revision_id)
    .bind(event_type)
    .bind(from_status.map(RevisionStatus::as_db))
    .bind(to_status.map(RevisionStatus::as_db))
    .bind(detail_code)
    .bind(created_at_ms)
    .execute(&mut **transaction)
    .await
    .map_err(|_| AppError::storage())?;
    Ok(())
}

fn validate_detail_code(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(AppError::history(
            "INVALID_EVENT_CODE",
            "El evento del historial no es válido.",
        ));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn now_ms() -> Result<i64, AppError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AppError::storage())?;
    i64::try_from(duration.as_millis()).map_err(|_| AppError::storage())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage;

    async fn pool() -> (SqlitePool, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("dbsual-history-{}.sqlite", Uuid::new_v4()));
        let pool = storage::open_at(path.clone()).await.unwrap();
        (pool, path)
    }

    #[test]
    fn draft_confirmation_and_recovery_gate_are_durable_and_ordered() {
        tauri::async_runtime::block_on(async {
            let (pool, path) = pool().await;
            let project = create_project(
                &pool,
                &Uuid::new_v4().to_string(),
                "customer_db",
                DatabaseEngine::Mysql,
                Some("8.4.0"),
            )
            .await
            .unwrap();
            let revision = create_draft(
                &pool,
                &project.id,
                "Actualizar tabla de clientes",
                &"a".repeat(64),
                Uuid::new_v4(),
                1,
                RecoveryState::Unavailable,
            )
            .await
            .unwrap();
            assert_eq!(revision.revision_number, 1);
            assert_eq!(revision.status, "draft");
            assert_eq!(
                begin_application(&pool, &revision.id)
                    .await
                    .unwrap_err()
                    .code,
                "INVALID_REVISION_TRANSITION"
            );
            confirm_draft(&pool, &revision.id).await.unwrap();
            assert_eq!(
                begin_application(&pool, &revision.id)
                    .await
                    .unwrap_err()
                    .code,
                "RECOVERY_NOT_VERIFIED"
            );
            let persisted = list_revisions(&pool, &project.id).await.unwrap();
            assert_eq!(persisted[0].status, "confirmed");
            assert!(persisted[0].confirmed_at_ms.is_some());
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM history_events WHERE revision_id = ?"
                )
                .bind(&revision.id)
                .fetch_one(&pool)
                .await
                .unwrap(),
                2
            );
            pool.close().await;
            drop(pool);
            let _ = std::fs::remove_file(path);
        });
    }

    #[test]
    fn project_identity_is_stable_and_history_events_are_immutable() {
        tauri::async_runtime::block_on(async {
            let (pool, path) = pool().await;
            let connection_id = Uuid::new_v4().to_string();
            let first = create_project(&pool, &connection_id, "sales", DatabaseEngine::Mysql, None)
                .await
                .unwrap();
            let again = create_project(
                &pool,
                &connection_id,
                "sales",
                DatabaseEngine::Mariadb,
                Some("8.0.41"),
            )
            .await
            .unwrap();
            assert_eq!(first.id, again.id);
            assert_eq!(again.server_version.as_deref(), Some("8.0.41"));
            assert_eq!(again.engine, "mariadb");
            let revision = create_draft(
                &pool,
                &first.id,
                "Propuesta de cambio",
                &"b".repeat(64),
                Uuid::new_v4(),
                2,
                RecoveryState::Verified,
            )
            .await
            .unwrap();
            assert_eq!(
                sqlx::query("DELETE FROM history_events WHERE revision_id = ?")
                    .bind(&revision.id)
                    .execute(&pool)
                    .await
                    .is_err(),
                true
            );
            confirm_draft(&pool, &revision.id).await.unwrap();
            begin_application(&pool, &revision.id).await.unwrap();
            finish_application(
                &pool,
                &revision.id,
                RevisionStatus::Uncertain,
                "connection_lost",
            )
            .await
            .unwrap();
            assert_eq!(
                list_revisions(&pool, &first.id).await.unwrap()[0].status,
                "uncertain"
            );
            pool.close().await;
            drop(pool);
            let _ = std::fs::remove_file(path);
        });
    }

    #[test]
    fn recovery_point_keeps_ciphertext_metadata_and_does_not_upgrade_partial_coverage() {
        tauri::async_runtime::block_on(async {
            let (pool, path) = pool().await;
            let project = create_project(
                &pool,
                &Uuid::new_v4().to_string(),
                "restore_source",
                DatabaseEngine::Mysql,
                Some("8.4.0"),
            )
            .await
            .unwrap();
            let point = create_recovery_point(
                &pool,
                &project.id,
                DatabaseEngine::Mysql,
                Some("8.4.0"),
                Uuid::new_v4(),
                &"a".repeat(64),
                4096,
                2048,
                "visible_tables_views_triggers",
                "captured",
            )
            .await
            .unwrap();
            assert_eq!(point.coverage, "visible_tables_views_triggers");
            assert_eq!(point.verification_state, "captured");
            assert_eq!(point.encrypted_bytes, 4096);
            assert_eq!(
                list_recovery_points(&pool, &project.id).await.unwrap(),
                [point]
            );

            let partial_as_verified = create_recovery_point(
                &pool,
                &project.id,
                DatabaseEngine::Mysql,
                Some("8.4.0"),
                Uuid::new_v4(),
                &"b".repeat(64),
                4096,
                2048,
                "visible_tables_only",
                "verified",
            )
            .await
            .unwrap_err();
            assert_eq!(partial_as_verified.code, "INVALID_RECOVERY_POINT");
            pool.close().await;
            let _ = std::fs::remove_file(path);
        });
    }

    #[test]
    fn row_recovery_artifact_is_captured_then_verified_and_bound_to_revision() {
        tauri::async_runtime::block_on(async {
            let (pool, path) = pool().await;
            let project = create_project(
                &pool,
                &Uuid::new_v4().to_string(),
                "row_recovery_db",
                DatabaseEngine::Mysql,
                Some("8.4.0"),
            )
            .await
            .unwrap();
            let revision = create_draft(
                &pool,
                &project.id,
                "Actualizar una fila",
                &"a".repeat(64),
                Uuid::new_v4(),
                1,
                RecoveryState::Pending,
            )
            .await
            .unwrap();
            let artifact_id = Uuid::new_v4();
            let artifact = record_row_recovery_artifact(
                &pool,
                &project.id,
                &revision.id,
                artifact_id,
                &"B".repeat(64),
                8192,
            )
            .await
            .unwrap();
            assert_eq!(artifact.kind, RowRecoveryArtifactKind::RowCompensation);
            assert_eq!(
                artifact.verification_state,
                RowRecoveryArtifactState::Captured
            );
            assert_eq!(artifact.ciphertext_sha256, "b".repeat(64));
            assert_eq!(artifact.encrypted_bytes, 8192);
            assert_eq!(
                get_row_recovery_artifact(&pool, &revision.id)
                    .await
                    .unwrap(),
                Some(artifact)
            );

            let mismatch = mark_row_recovery_artifact_verified(
                &pool,
                &revision.id,
                artifact_id,
                &"c".repeat(64),
            )
            .await
            .unwrap_err();
            assert_eq!(mismatch.code, "RECOVERY_ARTIFACT_MISMATCH");
            let verified = mark_row_recovery_artifact_verified(
                &pool,
                &revision.id,
                artifact_id,
                &"b".repeat(64),
            )
            .await
            .unwrap();
            assert_eq!(
                verified.verification_state,
                RowRecoveryArtifactState::Verified
            );
            let revision_recovery_state: (String,) =
                sqlx::query_as("SELECT recovery_state FROM history_revisions WHERE id = ?")
                    .bind(&revision.id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(revision_recovery_state.0, "verified");
            assert_eq!(
                mark_row_recovery_artifact_verified(
                    &pool,
                    &revision.id,
                    artifact_id,
                    &"b".repeat(64),
                )
                .await
                .unwrap(),
                verified,
                "verification is idempotent"
            );
            pool.close().await;
            drop(pool);
            let _ = std::fs::remove_file(path);
        });
    }

    #[test]
    fn row_recovery_artifact_rejects_invalid_or_rebound_metadata() {
        tauri::async_runtime::block_on(async {
            let (pool, path) = pool().await;
            let project = create_project(
                &pool,
                &Uuid::new_v4().to_string(),
                "row_recovery_validation",
                DatabaseEngine::Mysql,
                None,
            )
            .await
            .unwrap();
            let other_project = create_project(
                &pool,
                &Uuid::new_v4().to_string(),
                "other_row_recovery_validation",
                DatabaseEngine::Mysql,
                None,
            )
            .await
            .unwrap();
            let revision = create_draft(
                &pool,
                &project.id,
                "Actualizar fila",
                &"d".repeat(64),
                Uuid::new_v4(),
                1,
                RecoveryState::Pending,
            )
            .await
            .unwrap();

            let invalid_hash = record_row_recovery_artifact(
                &pool,
                &project.id,
                &revision.id,
                Uuid::new_v4(),
                "not-a-hash",
                100,
            )
            .await
            .unwrap_err();
            assert_eq!(invalid_hash.code, "INVALID_RECOVERY_ARTIFACT");
            let invalid_size = record_row_recovery_artifact(
                &pool,
                &project.id,
                &revision.id,
                Uuid::new_v4(),
                &"e".repeat(64),
                0,
            )
            .await
            .unwrap_err();
            assert_eq!(invalid_size.code, "INVALID_RECOVERY_ARTIFACT");
            let check_constraint = sqlx::query(
                "INSERT INTO row_recovery_artifacts(
                    id, project_id, revision_id, artifact_id, kind, ciphertext_sha256,
                    encrypted_bytes, verification_state, created_at_ms
                 ) VALUES (?, ?, ?, ?, 'full_database_backup', ?, 100, 'captured', 1)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(&project.id)
            .bind(&revision.id)
            .bind(Uuid::new_v4().to_string())
            .bind("e".repeat(64))
            .execute(&pool)
            .await;
            assert!(
                check_constraint.is_err(),
                "kind CHECK constraint must hold in SQLite"
            );
            let wrong_project = record_row_recovery_artifact(
                &pool,
                &other_project.id,
                &revision.id,
                Uuid::new_v4(),
                &"e".repeat(64),
                100,
            )
            .await
            .unwrap_err();
            assert_eq!(wrong_project.code, "HISTORY_REVISION_NOT_FOUND");

            let artifact_id = Uuid::new_v4();
            record_row_recovery_artifact(
                &pool,
                &project.id,
                &revision.id,
                artifact_id,
                &"e".repeat(64),
                100,
            )
            .await
            .unwrap();
            let duplicate = record_row_recovery_artifact(
                &pool,
                &project.id,
                &revision.id,
                Uuid::new_v4(),
                &"f".repeat(64),
                100,
            )
            .await
            .unwrap_err();
            assert_eq!(duplicate.code, "RECOVERY_ARTIFACT_ALREADY_EXISTS");

            confirm_draft(&pool, &revision.id).await.unwrap();
            let closed_draft = record_row_recovery_artifact(
                &pool,
                &project.id,
                &revision.id,
                Uuid::new_v4(),
                &"a".repeat(64),
                100,
            )
            .await
            .unwrap_err();
            assert_eq!(closed_draft.code, "INVALID_RECOVERY_ARTIFACT_TARGET");
            let closed_verification = mark_row_recovery_artifact_verified(
                &pool,
                &revision.id,
                artifact_id,
                &"e".repeat(64),
            )
            .await
            .unwrap_err();
            assert_eq!(closed_verification.code, "RECOVERY_ARTIFACT_NOT_FOUND");

            let incorrectly_initialized = create_draft(
                &pool,
                &project.id,
                "Borrador sin recuperación pendiente",
                &"f".repeat(64),
                Uuid::new_v4(),
                1,
                RecoveryState::Verified,
            )
            .await
            .unwrap();
            let non_pending = record_row_recovery_artifact(
                &pool,
                &project.id,
                &incorrectly_initialized.id,
                Uuid::new_v4(),
                &"a".repeat(64),
                100,
            )
            .await
            .unwrap_err();
            assert_eq!(non_pending.code, "INVALID_RECOVERY_ARTIFACT_TARGET");
            pool.close().await;
            drop(pool);
            let _ = std::fs::remove_file(path);
        });
    }

    #[tokio::test]
    async fn interrupted_applications_become_uncertain_once_after_reopen() {
        let (pool, path) = pool().await;
        let project = create_project(
            &pool,
            &Uuid::new_v4().to_string(),
            "reconcile_db",
            DatabaseEngine::Mysql,
            Some("8.4.11"),
        )
        .await
        .unwrap();
        let revision = create_draft(
            &pool,
            &project.id,
            "Cambio interrumpido",
            &"c".repeat(64),
            Uuid::new_v4(),
            1,
            RecoveryState::Verified,
        )
        .await
        .unwrap();
        confirm_draft(&pool, &revision.id).await.unwrap();
        begin_application(&pool, &revision.id).await.unwrap();
        pool.close().await;
        drop(pool);

        let reopened = storage::open_at(path.clone()).await.unwrap();
        assert_eq!(
            reconcile_interrupted_applications(&reopened).await.unwrap(),
            1
        );
        let status: (String,) = sqlx::query_as("SELECT status FROM history_revisions WHERE id = ?")
            .bind(&revision.id)
            .fetch_one(&reopened)
            .await
            .unwrap();
        assert_eq!(status.0, "uncertain");
        let event: (String, Option<String>, Option<String>, String) = sqlx::query_as(
            "SELECT detail_code, from_status, to_status, event_type FROM history_events
             WHERE revision_id = ? AND detail_code = 'interrupted_after_restart'",
        )
        .bind(&revision.id)
        .fetch_one(&reopened)
        .await
        .unwrap();
        assert_eq!(
            event,
            (
                "interrupted_after_restart".into(),
                Some("applying".into()),
                Some("uncertain".into()),
                "status_changed".into()
            )
        );
        assert_eq!(
            reconcile_interrupted_applications(&reopened).await.unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM history_events WHERE revision_id = ? AND detail_code = 'interrupted_after_restart'",
            )
            .bind(&revision.id)
            .fetch_one(&reopened)
            .await
            .unwrap(),
            1
        );
        reopened.close().await;
        let _ = std::fs::remove_file(path);
    }
}
