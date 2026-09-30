use std::{path::Path, time::Duration};

use futures_util::StreamExt;
use serde::Serialize;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Column, Executor, Row, SqlitePool,
};
use tauri::State;

use crate::{AppError, AppState};

pub(crate) async fn open_readonly(path: &Path) -> Result<SqlitePool, AppError> {
    let canonical = path.canonicalize().map_err(|_| invalid_file())?;
    if !canonical.is_file() {
        return Err(invalid_file());
    }
    let options = SqliteConnectOptions::new()
        .filename(canonical)
        .read_only(true)
        .create_if_missing(false)
        .busy_timeout(Duration::from_secs(5));
    SqlitePoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(8))
        .connect_with(options)
        .await
        .map_err(|_| {
            AppError::new(
                "SQLITE_OPEN_FAILED",
                "No se pudo abrir el archivo SQLite en modo de solo lectura.",
            )
        })
}

fn invalid_file() -> AppError {
    AppError::new(
        "SQLITE_FILE_INVALID",
        "Selecciona un archivo SQLite existente y accesible.",
    )
}

pub(crate) async fn databases(pool: &SqlitePool) -> Result<Vec<String>, AppError> {
    sqlx::query_scalar("SELECT name FROM pragma_database_list ORDER BY seq")
        .fetch_all(pool)
        .await
        .map_err(|_| {
            AppError::new(
                "METADATA_FAILED",
                "No se pudieron listar las bases adjuntas.",
            )
        })
}

pub(crate) async fn objects(
    pool: &SqlitePool,
    database: &str,
) -> Result<Vec<(String, String)>, AppError> {
    if database != "main" {
        return Err(AppError::new(
            "INVALID_DATABASE",
            "Solo se puede explorar la base principal de esta conexión.",
        ));
    }
    sqlx::query_as("SELECT name, type FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .fetch_all(pool).await
        .map_err(|_| AppError::new("METADATA_FAILED", "No se pudieron listar tablas y vistas SQLite."))
}

pub(crate) async fn columns(
    pool: &SqlitePool,
    database: &str,
    table: &str,
    object_type: &str,
) -> Result<Vec<ColumnMetadata>, AppError> {
    if database != "main"
        || !matches!(object_type, "table" | "view")
        || table.is_empty()
        || table.chars().any(char::is_control)
    {
        return Err(AppError::new(
            "INVALID_OBJECT",
            "La tabla, vista o base SQLite no es válida.",
        ));
    }
    let kind: Option<String> = sqlx::query_scalar(
        "SELECT type FROM sqlite_master WHERE name=? AND type IN ('table','view')",
    )
    .bind(table)
    .fetch_optional(pool)
    .await
    .map_err(|_| {
        AppError::new(
            "METADATA_FAILED",
            "No se pudieron consultar las columnas SQLite.",
        )
    })?;
    if kind.as_deref() != Some(object_type) {
        return Err(AppError::new(
            "OBJECT_CHANGED",
            "El objeto cambió desde la última actualización. Actualiza el explorador.",
        ));
    }
    let rows = sqlx::query(
        r#"SELECT name, type, "notnull", pk, dflt_value FROM pragma_table_info(?) ORDER BY cid"#,
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .map_err(|_| {
        AppError::new(
            "METADATA_FAILED",
            "No se pudieron consultar las columnas SQLite.",
        )
    })?;
    if rows.is_empty() {
        return Err(AppError::new(
            "OBJECT_NOT_FOUND",
            "La tabla o vista ya no existe o no es visible.",
        ));
    }
    Ok(rows
        .into_iter()
        .map(|row| {
            let pk: i64 = row.get("pk");
            ColumnMetadata {
                name: row.get("name"),
                data_type: row.get::<Option<String>, _>("type").unwrap_or_default(),
                is_nullable: row.get::<i64, _>("notnull") == 0,
                is_primary_key: pk > 0,
                primary_key_available: object_type == "table",
                default_value: row.get("dflt_value"),
                default_available: object_type == "table",
            }
        })
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnMetadata {
    pub(crate) name: String,
    pub(crate) data_type: String,
    pub(crate) is_nullable: bool,
    pub(crate) is_primary_key: bool,
    pub(crate) primary_key_available: bool,
    pub(crate) default_value: Option<String>,
    pub(crate) default_available: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlitePage {
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
    returned_rows: usize,
    has_more: bool,
    next_offset: Option<u64>,
    elapsed_ms: u64,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TablePageOptions {
    sort_column: Option<String>,
    sort_direction: Option<String>,
    filter_column: Option<String>,
    filter_mode: Option<String>,
    filter_value: Option<String>,
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn default_primary_key_order(primary_key: &[(i64, String)]) -> Option<String> {
    if primary_key.is_empty() {
        return None;
    }
    let mut primary_key = primary_key.to_vec();
    primary_key.sort_by_key(|(position, _)| *position);
    Some(
        primary_key
            .iter()
            .map(|(_, name)| quote_identifier(name))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

#[tauri::command]
pub async fn execute_sqlite_read_query(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    sql: String,
    offset: u64,
) -> Result<crate::sql_editor::SqlReadResult, AppError> {
    if database != "main" || offset > 1_000_000 {
        return Err(AppError::new(
            "INVALID_READ_QUERY",
            "La base o la página solicitada no es válida.",
        ));
    }
    crate::sql_editor::validate_read_query(&sql)
        .map_err(|error| AppError::new(error.code, error.message))?;
    let pool = state
        .active_sqlite
        .lock()
        .await
        .get(&connection_id)
        .cloned()
        .ok_or(AppError::new(
            "CONNECTION_CLOSED",
            "Abre la conexión para ejecutar consultas.",
        ))?;
    execute_sqlite_read_query_from_pool(&pool, &sql, offset).await
}

async fn execute_sqlite_read_query_from_pool(
    pool: &SqlitePool,
    sql: &str,
    offset: u64,
) -> Result<crate::sql_editor::SqlReadResult, AppError> {
    crate::sql_editor::validate_read_query(sql)
        .map_err(|error| AppError::new(error.code, error.message))?;
    let inner_sql = sql.trim().trim_end_matches(';').trim_end();
    let description = pool
        .describe(inner_sql)
        .await
        .map_err(|_| AppError::new("QUERY_FAILED", "SQLite rechazó la consulta de lectura."))?;
    let columns = description
        .columns()
        .iter()
        .map(|column| column.name().to_owned())
        .collect::<Vec<_>>();
    if columns.is_empty() {
        return Err(AppError::new(
            "QUERY_FAILED",
            "La consulta SQLite no devolvió columnas.",
        ));
    }
    let projection = columns.iter().map(|name| {
        let column = quote_identifier(name);
        let qualified = format!("\"__dbsual_result\".{column}");
        format!("CASE WHEN typeof({qualified})='blob' THEN '0x'||hex({qualified}) ELSE CAST({qualified} AS TEXT) END AS {column}")
    }).collect::<Vec<_>>().join(",");
    let bounded_sql =
        format!("SELECT {projection} FROM ({inner_sql}) AS \"__dbsual_result\" LIMIT 201 OFFSET ?");
    let started = std::time::Instant::now();
    let page = tokio::time::timeout(Duration::from_secs(15), async {
        let mut stream = sqlx::query(&bounded_sql).bind(offset as i64).fetch(pool);
        let mut rows = Vec::new();
        let mut payload_bytes = 0usize;
        let mut has_more = false;
        while let Some(result) = stream.next().await {
            let row = result.map_err(|_| AppError::new("QUERY_FAILED", "SQLite rechazó la consulta de lectura."))?;
            let values = (0..columns.len()).map(|index| row.try_get::<Option<String>, _>(index))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| AppError::new("QUERY_FAILED", "Una celda SQLite contiene un tipo que no se puede representar de forma segura."))?;
            let row_bytes = values.iter().flatten().map(String::len).sum::<usize>();
            if rows.len() >= 200 || (!rows.is_empty() && payload_bytes.saturating_add(row_bytes) > 1_048_576) {
                has_more = true;
                break;
            }
            payload_bytes = payload_bytes.saturating_add(row_bytes);
            rows.push(values);
        }
        Ok::<_, AppError>((rows, has_more))
    }).await.map_err(|_| AppError::new("QUERY_TIMEOUT", "La consulta SQLite superó el límite de 15 segundos."))??;
    let returned_rows = page.0.len();
    Ok(crate::sql_editor::SqlReadResult {
        columns,
        rows: page.0,
        returned_rows,
        has_more: page.1,
        next_offset: page.1.then_some(offset + returned_rows as u64),
        elapsed_ms: started.elapsed().as_millis(),
    })
}

#[tauri::command]
pub async fn read_sqlite_table_page(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    table: String,
    options: TablePageOptions,
    offset: u64,
) -> Result<SqlitePage, AppError> {
    if offset > 1_000_000
        || database != "main"
        || table.is_empty()
        || table.len() > 255
        || table.chars().any(char::is_control)
    {
        return Err(AppError::new(
            "INVALID_QUERY",
            "La solicitud de lectura SQLite no es válida.",
        ));
    }
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
    let kind: Option<String> = sqlx::query_scalar(
        "SELECT type FROM sqlite_master WHERE name=? AND type IN ('table','view')",
    )
    .bind(&table)
    .fetch_optional(&pool)
    .await
    .map_err(|_| AppError::new("METADATA_FAILED", "No se pudo validar la tabla SQLite."))?;
    if kind.as_deref() != Some("table") {
        return Err(AppError::new(
            "INVALID_OBJECT",
            "La cuadrícula SQLite solo permite leer tablas.",
        ));
    }
    let column_rows = sqlx::query("SELECT name, pk FROM pragma_table_info(?) ORDER BY cid")
        .bind(&table)
        .fetch_all(&pool)
        .await
        .map_err(|_| {
            AppError::new(
                "METADATA_FAILED",
                "No se pudieron leer las columnas SQLite.",
            )
        })?;
    let column_names = column_rows
        .iter()
        .map(|row| row.get::<String, _>("name"))
        .collect::<Vec<_>>();
    let primary_key = column_rows
        .iter()
        .filter_map(|row| {
            let position: i64 = row.get("pk");
            (position > 0).then(|| (position, row.get::<String, _>("name")))
        })
        .collect::<Vec<_>>();
    if column_names.is_empty() {
        return Err(AppError::new(
            "OBJECT_NOT_FOUND",
            "La tabla ya no existe o no contiene columnas.",
        ));
    }
    let valid_column = |name: &str| column_names.iter().any(|column| column == name);
    let mut query = format!(
        "SELECT {} FROM {}",
        column_names
            .iter()
            .map(|n| {
                let identifier = quote_identifier(n);
                format!("CASE WHEN typeof({identifier})='blob' THEN '0x'||hex({identifier}) ELSE CAST({identifier} AS TEXT) END")
            })
            .collect::<Vec<_>>()
            .join(","),
        quote_identifier(&table)
    );
    if let (Some(column), Some(mode), Some(value)) = (
        options.filter_column.as_deref(),
        options.filter_mode.as_deref(),
        options.filter_value.as_deref(),
    ) {
        if !valid_column(column) || value.len() > 4096 || !matches!(mode, "equals" | "contains") {
            return Err(AppError::new(
                "INVALID_QUERY",
                "El filtro SQLite no es válido.",
            ));
        }
        query.push_str(&format!(
            " WHERE {} {} ?",
            quote_identifier(column),
            if mode == "contains" { "LIKE" } else { "=" }
        ));
        if mode == "contains" {
            query.push_str(" ESCAPE '\\'");
        }
    } else if options.filter_column.is_some()
        || options.filter_mode.is_some()
        || options.filter_value.is_some()
    {
        return Err(AppError::new(
            "INVALID_QUERY",
            "El filtro SQLite no está completo.",
        ));
    }
    if let Some(column) = options.sort_column.as_deref() {
        if !valid_column(column)
            || !matches!(options.sort_direction.as_deref(), Some("asc" | "desc"))
        {
            return Err(AppError::new(
                "INVALID_QUERY",
                "El orden SQLite no es válido.",
            ));
        }
        query.push_str(&format!(
            " ORDER BY {} {}",
            quote_identifier(column),
            if options.sort_direction.as_deref() == Some("desc") {
                "DESC"
            } else {
                "ASC"
            }
        ));
    } else if options.sort_direction.is_some() {
        return Err(AppError::new(
            "INVALID_QUERY",
            "El orden SQLite no está completo.",
        ));
    } else if let Some(order) = default_primary_key_order(&primary_key) {
        // A stable default order keeps offset-based pages predictable when a
        // table has a declared key and the user has not selected a sort.
        query.push_str(" ORDER BY ");
        query.push_str(&order);
    }
    query.push_str(" LIMIT 201 OFFSET ?");
    let started = std::time::Instant::now();
    let mut statement = sqlx::query(&query);
    if let (Some(mode), Some(value)) = (
        options.filter_mode.as_deref(),
        options.filter_value.as_deref(),
    ) {
        if mode == "contains" {
            statement = statement.bind(format!(
                "%{}%",
                value
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            ));
        } else {
            statement = statement.bind(value);
        }
    }
    let mut fetched = statement.bind(offset as i64).fetch(&pool);
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    let mut payload_bytes = 0usize;
    let mut has_more = false;
    while let Some(row) = fetched.next().await {
        let row =
            row.map_err(|_| AppError::new("QUERY_FAILED", "No se pudo leer la tabla SQLite."))?;
        let values = (0..column_names.len())
            .map(|i| row.try_get::<Option<String>, _>(i))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| AppError::new("QUERY_FAILED", "Una celda SQLite contiene un tipo que no se puede representar de forma segura."))?;
        let row_bytes = values.iter().flatten().map(String::len).sum::<usize>();
        if rows.len() >= 200
            || (!rows.is_empty() && payload_bytes.saturating_add(row_bytes) > 1_048_576)
        {
            has_more = true;
            break;
        }
        payload_bytes = payload_bytes.saturating_add(row_bytes);
        rows.push(values);
    }
    let returned_rows = rows.len();
    Ok(SqlitePage {
        columns: column_names,
        rows,
        returned_rows,
        has_more,
        next_offset: has_more.then_some(offset + returned_rows as u64),
        elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{Connection, SqliteConnection};
    use uuid::Uuid;

    struct DbFile(std::path::PathBuf);
    impl Drop for DbFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    async fn sample_file() -> DbFile {
        let path = std::env::temp_dir().join(format!("dbsual-{}.sqlite", Uuid::new_v4()));
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
        sqlx::query("CREATE TABLE \"select\" (id INTEGER PRIMARY KEY, value TEXT, optional TEXT DEFAULT 'x')").execute(&mut connection).await.unwrap();
        sqlx::query(
            "INSERT INTO \"select\" (value, optional) VALUES ('alpha', NULL), ('beta', 'b')",
        )
        .execute(&mut connection)
        .await
        .unwrap();
        connection.close().await.unwrap();
        DbFile(path)
    }

    #[tokio::test]
    async fn opens_real_file_read_only_and_lists_metadata_and_rows() {
        let file = sample_file().await;
        let pool = open_readonly(&file.0).await.unwrap();
        assert_eq!(databases(&pool).await.unwrap(), vec!["main"]);
        assert_eq!(
            objects(&pool, "main").await.unwrap(),
            vec![("select".to_owned(), "table".to_owned())]
        );
        let columns = columns(&pool, "main", "select", "table").await.unwrap();
        assert_eq!(columns[0].name, "id");
        assert!(columns[0].is_primary_key);
        assert_eq!(columns[2].default_value.as_deref(), Some("'x'"));
        assert!(
            sqlx::query("INSERT INTO \"select\" (value) VALUES ('blocked')")
                .execute(&pool)
                .await
                .is_err()
        );
        let values: Vec<Option<String>> =
            sqlx::query_scalar("SELECT optional FROM \"select\" ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(values, vec![None, Some("b".into())]);
        pool.close().await;
    }

    #[tokio::test]
    async fn executes_only_bounded_reads_and_preserves_null_and_blob_values() {
        let file = sample_file().await;
        let pool = open_readonly(&file.0).await.unwrap();
        let result = execute_sqlite_read_query_from_pool(
            &pool,
            "SELECT id, optional, X'00FF' AS payload FROM \"select\" ORDER BY id;",
            0,
        )
        .await
        .unwrap();
        assert_eq!(result.columns, vec!["id", "optional", "payload"]);
        assert_eq!(
            result.rows[0],
            vec![Some("1".into()), None, Some("0x00FF".into())]
        );
        assert!(!result.has_more);
        let error = execute_sqlite_read_query_from_pool(&pool, "DELETE FROM \"select\"", 0)
            .await
            .unwrap_err();
        assert_eq!(error.code, "READ_ONLY_QUERY_REQUIRED");
        pool.close().await;
    }

    #[tokio::test]
    async fn select_results_are_paged_at_two_hundred_rows() {
        let file = sample_file().await;
        let options = SqliteConnectOptions::new()
            .filename(&file.0)
            .create_if_missing(false);
        let mut writable = SqliteConnection::connect_with(&options).await.unwrap();
        for index in 0..205 {
            sqlx::query("INSERT INTO \"select\" (value) VALUES (?)")
                .bind(format!("row-{index}"))
                .execute(&mut writable)
                .await
                .unwrap();
        }
        writable.close().await.unwrap();
        let pool = open_readonly(&file.0).await.unwrap();
        let first =
            execute_sqlite_read_query_from_pool(&pool, "SELECT id FROM \"select\" ORDER BY id", 0)
                .await
                .unwrap();
        assert_eq!(first.returned_rows, 200);
        assert!(first.has_more);
        assert_eq!(first.next_offset, Some(200));
        let second = execute_sqlite_read_query_from_pool(
            &pool,
            "SELECT id FROM \"select\" ORDER BY id",
            200,
        )
        .await
        .unwrap();
        assert_eq!(second.returned_rows, 7);
        assert!(!second.has_more);
        pool.close().await;
    }

    #[tokio::test]
    async fn refuses_missing_files_and_non_main_databases() {
        let missing =
            std::env::temp_dir().join(format!("dbsual-missing-{}.sqlite", Uuid::new_v4()));
        assert!(open_readonly(&missing).await.is_err());
        let file = sample_file().await;
        let pool = open_readonly(&file.0).await.unwrap();
        assert!(objects(&pool, "other").await.is_err());
        pool.close().await;
    }

    #[tokio::test]
    async fn default_order_uses_composite_primary_key_position() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE sample (tenant_id INTEGER, record_id INTEGER, PRIMARY KEY (tenant_id, record_id))")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO sample VALUES (2, 1), (1, 2), (1, 1)")
            .execute(&pool)
            .await
            .unwrap();
        let key = sqlx::query("SELECT name, pk FROM pragma_table_info(?) ORDER BY cid")
            .bind("sample")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|row| {
                let position: i64 = row.get("pk");
                (position > 0).then(|| (position, row.get::<String, _>("name")))
            })
            .collect::<Vec<_>>();
        let order = default_primary_key_order(&key).unwrap();
        let rows: Vec<(i64, i64)> = sqlx::query_as(&format!(
            "SELECT tenant_id, record_id FROM sample ORDER BY {order}"
        ))
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows, vec![(1, 1), (1, 2), (2, 1)]);
        pool.close().await;
    }
}
