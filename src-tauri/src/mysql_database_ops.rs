use crate::{
    history::{self, DatabaseEngine, RecoveryState, RevisionStatus, RevisionSummary},
    vault::{self, MasterKey, RecoveryEnvelope},
    AppError,
};
use sha2::{Digest, Sha256};
use sqlx::{MySqlPool, SqlitePool};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub(crate) fn valid_database_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$'))
}

pub(crate) fn create_plan(name: &str, server_uuid: &str, server_version: &str) -> String {
    let version_hash = Sha256::digest(server_version.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!(
        "-- DBSUAL_CREATE_DATABASE_V1 server_uuid={server_uuid} version_sha256={version_hash}\nCREATE DATABASE `{name}`;"
    )
}

pub(crate) async fn prepare_create_database_revision(
    storage: &SqlitePool,
    active: &MySqlPool,
    connection_id: &str,
    database_name: &str,
    artifacts_directory: PathBuf,
    vault_id: Uuid,
    master_key: MasterKey,
    envelope: RecoveryEnvelope,
) -> Result<RevisionSummary, AppError> {
    if !valid_database_name(database_name) {
        return Err(AppError::history(
            "INVALID_DATABASE_NAME",
            "El nombre debe tener hasta 64 caracteres ASCII: letras, números, _ o $.",
        ));
    }
    let (server_version, server_uuid): (String, String) =
        sqlx::query_as("SELECT VERSION(), @@server_uuid")
            .fetch_one(active)
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
    .bind(database_name)
    .fetch_optional(active)
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
    if envelope.vault_id() != vault_id {
        return Err(AppError::history(
            "VAULT_ID_MISMATCH",
            "La clave y la frase local no pertenecen al mismo depósito.",
        ));
    }
    let plan = create_plan(database_name, &server_uuid, &server_version);
    let plan_hash = Sha256::digest(plan.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let plan_for_encryption = plan.into_bytes();
    let encryption_directory = artifacts_directory.clone();
    let artifact = tauri::async_runtime::spawn_blocking(move || {
        vault::encrypt_artifact_bytes_to_file(
            &encryption_directory,
            &plan_for_encryption,
            vault_id,
            &master_key,
            &envelope,
        )
    })
    .await
    .map_err(|_| AppError::history("PLAN_ENCRYPTION_FAILED", "No se pudo proteger el plan."))?
    .map_err(|_| AppError::history("PLAN_ENCRYPTION_FAILED", "No se pudo proteger el plan."))?;
    let project = match history::create_project(
        storage,
        connection_id,
        database_name,
        DatabaseEngine::Mysql,
        Some(&server_version),
    )
    .await
    {
        Ok(project) => project,
        Err(error) => {
            remove_artifact(&artifacts_directory, artifact.artifact_id);
            return Err(error);
        }
    };
    let revision = history::create_draft(
        storage,
        &project.id,
        "Crear base de datos",
        &plan_hash,
        artifact.artifact_id,
        1,
        RecoveryState::NotRequired,
    )
    .await;
    if revision.is_err() {
        remove_artifact(&artifacts_directory, artifact.artifact_id);
    }
    revision
}

fn remove_artifact(directory: &Path, artifact_id: Uuid) {
    let _ = std::fs::remove_file(directory.join(format!("{artifact_id}.dbsual-artifact")));
}

pub(crate) async fn apply_create_database_revision(
    storage: &SqlitePool,
    active: &MySqlPool,
    revision_id: &str,
    database_name: &str,
    plan: &str,
) -> Result<(), AppError> {
    let (server_version, server_uuid): (String, String) =
        sqlx::query_as("SELECT VERSION(), @@server_uuid")
            .fetch_one(active)
            .await
            .map_err(|_| {
                AppError::history(
                    "HISTORY_TARGET_UNAVAILABLE",
                    "No se pudo verificar el servidor de destino.",
                )
            })?;
    if plan != create_plan(database_name, &server_uuid, &server_version) {
        return Err(AppError::history(
            "TARGET_CHANGED",
            "El servidor o la versión difieren del destino revisado. Prepara una revisión nueva.",
        ));
    }
    let existing: Option<(String,)> = sqlx::query_as(
        "SELECT CAST(SCHEMA_NAME AS CHAR) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
    )
    .bind(database_name)
    .fetch_optional(active)
    .await
    .map_err(|_| {
        AppError::history(
            "HISTORY_TARGET_UNAVAILABLE",
            "No se pudo comprobar el estado del destino.",
        )
    })?;
    if existing.is_some() {
        return Err(AppError::history(
            "TARGET_CONFLICT",
            "La base ya existe. Revisa de nuevo el destino antes de continuar.",
        ));
    }
    history::begin_application(storage, revision_id).await?;
    let execution = sqlx::query(&format!("CREATE DATABASE `{database_name}`"))
        .execute(active)
        .await;
    let present: Result<Option<(String,)>, sqlx::Error> = sqlx::query_as(
        "SELECT CAST(SCHEMA_NAME AS CHAR) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
    )
    .bind(database_name)
    .fetch_optional(active)
    .await;
    match (execution, present) {
        (Ok(_), Ok(Some(_))) => {
            history::finish_application(
                storage,
                revision_id,
                RevisionStatus::Applied,
                "database_created",
            )
            .await
        }
        (Err(_), Ok(None)) => {
            history::finish_application(
                storage,
                revision_id,
                RevisionStatus::Failed,
                "create_rejected",
            )
            .await?;
            Err(AppError::history(
                "DATABASE_CREATE_FAILED",
                "MySQL rechazó la creación de la base de datos.",
            ))
        }
        _ => {
            history::finish_application(
                storage,
                revision_id,
                RevisionStatus::Uncertain,
                "create_unverified",
            )
            .await?;
            Err(AppError::history(
                "APPLICATION_UNCERTAIN",
                "No se pudo verificar el resultado. Actualiza las bases de datos antes de reintentar.",
            ))
        }
    }
}
