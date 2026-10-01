use std::{collections::HashMap, path::Path, time::Duration};

use serde::{Deserialize, Serialize};
use sqlx::{
    mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlSslMode},
    ConnectOptions, MySqlPool, Row, SqlitePool,
};
use tauri::State;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{ssh, AppError, AppState};

fn postgres_error(error: crate::postgres::PostgresError) -> AppError {
    match error {
        crate::postgres::PostgresError::InvalidConfig => AppError::new(
            "INVALID_CONNECTION",
            "Revisa host, puerto, usuario y base de datos.",
        ),
        crate::postgres::PostgresError::InvalidTlsConfig => AppError::new(
            "INVALID_TLS_CONFIG",
            "La configuración TLS de PostgreSQL no es válida.",
        ),
        crate::postgres::PostgresError::ConnectionFailed => AppError::new(
            "CONNECTION_FAILED",
            "No se pudo conectar a PostgreSQL. Revisa el servidor, las credenciales y TLS.",
        ),
        crate::postgres::PostgresError::ServerNotPostgres => AppError::new(
            "SERVER_ENGINE_MISMATCH",
            "El servidor no se identificó como PostgreSQL.",
        ),
        crate::postgres::PostgresError::ServerVersionUnavailable => AppError::new(
            "SERVER_VERSION_UNAVAILABLE",
            "No se pudo determinar la versión de PostgreSQL.",
        ),
        crate::postgres::PostgresError::DatabaseListFailed => AppError::new(
            "METADATA_FAILED",
            "No se pudieron listar las bases de datos accesibles.",
        ),
        crate::postgres::PostgresError::MetadataFailed => AppError::new(
            "METADATA_FAILED",
            "No se pudieron consultar los objetos de PostgreSQL.",
        ),
        crate::postgres::PostgresError::ObjectKindChanged => AppError::new(
            "OBJECT_CHANGED",
            "El objeto cambió desde la última actualización. Actualiza el explorador.",
        ),
    }
}

pub struct ActiveConnections(pub Mutex<HashMap<String, MySqlPool>>);

impl Default for ActiveConnections {
    fn default() -> Self {
        Self(Mutex::new(HashMap::new()))
    }
}

pub struct ActiveTunnels(pub Mutex<HashMap<String, ssh::SshTunnel>>);

impl Default for ActiveTunnels {
    fn default() -> Self {
        Self(Mutex::new(HashMap::new()))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SavedConnection {
    id: String,
    name: String,
    engine: String,
    host: String,
    port: i64,
    #[sqlx(rename = "user_name")]
    user: String,
    #[sqlx(rename = "tls_mode")]
    tls_mode: String,
    tls_ca_path: Option<String>,
    tls_client_cert_path: Option<String>,
    tls_client_key_path: Option<String>,
    ssh_enabled: bool,
    ssh_host: Option<String>,
    ssh_port: Option<i64>,
    ssh_user: Option<String>,
    ssh_auth_method: Option<String>,
    ssh_key_path: Option<String>,
    ssh_agent_pipe: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionInput {
    id: Option<String>,
    name: String,
    engine: String,
    host: String,
    port: u16,
    user: String,
    password: Option<String>,
    tls_mode: String,
    tls_ca_path: Option<String>,
    tls_client_cert_path: Option<String>,
    tls_client_key_path: Option<String>,
    ssh_enabled: Option<bool>,
    ssh_host: Option<String>,
    ssh_port: Option<u16>,
    ssh_user: Option<String>,
    ssh_auth_method: Option<String>,
    ssh_key_path: Option<String>,
    ssh_agent_pipe: Option<String>,
    ssh_password: Option<String>,
    ssh_key_passphrase: Option<String>,
}

impl ConnectionInput {
    fn validate(&self) -> Result<(), AppError> {
        let simple = |s: &str, max: usize| {
            !s.trim().is_empty()
                && s == s.trim()
                && s.len() <= max
                && !s.chars().any(char::is_control)
        };
        if self.engine == "sqlite" {
            if !simple(&self.name, 100)
                || self.host.is_empty()
                || self.host.len() > 32767
                || self.host.chars().any(char::is_control)
                || !Path::new(&self.host).is_absolute()
                || self.port != 0
                || !self.user.is_empty()
                || self.tls_mode != "disabled"
                || self.password.is_some()
                || self.ssh_enabled.unwrap_or(false)
                || self.ssh_host.is_some()
                || self.ssh_user.is_some()
                || self.ssh_password.is_some()
                || self.ssh_key_path.is_some()
                || self.ssh_key_passphrase.is_some()
                || self.tls_ca_path.is_some()
                || self.tls_client_cert_path.is_some()
                || self.tls_client_key_path.is_some()
                || self
                    .id
                    .as_ref()
                    .is_some_and(|id| Uuid::parse_str(id).is_err())
            {
                return Err(AppError::new(
                    "INVALID_CONNECTION",
                    "Selecciona un archivo SQLite existente y accesible.",
                ));
            }
            return Ok(());
        }
        if !matches!(self.engine.as_str(), "mysql" | "mariadb" | "postgresql")
            || !simple(&self.name, 100)
            || !simple(&self.host, 255)
            || self.host.contains(['/', '\\', '@'])
            || self.port == 0
            || !simple(&self.user, 128)
            || !matches!(self.tls_mode.as_str(), "disabled" | "verifyIdentity")
            || self.password.as_ref().is_some_and(|s| s.len() > 4096)
            || self.ssh_password.as_ref().is_some_and(|s| s.len() > 4096)
            || self
                .ssh_key_passphrase
                .as_ref()
                .is_some_and(|s| s.len() > 4096)
            || [
                self.tls_ca_path.as_ref(),
                self.tls_client_cert_path.as_ref(),
                self.tls_client_key_path.as_ref(),
                self.ssh_host.as_ref(),
                self.ssh_user.as_ref(),
                self.ssh_auth_method.as_ref(),
                self.ssh_key_path.as_ref(),
                self.ssh_agent_pipe.as_ref(),
            ]
            .iter()
            .flatten()
            .any(|p| p.len() > 1024 || p.chars().any(char::is_control))
            || self
                .id
                .as_ref()
                .is_some_and(|id| Uuid::parse_str(id).is_err())
        {
            return Err(AppError::new(
                "INVALID_CONNECTION",
                "Revisa nombre, host, puerto, usuario y TLS.",
            ));
        }
        if self.engine == "postgresql" && self.ssh_enabled.unwrap_or(false) {
            return Err(AppError::new(
                "POSTGRES_SSH_UNAVAILABLE",
                "El túnel SSH para PostgreSQL todavía no está disponible.",
            ));
        }
        if self.engine == "postgresql"
            && (self.tls_client_cert_path.is_some() || self.tls_client_key_path.is_some())
        {
            return Err(AppError::new(
                "POSTGRES_CLIENT_TLS_UNAVAILABLE",
                "El certificado de cliente PostgreSQL todavía no está disponible.",
            ));
        }
        if self.tls_mode == "disabled"
            && (self.tls_ca_path.is_some()
                || self.tls_client_cert_path.is_some()
                || self.tls_client_key_path.is_some())
        {
            return Err(AppError::new(
                "INVALID_CONNECTION",
                "Activa TLS para usar certificados.",
            ));
        }
        if self.tls_client_cert_path.is_some() != self.tls_client_key_path.is_some() {
            return Err(AppError::new(
                "INVALID_CONNECTION",
                "Indica certificado y clave de cliente juntos.",
            ));
        }
        if self.ssh_enabled.unwrap_or(false) {
            let ssh_host = self.ssh_host.as_deref();
            let ssh_user = self.ssh_user.as_deref();
            let ssh_port = self.ssh_port.unwrap_or_default();
            let auth_method = self.ssh_auth_method.as_deref();
            if ssh_host.is_none_or(|host| !simple(host, 255) || host.contains(['/', '\\', '@']))
                || ssh_user.is_none_or(|user| !simple(user, 128))
                || ssh_port == 0
                || !matches!(auth_method, Some("password" | "privateKey" | "agent"))
            {
                return Err(AppError::new(
                    "INVALID_CONNECTION",
                    "Revisa host, puerto, usuario y método de autenticación SSH.",
                ));
            }
            match auth_method {
                Some("password")
                    if self.ssh_key_path.is_some() || self.ssh_key_passphrase.is_some() =>
                {
                    return Err(AppError::new(
                        "INVALID_CONNECTION",
                        "El método SSH por contraseña no admite archivos de clave.",
                    ));
                }
                Some("privateKey") if self.ssh_key_path.as_deref().is_none_or(str::is_empty) => {
                    return Err(AppError::new(
                        "INVALID_CONNECTION",
                        "Indica la ruta de la clave privada SSH.",
                    ));
                }
                Some("privateKey") if self.ssh_password.is_some() => {
                    return Err(AppError::new(
                        "INVALID_CONNECTION",
                        "El método por clave privada no admite contraseña SSH.",
                    ));
                }
                Some("agent")
                    if self.ssh_key_path.is_some()
                        || self.ssh_password.is_some()
                        || self.ssh_key_passphrase.is_some() =>
                {
                    return Err(AppError::new(
                        "INVALID_CONNECTION",
                        "El agente SSH usa su identidad del sistema y no admite secretos propios.",
                    ));
                }
                _ => {}
            }
            if auth_method == Some("agent")
                && self
                    .ssh_agent_pipe
                    .as_deref()
                    .is_some_and(|pipe| !pipe.starts_with(r"\\.\pipe\"))
            {
                return Err(AppError::new(
                    "INVALID_CONNECTION",
                    "La ruta del agente debe ser un named pipe de Windows.",
                ));
            }
            if auth_method != Some("agent") && self.ssh_agent_pipe.is_some() {
                return Err(AppError::new(
                    "INVALID_CONNECTION",
                    "La ruta named pipe solo se usa con autenticación por agente SSH.",
                ));
            }
            if auth_method != Some("privateKey") && self.ssh_key_passphrase.is_some() {
                return Err(AppError::new(
                    "INVALID_CONNECTION",
                    "La frase de paso solo se usa con una clave privada SSH.",
                ));
            }
            #[cfg(not(windows))]
            if auth_method == Some("agent") {
                return Err(AppError::new(
                    "SSH_AGENT_UNAVAILABLE",
                    "El agente OpenSSH por named pipe solo está disponible en Windows.",
                ));
            }
        }
        Ok(())
    }

    fn saved(&self, id: String, previous: Option<&SavedConnection>) -> SavedConnection {
        SavedConnection {
            id,
            name: self.name.clone(),
            engine: self.engine.clone(),
            host: self.host.clone(),
            port: self.port as i64,
            user: self.user.clone(),
            tls_mode: self.tls_mode.clone(),
            tls_ca_path: self.tls_ca_path.clone().filter(|s| !s.is_empty()),
            tls_client_cert_path: self.tls_client_cert_path.clone().filter(|s| !s.is_empty()),
            tls_client_key_path: self.tls_client_key_path.clone().filter(|s| !s.is_empty()),
            ssh_enabled: self.ssh_enabled.unwrap_or(false),
            ssh_host: self.ssh_host.clone().filter(|s| !s.is_empty()),
            ssh_port: self.ssh_port.map(i64::from),
            ssh_user: self.ssh_user.clone().filter(|s| !s.is_empty()),
            ssh_auth_method: self.ssh_auth_method.clone().filter(|s| !s.is_empty()),
            ssh_key_path: self.ssh_key_path.clone().filter(|s| !s.is_empty()),
            ssh_agent_pipe: self
                .ssh_agent_pipe
                .clone()
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    (self.ssh_auth_method.as_deref() == Some("agent"))
                        .then(|| r"\\.\pipe\openssh-ssh-agent".to_owned())
                }),
        }
        .preserve_ssh_config(self.ssh_enabled.is_none().then_some(previous).flatten())
    }
}

impl SavedConnection {
    fn preserve_ssh_config(mut self, previous: Option<&SavedConnection>) -> Self {
        if let Some(previous) = previous {
            self.ssh_enabled = previous.ssh_enabled;
            self.ssh_host.clone_from(&previous.ssh_host);
            self.ssh_port = previous.ssh_port;
            self.ssh_user.clone_from(&previous.ssh_user);
            self.ssh_auth_method.clone_from(&previous.ssh_auth_method);
            self.ssh_key_path.clone_from(&previous.ssh_key_path);
            self.ssh_agent_pipe.clone_from(&previous.ssh_agent_pipe);
        }
        self
    }
}

fn validate_resolved_ssh(saved: &SavedConnection) -> Result<(), AppError> {
    if !saved.ssh_enabled {
        return Ok(());
    }
    let simple = |value: &str, max: usize| {
        !value.trim().is_empty()
            && value == value.trim()
            && value.len() <= max
            && !value.chars().any(char::is_control)
    };
    let valid = saved
        .ssh_host
        .as_deref()
        .is_some_and(|host| simple(host, 255) && !host.contains(['/', '\\', '@']))
        && saved
            .ssh_port
            .is_some_and(|port| (1..=u16::MAX as i64).contains(&port))
        && saved
            .ssh_user
            .as_deref()
            .is_some_and(|user| simple(user, 128))
        && matches!(
            saved.ssh_auth_method.as_deref(),
            Some("password" | "privateKey" | "agent")
        )
        && saved
            .ssh_key_path
            .as_deref()
            .is_none_or(|path| simple(path, 1024))
        && saved
            .ssh_agent_pipe
            .as_deref()
            .is_none_or(|pipe| simple(pipe, 1024));
    if !valid {
        return Err(AppError::new(
            "INVALID_CONNECTION",
            "La configuración SSH guardada no es válida.",
        ));
    }
    if saved.ssh_auth_method.as_deref() == Some("privateKey")
        && saved.ssh_key_path.as_deref().is_none_or(str::is_empty)
    {
        return Err(AppError::new(
            "INVALID_CONNECTION",
            "Indica la ruta de la clave privada SSH.",
        ));
    }
    if saved.ssh_auth_method.as_deref() == Some("agent") {
        let pipe = saved.ssh_agent_pipe.as_deref().unwrap_or_default();
        if !pipe.starts_with(r"\\.\pipe\") {
            return Err(AppError::new(
                "INVALID_CONNECTION",
                "La ruta del agente debe ser un named pipe de Windows.",
            ));
        }
        #[cfg(not(windows))]
        return Err(AppError::new(
            "SSH_AGENT_UNAVAILABLE",
            "El agente OpenSSH por named pipe solo está disponible en Windows.",
        ));
    }
    Ok(())
}

fn credential(id: &str) -> Result<keyring::Entry, AppError> {
    keyring::Entry::new("com.dbsual.desktop.mysql", id).map_err(|_| {
        AppError::new(
            "CREDENTIAL_STORE",
            "No se pudo acceder a las credenciales de Windows.",
        )
    })
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionSecrets {
    version: Option<u8>,
    db_password: Option<String>,
    ssh_password: Option<String>,
    ssh_key_passphrase: Option<String>,
}

impl ConnectionSecrets {
    fn keyring_value(self) -> Result<Option<String>, AppError> {
        if self.db_password.is_none()
            && self.ssh_password.is_none()
            && self.ssh_key_passphrase.is_none()
        {
            return Ok(None);
        }
        serde_json::to_string(&serde_json::json!({
            "version": 1,
            "dbPassword": self.db_password,
            "sshPassword": self.ssh_password,
            "sshKeyPassphrase": self.ssh_key_passphrase,
        }))
        .map(Some)
        .map_err(|_| AppError::new("CREDENTIAL_STORE", "No se pudo preparar la credencial."))
    }
}

async fn read_secrets(id: String) -> Result<ConnectionSecrets, AppError> {
    tokio::task::spawn_blocking(move || match credential(&id)?.get_password() {
        Ok(value) => Ok(match serde_json::from_str::<ConnectionSecrets>(&value) {
            Ok(secrets) if secrets.version == Some(1) => secrets,
            _ => ConnectionSecrets {
                db_password: Some(value),
                ..ConnectionSecrets::default()
            },
        }),
        Err(keyring::Error::NoEntry) => Ok(ConnectionSecrets::default()),
        Err(_) => Err(AppError::new(
            "CREDENTIAL_STORE",
            "No se pudo leer la contraseña guardada en Windows.",
        )),
    })
    .await
    .map_err(|_| AppError::new("CREDENTIAL_STORE", "No se pudo leer la credencial."))?
}

async fn write_secrets(id: String, secrets: ConnectionSecrets) -> Result<(), AppError> {
    let value = secrets.keyring_value()?;
    tokio::task::spawn_blocking(move || {
        let entry = credential(&id)?;
        match value {
            Some(value) => entry.set_password(&value).map_err(|_| {
                AppError::new(
                    "CREDENTIAL_STORE",
                    "No se pudieron guardar las credenciales en Windows.",
                )
            }),
            None => match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(_) => Err(AppError::new(
                    "CREDENTIAL_STORE",
                    "No se pudieron quitar las credenciales de Windows.",
                )),
            },
        }
    })
    .await
    .map_err(|_| AppError::new("CREDENTIAL_STORE", "No se pudo actualizar la credencial."))?
}

async fn all(pool: &SqlitePool) -> Result<Vec<SavedConnection>, AppError> {
    sqlx::query_as::<_, SavedConnection>("SELECT id,name,engine,host,port,user_name,tls_mode,tls_ca_path,tls_client_cert_path,tls_client_key_path,ssh_enabled,ssh_host,ssh_port,ssh_user,ssh_auth_method,ssh_key_path,ssh_agent_pipe FROM connections ORDER BY name COLLATE NOCASE")
        .fetch_all(pool).await.map_err(|_| AppError::storage())
}

async fn one(pool: &SqlitePool, id: &str) -> Result<SavedConnection, AppError> {
    sqlx::query_as::<_, SavedConnection>("SELECT id,name,engine,host,port,user_name,tls_mode,tls_ca_path,tls_client_cert_path,tls_client_key_path,ssh_enabled,ssh_host,ssh_port,ssh_user,ssh_auth_method,ssh_key_path,ssh_agent_pipe FROM connections WHERE id = ?")
        .bind(id).fetch_optional(pool).await.map_err(|_| AppError::storage())?
        .ok_or(AppError::new("CONNECTION_NOT_FOUND", "La conexión ya no existe."))
}

fn options(
    saved: &SavedConnection,
    password: Option<&str>,
    socket_host: &str,
    socket_port: u16,
    tunneled: bool,
) -> Result<MySqlConnectOptions, AppError> {
    let mut opts = MySqlConnectOptions::new()
        .host(socket_host)
        .port(socket_port)
        .username(&saved.user)
        .disable_statement_logging()
        .ssl_mode(if saved.tls_mode == "verifyIdentity" {
            MySqlSslMode::VerifyIdentity
        } else {
            MySqlSslMode::Disabled
        });
    if tunneled && saved.tls_mode == "verifyIdentity" {
        opts = opts.ssl_hostname(&saved.host);
    }
    if let Some(password) = password {
        opts = opts.password(password);
    }
    for path in [
        &saved.tls_ca_path,
        &saved.tls_client_cert_path,
        &saved.tls_client_key_path,
    ]
    .into_iter()
    .flatten()
    {
        if !Path::new(path).is_file() {
            return Err(AppError::new(
                "CERTIFICATE_MISSING",
                "No se encontró un archivo TLS configurado.",
            ));
        }
    }
    if let Some(path) = &saved.tls_ca_path {
        opts = opts.ssl_ca(path);
    }
    if let Some(path) = &saved.tls_client_cert_path {
        opts = opts.ssl_client_cert(path);
    }
    if let Some(path) = &saved.tls_client_key_path {
        opts = opts.ssl_client_key(path);
    }
    Ok(opts)
}

fn connection_error(error: sqlx::Error, engine: &str) -> AppError {
    let mariadb = engine == "mariadb";
    match error {
        sqlx::Error::PoolTimedOut => AppError::new(
            "CONNECTION_TIMEOUT",
            if mariadb {
                "Se agotó el tiempo de conexión con MariaDB."
            } else {
                "Se agotó el tiempo de conexión con MySQL."
            },
        ),
        sqlx::Error::Io(ref io) if io.kind() == std::io::ErrorKind::TimedOut => AppError::new(
            "CONNECTION_TIMEOUT",
            if mariadb {
                "Se agotó el tiempo de conexión con MariaDB."
            } else {
                "Se agotó el tiempo de conexión con MySQL."
            },
        ),
        sqlx::Error::Io(ref io)
            if matches!(
                io.kind(),
                std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::HostUnreachable
                    | std::io::ErrorKind::NetworkUnreachable
            ) =>
        {
            AppError::new(
                "SERVER_UNAVAILABLE",
                if mariadb {
                    "No se puede acceder al servidor MariaDB indicado."
                } else {
                    "No se puede acceder al servidor MySQL indicado."
                },
            )
        }
        sqlx::Error::Database(ref db) if matches!(db.code().as_deref(), Some("1045" | "28000")) => {
            AppError::new(
                "AUTH_REJECTED",
                if mariadb {
                    "MariaDB rechazó el usuario o la contraseña."
                } else {
                    "MySQL rechazó el usuario o la contraseña."
                },
            )
        }
        _ => AppError::new(
            "CONNECTION_FAILED",
            if mariadb {
                "No se pudo conectar a MariaDB. Revisa el servidor, las credenciales y TLS."
            } else {
                "No se pudo conectar a MySQL. Revisa el servidor, las credenciales y TLS."
            },
        ),
    }
}

fn tls_error_message(engine: &str) -> &'static str {
    if engine == "mariadb" {
        "MariaDB no confirmó una conexión TLS."
    } else {
        "MySQL no confirmó una conexión TLS."
    }
}

fn validate_server_engine(engine: &str, version: &str) -> Result<(), AppError> {
    let is_mariadb = version.to_ascii_lowercase().contains("mariadb");
    match engine {
        "mysql" if is_mariadb => Err(AppError::new(
            "DATABASE_ENGINE_MISMATCH",
            "La conexión está configurada para MySQL, pero el servidor identificado es MariaDB.",
        )),
        "mariadb" if !is_mariadb => Err(AppError::new(
            "DATABASE_ENGINE_MISMATCH",
            "La conexión está configurada para MariaDB, pero el servidor no se identificó como MariaDB.",
        )),
        "mariadb" if !mariadb_version_supported(version) => Err(AppError::new(
            "DATABASE_VERSION_UNSUPPORTED",
            "DBSUAL admite MariaDB 10.6, 10.11 y 11.4 en esta versión.",
        )),
        _ => Ok(()),
    }
}

fn mariadb_version_supported(version: &str) -> bool {
    let components: Vec<u64> = version
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect();
    components
        .windows(2)
        .any(|pair| matches!(pair, [10, 6] | [10, 11] | [11, 4]))
}

async fn connect(
    saved: &SavedConnection,
    password: Option<&str>,
    tunnel_port: Option<u16>,
) -> Result<MySqlPool, AppError> {
    let (socket_host, socket_port) = tunnel_port
        .map(|port| ("127.0.0.1", port))
        .unwrap_or((&saved.host, saved.port as u16));
    let opts = options(
        saved,
        password,
        socket_host,
        socket_port,
        tunnel_port.is_some(),
    )?;
    MySqlPoolOptions::new()
        .max_connections(3)
        .acquire_timeout(Duration::from_secs(8))
        .connect_with(opts)
        .await
        .map_err(|error| connection_error(error, &saved.engine))
}

#[cfg(any(test, feature = "tauri-mock-ipc-tests"))]
pub(crate) async fn connect_mysql_test_from_env() -> Result<MySqlPool, AppError> {
    let input = ConnectionInput {
        id: None,
        name: "Disposable MySQL integration".into(),
        engine: "mysql".into(),
        host: std::env::var("DBSUAL_MYSQL_TEST_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
        port: std::env::var("DBSUAL_MYSQL_TEST_PORT")
            .ok()
            .and_then(|port| port.parse().ok())
            .unwrap_or(3306),
        user: std::env::var("DBSUAL_MYSQL_TEST_USER").unwrap_or_else(|_| "root".into()),
        password: Some(
            std::env::var("DBSUAL_MYSQL_TEST_PASSWORD")
                .expect("DBSUAL_MYSQL_TEST_PASSWORD must be set for this ignored test"),
        ),
        tls_mode: "disabled".into(),
        tls_ca_path: None,
        tls_client_cert_path: None,
        tls_client_key_path: None,
        ssh_enabled: Some(false),
        ssh_host: None,
        ssh_port: None,
        ssh_user: None,
        ssh_auth_method: None,
        ssh_key_path: None,
        ssh_agent_pipe: None,
        ssh_password: None,
        ssh_key_passphrase: None,
    };
    input.validate()?;
    let password = input.password.clone();
    let saved = input.saved(Uuid::new_v4().to_string(), None);
    connect(&saved, password.as_deref(), None).await
}

#[cfg(test)]
pub(crate) async fn connect_mariadb_test_from_env() -> Result<MySqlPool, AppError> {
    let input = ConnectionInput {
        id: None,
        name: "Disposable MariaDB integration".into(),
        engine: "mariadb".into(),
        host: std::env::var("DBSUAL_MARIADB_TEST_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
        port: std::env::var("DBSUAL_MARIADB_TEST_PORT")
            .ok()
            .and_then(|port| port.parse().ok())
            .unwrap_or(3306),
        user: std::env::var("DBSUAL_MARIADB_TEST_USER").unwrap_or_else(|_| "root".into()),
        password: Some(
            std::env::var("DBSUAL_MARIADB_TEST_PASSWORD")
                .expect("DBSUAL_MARIADB_TEST_PASSWORD must be set for this ignored test"),
        ),
        tls_mode: "disabled".into(),
        tls_ca_path: None,
        tls_client_cert_path: None,
        tls_client_key_path: None,
        ssh_enabled: Some(false),
        ssh_host: None,
        ssh_port: None,
        ssh_user: None,
        ssh_auth_method: None,
        ssh_key_path: None,
        ssh_agent_pipe: None,
        ssh_password: None,
        ssh_key_passphrase: None,
    };
    input.validate()?;
    let password = input.password.clone();
    let saved = input.saved(Uuid::new_v4().to_string(), None);
    connect(&saved, password.as_deref(), None).await
}

fn ssh_error(error: ssh::SshError) -> AppError {
    match error {
        ssh::SshError::UnknownHost {
            fingerprint,
            public_key,
        } => AppError::ssh_host_key(fingerprint, Some(public_key), false),
        ssh::SshError::ChangedHost {
            fingerprint,
            public_key,
        } => {
            AppError::ssh_host_key(fingerprint, Some(public_key), true)
        }
        ssh::SshError::AuthenticationRejected => AppError::new(
            "SSH_AUTH_REJECTED",
            "El servidor SSH rechazó las credenciales configuradas.",
        ),
        ssh::SshError::AgentUnavailable => AppError::new(
            "SSH_AGENT_UNAVAILABLE",
            "No se pudo acceder al agente OpenSSH de Windows.",
        ),
        ssh::SshError::AgentIdentityRejected => AppError::new(
            "SSH_AGENT_IDENTITY_REJECTED",
            "El agente OpenSSH no ofrece una identidad aceptada por el servidor.",
        ),
        ssh::SshError::Failed => AppError::new(
            "SSH_TUNNEL_FAILED",
            "No se pudo establecer el túnel SSH. Revisa el servidor, la autenticación y la clave de host.",
        ),
    }
}

fn merged_secrets(
    previous: &ConnectionSecrets,
    input: &ConnectionInput,
    saved: &SavedConnection,
) -> ConnectionSecrets {
    let mut merged = ConnectionSecrets {
        version: Some(1),
        db_password: previous.db_password.clone(),
        ssh_password: previous.ssh_password.clone(),
        ssh_key_passphrase: previous.ssh_key_passphrase.clone(),
    };
    if let Some(password) = input.password.as_ref().filter(|value| !value.is_empty()) {
        merged.db_password = Some(password.clone());
    }
    if !saved.ssh_enabled {
        merged.ssh_password = None;
        merged.ssh_key_passphrase = None;
    } else {
        if let Some(password) = input
            .ssh_password
            .as_ref()
            .filter(|value| !value.is_empty())
        {
            merged.ssh_password = Some(password.clone());
        }
        if let Some(passphrase) = input
            .ssh_key_passphrase
            .as_ref()
            .filter(|value| !value.is_empty())
        {
            merged.ssh_key_passphrase = Some(passphrase.clone());
        }
        match saved.ssh_auth_method.as_deref() {
            Some("password") => merged.ssh_key_passphrase = None,
            Some("privateKey") => merged.ssh_password = None,
            Some("agent") | None => {
                merged.ssh_password = None;
                merged.ssh_key_passphrase = None;
            }
            _ => {}
        }
    }
    merged
}

fn ssh_auth(
    saved: &SavedConnection,
    secrets: &ConnectionSecrets,
) -> Result<ssh::SshAuth, AppError> {
    match saved.ssh_auth_method.as_deref() {
        Some("password") => secrets
            .ssh_password
            .as_ref()
            .map(|secret| ssh::SshAuth::Password(secret.clone()))
            .ok_or(AppError::new(
                "SSH_CREDENTIAL_MISSING",
                "Guarda la contraseña SSH para esta conexión.",
            )),
        Some("privateKey") => {
            let path = saved.ssh_key_path.as_ref().ok_or(AppError::new(
                "INVALID_CONNECTION",
                "Indica la ruta de la clave privada SSH.",
            ))?;
            if !Path::new(path).is_file() {
                return Err(AppError::new(
                    "SSH_KEY_MISSING",
                    "No se encontró el archivo de clave privada SSH indicado.",
                ));
            }
            Ok(ssh::SshAuth::PrivateKey {
                path: path.into(),
                passphrase: secrets.ssh_key_passphrase.clone(),
            })
        }
        Some("agent") => Ok(ssh::SshAuth::WindowsAgent {
            pipe: saved
                .ssh_agent_pipe
                .as_deref()
                .unwrap_or(r"\\.\pipe\openssh-ssh-agent")
                .into(),
        }),
        _ => Err(AppError::new(
            "INVALID_CONNECTION",
            "El método de autenticación SSH guardado no es válido.",
        )),
    }
}

async fn open_ssh_tunnel(
    state: &AppState,
    saved: &SavedConnection,
    secrets: &ConnectionSecrets,
) -> Result<Option<ssh::SshTunnel>, AppError> {
    if !saved.ssh_enabled {
        return Ok(None);
    }
    let endpoint = ssh::SshEndpoint {
        host: saved
            .ssh_host
            .clone()
            .ok_or(AppError::new("INVALID_CONNECTION", "Falta el host SSH."))?,
        port: saved
            .ssh_port
            .and_then(|port| u16::try_from(port).ok())
            .filter(|port| *port != 0)
            .ok_or(AppError::new(
                "INVALID_CONNECTION",
                "El puerto SSH guardado no es válido.",
            ))?,
        username: saved
            .ssh_user
            .clone()
            .ok_or(AppError::new("INVALID_CONNECTION", "Falta el usuario SSH."))?,
        database_host: saved.host.clone(),
        database_port: saved.port as u16,
        known_hosts_path: state.ssh_known_hosts_path.clone(),
    };
    let auth = ssh_auth(saved, secrets)?;
    let tls = if saved.tls_mode == "verifyIdentity" {
        ssh::DatabaseTls::Enabled
    } else {
        ssh::DatabaseTls::Disabled
    };
    ssh::open_tunnel(endpoint, auth, tls)
        .await
        .map(Some)
        .map_err(ssh_error)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    server_version: String,
    tls_active: bool,
}

async fn server_info(pool: &MySqlPool, engine: &str) -> Result<TestResult, AppError> {
    let version: String = sqlx::query_scalar("SELECT VERSION()")
        .fetch_one(pool)
        .await
        .map_err(|error| connection_error(error, engine))?;
    let row = sqlx::query("SHOW STATUS LIKE 'Ssl_cipher'")
        .fetch_optional(pool)
        .await
        .map_err(|error| connection_error(error, engine))?;
    let tls_active = row
        .and_then(|r| r.try_get::<String, _>(1).ok())
        .is_some_and(|v| !v.is_empty());
    Ok(TestResult {
        server_version: version,
        tls_active,
    })
}

#[tauri::command]
pub async fn list_connections(
    state: State<'_, AppState>,
) -> Result<Vec<SavedConnection>, AppError> {
    all(state.pool()?).await
}

#[tauri::command]
pub async fn save_connection(
    state: State<'_, AppState>,
    input: ConnectionInput,
) -> Result<SavedConnection, AppError> {
    input.validate()?;
    let sqlite_path = if input.engine == "sqlite" {
        let path = Path::new(&input.host).canonicalize().map_err(|_| {
            AppError::new(
                "SQLITE_FILE_INVALID",
                "Selecciona un archivo SQLite existente y accesible.",
            )
        })?;
        if !path.is_file() {
            return Err(AppError::new(
                "SQLITE_FILE_INVALID",
                "Selecciona un archivo SQLite existente y accesible.",
            ));
        }
        let probe = crate::sqlite_adapter::open_readonly(&path).await?;
        probe.close().await;
        Some(path)
    } else {
        None
    };
    let pool = state.pool()?;
    let id = input
        .id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let previous = if input.id.is_some() {
        Some(one(pool, &id).await?)
    } else {
        None
    };
    let mut saved = input.saved(id.clone(), previous.as_ref());
    if let Some(path) = sqlite_path {
        saved.host = path.to_string_lossy().into_owned();
    }
    validate_resolved_ssh(&saved)?;
    let secret_values_supplied = input.password.as_ref().is_some_and(|p| !p.is_empty())
        || input.ssh_password.as_ref().is_some_and(|p| !p.is_empty())
        || input
            .ssh_key_passphrase
            .as_ref()
            .is_some_and(|p| !p.is_empty());
    let ssh_secret_shape_changed = previous.as_ref().is_some_and(|old| {
        old.ssh_enabled != saved.ssh_enabled
            || (saved.ssh_enabled && old.ssh_auth_method != saved.ssh_auth_method)
    });
    let old_secrets = if secret_values_supplied || ssh_secret_shape_changed {
        read_secrets(id.clone()).await?
    } else {
        ConnectionSecrets::default()
    };
    let new_secrets = merged_secrets(&old_secrets, &input, &saved);
    let changed_secrets = old_secrets.db_password != new_secrets.db_password
        || old_secrets.ssh_password != new_secrets.ssh_password
        || old_secrets.ssh_key_passphrase != new_secrets.ssh_key_passphrase;
    if changed_secrets {
        write_secrets(id.clone(), new_secrets).await?;
    }
    let result = sqlx::query("INSERT INTO connections (id,name,engine,host,port,user_name,tls_mode,tls_ca_path,tls_client_cert_path,tls_client_key_path,ssh_enabled,ssh_host,ssh_port,ssh_user,ssh_auth_method,ssh_key_path,ssh_agent_pipe) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,engine=excluded.engine,host=excluded.host,port=excluded.port,user_name=excluded.user_name,tls_mode=excluded.tls_mode,tls_ca_path=excluded.tls_ca_path,tls_client_cert_path=excluded.tls_client_cert_path,tls_client_key_path=excluded.tls_client_key_path,ssh_enabled=excluded.ssh_enabled,ssh_host=excluded.ssh_host,ssh_port=excluded.ssh_port,ssh_user=excluded.ssh_user,ssh_auth_method=excluded.ssh_auth_method,ssh_key_path=excluded.ssh_key_path,ssh_agent_pipe=excluded.ssh_agent_pipe")
        .bind(&saved.id).bind(&saved.name).bind(&saved.engine).bind(&saved.host).bind(saved.port).bind(&saved.user).bind(&saved.tls_mode)
        .bind(&saved.tls_ca_path).bind(&saved.tls_client_cert_path).bind(&saved.tls_client_key_path)
        .bind(saved.ssh_enabled).bind(&saved.ssh_host).bind(saved.ssh_port).bind(&saved.ssh_user).bind(&saved.ssh_auth_method).bind(&saved.ssh_key_path).bind(&saved.ssh_agent_pipe).execute(pool).await;
    if let Err(error) = result {
        if changed_secrets {
            write_secrets(id, old_secrets).await?;
        }
        return if error
            .as_database_error()
            .is_some_and(|e| e.is_unique_violation())
        {
            Err(AppError::new(
                "DUPLICATE_NAME",
                "Ya existe una conexión con ese nombre.",
            ))
        } else {
            Err(AppError::storage())
        };
    }
    if let Some(old) = state.active.0.lock().await.remove(&saved.id) {
        old.close().await;
    }
    if let Some(old) = state.active_postgres.lock().await.remove(&saved.id) {
        old.pool().close().await;
    }
    if let Some(old) = state.tunnels.0.lock().await.remove(&saved.id) {
        old.close().await;
    }
    Ok(saved)
}

/// Records the SSH key only after the UI explicitly invokes this command from
/// its approval action. Existing keys are never replaced by this flow.
#[tauri::command]
pub async fn approve_ssh_host_key(
    state: State<'_, AppState>,
    connection_id: String,
    fingerprint: String,
    public_key: String,
) -> Result<(), AppError> {
    if Uuid::parse_str(&connection_id).is_err() {
        return Err(AppError::new(
            "INVALID_CONNECTION",
            "El identificador de conexión no es válido.",
        ));
    }
    let saved = one(state.pool()?, &connection_id).await?;
    if !saved.ssh_enabled {
        return Err(AppError::new(
            "INVALID_CONNECTION",
            "La conexión no tiene un túnel SSH activo.",
        ));
    }
    let host = saved
        .ssh_host
        .as_deref()
        .ok_or(AppError::new("INVALID_CONNECTION", "Falta el host SSH."))?;
    let port = saved
        .ssh_port
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port != 0)
        .ok_or(AppError::new(
            "INVALID_CONNECTION",
            "El puerto SSH guardado no es válido.",
        ))?;
    let candidate = russh::keys::ssh_key::PublicKey::from_openssh(&public_key).map_err(|_| {
        AppError::new(
            "INVALID_SSH_HOST_KEY",
            "La clave SSH recibida no tiene un formato OpenSSH válido.",
        )
    })?;
    let actual_fingerprint = candidate
        .fingerprint(russh::keys::ssh_key::HashAlg::Sha256)
        .to_string();
    if actual_fingerprint != fingerprint {
        return Err(AppError::new(
            "SSH_HOST_KEY_CHALLENGE_MISMATCH",
            "La clave aprobada no coincide con la huella que mostró el diálogo.",
        ));
    }
    let recorded = ssh::record_known_host(&state.ssh_known_hosts_path, host, port, &public_key)
        .map_err(ssh_error)?;
    if recorded != actual_fingerprint {
        return Err(AppError::new(
            "SSH_HOST_KEY_CHALLENGE_MISMATCH",
            "La clave aprobada no coincide con la huella que mostró el diálogo.",
        ));
    }
    Ok(())
}

#[tauri::command]
pub async fn test_connection(
    state: State<'_, AppState>,
    input: ConnectionInput,
) -> Result<TestResult, AppError> {
    input.validate()?;
    let previous = match input.id.as_deref() {
        Some(id) => Some(one(state.pool()?, id).await?),
        None => None,
    };
    let saved = input.saved(input.id.clone().unwrap_or_default(), previous.as_ref());
    if saved.engine == "sqlite" {
        let pool = crate::sqlite_adapter::open_readonly(Path::new(&saved.host)).await?;
        let version: String = sqlx::query_scalar("SELECT sqlite_version()")
            .fetch_one(&pool)
            .await
            .map_err(|_| {
                AppError::new(
                    "SQLITE_OPEN_FAILED",
                    "No se pudo consultar el archivo SQLite.",
                )
            })?;
        pool.close().await;
        return Ok(TestResult {
            server_version: version,
            tls_active: false,
        });
    }
    validate_resolved_ssh(&saved)?;
    let stored_secrets = match input.id.as_ref() {
        Some(id) => read_secrets(id.clone()).await?,
        None => ConnectionSecrets::default(),
    };
    let secrets = merged_secrets(&stored_secrets, &input, &saved);
    if saved.engine == "postgresql" {
        let connection = crate::postgres::connect(crate::postgres::PostgresConnectConfig {
            host: saved.host.clone(),
            port: saved.port as u16,
            username: saved.user.clone(),
            password: secrets.db_password,
            database: None,
            tls_mode: if saved.tls_mode == "verifyIdentity" {
                crate::postgres::PostgresTlsMode::VerifyIdentity
            } else {
                crate::postgres::PostgresTlsMode::Disabled
            },
            tls_ca_path: saved.tls_ca_path.as_deref().map(Into::into),
        })
        .await
        .map_err(postgres_error)?;
        let result = TestResult {
            server_version: connection.server_version().to_owned(),
            tls_active: connection.tls_active(),
        };
        connection.pool().close().await;
        return Ok(result);
    }
    let mut tunnel = open_ssh_tunnel(&state, &saved, &secrets).await?;
    let pool = match connect(
        &saved,
        secrets.db_password.as_deref(),
        tunnel.as_ref().map(ssh::SshTunnel::local_port),
    )
    .await
    {
        Ok(pool) => pool,
        Err(error) => {
            if let Some(tunnel) = tunnel.take() {
                tunnel.close().await;
            }
            return Err(error);
        }
    };
    let info = server_info(&pool, &saved.engine).await;
    pool.close().await;
    if let Some(tunnel) = tunnel.take() {
        tunnel.close().await;
    }
    let info = info?;
    validate_server_engine(&saved.engine, &info.server_version)?;
    if saved.tls_mode == "verifyIdentity" && !info.tls_active {
        return Err(AppError::new(
            "TLS_REQUIRED",
            tls_error_message(&saved.engine),
        ));
    }
    Ok(info)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    id: String,
    state: &'static str,
    server_version: Option<String>,
    tls_active: Option<bool>,
}

#[tauri::command]
pub async fn open_connection(
    state: State<'_, AppState>,
    id: String,
) -> Result<ConnectionStatus, AppError> {
    let saved = one(state.pool()?, &id).await?;
    if saved.engine == "sqlite" {
        let pool = crate::sqlite_adapter::open_readonly(Path::new(&saved.host)).await?;
        let version: String = sqlx::query_scalar("SELECT sqlite_version()")
            .fetch_one(&pool)
            .await
            .map_err(|_| {
                AppError::new(
                    "SQLITE_OPEN_FAILED",
                    "No se pudo consultar el archivo SQLite.",
                )
            })?;
        if let Some(old) = state.active_sqlite.lock().await.insert(id.clone(), pool) {
            old.close().await;
        }
        if let Some(old) = state.active.0.lock().await.remove(&id) {
            old.close().await;
        }
        if let Some(old) = state.active_postgres.lock().await.remove(&id) {
            old.pool().close().await;
        }
        return Ok(ConnectionStatus {
            id,
            state: "connected",
            server_version: Some(version),
            tls_active: Some(false),
        });
    }
    let secrets = read_secrets(id.clone()).await?;
    if saved.engine == "postgresql" {
        let connection = crate::postgres::connect(crate::postgres::PostgresConnectConfig {
            host: saved.host.clone(),
            port: saved.port as u16,
            username: saved.user.clone(),
            password: secrets.db_password,
            database: None,
            tls_mode: if saved.tls_mode == "verifyIdentity" {
                crate::postgres::PostgresTlsMode::VerifyIdentity
            } else {
                crate::postgres::PostgresTlsMode::Disabled
            },
            tls_ca_path: saved.tls_ca_path.as_deref().map(Into::into),
        })
        .await
        .map_err(postgres_error)?;
        let status = ConnectionStatus {
            id: id.clone(),
            state: "connected",
            server_version: Some(connection.server_version().to_owned()),
            tls_active: Some(connection.tls_active()),
        };
        if let Some(old) = state
            .active_postgres
            .lock()
            .await
            .insert(id.clone(), connection)
        {
            old.pool().close().await;
        }
        if let Some(old) = state.active.0.lock().await.remove(&id) {
            old.close().await;
        }
        return Ok(status);
    }
    let mut tunnel = open_ssh_tunnel(&state, &saved, &secrets).await?;
    let pool = match connect(
        &saved,
        secrets.db_password.as_deref(),
        tunnel.as_ref().map(ssh::SshTunnel::local_port),
    )
    .await
    {
        Ok(pool) => pool,
        Err(error) => {
            if let Some(tunnel) = tunnel.take() {
                tunnel.close().await;
            }
            return Err(error);
        }
    };
    let info = match server_info(&pool, &saved.engine).await {
        Ok(info) => info,
        Err(error) => {
            pool.close().await;
            if let Some(tunnel) = tunnel.take() {
                tunnel.close().await;
            }
            return Err(error);
        }
    };
    if let Err(error) = validate_server_engine(&saved.engine, &info.server_version) {
        pool.close().await;
        if let Some(tunnel) = tunnel.take() {
            tunnel.close().await;
        }
        return Err(error);
    }
    if saved.tls_mode == "verifyIdentity" && !info.tls_active {
        pool.close().await;
        if let Some(tunnel) = tunnel.take() {
            tunnel.close().await;
        }
        return Err(AppError::new(
            "TLS_REQUIRED",
            tls_error_message(&saved.engine),
        ));
    }
    let old = state.active.0.lock().await.insert(id.clone(), pool);
    if let Some(old) = old {
        old.close().await;
    }
    let old_tunnel = if let Some(tunnel) = tunnel.take() {
        state.tunnels.0.lock().await.insert(id.clone(), tunnel)
    } else {
        state.tunnels.0.lock().await.remove(&id)
    };
    if let Some(old_tunnel) = old_tunnel {
        old_tunnel.close().await;
    }
    Ok(ConnectionStatus {
        id,
        state: "connected",
        server_version: Some(info.server_version),
        tls_active: Some(info.tls_active),
    })
}

#[tauri::command]
pub async fn disconnect_connection(
    state: State<'_, AppState>,
    id: String,
) -> Result<ConnectionStatus, AppError> {
    if let Some(connection) = state.active_postgres.lock().await.remove(&id) {
        connection.pool().close().await;
    }
    if let Some(pool) = state.active_sqlite.lock().await.remove(&id) {
        pool.close().await;
    }
    if let Some(pool) = state.active.0.lock().await.remove(&id) {
        crate::sql_editor::cancel_connection_read_queries(
            pool.clone(),
            std::sync::Arc::clone(&state.active_read_queries),
            &id,
        )
        .await;
        pool.close().await;
    }
    if let Some(tunnel) = state.tunnels.0.lock().await.remove(&id) {
        tunnel.close().await;
    }
    Ok(ConnectionStatus {
        id,
        state: "disconnected",
        server_version: None,
        tls_active: None,
    })
}

#[tauri::command]
pub async fn remove_connection(state: State<'_, AppState>, id: String) -> Result<(), AppError> {
    one(state.pool()?, &id).await?;
    let previous = read_secrets(id.clone()).await?;
    write_secrets(id.clone(), ConnectionSecrets::default()).await?;
    if sqlx::query("DELETE FROM connections WHERE id=?")
        .bind(&id)
        .execute(state.pool()?)
        .await
        .is_err()
    {
        write_secrets(id, previous).await?;
        return Err(AppError::storage());
    }
    if let Some(pool) = state.active.0.lock().await.remove(&id) {
        crate::sql_editor::cancel_connection_read_queries(
            pool.clone(),
            std::sync::Arc::clone(&state.active_read_queries),
            &id,
        )
        .await;
        pool.close().await;
    }
    if let Some(connection) = state.active_postgres.lock().await.remove(&id) {
        connection.pool().close().await;
    }
    if let Some(pool) = state.active_sqlite.lock().await.remove(&id) {
        pool.close().await;
    }
    if let Some(tunnel) = state.tunnels.0.lock().await.remove(&id) {
        tunnel.close().await;
    }
    Ok(())
}

async fn active_pool(state: &AppState, id: &str) -> Result<MySqlPool, AppError> {
    state
        .active
        .0
        .lock()
        .await
        .get(id)
        .cloned()
        .ok_or(AppError::new(
            "CONNECTION_CLOSED",
            "Abre la conexión para explorarla.",
        ))
}

#[tauri::command]
pub async fn list_databases(
    state: State<'_, AppState>,
    connection_id: String,
) -> Result<Vec<String>, AppError> {
    let saved = one(state.pool()?, &connection_id).await?;
    if saved.engine == "sqlite" {
        let pool = state
            .active_sqlite
            .lock()
            .await
            .get(&connection_id)
            .cloned()
            .ok_or(AppError::new(
                "CONNECTION_CLOSED",
                "Abre la conexión para explorarla.",
            ))?;
        return crate::sqlite_adapter::databases(&pool).await;
    }
    if saved.engine == "postgresql" {
        if !state
            .active_postgres
            .lock()
            .await
            .contains_key(&connection_id)
        {
            return Err(AppError::new(
                "NOT_CONNECTED",
                "Abre la conexión PostgreSQL antes de explorarla.",
            ));
        }
        let active = state.active_postgres.lock().await;
        let connection = active.get(&connection_id).ok_or(AppError::new(
            "CONNECTION_CLOSED",
            "Abre la conexión para explorarla.",
        ))?;
        return connection
            .list_databases()
            .await
            .map(|databases| {
                databases
                    .into_iter()
                    .map(|database| database.name)
                    .collect()
            })
            .map_err(postgres_error);
    }
    let pool = active_pool(&state, &connection_id).await?;
    sqlx::query_scalar("SHOW DATABASES")
        .fetch_all(&pool)
        .await
        .map_err(|_| {
            AppError::new(
                "METADATA_FAILED",
                "No se pudieron listar las bases de datos.",
            )
        })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerProcess {
    id: u64,
    user: Option<String>,
    host: Option<String>,
    database: Option<String>,
    command: Option<String>,
    state: Option<String>,
    duration_seconds: i64,
}

#[tauri::command]
pub async fn list_server_processes(
    state: State<'_, AppState>,
    connection_id: String,
) -> Result<Vec<ServerProcess>, AppError> {
    let pool = active_pool(&state, &connection_id).await?;
    server_process_rows(&pool).await.map_err(|_| {
        AppError::new(
            "SERVER_METADATA_FAILED",
            "No se pudieron consultar los procesos visibles para esta cuenta.",
        )
    })
}

async fn server_process_rows(pool: &MySqlPool) -> Result<Vec<ServerProcess>, sqlx::Error> {
    let rows: Vec<(u64, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, i64)> = sqlx::query_as(
        "SELECT CAST(ID AS UNSIGNED),USER,HOST,DB,COMMAND,STATE,TIME FROM information_schema.PROCESSLIST ORDER BY ID LIMIT 500",
    ).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, user, host, database, command, state, duration_seconds)| ServerProcess {
                id,
                user,
                host,
                database,
                command,
                state,
                duration_seconds,
            },
        )
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisibleUser {
    name: String,
    host: String,
    authentication_plugin: Option<String>,
    locked: Option<bool>,
}

#[tauri::command]
pub async fn list_server_users(
    state: State<'_, AppState>,
    connection_id: String,
) -> Result<Vec<VisibleUser>, AppError> {
    let saved = one(state.pool()?, &connection_id).await?;
    let pool = active_pool(&state, &connection_id).await?;
    server_user_rows(&pool, &saved.engine).await.map_err(|_| {
        AppError::new(
            "SERVER_METADATA_FAILED",
            "No se pudieron consultar las cuentas visibles; esta cuenta puede carecer de permiso.",
        )
    })
}

async fn server_user_rows(pool: &MySqlPool, engine: &str) -> Result<Vec<VisibleUser>, sqlx::Error> {
    let lock_state = if engine == "mariadb" {
        "CAST(NULL AS CHAR)"
    } else {
        "CAST(account_locked AS CHAR)"
    };
    let rows: Vec<(String, String, Option<String>, Option<String>)> = sqlx::query_as(&format!(
        "SELECT CAST(User AS CHAR),CAST(Host AS CHAR),CAST(plugin AS CHAR),{lock_state} FROM mysql.user ORDER BY User,Host LIMIT 500"
    ))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(name, host, authentication_plugin, locked)| VisibleUser {
            name,
            host,
            authentication_plugin,
            locked: locked.as_deref().map(|value| value == "Y"),
        })
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerVariable {
    name: String,
    value: String,
    scope: &'static str,
}

fn sensitive_variable(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "password",
        "secret",
        "token",
        "private_key",
        "ssl_key",
        "key_file",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}

#[tauri::command]
pub async fn list_server_variables(
    state: State<'_, AppState>,
    connection_id: String,
) -> Result<Vec<ServerVariable>, AppError> {
    let pool = active_pool(&state, &connection_id).await?;
    server_variable_rows(&pool).await.map_err(|_| {
        AppError::new(
            "SERVER_METADATA_FAILED",
            "No se pudieron consultar las variables globales visibles.",
        )
    })
}

async fn server_variable_rows(pool: &MySqlPool) -> Result<Vec<ServerVariable>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as("SHOW GLOBAL VARIABLES")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .take(1000)
        .map(|(name, value)| ServerVariable {
            value: if sensitive_variable(&name) {
                "Oculto".into()
            } else {
                value
            },
            name,
            scope: "global",
        })
        .collect())
}

#[derive(Serialize)]
pub struct DatabaseObject {
    name: String,
    kind: &'static str,
    schema: Option<String>,
}

#[tauri::command]
pub async fn list_database_objects(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
) -> Result<Vec<DatabaseObject>, AppError> {
    let saved = one(state.pool()?, &connection_id).await?;
    if saved.engine == "sqlite" {
        let pool = state
            .active_sqlite
            .lock()
            .await
            .get(&connection_id)
            .cloned()
            .ok_or(AppError::new(
                "CONNECTION_CLOSED",
                "Abre la conexión para explorarla.",
            ))?;
        return Ok(crate::sqlite_adapter::objects(&pool, &database)
            .await?
            .into_iter()
            .map(|(name, kind)| DatabaseObject {
                name,
                kind: if kind == "view" { "view" } else { "table" },
                schema: None,
            })
            .collect());
    }
    if saved.engine == "postgresql" {
        if !state
            .active_postgres
            .lock()
            .await
            .contains_key(&connection_id)
        {
            return Err(AppError::new(
                "NOT_CONNECTED",
                "Abre la conexión PostgreSQL antes de explorarla.",
            ));
        }
        let secrets = read_secrets(connection_id.clone()).await?;
        let connection = crate::postgres::connect(crate::postgres::PostgresConnectConfig {
            host: saved.host,
            port: saved.port as u16,
            username: saved.user,
            password: secrets.db_password,
            database: Some(database),
            tls_mode: if saved.tls_mode == "verifyIdentity" {
                crate::postgres::PostgresTlsMode::VerifyIdentity
            } else {
                crate::postgres::PostgresTlsMode::Disabled
            },
            tls_ca_path: saved.tls_ca_path.as_deref().map(Into::into),
        })
        .await
        .map_err(postgres_error)?;
        let result = connection.list_objects().await.map_err(postgres_error)?;
        connection.pool().close().await;
        return Ok(result
            .into_iter()
            .map(|item| DatabaseObject {
                name: item.name,
                kind: if item.kind == "view" { "view" } else { "table" },
                schema: Some(item.schema),
            })
            .collect());
    }
    let pool = active_pool(&state, &connection_id).await?;
    let rows = database_object_rows(&pool, &database)
        .await
        .map_err(|_| AppError::new("METADATA_FAILED", "No se pudieron listar tablas y vistas."))?;
    Ok(rows
        .into_iter()
        .map(|(name, kind)| DatabaseObject {
            name,
            kind: if kind == "VIEW" { "view" } else { "table" },
            schema: None,
        })
        .collect())
}

async fn database_object_rows(
    pool: &MySqlPool,
    database: &str,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    sqlx::query_as("SELECT CAST(TABLE_NAME AS CHAR),CAST(TABLE_TYPE AS CHAR) FROM information_schema.TABLES WHERE TABLE_SCHEMA=? ORDER BY TABLE_NAME")
        .bind(database)
        .fetch_all(pool)
        .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    name: String,
    data_type: String,
    is_nullable: bool,
    is_primary_key: bool,
    primary_key_available: bool,
    default_value: Option<String>,
    default_available: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableStructure {
    indexes: Vec<TableIndexInfo>,
    constraints: Vec<TableConstraintInfo>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableIndexInfo {
    name: String,
    unique: bool,
    index_type: String,
    columns: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableConstraintInfo {
    name: String,
    kind: String,
    columns: Vec<String>,
    referenced_database: Option<String>,
    referenced_table: Option<String>,
    referenced_columns: Vec<String>,
}

#[tauri::command]
pub async fn list_columns(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    object_name: String,
    object_type: String,
    object_schema: Option<String>,
) -> Result<Vec<ColumnInfo>, AppError> {
    if !matches!(object_type.as_str(), "table" | "view") {
        return Err(AppError::new(
            "INVALID_OBJECT",
            "El tipo de objeto no es válido.",
        ));
    }
    let saved = one(state.pool()?, &connection_id).await?;
    if saved.engine == "sqlite" {
        let pool = state
            .active_sqlite
            .lock()
            .await
            .get(&connection_id)
            .cloned()
            .ok_or(AppError::new(
                "CONNECTION_CLOSED",
                "Abre la conexión para explorarla.",
            ))?;
        let columns =
            crate::sqlite_adapter::columns(&pool, &database, &object_name, &object_type).await?;
        return Ok(columns
            .into_iter()
            .map(|column| ColumnInfo {
                name: column.name,
                data_type: column.data_type,
                is_nullable: column.is_nullable,
                is_primary_key: column.is_primary_key,
                primary_key_available: column.primary_key_available,
                default_value: column.default_value,
                default_available: column.default_available,
            })
            .collect());
    }
    if saved.engine == "postgresql" {
        let schema = object_schema
            .filter(|schema| !schema.is_empty())
            .ok_or_else(|| {
                AppError::new("INVALID_OBJECT", "Falta el esquema del objeto PostgreSQL.")
            })?;
        let secrets = read_secrets(connection_id.clone()).await?;
        let connection = crate::postgres::connect(crate::postgres::PostgresConnectConfig {
            host: saved.host,
            port: saved.port as u16,
            username: saved.user,
            password: secrets.db_password,
            database: Some(database),
            tls_mode: if saved.tls_mode == "verifyIdentity" {
                crate::postgres::PostgresTlsMode::VerifyIdentity
            } else {
                crate::postgres::PostgresTlsMode::Disabled
            },
            tls_ca_path: saved.tls_ca_path.as_deref().map(Into::into),
        })
        .await
        .map_err(postgres_error)?;
        let result = connection
            .list_columns(&schema, &object_name, object_type == "view")
            .await
            .map_err(postgres_error)?;
        connection.pool().close().await;
        if result.is_empty() {
            return Err(AppError::new(
                "OBJECT_NOT_FOUND",
                "La tabla o vista ya no existe o no es visible.",
            ));
        }
        return Ok(result
            .into_iter()
            .map(|column| ColumnInfo {
                name: column.name,
                data_type: column.data_type,
                is_nullable: column.nullable,
                is_primary_key: column.primary_key,
                primary_key_available: column.primary_key_available,
                default_value: column.default_value,
                default_available: column.default_available,
            })
            .collect());
    }
    let pool = active_pool(&state, &connection_id).await?;
    let rows = column_metadata_rows(&pool, &database, &object_name)
        .await
        .map_err(|_| AppError::new("METADATA_FAILED", "No se pudieron listar las columnas."))?;
    if rows.is_empty() {
        return Err(AppError::new(
            "OBJECT_NOT_FOUND",
            "La tabla o vista ya no existe o no es visible.",
        ));
    }
    let actual_kind = if rows[0].5 == "VIEW" { "view" } else { "table" };
    if actual_kind != object_type {
        return Err(AppError::new(
            "OBJECT_CHANGED",
            "El objeto cambió desde la última actualización. Actualiza el explorador.",
        ));
    }
    Ok(rows
        .into_iter()
        .map(
            |(name, data_type, nullable, key, default_value, table_type)| {
                let table_metadata = table_type == "BASE TABLE";
                ColumnInfo {
                    name,
                    data_type,
                    is_nullable: nullable == "YES",
                    is_primary_key: key == "PRI",
                    primary_key_available: table_metadata,
                    default_value,
                    default_available: table_metadata,
                }
            },
        )
        .collect())
}

#[tauri::command]
pub async fn get_table_structure(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    table: String,
) -> Result<TableStructure, AppError> {
    if connection_id.is_empty()
        || connection_id.len() > 128
        || database.is_empty()
        || database.len() > 255
        || table.is_empty()
        || table.len() > 255
        || database.chars().any(char::is_control)
        || table.chars().any(char::is_control)
    {
        return Err(AppError::new(
            "INVALID_OBJECT",
            "La tabla seleccionada no es válida.",
        ));
    }
    let pool = active_pool(&state, &connection_id).await?;
    table_structure(&pool, &database, &table)
        .await
        .map_err(|error| match error {
            TableStructureError::NotFound => {
                AppError::new("OBJECT_NOT_FOUND", "La tabla ya no existe o no es visible.")
            }
            TableStructureError::NotTable => AppError::new(
                "INVALID_OBJECT_TYPE",
                "La estructura detallada solo está disponible para tablas MySQL.",
            ),
            TableStructureError::Database => AppError::new(
                "METADATA_FAILED",
                "No se pudo consultar la estructura de la tabla.",
            ),
        })
}

#[derive(Debug, PartialEq, Eq)]
enum TableStructureError {
    NotFound,
    NotTable,
    Database,
}

async fn table_structure(
    pool: &MySqlPool,
    database: &str,
    table: &str,
) -> Result<TableStructure, TableStructureError> {
    let table_type: Option<String> = sqlx::query_scalar(
        "SELECT CAST(TABLE_TYPE AS CHAR) FROM information_schema.TABLES WHERE TABLE_SCHEMA=? AND TABLE_NAME=?",
    )
    .bind(database)
    .bind(table)
    .fetch_optional(pool)
    .await
    .map_err(|_| TableStructureError::Database)?;
    match table_type.as_deref() {
        None => return Err(TableStructureError::NotFound),
        Some("BASE TABLE") => {}
        Some(_) => return Err(TableStructureError::NotTable),
    }

    let index_rows: Vec<(String, i64, String, i64, String)> = sqlx::query_as(
        "SELECT CAST(INDEX_NAME AS CHAR), NON_UNIQUE, CAST(COLUMN_NAME AS CHAR), CAST(SEQ_IN_INDEX AS SIGNED), CAST(INDEX_TYPE AS CHAR) FROM information_schema.STATISTICS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY INDEX_NAME, SEQ_IN_INDEX",
    )
    .bind(database)
    .bind(table)
    .fetch_all(pool)
    .await
    .map_err(|_| TableStructureError::Database)?;
    let mut indexes: Vec<TableIndexInfo> = Vec::new();
    for (name, non_unique, column, _, index_type) in index_rows {
        if let Some(index) = indexes.last_mut().filter(|index| index.name == name) {
            index.columns.push(column);
        } else {
            indexes.push(TableIndexInfo {
                name,
                unique: non_unique == 0,
                index_type,
                columns: vec![column],
            });
        }
    }

    let constraint_rows: Vec<(
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT CAST(TC.CONSTRAINT_NAME AS CHAR), CAST(TC.CONSTRAINT_TYPE AS CHAR), CAST(KCU.COLUMN_NAME AS CHAR), CAST(KCU.REFERENCED_TABLE_SCHEMA AS CHAR), CAST(KCU.REFERENCED_TABLE_NAME AS CHAR), CAST(KCU.REFERENCED_COLUMN_NAME AS CHAR) FROM information_schema.TABLE_CONSTRAINTS TC LEFT JOIN information_schema.KEY_COLUMN_USAGE KCU ON KCU.CONSTRAINT_SCHEMA=TC.CONSTRAINT_SCHEMA AND KCU.TABLE_NAME=TC.TABLE_NAME AND KCU.CONSTRAINT_NAME=TC.CONSTRAINT_NAME WHERE TC.TABLE_SCHEMA=? AND TC.TABLE_NAME=? ORDER BY TC.CONSTRAINT_NAME, KCU.ORDINAL_POSITION",
    )
    .bind(database)
    .bind(table)
    .fetch_all(pool)
    .await
    .map_err(|_| TableStructureError::Database)?;
    let mut constraints: Vec<TableConstraintInfo> = Vec::new();
    for (name, kind, column, referenced_database, referenced_table, referenced_column) in
        constraint_rows
    {
        let constraint = if let Some(constraint) = constraints
            .iter_mut()
            .find(|constraint| constraint.name == name)
        {
            constraint
        } else {
            constraints.push(TableConstraintInfo {
                name,
                kind,
                columns: Vec::new(),
                referenced_database: None,
                referenced_table: None,
                referenced_columns: Vec::new(),
            });
            constraints.last_mut().expect("just inserted a constraint")
        };
        if let Some(column) = column {
            constraint.columns.push(column);
        }
        if let Some(referenced_database) = referenced_database {
            constraint.referenced_database = Some(referenced_database);
        }
        if let Some(referenced_table) = referenced_table {
            constraint.referenced_table = Some(referenced_table);
        }
        if let Some(referenced_column) = referenced_column {
            constraint.referenced_columns.push(referenced_column);
        }
    }

    Ok(TableStructure {
        indexes,
        constraints,
    })
}

async fn column_metadata_rows(
    pool: &MySqlPool,
    database: &str,
    object_name: &str,
) -> Result<Vec<(String, String, String, String, Option<String>, String)>, sqlx::Error> {
    sqlx::query_as("SELECT CAST(C.COLUMN_NAME AS CHAR),CAST(C.COLUMN_TYPE AS CHAR),CAST(C.IS_NULLABLE AS CHAR),CAST(C.COLUMN_KEY AS CHAR),CAST(C.COLUMN_DEFAULT AS CHAR),CAST(T.TABLE_TYPE AS CHAR) FROM information_schema.COLUMNS C JOIN information_schema.TABLES T ON T.TABLE_SCHEMA=C.TABLE_SCHEMA AND T.TABLE_NAME=C.TABLE_NAME WHERE C.TABLE_SCHEMA=? AND C.TABLE_NAME=? ORDER BY C.ORDINAL_POSITION")
        .bind(database)
        .bind(object_name)
        .fetch_all(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mariadb_engine_and_supported_version_are_verified() {
        for version in [
            "10.6.22-MariaDB",
            "10.11.13-MariaDB",
            "11.4.7-MariaDB",
            "5.5.5-10.11.13-MariaDB",
        ] {
            assert!(
                validate_server_engine("mariadb", version).is_ok(),
                "{version}"
            );
        }
        for version in ["10.5.28-MariaDB", "11.8.1-MariaDB", "8.4.11"] {
            assert!(
                validate_server_engine("mariadb", version).is_err(),
                "{version}"
            );
        }
        assert!(validate_server_engine("mysql", "10.11.13-MariaDB").is_err());
        assert!(validate_server_engine("mariadb", "8.4.11").is_err());
        assert!(validate_server_engine("mysql", "8.4.11").is_ok());
    }

    #[test]
    fn connection_input_accepts_only_implemented_server_families() {
        let mut input = ConnectionInput {
            id: None,
            name: "Local DB".into(),
            engine: "mariadb".into(),
            host: "localhost".into(),
            port: 3306,
            user: "dbsual".into(),
            password: None,
            tls_mode: "disabled".into(),
            tls_ca_path: None,
            tls_client_cert_path: None,
            tls_client_key_path: None,
            ssh_enabled: Some(false),
            ssh_host: None,
            ssh_port: None,
            ssh_user: None,
            ssh_auth_method: None,
            ssh_key_path: None,
            ssh_agent_pipe: None,
            ssh_password: None,
            ssh_key_passphrase: None,
        };
        assert!(input.validate().is_ok());
        input.engine = "postgresql".into();
        assert!(input.validate().is_ok());
        input.ssh_enabled = Some(true);
        assert_eq!(
            input.validate().unwrap_err().code,
            "POSTGRES_SSH_UNAVAILABLE"
        );
        input.ssh_enabled = Some(false);
        input.tls_client_cert_path = Some("client.pem".into());
        input.tls_client_key_path = Some("client.key".into());
        assert_eq!(
            input.validate().unwrap_err().code,
            "POSTGRES_CLIENT_TLS_UNAVAILABLE"
        );
        input.engine = "postgres".into();
        assert!(input.validate().is_err());
        input.engine = "sqlite".into();
        input.host = std::env::temp_dir()
            .join("dbsual-validation.sqlite")
            .to_string_lossy()
            .into_owned();
        input.port = 0;
        input.user.clear();
        input.tls_ca_path = None;
        input.tls_client_cert_path = None;
        input.tls_client_key_path = None;
        assert!(input.validate().is_ok());
        input.port = 1;
        assert!(input.validate().is_err());
    }

    #[tokio::test]
    #[ignore = "requiere DBSUAL_MARIADB_TEST_PASSWORD y MariaDB 10.6, 10.11 o 11.4 desechable"]
    async fn mariadb_connection_identifies_server_and_reads_catalog() {
        let pool = connect_mariadb_test_from_env()
            .await
            .expect("connect to disposable MariaDB");
        let info = server_info(&pool, "mariadb")
            .await
            .expect("read MariaDB server info");
        validate_server_engine("mariadb", &info.server_version)
            .expect("require supported MariaDB family/version");
        println!("Verified MariaDB server version: {}", info.server_version);
        let result: i32 = sqlx::query_scalar("SELECT 2 + 2")
            .fetch_one(&pool)
            .await
            .expect("execute a read-only MariaDB query");
        assert_eq!(result, 4);
        let databases: Vec<String> = sqlx::query_scalar("SHOW DATABASES")
            .fetch_all(&pool)
            .await
            .expect("list visible MariaDB databases");
        assert!(!databases.is_empty());
        server_process_rows(&pool)
            .await
            .expect("list visible MariaDB processes");
        let users = server_user_rows(&pool, "mariadb")
            .await
            .expect("list visible MariaDB accounts");
        assert!(!users.is_empty());
        let variables = server_variable_rows(&pool)
            .await
            .expect("list visible MariaDB global variables");
        assert!(!variables.is_empty());
        let database = format!("dbsual_mariadb_{}", Uuid::new_v4().simple());
        let probe_result = async {
            sqlx::query(&format!("CREATE DATABASE `{database}`"))
                .execute(&pool)
                .await
                .map_err(|error| error.to_string())?;
            sqlx::query(&format!(
                "CREATE TABLE `{database}`.`parent` (id INT PRIMARY KEY, label VARCHAR(64) NOT NULL) ENGINE=InnoDB"
            ))
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
            sqlx::query(&format!(
                "CREATE TABLE `{database}`.`child` (id INT PRIMARY KEY, parent_id INT NOT NULL, CONSTRAINT fk_parent FOREIGN KEY (parent_id) REFERENCES `{database}`.`parent` (id)) ENGINE=InnoDB"
            ))
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
            sqlx::query(&format!(
                "INSERT INTO `{database}`.`parent` (id, label) VALUES (1, 'maria')"
            ))
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
            sqlx::query(&format!(
                "INSERT INTO `{database}`.`child` (id, parent_id) VALUES (2, 1)"
            ))
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
            sqlx::query(&format!(
                "CREATE VIEW `{database}`.`parent_view` AS SELECT id, label FROM `{database}`.`parent`"
            ))
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
            let objects = database_object_rows(&pool, &database)
                .await
                .map_err(|error| error.to_string())?;
            if !objects.iter().any(|(name, kind)| name == "parent" && kind == "BASE TABLE")
                || !objects.iter().any(|(name, kind)| name == "parent_view" && kind == "VIEW")
            {
                return Err("MariaDB catalog did not return expected table and view kinds".into());
            }
            let columns = column_metadata_rows(&pool, &database, "parent")
                .await
                .map_err(|error| error.to_string())?;
            if columns.len() != 2 || columns[0].0 != "id" {
                return Err("MariaDB column metadata was incomplete or out of order".into());
            }
            let structure = table_structure(&pool, &database, "child")
                .await
                .map_err(|error| format!("MariaDB table structure: {error:?}"))?;
            if !structure
                .constraints
                .iter()
                .any(|constraint| constraint.name == "fk_parent")
            {
                return Err("MariaDB foreign key metadata was not returned".into());
            }
            let read = crate::sql_editor::run_read_query(
                pool.clone(),
                database.clone(),
                "SELECT parent_id FROM child ORDER BY id".into(),
                0,
            )
            .await
            .map_err(|error| error.message.to_owned())?;
            if read.columns != ["parent_id"] || read.rows != [vec![Some("1".into())]] {
                return Err("MariaDB read query returned unexpected rows".into());
            }
            Ok::<(), String>(())
        }
        .await;
        let cleanup_result = sqlx::query(&format!("DROP DATABASE IF EXISTS `{database}`"))
            .execute(&pool)
            .await;
        pool.close().await;
        cleanup_result.expect("remove disposable MariaDB fixture");
        probe_result.expect("explore MariaDB and read a disposable fixture");
    }

    #[test]
    fn server_variables_with_secret_markers_are_redacted() {
        for name in [
            "password_hash",
            "ssl_key",
            "private_key_path",
            "api_token",
            "session_secret",
        ] {
            assert!(sensitive_variable(name), "{name} should be redacted");
        }
        assert!(!sensitive_variable("max_connections"));
        assert!(!sensitive_variable("innodb_buffer_pool_size"));
    }

    #[tokio::test]
    #[ignore = "requiere DBSUAL_MYSQL_TEST_PASSWORD y un MySQL desechable"]
    async fn mysql_server_metadata_queries_return_only_safe_process_fields() {
        let mut input = input();
        input.host = std::env::var("DBSUAL_MYSQL_TEST_HOST").unwrap_or_else(|_| "127.0.0.1".into());
        input.port = std::env::var("DBSUAL_MYSQL_TEST_PORT")
            .ok()
            .and_then(|port| port.parse().ok())
            .unwrap_or(3306);
        input.user = std::env::var("DBSUAL_MYSQL_TEST_USER").unwrap_or_else(|_| "root".into());
        input.password = Some(std::env::var("DBSUAL_MYSQL_TEST_PASSWORD").expect("test password"));
        let saved = input.saved("integration-server-metadata".into(), None);
        let pool = connect(&saved, input.password.as_deref(), None)
            .await
            .expect("connect MySQL");

        let processes = server_process_rows(&pool)
            .await
            .expect("list visible processes");
        assert!(!processes.is_empty());
        let process_json =
            serde_json::to_string(&processes).expect("serialize safe process metadata");
        assert!(!process_json.to_ascii_lowercase().contains("info"));

        let users = server_user_rows(&pool, "mysql")
            .await
            .expect("list visible MySQL accounts");
        assert!(users.iter().any(|user| user.name == input.user));
        let variables = server_variable_rows(&pool)
            .await
            .expect("list global variables");
        assert!(variables
            .iter()
            .any(|variable| variable.name == "max_connections"));
        assert!(variables.iter().all(|variable| variable.scope == "global"));
        pool.close().await;
    }
    use tokio::io::AsyncWriteExt;

    fn input() -> ConnectionInput {
        ConnectionInput {
            id: None,
            name: "Local".into(),
            engine: "mysql".into(),
            host: "localhost".into(),
            port: 3306,
            user: "root".into(),
            password: None,
            tls_mode: "disabled".into(),
            tls_ca_path: None,
            tls_client_cert_path: None,
            tls_client_key_path: None,
            ssh_enabled: None,
            ssh_host: None,
            ssh_port: None,
            ssh_user: None,
            ssh_auth_method: None,
            ssh_key_path: None,
            ssh_agent_pipe: None,
            ssh_password: None,
            ssh_key_passphrase: None,
        }
    }
    #[test]
    fn rejects_invalid_target() {
        let mut input = input();
        input.host = "localhost/path".into();
        assert!(input.validate().is_err());
        input.host = "localhost".into();
        input.port = 0;
        assert!(input.validate().is_err());
    }
    #[test]
    fn requires_tls_for_certificate_paths() {
        let mut input = input();
        input.tls_ca_path = Some("ca.pem".into());
        assert!(input.validate().is_err());
    }

    #[test]
    fn adding_ssh_credentials_preserves_the_saved_database_password() {
        let mut input = input();
        input.ssh_enabled = Some(true);
        input.ssh_host = Some("bastion.example".into());
        input.ssh_port = Some(22);
        input.ssh_user = Some("operator".into());
        input.ssh_auth_method = Some("password".into());
        input.ssh_password = Some("ssh-secret".into());
        let saved = input.saved("connection-id".into(), None);
        let previous = ConnectionSecrets {
            version: Some(1),
            db_password: Some("db-secret".into()),
            ..ConnectionSecrets::default()
        };

        let merged = merged_secrets(&previous, &input, &saved);

        assert_eq!(merged.db_password.as_deref(), Some("db-secret"));
        assert_eq!(merged.ssh_password.as_deref(), Some("ssh-secret"));
    }

    #[test]
    fn changing_ssh_auth_method_drops_only_obsolete_ssh_secrets() {
        let mut input = input();
        input.ssh_enabled = Some(true);
        input.ssh_host = Some("bastion.example".into());
        input.ssh_port = Some(22);
        input.ssh_user = Some("operator".into());
        input.ssh_auth_method = Some("privateKey".into());
        input.ssh_key_path = Some("C:\\keys\\id_ed25519".into());
        input.ssh_key_passphrase = Some("key-secret".into());
        let saved = input.saved("connection-id".into(), None);
        let previous = ConnectionSecrets {
            version: Some(1),
            db_password: Some("db-secret".into()),
            ssh_password: Some("old-ssh-secret".into()),
            ..ConnectionSecrets::default()
        };

        let merged = merged_secrets(&previous, &input, &saved);

        assert_eq!(merged.db_password.as_deref(), Some("db-secret"));
        assert_eq!(merged.ssh_password, None);
        assert_eq!(merged.ssh_key_passphrase.as_deref(), Some("key-secret"));
    }

    #[tokio::test]
    #[ignore = "requiere DBSUAL_MYSQL_TEST_PASSWORD y una instancia MySQL desechable"]
    async fn mysql_table_page_sorts_and_binds_filter_values() {
        let mut input = input();
        input.host = std::env::var("DBSUAL_MYSQL_TEST_HOST").unwrap_or_else(|_| "127.0.0.1".into());
        input.port = std::env::var("DBSUAL_MYSQL_TEST_PORT")
            .ok()
            .and_then(|port| port.parse().ok())
            .unwrap_or(3306);
        input.user = std::env::var("DBSUAL_MYSQL_TEST_USER").unwrap_or_else(|_| "root".into());
        input.password = Some(
            std::env::var("DBSUAL_MYSQL_TEST_PASSWORD")
                .expect("DBSUAL_MYSQL_TEST_PASSWORD must be set for this ignored test"),
        );
        let saved = input.saved("integration-table-page".into(), None);
        let pool = connect(&saved, input.password.as_deref(), None)
            .await
            .expect("connect to disposable MySQL server");
        let database = format!("dbsual_page_{}", Uuid::new_v4().simple());
        let table = "odd`table";
        let outcome: Result<_, String> = async {
            sqlx::query(&format!("CREATE DATABASE `{database}`"))
                .execute(&pool)
                .await
                .map_err(|error| error.to_string())?;
            sqlx::query(&format!(
                "CREATE TABLE `{database}`.`odd``table` (id INT PRIMARY KEY, label VARCHAR(80) NULL)"
            ))
            .execute(&pool)
            .await
            .map_err(|error| error.to_string())?;
            for id in 1..=205 {
                let label = match id {
                    3 => Some("needle' OR 1=1 --"),
                    4 => None,
                    _ => Some("same"),
                };
                sqlx::query(&format!(
                    "INSERT INTO `{database}`.`odd``table` (id, label) VALUES (?, ?)"
                ))
                .bind(id)
                .bind(label)
                .execute(&pool)
                .await
                    .map_err(|error| error.to_string())?;
            }
            let metadata_probe: Option<String> = sqlx::query_scalar(
                "SELECT CAST(TABLE_TYPE AS CHAR) FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?",
            )
            .bind(&database)
            .bind(table)
            .fetch_optional(&pool)
            .await
            .map_err(|error| error.to_string())?;
            assert_eq!(metadata_probe.as_deref(), Some("BASE TABLE"));
            let sorted = crate::sql_editor::run_table_page_registered(
                pool.clone(),
                database.clone(),
                table.into(),
                crate::sql_editor::TablePageOptions {
                    sort_column: Some("label".into()),
                    sort_direction: Some("asc".into()),
                    filter_column: None,
                    filter_mode: None,
                    filter_value: None,
                },
                0,
                None,
            )
            .await
            .map_err(|error| error.message.to_owned())?;
            let next_page = crate::sql_editor::run_table_page_registered(
                pool.clone(),
                database.clone(),
                table.into(),
                crate::sql_editor::TablePageOptions {
                    sort_column: Some("label".into()),
                    sort_direction: Some("asc".into()),
                    filter_column: None,
                    filter_mode: None,
                    filter_value: None,
                },
                sorted.next_offset.expect("sorted first page should have a continuation"),
                None,
            )
            .await
            .map_err(|error| error.message.to_owned())?;
            let filtered = crate::sql_editor::run_table_page_registered(
                pool.clone(),
                database.clone(),
                table.into(),
                crate::sql_editor::TablePageOptions {
                    sort_column: None,
                    sort_direction: None,
                    filter_column: Some("label".into()),
                    filter_mode: Some("contains".into()),
                    filter_value: Some("needle' OR 1=1 --".into()),
                },
                0,
                None,
            )
            .await
            .map_err(|error| error.message.to_owned())?;
            let invalid_column = crate::sql_editor::run_table_page_registered(
                pool.clone(),
                database.clone(),
                table.into(),
                crate::sql_editor::TablePageOptions {
                    sort_column: Some("id` DESC, (SELECT 1) --".into()),
                    sort_direction: Some("asc".into()),
                    filter_column: None,
                    filter_mode: None,
                    filter_value: None,
                },
                0,
                None,
            )
            .await;
            Ok((sorted, next_page, filtered, invalid_column))
        }
        .await;

        let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS `{database}`"))
            .execute(&pool)
            .await;
        pool.close().await;
        let (sorted, next_page, filtered, invalid_column) =
            outcome.expect("query disposable table pages");
        assert_eq!(
            sorted
                .rows
                .iter()
                .map(|row| row[0].as_deref())
                .take(4)
                .collect::<Vec<_>>(),
            [Some("4"), Some("3"), Some("1"), Some("2")]
        );
        assert_eq!(sorted.returned_rows, 200);
        assert!(sorted.has_more);
        assert_eq!(sorted.rows[199][0].as_deref(), Some("200"));
        assert_eq!(next_page.returned_rows, 5);
        assert_eq!(next_page.rows[0][0].as_deref(), Some("201"));
        assert_eq!(filtered.returned_rows, 1);
        assert_eq!(filtered.rows[0][0].as_deref(), Some("3"));
        assert_eq!(filtered.rows[0][1].as_deref(), Some("needle' OR 1=1 --"));
        assert_eq!(invalid_column.unwrap_err().code, "COLUMN_NOT_FOUND");
    }

    #[tokio::test]
    #[ignore = "requiere DBSUAL_MYSQL_TEST_PASSWORD y una instancia MySQL desechable"]
    async fn mysql_service_connection_and_read_query_work_against_a_real_server() {
        let mut input = input();
        input.host = std::env::var("DBSUAL_MYSQL_TEST_HOST").unwrap_or_else(|_| "127.0.0.1".into());
        input.port = std::env::var("DBSUAL_MYSQL_TEST_PORT")
            .ok()
            .and_then(|port| port.parse().ok())
            .unwrap_or(3306);
        input.user = std::env::var("DBSUAL_MYSQL_TEST_USER").unwrap_or_else(|_| "root".into());
        input.password = Some(
            std::env::var("DBSUAL_MYSQL_TEST_PASSWORD")
                .expect("DBSUAL_MYSQL_TEST_PASSWORD must be set for this ignored test"),
        );
        let saved = input.saved("integration-mysql".into(), None);
        let pool = connect(&saved, input.password.as_deref(), None)
            .await
            .expect("connect to disposable MySQL server");
        let info = server_info(&pool, "mysql")
            .await
            .expect("read MySQL server info");
        assert!(info.server_version.contains("8.4") || info.server_version.contains("8.0"));
        assert!(!info.tls_active);

        let database = format!("dbsual_test_{}", Uuid::new_v4().simple());
        let table_stream_database = format!("dbsual_stream_{}", Uuid::new_v4().simple());
        let restored_database = format!("dbsual_restore_{}", Uuid::new_v4().simple());
        let encrypted_restore_database =
            format!("dbsual_encrypted_restore_{}", Uuid::new_v4().simple());
        let tampered_restore_database =
            format!("dbsual_tampered_restore_{}", Uuid::new_v4().simple());
        let wrong_hash_restore_database =
            format!("dbsual_wrong_hash_restore_{}", Uuid::new_v4().simple());
        let restored_stream_database = format!("dbsual_stream_restore_{}", Uuid::new_v4().simple());
        let failed_stream_database = format!("dbsual_stream_failed_{}", Uuid::new_v4().simple());
        let cancelled_stream_database =
            format!("dbsual_stream_cancelled_{}", Uuid::new_v4().simple());
        let restored_generated_database = format!("dbsual_generated_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE `{database}`"))
            .execute(&pool)
            .await
            .expect("create disposable schema");
        sqlx::query(&format!("CREATE DATABASE `{table_stream_database}`"))
            .execute(&pool)
            .await
            .expect("create disposable table-only schema");
        let backup_vault = crate::vault::create_vault().expect("create temporary test vault");
        let setup = async {
            sqlx::query(&format!(
                "CREATE TABLE `{database}`.sample (id INT PRIMARY KEY, payload TEXT NULL, bytes BLOB NULL)"
            ))
            .execute(&pool)
            .await?;
            sqlx::raw_sql(&format!(
                "CREATE TABLE `{database}`.structure_parent (tenant_id INT NOT NULL, parent_id INT NOT NULL, label VARCHAR(80) NOT NULL, PRIMARY KEY (tenant_id, parent_id), UNIQUE KEY uq_parent_label (label)); CREATE TABLE `{database}`.structure_child (tenant_id INT NOT NULL, child_id INT NOT NULL, parent_id INT NOT NULL, PRIMARY KEY (tenant_id, child_id), KEY ix_child_parent (tenant_id, parent_id), CONSTRAINT fk_structure_parent FOREIGN KEY (tenant_id, parent_id) REFERENCES structure_parent (tenant_id, parent_id))"
            ))
            .execute(&pool)
            .await?;
            let structure = table_structure(&pool, &database, "structure_child")
                .await
                .expect("read table indexes and constraints");
            let primary = structure
                .constraints
                .iter()
                .find(|constraint| constraint.kind == "PRIMARY KEY")
                .expect("composite primary key");
            assert_eq!(primary.columns, ["tenant_id", "child_id"]);
            let foreign = structure
                .constraints
                .iter()
                .find(|constraint| constraint.name == "fk_structure_parent")
                .expect("foreign key");
            assert_eq!(foreign.kind, "FOREIGN KEY");
            assert_eq!(foreign.columns, ["tenant_id", "parent_id"]);
            assert_eq!(foreign.referenced_database.as_deref(), Some(database.as_str()));
            assert_eq!(foreign.referenced_table.as_deref(), Some("structure_parent"));
            assert_eq!(foreign.referenced_columns, ["tenant_id", "parent_id"]);
            assert!(structure.indexes.iter().any(|index| {
                index.name == "ix_child_parent" && index.columns == ["tenant_id", "parent_id"]
            }));
            sqlx::query(&format!(
                "CREATE TABLE `{database}`.empty_sample (id INT PRIMARY KEY, note TEXT NULL)"
            ))
            .execute(&pool)
            .await?;
            sqlx::query(&format!(
                "CREATE TABLE `{database}`.generated_sample (id INT PRIMARY KEY, doubled INT GENERATED ALWAYS AS (id * 2) STORED)"
            ))
            .execute(&pool)
            .await?;
            sqlx::query(&format!(
                "INSERT INTO `{database}`.generated_sample (id) VALUES (7)"
            ))
            .execute(&pool)
            .await?;
            sqlx::query(&format!(
                "INSERT INTO `{database}`.sample VALUES (1, ?, ?), (2, NULL, NULL), (3, ?, NULL)"
            ))
            .bind("Bogotá · comma, quote \" and newline\nsecond line")
            .bind([0_u8, 255, 10].as_slice())
            .bind("\\N")
            .execute(&pool)
            .await?;
            sqlx::raw_sql(&format!(
                "CREATE VIEW `{database}`.sample_view AS SELECT id, payload FROM `{database}`.sample"
            ))
            .execute(&pool)
            .await?;
            sqlx::raw_sql(&format!(
                "CREATE TRIGGER `{database}`.sample_trigger AFTER INSERT ON `{database}`.sample FOR EACH ROW SET @dbsual_backup_test = NEW.id"
            ))
            .execute(&pool)
            .await?;
            sqlx::raw_sql(&format!(
                "CREATE PROCEDURE `{database}`.sample_procedure() SELECT 1"
            ))
            .execute(&pool)
            .await?;
            sqlx::raw_sql(&format!(
                "CREATE EVENT `{database}`.sample_event ON SCHEDULE AT CURRENT_TIMESTAMP + INTERVAL 1 DAY DO SELECT 1"
            ))
            .execute(&pool)
            .await?;
            let backup_inventory = crate::mysql_backup::inspect_mysql_database(&pool, &database)
                .await?;
            assert_eq!(backup_inventory.coverage, "visible_objects_only");
            assert!(backup_inventory.visible_tables_are_transactional);
            assert_eq!(backup_inventory.character_set, "utf8mb4");
            assert_eq!(backup_inventory.collation, "utf8mb4_0900_ai_ci");
            assert!(backup_inventory.objects.iter().any(|object| object.name == "sample_view" && object.kind == "VIEW"));
            assert!(backup_inventory.routines.iter().any(|routine| routine.name == "sample_procedure" && routine.kind == "PROCEDURE"));
            assert!(backup_inventory.triggers.iter().any(|trigger| trigger.name == "sample_trigger" && trigger.table_name == "sample"));
            assert!(backup_inventory.events.iter().any(|event| event == "sample_event"));
            let definitions = crate::mysql_backup::read_mysql_definitions(&pool, &backup_inventory)
                .await?;
            assert!(definitions.iter().any(|item| item.name == "sample" && item.create_sql.contains("CREATE TABLE")));
            assert!(definitions.iter().any(|item| item.name == "sample_view" && item.create_sql.contains("CREATE")));
            assert!(definitions.iter().any(|item| item.name == "sample_procedure" && item.create_sql.contains("PROCEDURE")));
            assert!(definitions.iter().any(|item| item.name == "sample_trigger" && item.create_sql.contains("CREATE DEFINER")));
            assert!(definitions.iter().any(|item| item.name == "sample_event" && item.create_sql.contains("CREATE DEFINER")));
            for object_name in ["sample_procedure", "sample_trigger", "sample_event"] {
                let definition = definitions
                    .iter()
                    .find(|item| item.name == object_name)
                    .expect("programmable object definition");
                assert!(definition.definer.as_deref().is_some_and(|value| !value.is_empty()));
                assert!(definition.sql_mode.is_some());
                assert!(definition.character_set_client.is_some());
                assert!(definition.collation_connection.is_some());
                assert!(definition.database_collation.is_some());
            }
            let mut streamed_table = Vec::new();
            let streamed_rows = crate::mysql_backup::stream_mysql_table_rows(
                &pool,
                &backup_inventory,
                "sample",
                &mut streamed_table,
            )
            .await?;
            assert_eq!(streamed_rows, 3);
            let records: Vec<serde_json::Value> = String::from_utf8(streamed_table.clone())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(records[0]["record"], "table");
            assert_eq!(records[0]["valueEncoding"], "hex-text-protocol");
            assert!(records[0]["createSql"].as_str().unwrap().contains("CREATE TABLE"));
            assert_eq!(records[0]["columns"][0]["generated"], false);
            assert_eq!(records.last().unwrap()["rows"], 3);
            assert!(records.iter().any(|record| record["values"][2]["hex"] == "00ff0a"));
            assert!(records.iter().any(|record| record["values"][1].is_null()));
            assert!(records.iter().any(|record| record["values"][1]["hex"] == "5c4e"));
            let mut empty_table_stream = Vec::new();
            assert_eq!(
                crate::mysql_backup::stream_mysql_table_rows(
                    &pool,
                    &backup_inventory,
                    "empty_sample",
                    &mut empty_table_stream,
                )
                .await?,
                0
            );
            let empty_records: Vec<serde_json::Value> = String::from_utf8(empty_table_stream)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(empty_records[0]["columns"].as_array().unwrap().len(), 2);
            assert_eq!(empty_records.last().unwrap()["record"], "end");
            let mut generated_table_stream = Vec::new();
            assert_eq!(
                crate::mysql_backup::stream_mysql_table_rows(
                    &pool,
                    &backup_inventory,
                    "generated_sample",
                    &mut generated_table_stream,
                )
                .await?,
                1
            );
            let generated_records: Vec<serde_json::Value> =
                String::from_utf8(generated_table_stream.clone())
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
            assert_eq!(generated_records[0]["columns"][0]["generated"], false);
            assert_eq!(generated_records[0]["columns"][1]["generated"], true);
            assert!(generated_records[0]["createSql"].as_str().unwrap().contains("GENERATED ALWAYS"));
            assert_eq!(generated_records[1]["values"].as_array().unwrap().len(), 2);
            let mut sample_reader =
                tokio::io::BufReader::new(std::io::Cursor::new(streamed_table.clone()));
            assert_eq!(
                crate::mysql_backup::restore_mysql_table_stream(
                    &pool,
                    &restored_database,
                    &mut sample_reader,
                )
                .await
                .expect("restore one captured table into a new database"),
                3
            );
            let restored_rows: Vec<(i32, Option<String>, Option<Vec<u8>>)> = sqlx::query_as(
                &format!("SELECT id, payload, bytes FROM `{restored_database}`.sample ORDER BY id"),
            )
            .fetch_all(&pool)
            .await?;
            assert_eq!(restored_rows.len(), 3);
            assert_eq!(restored_rows[0].0, 1);
            assert_eq!(
                restored_rows[0].1.as_deref(),
                Some("Bogotá · comma, quote \" and newline\nsecond line")
            );
            assert_eq!(restored_rows[0].2.as_deref(), Some([0_u8, 255, 10].as_slice()));
        assert_eq!(restored_rows[1].1, None);
            assert_eq!(restored_rows[2].1.as_deref(), Some("\\N"));
            sqlx::raw_sql(&format!(
                "CREATE TABLE `{table_stream_database}`.a_parent (id INT PRIMARY KEY, label VARCHAR(80) NOT NULL); CREATE TABLE `{table_stream_database}`.z_child (id INT PRIMARY KEY, parent_id INT NULL, payload BLOB NULL, CONSTRAINT fk_parent FOREIGN KEY (parent_id) REFERENCES a_parent(id))"
            ))
            .execute(&pool)
            .await?;
            sqlx::raw_sql(&format!(
                "ALTER TABLE `{table_stream_database}`.a_parent ADD COLUMN child_id INT NULL, ADD COLUMN hidden_value VARCHAR(24) NOT NULL DEFAULT 'invisible-default' INVISIBLE, ADD CONSTRAINT fk_cycle_child FOREIGN KEY (child_id) REFERENCES z_child(id)"
            ))
            .execute(&pool)
            .await?;
            sqlx::query(&format!(
                "CREATE TABLE `{table_stream_database}`.audit_log (id INT NOT NULL AUTO_INCREMENT PRIMARY KEY, parent_id INT NOT NULL)"
            ))
            .execute(&pool)
            .await?;
            sqlx::raw_sql(&format!(
                "CREATE TRIGGER `{table_stream_database}`.parent_audit AFTER INSERT ON `{table_stream_database}`.a_parent FOR EACH ROW INSERT INTO `{table_stream_database}`.audit_log (parent_id) VALUES (NEW.id)"
            ))
            .execute(&pool)
            .await?;
            let mut cycle_connection = pool.acquire().await?;
            sqlx::raw_sql("SET SESSION FOREIGN_KEY_CHECKS = 0")
                .execute(&mut *cycle_connection)
                .await?;
            sqlx::query(&format!(
                "INSERT INTO `{table_stream_database}`.a_parent (id, label, child_id) VALUES (1, 'uno', 9), (2, 'dos', NULL)"
            ))
            .execute(&mut *cycle_connection)
            .await?;
            sqlx::query(&format!(
                "INSERT INTO `{table_stream_database}`.z_child VALUES (9, 1, ?), (10, NULL, NULL)"
            ))
            .bind([0_u8, 255, 17].as_slice())
            .execute(&mut *cycle_connection)
            .await?;
            sqlx::raw_sql("SET SESSION FOREIGN_KEY_CHECKS = 1")
                .execute(&mut *cycle_connection)
                .await?;
            drop(cycle_connection);
            let source_audit_rows: i64 = sqlx::query_scalar(&format!(
                "SELECT COUNT(*) FROM `{table_stream_database}`.audit_log"
            ))
            .fetch_one(&pool)
            .await?;
            assert_eq!(source_audit_rows, 2);
            sqlx::raw_sql(&format!(
                "CREATE VIEW `{table_stream_database}`.z_source AS SELECT id, label FROM `{table_stream_database}`.a_parent; CREATE VIEW `{table_stream_database}`.a_dependent AS SELECT id, label FROM `{table_stream_database}`.z_source"
            ))
            .execute(&pool)
            .await?;
            let table_stream_inventory = crate::mysql_backup::inspect_mysql_database(
                &pool,
                &table_stream_database,
            )
            .await?;
            let table_stream_definitions = crate::mysql_backup::read_mysql_definitions(
                &pool,
                &table_stream_inventory,
            )
            .await?;
            assert_eq!(table_stream_definitions.len(), 6);
            let table_stream_inspection = crate::mysql_backup::MysqlBackupInspection {
                inventory: table_stream_inventory,
                definitions: table_stream_definitions,
            };
            let mut table_stream_connection = pool.acquire().await?;
            let mut database_stream = Vec::new();
            crate::mysql_backup::stream_mysql_database(
                &mut table_stream_connection,
                &table_stream_inspection,
                &mut database_stream,
            )
            .await?;
            drop(table_stream_connection);
            let mut unavailable_definer_records: Vec<serde_json::Value> =
                String::from_utf8(database_stream.clone())
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
            let view_record = unavailable_definer_records
                .iter_mut()
                .find(|record| record["record"] == "definition" && record["kind"] == "VIEW")
                .expect("view definition is in backup manifest");
            view_record["definer"] = serde_json::Value::String("missing_user@%".into());
            let unavailable_definer_stream = unavailable_definer_records
                .iter()
                .map(serde_json::to_string)
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .join("\n");
            let mut unavailable_definer_reader = tokio::io::BufReader::new(
                std::io::Cursor::new(unavailable_definer_stream),
            );
            assert!(crate::mysql_backup::restore_mysql_database_table_stream(
                &pool,
                &failed_stream_database,
                &mut unavailable_definer_reader,
            )
            .await
            .is_err());
            let unavailable_definer_target: Option<(Vec<u8>,)> = sqlx::query_as(
                "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
            )
            .bind(&failed_stream_database)
            .fetch_optional(&pool)
            .await?;
            assert!(unavailable_definer_target.is_none());
            let mut database_stream_reader =
                tokio::io::BufReader::new(std::io::Cursor::new(database_stream.clone()));
            assert_eq!(
                crate::mysql_backup::restore_mysql_database_table_stream(
                    &pool,
                    &restored_stream_database,
                    &mut database_stream_reader,
                )
                .await
                .expect("restore all base tables from one complete table-only stream"),
                (3, 6)
            );
            let restored_child: Vec<(i32, Option<i32>, Option<Vec<u8>>)> = sqlx::query_as(
                &format!("SELECT id, parent_id, payload FROM `{restored_stream_database}`.z_child ORDER BY id"),
            )
            .fetch_all(&pool)
            .await?;
            assert_eq!(restored_child.len(), 2);
            assert_eq!(restored_child[0], (9, Some(1), Some(vec![0, 255, 17])));
            assert_eq!(restored_child[1], (10, None, None));
            let restored_view_rows: Vec<(i32, String)> = sqlx::query_as(&format!(
                "SELECT id, label FROM `{restored_stream_database}`.a_dependent ORDER BY id"
            ))
            .fetch_all(&pool)
            .await?;
            assert_eq!(restored_view_rows, [(1, "uno".into()), (2, "dos".into())]);
            let restored_audit_rows: i64 = sqlx::query_scalar(&format!(
                "SELECT COUNT(*) FROM `{restored_stream_database}`.audit_log"
            ))
            .fetch_one(&pool)
            .await?;
            assert_eq!(restored_audit_rows, source_audit_rows);
            sqlx::query(&format!(
                "INSERT INTO `{restored_stream_database}`.a_parent (id, label) VALUES (3, 'tres')"
            ))
            .execute(&pool)
            .await?;
            let audit_rows_after_insert: i64 = sqlx::query_scalar(&format!(
                "SELECT COUNT(*) FROM `{restored_stream_database}`.audit_log"
            ))
            .fetch_one(&pool)
            .await?;
            assert_eq!(audit_rows_after_insert, 3);
            let mut invalid_database_records: Vec<serde_json::Value> =
                String::from_utf8(database_stream.clone())
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
            let footer = invalid_database_records.last_mut().unwrap();
            assert_eq!(footer["record"], "database_end");
            footer["streamComplete"] = serde_json::Value::Bool(false);
            let invalid_database_stream = invalid_database_records
                .iter()
                .map(serde_json::to_string)
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .join("\n");
            let mut invalid_database_reader =
                tokio::io::BufReader::new(std::io::Cursor::new(invalid_database_stream));
            assert!(crate::mysql_backup::restore_mysql_database_table_stream(
                &pool,
                &failed_stream_database,
                &mut invalid_database_reader,
            )
            .await
            .is_err());
            let failed_stream_exists: Option<(Vec<u8>,)> = sqlx::query_as(
                "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
            )
            .bind(&failed_stream_database)
            .fetch_optional(&pool)
            .await?;
            assert!(failed_stream_exists.is_none());
            let mut definitions_connection = pool.acquire().await?;
            let mut definitions_stream = Vec::new();
            crate::mysql_backup::stream_mysql_database(
                &mut definitions_connection,
                &crate::mysql_backup::MysqlBackupInspection {
                    inventory: backup_inventory.clone(),
                    definitions: definitions.clone(),
                },
                &mut definitions_stream,
            )
            .await?;
            drop(definitions_connection);
            let serialized_definitions: Vec<serde_json::Value> =
                String::from_utf8(definitions_stream.clone())
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                    .filter(|record| record["record"] == "definition")
                    .collect();
            for object_name in ["sample_procedure", "sample_trigger", "sample_event"] {
                let metadata = serialized_definitions
                    .iter()
                    .find(|record| record["name"] == object_name)
                    .expect("programmable object metadata is serialized");
                assert!(metadata["definer"].as_str().is_some());
                assert!(metadata["sqlMode"].as_str().is_some());
                assert!(metadata["characterSetClient"].as_str().is_some());
                assert!(metadata["collationConnection"].as_str().is_some());
                assert!(metadata["databaseCollation"].as_str().is_some());
            }
            let mut definitions_reader =
                tokio::io::BufReader::new(std::io::Cursor::new(definitions_stream));
            assert!(crate::mysql_backup::restore_mysql_database_table_stream(
                &pool,
                &failed_stream_database,
                &mut definitions_reader,
            )
            .await
            .is_err());
            let rejected_definitions_exists: Option<(Vec<u8>,)> = sqlx::query_as(
                "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
            )
            .bind(&failed_stream_database)
            .fetch_optional(&pool)
            .await?;
            assert!(rejected_definitions_exists.is_none());
            let database_stream_text = std::str::from_utf8(&database_stream).unwrap();
            let footer_start = database_stream_text
                .rfind("{\"record\":\"database_end\"")
                .expect("captured stream has a database footer");
            let (mut incomplete_reader, mut incomplete_writer) =
                tokio::io::duplex(database_stream.len().max(1));
            incomplete_writer
                .write_all(&database_stream[..footer_start])
                .await?;
            let mut cancellation_reader = tokio::io::BufReader::new(&mut incomplete_reader);
            let mut restore_future = Box::pin(
                crate::mysql_backup::restore_mysql_database_table_stream(
                    &pool,
                    &cancelled_stream_database,
                    &mut cancellation_reader,
                ),
            );
            let mut destination_created = false;
            for _ in 0..100 {
                tokio::select! {
                    result = &mut restore_future => panic!("truncated stream must remain pending, got {result:?}"),
                    _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
                }
                let found: Option<(Vec<u8>,)> = sqlx::query_as(
                    "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
                )
                .bind(&cancelled_stream_database)
                .fetch_optional(&pool)
                .await?;
                if found.is_some() {
                    destination_created = true;
                    break;
                }
            }
            assert!(destination_created, "restore should create its provisional target");
            drop(restore_future);
            drop(cancellation_reader);
            drop(incomplete_writer);
            let mut cancelled_destination_removed = false;
            for _ in 0..1500 {
                let found: Option<(Vec<u8>,)> = sqlx::query_as(
                    "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
                )
                .bind(&cancelled_stream_database)
                .fetch_optional(&pool)
                .await?;
                if found.is_none() {
                    cancelled_destination_removed = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            assert!(cancelled_destination_removed, "cancelled restore must remove its provisional target");
            let mut existing_target_reader =
            tokio::io::BufReader::new(std::io::Cursor::new(streamed_table.clone()));
        let existing_target_result = crate::mysql_backup::restore_mysql_table_stream(
            &pool,
            &restored_database,
            &mut existing_target_reader,
        )
        .await;
        assert!(matches!(
            &existing_target_result,
            Err(crate::mysql_backup::MysqlTableRestoreError::DestinationExists)
        ), "existing destination must be preserved: {existing_target_result:?}");
        let mut generated_reader =
                tokio::io::BufReader::new(std::io::Cursor::new(generated_table_stream));
            assert_eq!(
                crate::mysql_backup::restore_mysql_table_stream(
                    &pool,
                    &restored_generated_database,
                    &mut generated_reader,
                )
                .await
                .expect("restore a generated-column table without inserting its derived value"),
                1
            );
            let generated_value: (i32, i32) = sqlx::query_as(&format!(
                "SELECT id, doubled FROM `{restored_generated_database}`.generated_sample"
            ))
            .fetch_one(&pool)
            .await?;
        assert_eq!(generated_value, (7, 14));
        let failed_restore_database = format!("dbsual_failed_restore_{}", Uuid::new_v4().simple());
        let mut corrupted_records: Vec<serde_json::Value> = String::from_utf8(streamed_table.clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        corrupted_records[1]["values"][0]["hex"] = serde_json::Value::String("not-hex".into());
        let corrupted_stream = corrupted_records
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        let mut corrupted_reader = tokio::io::BufReader::new(std::io::Cursor::new(corrupted_stream));
        assert!(crate::mysql_backup::restore_mysql_table_stream(
            &pool,
            &failed_restore_database,
            &mut corrupted_reader,
        )
        .await
        .is_err());
        let failed_restore_exists: Option<(Vec<u8>,)> = sqlx::query_as(
            "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
        )
        .bind(&failed_restore_database)
        .fetch_optional(&pool)
        .await?;
        assert!(failed_restore_exists.is_none(), "failed restore must remove its partial database");
            let artifact_directory = std::env::temp_dir()
                .join(format!("dbsual-mysql-backup-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&artifact_directory).expect("create test artifact directory");
            let mut backup_lock_connection = pool.acquire().await?;
            sqlx::raw_sql("LOCK INSTANCE FOR BACKUP")
                .execute(&mut *backup_lock_connection)
                .await?;
            let mut concurrent_ddl_connection = pool.acquire().await?;
            sqlx::raw_sql("SET SESSION lock_wait_timeout = 1")
                .execute(&mut *concurrent_ddl_connection)
                .await?;
            let concurrent_ddl = sqlx::raw_sql(&format!(
                "ALTER TABLE `{database}`.sample ADD COLUMN blocked_while_backup_locked INT"
            ))
            .execute(&mut *concurrent_ddl_connection)
            .await;
            drop(concurrent_ddl_connection);
            sqlx::raw_sql("UNLOCK INSTANCE")
                .execute(&mut *backup_lock_connection)
                .await?;
            drop(backup_lock_connection);
            let concurrent_ddl_error = concurrent_ddl
                .expect_err("instance backup lock must reject concurrent permanent DDL");
            let concurrent_ddl_message = concurrent_ddl_error.to_string().to_ascii_lowercase();
            assert!(
                concurrent_ddl_message.contains("lock wait timeout")
                    || concurrent_ddl_message.contains("backup lock"),
                "DDL must fail because the instance backup lock is active: {concurrent_ddl_error}"
            );
            let (artifact_key, artifact_recovery) = backup_vault.artifact_material();
            let published = tokio::time::timeout(
                std::time::Duration::from_secs(60),
                crate::mysql_backup::encrypt_mysql_database_stream(
                    &pool,
                    &database,
                    artifact_directory.clone(),
                    artifact_key,
                    artifact_recovery,
                ),
            )
            .await
            .expect("database stream encryption completes")
            .expect("publish an authenticated encrypted stream");
            let artifact_path = artifact_directory.join(format!(
                "{}.dbsual-artifact",
                published.artifact.artifact_id
            ));
            let mut artifact_reader = std::fs::File::open(&artifact_path)
                .expect("open encrypted artifact for in-memory test verification");
            let mut plaintext = Vec::new();
            let verified = crate::vault::decrypt_artifact(
                &mut artifact_reader,
                &mut plaintext,
                &backup_vault.recovery_phrase,
            )
            .expect("verify and decrypt the complete test stream");
            assert_eq!(verified, published.artifact);
            let encrypted_stream = String::from_utf8(plaintext).unwrap();
            assert!(encrypted_stream.contains("dbsual-mysql-stream-v1"));
            assert!(encrypted_stream.contains("00ff0a"));
            assert!(encrypted_stream.contains("\"streamComplete\":true"));
            crate::vault::store_master_key(backup_vault.vault_id, &backup_vault.master_key)
                .expect("store temporary key for authenticated restore");
            let (snapshot_key, snapshot_recovery) = backup_vault.artifact_material();
            let table_snapshot = crate::mysql_backup::encrypt_mysql_table_recovery_snapshot(
                &pool,
                &table_stream_database,
                artifact_directory.clone(),
                snapshot_key,
                snapshot_recovery,
            )
            .await
            .expect("capture encrypted partial table-and-view recovery point");
            // A captured subset must never silently stand in for the whole
            // database. Adding an unsupported routine makes capture fail
            // before a ciphertext artifact is published.
            sqlx::raw_sql(&format!(
                "CREATE PROCEDURE `{table_stream_database}`.unsupported_procedure() SELECT 1"
            ))
            .execute(&pool)
            .await?;
            let artifact_count_before_rejection = std::fs::read_dir(&artifact_directory)
                .expect("read artifact directory before unsupported-object capture")
                .count();
            let (unsupported_key, unsupported_recovery) = backup_vault.artifact_material();
            let unsupported_capture = crate::mysql_backup::encrypt_mysql_table_recovery_snapshot(
                &pool,
                &table_stream_database,
                artifact_directory.clone(),
                unsupported_key,
                unsupported_recovery,
            )
            .await;
            assert!(
                unsupported_capture.is_err(),
                "a routine outside partial recovery coverage must reject capture"
            );
            let artifact_count_after_rejection = std::fs::read_dir(&artifact_directory)
                .expect("read artifact directory after unsupported-object capture")
                .count();
            assert_eq!(
                artifact_count_after_rejection, artifact_count_before_rejection,
                "rejected partial capture must not publish ciphertext"
            );
            let snapshot_path = artifact_directory.join(format!(
                "{}.dbsual-artifact",
                table_snapshot.artifact.artifact_id
            ));
            let tampered_path = artifact_directory.join("altered.dbsual-artifact");
            let mut tampered_bytes =
                std::fs::read(&snapshot_path).expect("read encrypted snapshot");
            *tampered_bytes.last_mut().expect("artifact has a footer") ^= 1;
            std::fs::write(&tampered_path, &tampered_bytes).expect("write altered snapshot");
            let rejected = crate::mysql_backup::restore_mysql_encrypted_table_snapshot(
                &pool,
                &tampered_restore_database,
                tampered_path,
                backup_vault.vault_id,
                table_snapshot.artifact.artifact_id,
                &table_snapshot.ciphertext_sha256,
                table_snapshot.encrypted_bytes,
            )
            .await;
            assert!(matches!(
                rejected,
                Err(crate::mysql_backup::MysqlTableRestoreError::InvalidArtifact)
            ));
            let tampered_target_exists: Option<(Vec<u8>,)> = sqlx::query_as(
                "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
            )
            .bind(&tampered_restore_database)
            .fetch_optional(&pool)
            .await?;
            assert!(tampered_target_exists.is_none());
            let wrong_hash = crate::mysql_backup::restore_mysql_encrypted_table_snapshot(
                &pool,
                &wrong_hash_restore_database,
                snapshot_path.clone(),
                backup_vault.vault_id,
                table_snapshot.artifact.artifact_id,
                &"0".repeat(64),
                table_snapshot.encrypted_bytes,
            )
            .await;
            assert!(matches!(
                wrong_hash,
                Err(crate::mysql_backup::MysqlTableRestoreError::InvalidArtifact)
            ));
            let wrong_hash_target_exists: Option<(Vec<u8>,)> = sqlx::query_as(
                "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
            )
            .bind(&wrong_hash_restore_database)
            .fetch_optional(&pool)
            .await?;
            assert!(wrong_hash_target_exists.is_none());
            let mut malformed_stream = b"not-json\n".to_vec();
            malformed_stream.resize(1024 * 1024, b'x');
            let malformed_artifact = crate::vault::encrypt_artifact_reader_to_directory(
                &artifact_directory,
                &mut std::io::Cursor::new(malformed_stream),
                backup_vault.vault_id,
                &backup_vault.master_key,
                &backup_vault.recovery_envelope,
            )
            .expect("publish encrypted malformed-stream fixture");
            let malformed_target = format!("{database}_malformed_restore");
            let malformed_path = artifact_directory.join(format!(
                "{}.dbsual-artifact",
                malformed_artifact.artifact.artifact_id
            ));
            let malformed_restore = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                crate::mysql_backup::restore_mysql_encrypted_table_snapshot(
                    &pool,
                    &malformed_target,
                    malformed_path,
                    backup_vault.vault_id,
                    malformed_artifact.artifact.artifact_id,
                    &malformed_artifact.ciphertext_sha256,
                    malformed_artifact.encrypted_bytes,
                ),
            )
            .await
            .expect("malformed restore closes its bounded decrypt stream")
            .expect_err("reject malformed plaintext after authenticating its artifact");
            assert!(matches!(
                malformed_restore,
                crate::mysql_backup::MysqlTableRestoreError::InvalidStream
            ));
            let malformed_target_exists: Option<(Vec<u8>,)> = sqlx::query_as(
                "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
            )
            .bind(&malformed_target)
            .fetch_optional(&pool)
            .await?;
            assert!(malformed_target_exists.is_none());
            let (restored_tables, restored_rows) =
                crate::mysql_backup::restore_mysql_encrypted_table_snapshot(
                    &pool,
                    &encrypted_restore_database,
                    snapshot_path,
                    backup_vault.vault_id,
                    table_snapshot.artifact.artifact_id,
                    &table_snapshot.ciphertext_sha256,
                    table_snapshot.encrypted_bytes,
                )
                .await
                .expect("authenticate and restore encrypted snapshot");
            assert_eq!((restored_tables, restored_rows), (3, 6));
            let restored_parent_rows: Vec<(i32, String)> = sqlx::query_as(&format!(
                "SELECT id, label FROM `{encrypted_restore_database}`.a_parent ORDER BY id"
            ))
            .fetch_all(&pool)
            .await?;
            assert_eq!(restored_parent_rows, [(1, "uno".into()), (2, "dos".into())]);
            let restored_invisible_rows: Vec<(i32, String)> = sqlx::query_as(&format!(
                "SELECT id, hidden_value FROM `{encrypted_restore_database}`.a_parent ORDER BY id"
            ))
            .fetch_all(&pool)
            .await?;
            assert_eq!(
                restored_invisible_rows,
                [(1, "invisible-default".into()), (2, "invisible-default".into())]
            );
            crate::vault::remove_master_key(backup_vault.vault_id)
                .expect("remove temporary key after restore");
            std::fs::remove_dir_all(&artifact_directory).expect("remove temporary backup artifacts");
            let backup_user = format!("dbsual_{}", &Uuid::new_v4().simple().to_string()[..20]);
            let backup_password = Uuid::new_v4().to_string();
            sqlx::raw_sql(&format!(
                "CREATE USER '{backup_user}'@'%' IDENTIFIED BY '{backup_password}'"
            ))
            .execute(&pool)
            .await?;
            sqlx::raw_sql(&format!(
                "GRANT BACKUP_ADMIN, SHOW_ROUTINE ON *.* TO '{backup_user}'@'%'"
            ))
            .execute(&pool)
            .await?;
            sqlx::raw_sql(&format!(
                "GRANT SELECT, SHOW VIEW, TRIGGER, EVENT ON `{database}`.* TO '{backup_user}'@'%'"
            ))
            .execute(&pool)
            .await?;
            let least_privilege_pool = MySqlPoolOptions::new()
                .max_connections(4)
                .connect_with(
                    MySqlConnectOptions::new()
                        .host(&input.host)
                        .port(input.port)
                        .username(&backup_user)
                        .password(&backup_password),
                )
                .await?;
            let least_privilege_inventory =
                crate::mysql_backup::inspect_mysql_database(&least_privilege_pool, &database)
                    .await?;
            assert!(least_privilege_inventory
                .objects
                .iter()
                .any(|object| object.name == "sample_view"));
            assert!(least_privilege_inventory
                .routines
                .iter()
                .any(|routine| routine.name == "sample_procedure"));
            assert!(least_privilege_inventory
                .triggers
                .iter()
                .any(|trigger| trigger.name == "sample_trigger"));
            assert!(least_privilege_inventory
                .events
                .iter()
                .any(|event| event == "sample_event"));
            let least_privilege_definitions = crate::mysql_backup::read_mysql_definitions(
                &least_privilege_pool,
                &least_privilege_inventory,
            )
            .await?;
            assert_eq!(least_privilege_definitions.len(), definitions.len());
            std::fs::create_dir_all(&artifact_directory)
                .expect("create least-privilege artifact directory");
            let (artifact_key, artifact_recovery) = backup_vault.artifact_material();
            crate::mysql_backup::encrypt_mysql_database_stream(
                &least_privilege_pool,
                &database,
                artifact_directory.clone(),
                artifact_key,
                artifact_recovery,
            )
            .await
            .expect("least-privilege account captures and encrypts the database");
            assert_eq!(
                std::fs::read_dir(&artifact_directory)
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "dbsual-artifact"))
                    .count(),
                1
            );
            least_privilege_pool.close().await;
            sqlx::raw_sql(&format!("DROP USER '{backup_user}'@'%'"))
                .execute(&pool)
                .await?;
            std::fs::remove_dir_all(&artifact_directory)
                .expect("remove least-privilege artifacts");
            let restricted_user =
                format!("dbsual_{}", &Uuid::new_v4().simple().to_string()[..20]);
            let restricted_password = Uuid::new_v4().to_string();
            sqlx::raw_sql(&format!(
                "CREATE USER '{restricted_user}'@'%' IDENTIFIED BY '{restricted_password}'"
            ))
            .execute(&pool)
            .await?;
            sqlx::raw_sql(&format!(
                "GRANT BACKUP_ADMIN ON *.* TO '{restricted_user}'@'%'"
            ))
            .execute(&pool)
            .await?;
            sqlx::raw_sql(&format!(
                "GRANT SELECT ON `{database}`.* TO '{restricted_user}'@'%'"
            ))
            .execute(&pool)
            .await?;
            let restricted_pool = MySqlPoolOptions::new()
                .max_connections(4)
                .connect_with(
                    MySqlConnectOptions::new()
                        .host(&input.host)
                        .port(input.port)
                        .username(&restricted_user)
                        .password(&restricted_password),
                )
                .await?;
            std::fs::create_dir_all(&artifact_directory)
                .expect("create restricted-account artifact directory");
            let (artifact_key, artifact_recovery) = backup_vault.artifact_material();
            let restricted_capture = crate::mysql_backup::encrypt_mysql_database_stream(
                &restricted_pool,
                &database,
                artifact_directory.clone(),
                artifact_key,
                artifact_recovery,
            )
            .await;
            let restricted_error = restricted_capture
                .expect_err("missing object-definition permissions must reject the capture");
            assert!(
                matches!(
                    &restricted_error,
                    crate::mysql_backup::MysqlBackupStreamError::Database(
                        sqlx::Error::Database(error)
                    ) if error.message().contains("SHOW VIEW command denied")
                ),
                "capture should expose the server's privilege denial: {restricted_error:?}"
            );
            assert_eq!(
                std::fs::read_dir(&artifact_directory)
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "dbsual-artifact"))
                    .count(),
                0,
                "a capture with missing object-definition privileges must not publish an artifact"
            );
            restricted_pool.close().await;
            sqlx::raw_sql(&format!("DROP USER '{restricted_user}'@'%'"))
                .execute(&pool)
                .await?;
            std::fs::remove_dir_all(&artifact_directory)
                .expect("remove restricted-account artifacts");
            sqlx::query(&format!(
                "CREATE TABLE `{database}`.nontransactional_sample (id INT PRIMARY KEY) ENGINE=MyISAM"
            ))
            .execute(&pool)
            .await?;
            let mixed_inventory = crate::mysql_backup::inspect_mysql_database(&pool, &database)
                .await?;
            assert!(!mixed_inventory.visible_tables_are_transactional);
            let (artifact_key, artifact_recovery) = backup_vault.artifact_material();
            let rejected = crate::mysql_backup::encrypt_mysql_database_stream(
                &pool,
                &database,
                artifact_directory.clone(),
                artifact_key,
                artifact_recovery,
            )
            .await;
            assert!(rejected.is_err());
            assert_eq!(
                std::fs::read_dir(&artifact_directory)
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "dbsual-artifact"))
                    .count(),
                0,
                "failed capture must not publish another ciphertext artifact"
            );
            std::fs::remove_dir_all(&artifact_directory)
                .expect("remove temporary backup artifacts after rejection");
            let objects = database_object_rows(&pool, &database).await?;
            assert!(objects.iter().any(|(name, kind)| name == "sample" && kind == "BASE TABLE"));
            let columns = column_metadata_rows(&pool, &database, "sample").await?;
            assert_eq!(columns.iter().map(|column| column.0.as_str()).collect::<Vec<_>>(), ["id", "payload", "bytes"]);
            assert!(columns.iter().all(|column| column.5 == "BASE TABLE"));
            let rows = crate::sql_editor::run_read_query(
                pool.clone(),
                database.clone(),
                "SELECT id, payload FROM sample ORDER BY id".into(),
                0,
            )
            .await
            .expect("read rows through the DBSUAL read-query path");
            assert_eq!(rows.columns, ["id", "payload"]);
            assert_eq!(
                rows.rows[0],
                [
                    Some("1".into()),
                    Some("Bogotá · comma, quote \" and newline\nsecond line".into())
                ]
            );

            let empty = crate::sql_editor::run_read_query(
                pool.clone(),
                database.clone(),
                "SELECT id, payload FROM sample WHERE id < 0".into(),
                0,
            )
            .await
            .expect("read empty result through the DBSUAL read-query path");
            assert_eq!(empty.columns, ["id", "payload"]);
            assert!(empty.rows.is_empty());

            sqlx::query(&format!("CREATE TABLE `{database}`.page_sample (id INT PRIMARY KEY)"))
                .execute(&pool)
                .await?;
            for id in 1..=205 {
                sqlx::query(&format!("INSERT INTO `{database}`.page_sample (id) VALUES (?)"))
                    .bind(id)
                    .execute(&pool)
                    .await?;
            }
            let first_page = crate::sql_editor::run_read_query(
                pool.clone(),
                database.clone(),
                "SELECT id FROM page_sample ORDER BY id".into(),
                0,
            )
            .await
            .expect("read first page through the DBSUAL read-query path");
            assert_eq!(first_page.returned_rows, 200);
            assert!(first_page.has_more);
            assert_eq!(first_page.next_offset, Some(200));
            assert_eq!(first_page.rows[0][0].as_deref(), Some("1"));
            let final_page = crate::sql_editor::run_read_query(
                pool.clone(),
                database.clone(),
                "SELECT id FROM page_sample ORDER BY id".into(),
                first_page.next_offset.expect("next page offset"),
            )
            .await
            .expect("read final page through the DBSUAL read-query path");
            assert_eq!(final_page.returned_rows, 5);
            assert!(!final_page.has_more);
            assert_eq!(final_page.next_offset, None);
            assert_eq!(final_page.rows[0][0].as_deref(), Some("201"));
            assert_eq!(final_page.rows[4][0].as_deref(), Some("205"));

            let query_id = Uuid::new_v4().to_string();
            let active_queries = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
            let task_queries = std::sync::Arc::clone(&active_queries);
            let query_pool = pool.clone();
            let query_database = database.clone();
            let task_id = query_id.clone();
            let task_connection_id = saved.id.clone();
            let query_task = tokio::task::spawn_blocking(move || {
                tauri::async_runtime::block_on(crate::sql_editor::run_read_query_registered(
                    query_pool,
                    query_database,
                    "SELECT SLEEP(20)".into(),
                    0,
                    Some((task_queries, task_id, task_connection_id)),
                ))
            });
            let active_thread = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    if let Some((_, thread_id)) = active_queries.lock().unwrap().get(&query_id) {
                        break *thread_id;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("read query becomes cancellable");
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            crate::sql_editor::cancel_mysql_query(pool.clone(), active_thread)
                .await
                .expect("MySQL confirms the query cancellation");
            let cancelled = query_task
                .await
                .expect("cancellable query task completes")
                .err()
                .expect("cancelled query must not report successful results");
            assert_eq!(cancelled.code, "QUERY_CANCELLED");
            assert!(!active_queries.lock().unwrap().contains_key(&query_id));

            let csv_path =
                std::env::temp_dir().join(format!("dbsual-mysql-export-{}.csv", Uuid::new_v4()));
            let exported = crate::sql_editor::run_export_table_csv(
                pool.clone(),
                database.clone(),
                "sample".into(),
                csv_path.clone(),
                b';',
                "\\N".into(),
            )
            .await
            .expect("export complete CSV from live MySQL");
            assert_eq!(exported.rows_written, 3);
            let mut csv_reader = csv::ReaderBuilder::new()
                .delimiter(b';')
                .from_path(&csv_path)
                .expect("read completed CSV");
            assert_eq!(
                csv_reader.headers().unwrap().iter().collect::<Vec<_>>(),
                ["id", "payload", "bytes"]
            );
            let records = csv_reader
                .records()
                .collect::<Result<Vec<_>, _>>()
                .expect("parse exported CSV records");
            assert_eq!(
                records[0].get(1),
                Some("Bogotá · comma, quote \" and newline\nsecond line")
            );
            assert_eq!(records[1].get(1), Some("\\N"));
            assert_eq!(records[0].get(2), Some("0x00ff0a"));
            assert_eq!(records[1].get(2), Some("\\N"));
            assert_eq!(records[2].get(1), Some("\\\\N"));
            let duplicate = crate::sql_editor::run_export_table_csv(
                pool.clone(),
                database.clone(),
                "sample".into(),
                csv_path.clone(),
                b';',
                "\\N".into(),
            )
            .await
            .unwrap_err();
            assert_eq!(duplicate.code, "EXPORT_DESTINATION_EXISTS");
            std::fs::remove_file(csv_path).expect("remove exported test CSV");
            Ok::<(), sqlx::Error>(())
        }
        .await;
        let cleanup = sqlx::query(&format!("DROP DATABASE `{database}`"))
            .execute(&pool)
            .await;
        let _ = sqlx::query(&format!(
            "DROP DATABASE IF EXISTS `{table_stream_database}`"
        ))
        .execute(&pool)
        .await;
        let _ = sqlx::query(&format!(
            "DROP DATABASE IF EXISTS `{restored_stream_database}`"
        ))
        .execute(&pool)
        .await;
        let _ = sqlx::query(&format!(
            "DROP DATABASE IF EXISTS `{failed_stream_database}`"
        ))
        .execute(&pool)
        .await;
        let _ = sqlx::query(&format!(
            "DROP DATABASE IF EXISTS `{cancelled_stream_database}`"
        ))
        .execute(&pool)
        .await;
        let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS `{restored_database}`"))
            .execute(&pool)
            .await;
        let _ = sqlx::query(&format!(
            "DROP DATABASE IF EXISTS `{restored_generated_database}`"
        ))
        .execute(&pool)
        .await;
        let _ = sqlx::query(&format!(
            "DROP DATABASE IF EXISTS `{encrypted_restore_database}`"
        ))
        .execute(&pool)
        .await;
        let _ = sqlx::query(&format!(
            "DROP DATABASE IF EXISTS `{tampered_restore_database}`"
        ))
        .execute(&pool)
        .await;
        let _ = sqlx::query(&format!(
            "DROP DATABASE IF EXISTS `{wrong_hash_restore_database}`"
        ))
        .execute(&pool)
        .await;
        let key_cleanup = crate::vault::remove_master_key(backup_vault.vault_id);
        pool.close().await;
        setup.expect("run live MySQL read checks");
        cleanup.expect("drop disposable schema");
        key_cleanup.expect("remove temporary backup key");
    }
}
