//! Adaptador directo de PostgreSQL. Se mantiene separado del adaptador MySQL.

use std::{path::PathBuf, time::Duration};

use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions, PgSslMode},
    ConnectOptions, PgPool, Row,
};

const MAX_HOST_BYTES: usize = 255;
const MAX_USER_BYTES: usize = 128;
const MAX_PASSWORD_BYTES: usize = 4096;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostgresTlsMode {
    Disabled,
    VerifyIdentity,
}

#[derive(Clone)]
pub struct PostgresConnectConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
    pub database: Option<String>,
    pub tls_mode: PostgresTlsMode,
    pub tls_ca_path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostgresDatabase {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostgresObject {
    pub schema: String,
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostgresColumn {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
    pub default_value: Option<String>,
    pub primary_key_available: bool,
    pub default_available: bool,
}

/// Errores estables y libres de datos de conexión o mensajes del servidor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostgresError {
    InvalidConfig,
    InvalidTlsConfig,
    ConnectionFailed,
    ServerNotPostgres,
    ServerVersionUnavailable,
    DatabaseListFailed,
    MetadataFailed,
    ObjectKindChanged,
}

impl std::fmt::Display for PostgresError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidConfig => "Revisa host, puerto, usuario y base de datos.",
            Self::InvalidTlsConfig => "La configuración TLS de PostgreSQL no es válida.",
            Self::ConnectionFailed => "No se pudo conectar al servidor PostgreSQL.",
            Self::ServerNotPostgres => "El servidor no se identificó como PostgreSQL.",
            Self::ServerVersionUnavailable => "No se pudo determinar la versión de PostgreSQL.",
            Self::DatabaseListFailed => "No se pudieron listar las bases de datos accesibles.",
            Self::MetadataFailed => "No se pudieron consultar los objetos de PostgreSQL.",
            Self::ObjectKindChanged => "El tipo del objeto PostgreSQL cambió.",
        })
    }
}

impl std::error::Error for PostgresError {}

#[derive(Clone)]
pub struct PostgresConnection {
    pool: PgPool,
    config: PostgresConnectConfig,
    server_version: String,
    tls_active: bool,
}

/// Escapa un identificador PostgreSQL para construir SQL a partir de nombres
/// validados del catálogo. Los valores de datos deben seguir usando parámetros.
pub(crate) fn quote_ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

impl PostgresConnection {
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn server_version(&self) -> &str {
        &self.server_version
    }

    pub fn tls_active(&self) -> bool {
        self.tls_active
    }

    pub async fn connect_to_database(
        &self,
        database: &str,
    ) -> Result<PostgresConnection, PostgresError> {
        let mut config = self.config.clone();
        config.database = Some(database.to_owned());
        connect(config).await
    }

    /// Lista bases a las que el rol actual puede conectarse; no modifica el servidor.
    pub async fn list_databases(&self) -> Result<Vec<PostgresDatabase>, PostgresError> {
        let rows = sqlx::query(
            "SELECT datname FROM pg_catalog.pg_database \
             WHERE datallowconn AND pg_catalog.has_database_privilege(datname, 'CONNECT') \
             ORDER BY datname",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|_| PostgresError::DatabaseListFailed)?;

        Ok(rows
            .into_iter()
            .map(|row| PostgresDatabase {
                name: row.get("datname"),
            })
            .collect())
    }

    pub async fn list_objects(&self) -> Result<Vec<PostgresObject>, PostgresError> {
        let rows = sqlx::query(
            "SELECT table_schema, table_name, CASE WHEN table_type = 'VIEW' THEN 'view' ELSE 'table' END AS kind \
             FROM information_schema.tables \
             WHERE table_schema NOT IN ('pg_catalog', 'information_schema') \
               AND table_type IN ('BASE TABLE', 'VIEW') \
             ORDER BY table_schema, kind, table_name",
        )
        .fetch_all(&self.pool).await.map_err(|_| PostgresError::MetadataFailed)?;
        Ok(rows
            .into_iter()
            .map(|row| PostgresObject {
                schema: row.get("table_schema"),
                name: row.get("table_name"),
                kind: row.get("kind"),
            })
            .collect())
    }

    pub async fn list_columns(
        &self,
        schema: &str,
        object: &str,
        is_view: bool,
    ) -> Result<Vec<PostgresColumn>, PostgresError> {
        let rows = sqlx::query(
            "SELECT c.column_name, pg_catalog.format_type(a.atttypid, a.atttypmod) AS data_type, \
                    c.is_nullable = 'YES' AS nullable, \
                    cls.relkind = 'v' AS is_view, \
                    EXISTS (SELECT 1 FROM pg_catalog.pg_index i \
                      JOIN LATERAL unnest(i.indkey) WITH ORDINALITY k(attnum, ord) ON true \
                      WHERE i.indrelid = cls.oid \
                        AND i.indisprimary AND k.attnum = a.attnum) AS primary_key, \
                    c.column_default \
             FROM information_schema.columns c \
             JOIN pg_catalog.pg_namespace n ON n.nspname = c.table_schema \
             JOIN pg_catalog.pg_class cls ON cls.relnamespace = n.oid AND cls.relname = c.table_name \
             JOIN pg_catalog.pg_attribute a ON a.attrelid = cls.oid AND a.attname = c.column_name \
             WHERE c.table_schema = $1 AND c.table_name = $2 \
             ORDER BY c.ordinal_position",
        ).bind(schema).bind(object).fetch_all(&self.pool).await.map_err(|_| PostgresError::MetadataFailed)?;
        if rows
            .first()
            .is_some_and(|row| row.get::<bool, _>("is_view") != is_view)
        {
            return Err(PostgresError::ObjectKindChanged);
        }
        Ok(rows
            .into_iter()
            .map(|row| PostgresColumn {
                name: row.get("column_name"),
                data_type: row.get("data_type"),
                nullable: row.get("nullable"),
                primary_key: !is_view && row.get::<bool, _>("primary_key"),
                default_value: row.get("column_default"),
                primary_key_available: !is_view,
                default_available: !is_view,
            })
            .collect())
    }
}

/// Abre un pool PostgreSQL dedicado y comprueba la identidad de familia/versión.
pub async fn connect(config: PostgresConnectConfig) -> Result<PostgresConnection, PostgresError> {
    validate_config(&config)?;

    let mut options = PgConnectOptions::new()
        .host(&config.host)
        .port(config.port)
        .username(&config.username)
        .ssl_mode(match config.tls_mode {
            PostgresTlsMode::Disabled => PgSslMode::Disable,
            PostgresTlsMode::VerifyIdentity => PgSslMode::VerifyFull,
        })
        .disable_statement_logging();

    if let Some(password) = config.password.as_deref() {
        options = options.password(password);
    }
    if let Some(database) = config.database.as_deref() {
        options = options.database(database);
    }
    if let Some(ca_path) = config.tls_ca_path.as_deref() {
        options = options.ssl_root_cert(ca_path);
    }

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(CONNECT_TIMEOUT)
        .connect_with(options)
        .await
        .map_err(|_| PostgresError::ConnectionFailed)?;

    let row = match sqlx::query(
        "SELECT version() AS version, current_setting('server_version') AS server_version",
    )
    .fetch_one(&pool)
    .await
    {
        Ok(row) => row,
        Err(_) => {
            pool.close().await;
            return Err(PostgresError::ServerVersionUnavailable);
        }
    };
    let banner: String = row
        .try_get("version")
        .map_err(|_| PostgresError::ServerVersionUnavailable)?;
    let server_version: String = row
        .try_get("server_version")
        .map_err(|_| PostgresError::ServerVersionUnavailable)?;
    if !banner.starts_with("PostgreSQL ") || !valid_server_version(&server_version) {
        pool.close().await;
        return Err(PostgresError::ServerNotPostgres);
    }
    let tls_active = match sqlx::query_scalar::<_, bool>(
        "SELECT ssl FROM pg_catalog.pg_stat_ssl WHERE pid = pg_backend_pid()",
    )
    .fetch_one(&pool)
    .await
    {
        Ok(active) => active,
        Err(_) => {
            pool.close().await;
            return Err(PostgresError::ConnectionFailed);
        }
    };

    Ok(PostgresConnection {
        pool,
        config,
        server_version,
        tls_active,
    })
}

fn validate_config(config: &PostgresConnectConfig) -> Result<(), PostgresError> {
    let valid_text = |value: &str, max: usize| {
        !value.trim().is_empty()
            && value == value.trim()
            && value.len() <= max
            && !value.chars().any(char::is_control)
    };
    if !valid_text(&config.host, MAX_HOST_BYTES)
        || config.host.contains(['/', '\\', '@'])
        || config.port == 0
        || !valid_text(&config.username, MAX_USER_BYTES)
        || config
            .password
            .as_ref()
            .is_some_and(|p| p.len() > MAX_PASSWORD_BYTES)
        || config
            .database
            .as_ref()
            .is_some_and(|db| !valid_text(db, 63))
    {
        return Err(PostgresError::InvalidConfig);
    }
    match config.tls_mode {
        PostgresTlsMode::Disabled if config.tls_ca_path.is_some() => {
            Err(PostgresError::InvalidTlsConfig)
        }
        PostgresTlsMode::VerifyIdentity => {
            if config
                .tls_ca_path
                .as_ref()
                .is_some_and(|path| path.as_os_str().is_empty())
            {
                Err(PostgresError::InvalidTlsConfig)
            } else {
                Ok(())
            }
        }
        _ => Ok(()),
    }
}

fn valid_server_version(value: &str) -> bool {
    let numeric = value.split_whitespace().next().unwrap_or_default();
    let parts: Vec<_> = numeric.split('.').collect();
    let Some(major) = parts.first().and_then(|part| part.parse::<u16>().ok()) else {
        return false;
    };
    let Some(minor) = parts.get(1).and_then(|part| part.parse::<u16>().ok()) else {
        return false;
    };
    if major < 9 || minor > 99 {
        return false;
    }
    match (major < 10, parts.as_slice()) {
        (true, [_, _, patch]) => patch.parse::<u16>().is_ok(),
        (false, [_, _]) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> PostgresConnectConfig {
        PostgresConnectConfig {
            host: "localhost".into(),
            port: 5432,
            username: "dbsual_test".into(),
            password: None,
            database: None,
            tls_mode: PostgresTlsMode::VerifyIdentity,
            tls_ca_path: None,
        }
    }

    #[test]
    fn accepts_valid_direct_config_and_tls_modes() {
        let mut value = config();
        assert_eq!(validate_config(&value), Ok(()));
        value.tls_mode = PostgresTlsMode::Disabled;
        assert_eq!(validate_config(&value), Ok(()));
    }

    #[test]
    fn rejects_invalid_network_and_database_values() {
        let mut value = config();
        value.port = 0;
        assert_eq!(validate_config(&value), Err(PostgresError::InvalidConfig));
        value = config();
        value.host = "host/name".into();
        assert_eq!(validate_config(&value), Err(PostgresError::InvalidConfig));
        value = config();
        value.database = Some(" bad ".into());
        assert_eq!(validate_config(&value), Err(PostgresError::InvalidConfig));
    }

    #[test]
    fn rejects_ca_when_tls_is_disabled() {
        let mut value = config();
        value.tls_mode = PostgresTlsMode::Disabled;
        value.tls_ca_path = Some("ca.pem".into());
        assert_eq!(
            validate_config(&value),
            Err(PostgresError::InvalidTlsConfig)
        );
    }

    #[test]
    fn parses_postgres_server_versions() {
        for version in [
            "16.4",
            "16.11 (Debian 16.11-1.pgdg)",
            "17.0",
            "18.1",
            "9.6.24",
        ] {
            assert!(valid_server_version(version), "{version}");
        }
        for version in [
            "",
            "16",
            "x.4",
            "16.4.1",
            "9.6",
            "8.4",
            "19.no",
            "16.bad (vendor suffix)",
        ] {
            assert!(!valid_server_version(version), "{version}");
        }
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requiere un servidor PostgreSQL desechable y DBSUAL_POSTGRES_TEST_*"]
    async fn connects_and_lists_accessible_databases() {
        let host = std::env::var("DBSUAL_POSTGRES_TEST_HOST").expect("host de prueba requerido");
        let port = std::env::var("DBSUAL_POSTGRES_TEST_PORT")
            .expect("puerto de prueba requerido")
            .parse()
            .expect("puerto numérico requerido");
        let username =
            std::env::var("DBSUAL_POSTGRES_TEST_USER").expect("usuario de prueba requerido");
        let password = std::env::var("DBSUAL_POSTGRES_TEST_PASSWORD").ok();
        let connection = connect(PostgresConnectConfig {
            host,
            port,
            username,
            password,
            database: None,
            tls_mode: PostgresTlsMode::Disabled,
            tls_ca_path: None,
        })
        .await
        .expect("conexión PostgreSQL desechable");
        let databases = connection
            .list_databases()
            .await
            .expect("listado accesible");
        assert!(databases.iter().any(|db| db.name == "postgres"));
        assert!(!connection.server_version().is_empty());
        assert!(!connection.tls_active());
    }

    #[tokio::test]
    #[ignore = "requiere un PostgreSQL desechable con permiso CREATE SCHEMA y DBSUAL_POSTGRES_TEST_*"]
    async fn lists_objects_and_columns_by_schema() {
        let host = std::env::var("DBSUAL_POSTGRES_TEST_HOST").expect("host de prueba requerido");
        let port = std::env::var("DBSUAL_POSTGRES_TEST_PORT")
            .expect("puerto requerido")
            .parse()
            .expect("puerto numérico");
        let username = std::env::var("DBSUAL_POSTGRES_TEST_USER").expect("usuario requerido");
        let password = std::env::var("DBSUAL_POSTGRES_TEST_PASSWORD").ok();
        let connection = connect(PostgresConnectConfig {
            host,
            port,
            username,
            password,
            database: Some("postgres".into()),
            tls_mode: PostgresTlsMode::Disabled,
            tls_ca_path: None,
        })
        .await
        .expect("conexión PostgreSQL desechable");
        let suffix = std::process::id();
        let schema_a = format!("dbsual_a_{suffix}");
        let schema_b = format!("dbsual_b_{suffix}");
        let setup = async {
            sqlx::query(&format!("CREATE SCHEMA {schema_a}")).execute(connection.pool()).await?;
            sqlx::query(&format!("CREATE SCHEMA {schema_b}")).execute(connection.pool()).await?;
            sqlx::query(&format!("CREATE TABLE {schema_a}.duplicate_name (id integer PRIMARY KEY, label text DEFAULT 'value')")).execute(connection.pool()).await?;
            sqlx::query(&format!("CREATE TABLE {schema_b}.duplicate_name (uid uuid NOT NULL)")).execute(connection.pool()).await?;
            sqlx::query(&format!("CREATE VIEW {schema_a}.sample_view AS SELECT id FROM {schema_a}.duplicate_name"))
                .execute(connection.pool()).await?;
            Ok::<(), sqlx::Error>(())
        }.await;
        setup.expect("crear esquemas de prueba");

        let objects = connection
            .list_objects()
            .await
            .expect("catálogo por esquema");
        assert!(objects.iter().any(|item| item.schema == schema_a
            && item.name == "duplicate_name"
            && item.kind == "table"));
        assert!(objects.iter().any(|item| item.schema == schema_b
            && item.name == "duplicate_name"
            && item.kind == "table"));
        assert!(objects.iter().any(|item| item.schema == schema_a
            && item.name == "sample_view"
            && item.kind == "view"));
        let columns = connection
            .list_columns(&schema_a, "duplicate_name", false)
            .await
            .expect("columnas con PK/default");
        assert_eq!(columns.len(), 2);
        assert!(columns
            .iter()
            .any(|column| column.name == "id" && column.primary_key && !column.nullable));
        assert!(columns
            .iter()
            .any(|column| column.name == "label" && column.default_value.is_some()));
        let view_columns = connection
            .list_columns(&schema_a, "sample_view", true)
            .await
            .expect("columnas de vista");
        assert_eq!(view_columns.len(), 1);
        assert!(!view_columns[0].primary_key_available && !view_columns[0].default_available);
        assert_eq!(
            connection
                .list_columns(&schema_a, "sample_view", false)
                .await,
            Err(PostgresError::ObjectKindChanged)
        );

        sqlx::query(&format!("DROP SCHEMA {schema_a} CASCADE"))
            .execute(connection.pool())
            .await
            .expect("limpiar primer esquema de prueba");
        sqlx::query(&format!("DROP SCHEMA {schema_b} CASCADE"))
            .execute(connection.pool())
            .await
            .expect("limpiar segundo esquema de prueba");
        connection.pool().close().await;
    }
}
