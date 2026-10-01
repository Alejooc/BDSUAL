//! Ejecución acotada de consultas de lectura para el editor SQL.
//!
//! El comando está conectado desde `lib.rs` y solo habilita lecturas acotadas.

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::{Column, Connection, Executor, MySqlPool, Row, TypeInfo, ValueRef};
use tauri::State;

const MAX_QUERY_BYTES: usize = 64 * 1024;
const MAX_RESULT_ROWS: usize = 200;
const MAX_RESULT_BYTES: usize = 1024 * 1024;
const MAX_RESULT_OFFSET: u64 = 1_000_000;

pub(crate) type ActiveReadQueries = Mutex<HashMap<String, (String, u64)>>;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlReadError {
    pub code: &'static str,
    pub message: &'static str,
}

impl SqlReadError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlReadResult {
    pub(crate) columns: Vec<String>,
    pub(crate) rows: Vec<Vec<Option<String>>>,
    pub(crate) returned_rows: usize,
    pub(crate) has_more: bool,
    pub(crate) next_offset: Option<u64>,
    pub(crate) elapsed_ms: u128,
    pub(crate) total_rows: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TablePageOptions {
    pub sort_column: Option<String>,
    pub sort_direction: Option<String>,
    pub filter_column: Option<String>,
    pub filter_mode: Option<String>,
    pub filter_value: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CsvExportResult {
    pub(crate) rows_written: u64,
}

struct ActiveReadGuard {
    registry: Arc<ActiveReadQueries>,
    query_id: String,
}

impl Drop for ActiveReadGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = self.registry.lock() {
            active.remove(&self.query_id);
        }
    }
}

#[tauri::command]
pub async fn export_table_csv(
    state: State<'_, crate::AppState>,
    connection_id: String,
    database: String,
    table: String,
    path: String,
    delimiter: String,
    null_marker: String,
) -> Result<CsvExportResult, SqlReadError> {
    if connection_id.len() > 128
        || database.is_empty()
        || database.len() > 255
        || table.is_empty()
        || table.len() > 255
        || database.chars().any(char::is_control)
        || table.chars().any(char::is_control)
        || null_marker.len() > 128
    {
        return Err(SqlReadError::new(
            "INVALID_EXPORT_OPTIONS",
            "Revisa la tabla y las opciones de exportación.",
        ));
    }
    let delimiter = match delimiter.as_str() {
        "comma" => b',',
        "semicolon" => b';',
        "tab" => b'\t',
        _ => {
            return Err(SqlReadError::new(
                "INVALID_EXPORT_OPTIONS",
                "El separador CSV no es válido.",
            ));
        }
    };
    let destination = PathBuf::from(path);
    if !destination.is_absolute() || destination.file_name().is_none() {
        return Err(SqlReadError::new(
            "INVALID_EXPORT_PATH",
            "Elige una ruta absoluta para guardar el archivo CSV.",
        ));
    }
    let pool = match state.active.0.try_lock() {
        Ok(active) => active.get(&connection_id).cloned(),
        Err(_) => {
            return Err(SqlReadError::new(
                "CONNECTION_BUSY",
                "La lista de conexiones está ocupada; vuelve a intentarlo.",
            ));
        }
    }
    .ok_or(SqlReadError::new(
        "CONNECTION_CLOSED",
        "Abre la conexión antes de exportar.",
    ))?;
    drop(state);
    tauri::async_runtime::spawn_blocking(move || {
        tauri::async_runtime::block_on(run_export_table_csv(
            pool,
            database,
            table,
            destination,
            delimiter,
            null_marker,
        ))
    })
    .await
    .map_err(|_| {
        SqlReadError::new(
            "CSV_EXPORT_FAILED",
            "No se pudo completar la exportación CSV.",
        )
    })?
}

pub(crate) async fn run_export_table_csv(
    pool: MySqlPool,
    database: String,
    table: String,
    destination: PathBuf,
    delimiter: u8,
    null_marker: String,
) -> Result<CsvExportResult, SqlReadError> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty());
    let Some(parent) = parent else {
        return Err(invalid_export_path());
    };
    if !parent.is_dir() || destination.exists() {
        return Err(SqlReadError::new(
            if destination.exists() {
                "EXPORT_DESTINATION_EXISTS"
            } else {
                "EXPORT_DIRECTORY_UNAVAILABLE"
            },
            if destination.exists() {
                "El archivo ya existe; elige otra ruta para evitar sobrescribirlo."
            } else {
                "La carpeta elegida no está disponible."
            },
        ));
    }
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(invalid_export_path)?;
    let temporary = parent.join(format!(".{file_name}.{}.dbsual-tmp", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|_| {
            SqlReadError::new(
                "EXPORT_DIRECTORY_UNAVAILABLE",
                "No se pudo preparar el archivo temporal de exportación.",
            )
        })?;

    let result =
        export_rows_to_file(&pool, &database, &table, &mut file, delimiter, &null_marker).await;
    match result {
        Ok(rows_written) => {
            if file.sync_all().is_err() {
                drop(file);
                let _ = fs::remove_file(&temporary);
                return Err(SqlReadError::new(
                    "CSV_EXPORT_FAILED",
                    "No se pudo guardar el archivo CSV completo.",
                ));
            }
            drop(file);
            if crate::vault::publish_without_overwrite(&temporary, &destination).is_err() {
                let _ = fs::remove_file(&temporary);
                return Err(SqlReadError::new(
                    "EXPORT_DESTINATION_EXISTS",
                    "No se pudo publicar el CSV porque la ruta ya existe o no está disponible.",
                ));
            }
            Ok(CsvExportResult { rows_written })
        }
        Err(error) => {
            drop(file);
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

async fn export_rows_to_file(
    pool: &MySqlPool,
    database: &str,
    table: &str,
    file: &mut File,
    delimiter: u8,
    null_marker: &str,
) -> Result<u64, SqlReadError> {
    let mut connection = pool
        .acquire()
        .await
        .map_err(|_| SqlReadError::new("CONNECTION_CLOSED", "La conexión ya no está disponible."))?
        .detach();
    let escaped_database = database.replace('`', "``");
    let escaped_table = table.replace('`', "``");
    connection
        .execute(format!("USE `{escaped_database}`").as_str())
        .await
        .map_err(|_| {
            SqlReadError::new(
                "EXPORT_DATABASE_UNAVAILABLE",
                "No se pudo seleccionar la base de datos de exportación.",
            )
        })?;
    let table_type: Option<String> = sqlx::query_scalar(
        "SELECT CAST(TABLE_TYPE AS CHAR) FROM information_schema.TABLES WHERE TABLE_SCHEMA=? AND TABLE_NAME=?",
    )
    .bind(database)
    .bind(table)
    .fetch_optional(&mut connection)
    .await
    .map_err(|_| {
        SqlReadError::new(
            "EXPORT_METADATA_UNAVAILABLE",
            "No se pudo verificar la tabla en el catálogo del servidor.",
        )
    })?;
    if table_type.as_deref() != Some("BASE TABLE") {
        return Err(invalid_export_target());
    }
    connection
        .execute("START TRANSACTION READ ONLY")
        .await
        .map_err(|_| {
            SqlReadError::new(
                "READ_ONLY_UNAVAILABLE",
                "El servidor no pudo garantizar una transacción de solo lectura.",
            )
        })?;
    let query = format!("SELECT * FROM `{escaped_database}`.`{escaped_table}`");
    let columns = (&mut connection)
        .describe(&query)
        .await
        .map_err(|_| {
            SqlReadError::new(
                "EXPORT_COLUMNS_UNAVAILABLE",
                "No se pudieron obtener las columnas de la tabla.",
            )
        })?
        .columns()
        .iter()
        .map(|column| column.name().to_owned())
        .collect::<Vec<_>>();
    let mut writer = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .from_writer(&mut *file);
    writer
        .write_record(&columns)
        .map_err(|_| csv_write_error())?;
    let mut rows_written = 0u64;
    let mut stream = sqlx::query(&query).fetch(&mut connection);
    let write_result = async {
        while let Some(row) = stream.try_next().await.map_err(|_| {
            SqlReadError::new(
                "CSV_EXPORT_FAILED",
                "El servidor interrumpió la lectura de la tabla.",
            )
        })? {
            let mut values = Vec::with_capacity(row.len());
            for index in 0..row.len() {
                let value = row.try_get_raw(index).map_err(|_| csv_value_error())?;
                if value.is_null() {
                    values.push(null_marker.to_owned());
                    continue;
                }
                let bytes = value.as_bytes().map_err(|_| csv_value_error())?;
                let type_name = row.columns()[index].type_info().name().to_ascii_uppercase();
                if matches!(
                    type_name.as_str(),
                    "BINARY"
                        | "VARBINARY"
                        | "TINYBLOB"
                        | "BLOB"
                        | "MEDIUMBLOB"
                        | "LONGBLOB"
                        | "GEOMETRY"
                        | "BIT"
                ) {
                    values.push(escape_csv_cell(
                        &format!(
                            "0x{}",
                            bytes
                                .iter()
                                .map(|byte| format!("{byte:02x}"))
                                .collect::<String>()
                        ),
                        null_marker,
                    ));
                } else {
                    let text = String::from_utf8(bytes.to_vec()).map_err(|_| csv_value_error())?;
                    values.push(escape_csv_cell(&text, null_marker));
                }
            }
            writer
                .write_record(&values)
                .map_err(|_| csv_write_error())?;
            rows_written = rows_written.saturating_add(1);
        }
        Ok::<(), SqlReadError>(())
    }
    .await;
    drop(stream);
    let rollback = connection.execute("ROLLBACK").await;
    if rollback.is_err() {
        let _ = connection.close().await;
        return Err(SqlReadError::new(
            "QUERY_STATE_UNKNOWN",
            "La lectura terminó, pero no se pudo cerrar la transacción de forma segura.",
        ));
    }
    write_result?;
    writer.flush().map_err(|_| csv_write_error())?;
    Ok(rows_written)
}

fn escape_csv_cell(value: &str, null_marker: &str) -> String {
    if value == null_marker || value.starts_with('\\') {
        format!("\\{value}")
    } else {
        value.to_owned()
    }
}

fn invalid_export_path() -> SqlReadError {
    SqlReadError::new("INVALID_EXPORT_PATH", "Elige una ruta de archivo válida.")
}

fn invalid_export_target() -> SqlReadError {
    SqlReadError::new(
        "INVALID_EXPORT_TARGET",
        "La base o tabla ya no existe o no es una tabla visible.",
    )
}

fn csv_write_error() -> SqlReadError {
    SqlReadError::new("CSV_EXPORT_FAILED", "No se pudo escribir el archivo CSV.")
}

fn csv_value_error() -> SqlReadError {
    SqlReadError::new(
        "CSV_EXPORT_VALUE_UNSUPPORTED",
        "La tabla contiene un valor que no se puede representar como texto UTF-8 o hexadecimal.",
    )
}

/// Acepta una única consulta SELECT y rechaza palabras y construcciones con
/// efectos laterales. El servidor añade una transacción de solo lectura como
/// segunda barrera; este análisis conservador no sustituye esa barrera.
pub(crate) fn validate_read_query(sql: &str) -> Result<(), SqlReadError> {
    if sql.trim().is_empty() || sql.len() > MAX_QUERY_BYTES {
        return Err(SqlReadError::new(
            "INVALID_READ_QUERY",
            "La consulta está vacía o supera el tamaño permitido.",
        ));
    }

    let mut words = Vec::new();
    let mut chars = sql.char_indices().peekable();
    let mut ended = false;
    while let Some((_, ch)) = chars.next() {
        if ended {
            if ch == ';' || ch.is_whitespace() {
                continue;
            }
            return Err(invalid_query());
        }
        match ch {
            '\'' | '"' | '`' => {
                let quote = ch;
                let mut closed = false;
                while let Some((_, next)) = chars.next() {
                    if next == '\\' && quote != '`' {
                        chars.next();
                    } else if next == quote {
                        if chars.peek().is_some_and(|(_, peek)| *peek == quote) {
                            chars.next();
                        } else {
                            closed = true;
                            break;
                        }
                    }
                }
                if !closed {
                    return Err(invalid_query());
                }
            }
            '-' if chars.peek().is_some_and(|(_, next)| *next == '-') => {
                chars.next();
                while let Some((_, next)) = chars.next() {
                    if next == '\n' {
                        break;
                    }
                }
            }
            '#' => {
                while let Some((_, next)) = chars.next() {
                    if next == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek().is_some_and(|(_, next)| *next == '*') => {
                chars.next();
                if chars.peek().is_some_and(|(_, next)| *next == '!') {
                    return Err(invalid_query());
                }
                let mut closed = false;
                while let Some((_, next)) = chars.next() {
                    if next == '*' && chars.peek().is_some_and(|(_, peek)| *peek == '/') {
                        chars.next();
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    return Err(invalid_query());
                }
            }
            ';' => ended = true,
            ':' if chars.peek().is_some_and(|(_, next)| *next == '=') => {
                return Err(invalid_query())
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let mut word = String::from(c);
                while chars.peek().is_some_and(|(_, next)| {
                    next.is_ascii_alphanumeric() || *next == '_' || *next == '$'
                }) {
                    word.push(chars.next().expect("peeked character").1);
                }
                words.push(word.to_ascii_uppercase());
            }
            _ => {}
        }
    }

    let forbidden = [
        "INTO",
        "UPDATE",
        "DELETE",
        "INSERT",
        "REPLACE",
        "CREATE",
        "ALTER",
        "DROP",
        "TRUNCATE",
        "CALL",
        "DO",
        "SET",
        "LOCK",
        "UNLOCK",
        "GRANT",
        "REVOKE",
        "LOAD",
        "OUTFILE",
        "DUMPFILE",
        "HANDLER",
        "GET_LOCK",
        "RELEASE_LOCK",
        "SLEEP",
        "BENCHMARK",
        "PROCEDURE",
    ];
    if words.first().map(String::as_str) != Some("SELECT")
        || words.iter().any(|word| forbidden.contains(&word.as_str()))
    {
        return Err(invalid_query());
    }
    Ok(())
}

pub(crate) fn is_single_change_plan(sql: &str) -> bool {
    if sql.trim().is_empty() || sql.len() > MAX_QUERY_BYTES {
        return false;
    }
    let mut chars = sql.chars().peekable();
    let mut first_word: Option<String> = None;
    let mut terminated = false;
    while let Some(ch) = chars.next() {
        match ch {
            '-' if chars.peek() == Some(&'-') => {
                chars.next();
                for next in chars.by_ref() {
                    if next == '\n' {
                        break;
                    }
                }
            }
            '#' => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                if chars.peek() == Some(&'!') {
                    return false;
                }
                let mut closed = false;
                while let Some(next) = chars.next() {
                    if next == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    return false;
                }
            }
            '\'' | '"' | '`' => {
                if terminated {
                    return false;
                }
                let quote = ch;
                let mut closed = false;
                while let Some(next) = chars.next() {
                    if next == '\\' && quote != '`' {
                        if chars.next().is_none() {
                            return false;
                        }
                    } else if next == quote {
                        if chars.peek() == Some(&quote) {
                            chars.next();
                        } else {
                            closed = true;
                            break;
                        }
                    }
                }
                if !closed {
                    return false;
                }
            }
            ';' => {
                if terminated {
                    return false;
                }
                terminated = true;
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                if terminated {
                    return false;
                }
                let mut word = String::from(c);
                while chars.peek().is_some_and(|next| {
                    next.is_ascii_alphanumeric() || *next == '_' || *next == '$'
                }) {
                    word.push(chars.next().expect("peeked character"));
                }
                if first_word.is_none() {
                    first_word = Some(word.to_ascii_uppercase());
                }
            }
            c if c.is_whitespace() => {}
            _ if terminated => return false,
            _ => {}
        }
    }
    !matches!(
        first_word.as_deref(),
        None | Some("SELECT")
            | Some("WITH")
            | Some("SHOW")
            | Some("DESCRIBE")
            | Some("DESC")
            | Some("EXPLAIN")
            | Some("TABLE")
            | Some("VALUES")
    )
}

fn invalid_query() -> SqlReadError {
    SqlReadError::new(
        "READ_ONLY_QUERY_REQUIRED",
        "Solo se permite una consulta SELECT de lectura en esta acción.",
    )
}

fn is_mysql_error_number(error: &sqlx::Error, number: u16) -> bool {
    matches!(error, sqlx::Error::Database(database)
        if database.try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>()
            .is_some_and(|mysql| mysql.number() == number))
}

fn validate_result_offset(offset: u64) -> Result<(), SqlReadError> {
    if offset > MAX_RESULT_OFFSET {
        return Err(SqlReadError::new(
            "INVALID_RESULT_OFFSET",
            "El tramo solicitado supera el límite de consulta. Agrega filtros para reducir el resultado.",
        ));
    }
    Ok(())
}

/// Ejecuta SELECT en el pool MySQL ya abierto. No guarda SQL ni filas en disco
/// ni registra el contenido de la consulta en errores.
#[tauri::command]
pub async fn execute_read_query(
    state: State<'_, crate::AppState>,
    connection_id: String,
    database: String,
    sql: String,
    offset: u64,
    query_id: String,
) -> Result<SqlReadResult, SqlReadError> {
    validate_read_query(&sql)?;
    validate_result_offset(offset)?;
    if uuid::Uuid::parse_str(&query_id).is_err() {
        return Err(SqlReadError::new(
            "INVALID_QUERY_ID",
            "No se pudo identificar de forma segura esta consulta.",
        ));
    }
    if connection_id.len() > 128
        || database.is_empty()
        || database.len() > 255
        || database.chars().any(char::is_control)
    {
        return Err(SqlReadError::new(
            "INVALID_QUERY_TARGET",
            "El destino de la consulta no es válido.",
        ));
    }

    let pool = match state.active.0.try_lock() {
        Ok(active) => active.get(&connection_id).cloned(),
        Err(_) => {
            return Err(SqlReadError::new(
                "CONNECTION_BUSY",
                "La lista de conexiones está ocupada; vuelve a intentarlo.",
            ));
        }
    }
    .ok_or(SqlReadError::new(
        "CONNECTION_CLOSED",
        "Abre la conexión antes de ejecutar SQL.",
    ))?;
    let registry = Arc::clone(&state.active_read_queries);
    drop(state);
    tauri::async_runtime::spawn_blocking(move || {
        tauri::async_runtime::block_on(run_read_query_registered(
            pool,
            database,
            sql,
            offset,
            Some((registry, query_id, connection_id)),
        ))
    })
    .await
    .map_err(|_| {
        SqlReadError::new(
            "QUERY_FAILED",
            "No se pudo completar la consulta en el núcleo de DBSUAL.",
        )
    })?
}

/// Lee una página de tabla con orden y filtro opcionales. Los valores del
/// filtro se enlazan como parámetros; los identificadores se validan contra
/// information_schema antes de construir la consulta.
#[tauri::command]
pub async fn read_table_page(
    state: State<'_, crate::AppState>,
    connection_id: String,
    database: String,
    table: String,
    options: TablePageOptions,
    offset: u64,
    query_id: String,
) -> Result<SqlReadResult, SqlReadError> {
    validate_table_page_request(
        &connection_id,
        &database,
        &table,
        &options,
        offset,
        &query_id,
    )?;
    let pool = match state.active.0.try_lock() {
        Ok(active) => active.get(&connection_id).cloned(),
        Err(_) => {
            return Err(SqlReadError::new(
                "CONNECTION_BUSY",
                "La lista de conexiones está ocupada; vuelve a intentarlo.",
            ));
        }
    }
    .ok_or(SqlReadError::new(
        "CONNECTION_CLOSED",
        "Abre la conexión antes de consultar la tabla.",
    ))?;
    let registry = Arc::clone(&state.active_read_queries);
    drop(state);
    tauri::async_runtime::spawn_blocking(move || {
        tauri::async_runtime::block_on(run_table_page_registered(
            pool,
            database,
            table,
            options,
            offset,
            Some((registry, query_id, connection_id)),
        ))
    })
    .await
    .map_err(|_| {
        SqlReadError::new(
            "QUERY_FAILED",
            "No se pudo completar la consulta de tabla en el núcleo de DBSUAL.",
        )
    })?
}

#[tauri::command]
pub async fn cancel_read_query(
    state: State<'_, crate::AppState>,
    query_id: String,
) -> Result<(), SqlReadError> {
    if uuid::Uuid::parse_str(&query_id).is_err() {
        return Err(SqlReadError::new(
            "INVALID_QUERY_ID",
            "No se pudo identificar de forma segura esta consulta.",
        ));
    }
    let target = state
        .active_read_queries
        .lock()
        .map_err(|_| {
            SqlReadError::new(
                "QUERY_CANCEL_UNCONFIRMED",
                "No se pudo verificar el estado de la consulta.",
            )
        })?
        .get(&query_id)
        .cloned()
        .ok_or(SqlReadError::new(
            "QUERY_NOT_ACTIVE",
            "La consulta ya terminó o no se encuentra activa.",
        ))?;
    let pool = state
        .active
        .0
        .try_lock()
        .map_err(|_| {
            SqlReadError::new(
                "QUERY_CANCEL_UNCONFIRMED",
                "La conexión está ocupada; no se pudo enviar la cancelación.",
            )
        })?
        .get(&target.0)
        .cloned()
        .ok_or(SqlReadError::new(
            "CONNECTION_CLOSED",
            "La conexión se cerró; no se pudo confirmar la cancelación.",
        ))?;
    cancel_mysql_query(pool, target.1).await
}

pub(crate) async fn cancel_connection_read_queries(
    pool: MySqlPool,
    registry: Arc<ActiveReadQueries>,
    connection_id: &str,
) {
    let thread_ids = connection_query_threads(&registry, connection_id);
    for thread_id in thread_ids {
        let _ = cancel_mysql_query(pool.clone(), thread_id).await;
    }
}

fn connection_query_threads(registry: &ActiveReadQueries, connection_id: &str) -> Vec<u64> {
    let mut thread_ids: Vec<u64> = registry
        .lock()
        .map(|active| {
            active
                .values()
                .filter_map(|(active_connection, thread_id)| {
                    (active_connection == connection_id).then_some(*thread_id)
                })
                .collect()
        })
        .unwrap_or_default();
    thread_ids.sort_unstable();
    thread_ids
}

pub(crate) async fn cancel_mysql_query(
    pool: MySqlPool,
    thread_id: u64,
) -> Result<(), SqlReadError> {
    sqlx::query(&format!("KILL QUERY {thread_id}"))
        .execute(&pool)
        .await
        .map_err(|error| {
            if is_mysql_error_number(&error, 1094) {
                SqlReadError::new(
                    "QUERY_NOT_ACTIVE",
                    "La consulta terminó antes de que MySQL recibiera la cancelación.",
                )
            } else {
                SqlReadError::new(
                    "QUERY_CANCEL_UNCONFIRMED",
                    "MySQL no confirmó la cancelación de la consulta.",
                )
            }
        })
        .map(|_| ())
}

#[cfg(test)]
pub(crate) async fn run_read_query(
    pool: MySqlPool,
    database: String,
    sql: String,
    offset: u64,
) -> Result<SqlReadResult, SqlReadError> {
    run_read_query_registered(pool, database, sql, offset, None).await
}

fn validate_table_page_request(
    connection_id: &str,
    database: &str,
    table: &str,
    options: &TablePageOptions,
    offset: u64,
    query_id: &str,
) -> Result<(), SqlReadError> {
    validate_result_offset(offset)?;
    let valid_identifier = |value: &str| {
        !value.is_empty() && value.len() <= 255 && !value.chars().any(char::is_control)
    };
    if connection_id.is_empty()
        || connection_id.len() > 128
        || !valid_identifier(database)
        || !valid_identifier(table)
        || uuid::Uuid::parse_str(query_id).is_err()
    {
        return Err(SqlReadError::new(
            "INVALID_QUERY_TARGET",
            "El destino de la consulta no es válido.",
        ));
    }
    match (&options.sort_column, &options.sort_direction) {
        (Some(column), Some(direction))
            if valid_identifier(column) && matches!(direction.as_str(), "asc" | "desc") => {}
        (None, None) => {}
        _ => {
            return Err(SqlReadError::new(
                "INVALID_TABLE_QUERY_OPTIONS",
                "La columna o dirección de orden no es válida.",
            ));
        }
    }
    match (
        &options.filter_column,
        &options.filter_mode,
        &options.filter_value,
    ) {
        (Some(column), Some(mode), Some(value))
            if valid_identifier(column)
                && value.len() <= 4096
                && matches!(mode.as_str(), "equals" | "contains") => {}
        (None, None, None) => {}
        _ => {
            return Err(SqlReadError::new(
                "INVALID_TABLE_QUERY_OPTIONS",
                "La columna o el filtro no es válido.",
            ));
        }
    }
    Ok(())
}

fn quote_mysql_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}

pub(crate) async fn run_table_page_registered(
    pool: MySqlPool,
    database: String,
    table: String,
    options: TablePageOptions,
    offset: u64,
    registration: Option<(Arc<ActiveReadQueries>, String, String)>,
) -> Result<SqlReadResult, SqlReadError> {
    let object_type: Option<String> = sqlx::query_scalar(
        "SELECT CAST(TABLE_TYPE AS CHAR) FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?",
    )
    .bind(&database)
    .bind(&table)
    .fetch_optional(&pool)
    .await
    .map_err(|_| SqlReadError::new("QUERY_FAILED", "No se pudo validar la tabla."))?;
    let object_type = object_type.ok_or(SqlReadError::new(
        "OBJECT_NOT_FOUND",
        "La tabla ya no existe o no es visible.",
    ))?;
    if !matches!(object_type.as_str(), "BASE TABLE" | "VIEW") {
        return Err(SqlReadError::new(
            "INVALID_OBJECT_TYPE",
            "El objeto seleccionado no es una tabla ni una vista.",
        ));
    }
    let columns: Vec<(String, String)> = sqlx::query_as(
        "SELECT CAST(COLUMN_NAME AS CHAR), CAST(DATA_TYPE AS CHAR) FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
    )
    .bind(&database)
    .bind(&table)
    .fetch_all(&pool)
    .await
    .map_err(|_| SqlReadError::new("QUERY_FAILED", "No se pudieron validar las columnas."))?;
    if columns.is_empty() {
        return Err(SqlReadError::new(
            "OBJECT_NOT_FOUND",
            "La tabla ya no existe o no es visible.",
        ));
    }
    for requested in [options.sort_column.as_ref(), options.filter_column.as_ref()]
        .into_iter()
        .flatten()
    {
        if !columns.iter().any(|(column, _)| column == requested) {
            return Err(SqlReadError::new(
                "COLUMN_NOT_FOUND",
                "La columna seleccionada ya no existe en esta tabla.",
            ));
        }
    }

    let primary_keys: Vec<String> = if object_type == "BASE TABLE" {
        sqlx::query_scalar(
            "SELECT CAST(COLUMN_NAME AS CHAR) FROM information_schema.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND CONSTRAINT_NAME = 'PRIMARY' ORDER BY ORDINAL_POSITION",
        )
        .bind(&database)
        .bind(&table)
        .fetch_all(&pool)
        .await
        .map_err(|_| SqlReadError::new("QUERY_FAILED", "No se pudo preparar el orden estable."))?
    } else {
        Vec::new()
    };

    let quoted_table = quote_mysql_identifier(&table);
    let projection = columns
        .iter()
        .map(|(name, data_type)| {
            let quoted = quote_mysql_identifier(name);
            if matches!(
                data_type.to_ascii_lowercase().as_str(),
                "binary"
                    | "varbinary"
                    | "tinyblob"
                    | "blob"
                    | "mediumblob"
                    | "longblob"
                    | "geometry"
            ) {
                quoted
            } else {
                format!("CAST({quoted} AS CHAR) AS {quoted}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let table_alias = quote_mysql_identifier("_dbsual_source");
    let mut sql = format!("SELECT {projection} FROM {quoted_table} AS {table_alias}");
    let bind_value = if let (Some(column), Some(mode), Some(value)) = (
        options.filter_column.as_deref(),
        options.filter_mode.as_deref(),
        options.filter_value,
    ) {
        let column = quote_mysql_identifier(column);
        match mode {
            "equals" => sql.push_str(&format!(" WHERE CAST({column} AS CHAR) = ?")),
            "contains" => sql.push_str(&format!(" WHERE LOCATE(?, CAST({column} AS CHAR)) > 0")),
            _ => {
                return Err(SqlReadError::new(
                    "INVALID_TABLE_QUERY_OPTIONS",
                    "El filtro no es válido.",
                ));
            }
        }
        Some(value)
    } else {
        None
    };

    let mut order_columns: Vec<(String, &str)> = Vec::new();
    if let (Some(column), Some(direction)) = (
        options.sort_column.as_ref(),
        options.sort_direction.as_deref(),
    ) {
        order_columns.push((
            column.clone(),
            if direction == "desc" { "DESC" } else { "ASC" },
        ));
    }
    for key in primary_keys {
        if !order_columns.iter().any(|(column, _)| column == &key) {
            order_columns.push((key, "ASC"));
        }
    }
    if !order_columns.is_empty() {
        sql.push_str(" ORDER BY ");
        sql.push_str(
            &order_columns
                .iter()
                .map(|(column, direction)| {
                    format!(
                        "{table_alias}.{} {direction}",
                        quote_mysql_identifier(column)
                    )
                })
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    let total_rows = if offset == 0 {
        let quoted_database = quote_mysql_identifier(&database);
        let mut count_sql =
            format!("SELECT COUNT(*) FROM {quoted_database}.{quoted_table} AS {table_alias}");
        if let Some(value) = bind_value.as_ref() {
            let column = quote_mysql_identifier(options.filter_column.as_deref().ok_or(
                SqlReadError::new("INVALID_TABLE_QUERY_OPTIONS", "El filtro no es válido."),
            )?);
            match options.filter_mode.as_deref() {
                Some("equals") => count_sql.push_str(&format!(" WHERE CAST({column} AS CHAR) = ?")),
                Some("contains") => {
                    count_sql.push_str(&format!(" WHERE LOCATE(?, CAST({column} AS CHAR)) > 0"))
                }
                _ => {
                    return Err(SqlReadError::new(
                        "INVALID_TABLE_QUERY_OPTIONS",
                        "El filtro no es válido.",
                    ))
                }
            }
            Some(
                sqlx::query_scalar::<_, i64>(&count_sql)
                    .bind(value)
                    .fetch_one(&pool)
                    .await
                    .map_err(|_| {
                        SqlReadError::new(
                            "QUERY_FAILED",
                            "No se pudo contar los registros de la tabla.",
                        )
                    })? as u64,
            )
        } else {
            Some(
                sqlx::query_scalar::<_, i64>(&count_sql)
                    .fetch_one(&pool)
                    .await
                    .map_err(|_| {
                        SqlReadError::new(
                            "QUERY_FAILED",
                            "No se pudo contar los registros de la tabla.",
                        )
                    })? as u64,
            )
        }
    } else {
        None
    };
    let mut result = run_read_query_registered_with_bind(
        pool,
        database,
        sql,
        offset,
        registration,
        bind_value,
        true,
    )
    .await?;
    result.total_rows = total_rows;
    Ok(result)
}

pub(crate) async fn run_read_query_registered(
    pool: MySqlPool,
    database: String,
    sql: String,
    offset: u64,
    registration: Option<(Arc<ActiveReadQueries>, String, String)>,
) -> Result<SqlReadResult, SqlReadError> {
    run_read_query_registered_with_bind(pool, database, sql, offset, registration, None, false)
        .await
}

async fn run_read_query_registered_with_bind(
    pool: MySqlPool,
    database: String,
    sql: String,
    offset: u64,
    registration: Option<(Arc<ActiveReadQueries>, String, String)>,
    bind_value: Option<String>,
    direct_page_query: bool,
) -> Result<SqlReadResult, SqlReadError> {
    validate_result_offset(offset)?;
    let started = Instant::now();
    let mut connection = pool
        .acquire()
        .await
        .map_err(|_| {
            SqlReadError::new(
                "QUERY_FAILED",
                "No se pudo preparar la conexión para la consulta.",
            )
        })?
        .detach();
    let _active_read = if let Some((registry, query_id, connection_id)) = registration {
        let thread_id: u64 = sqlx::query_scalar("SELECT CONNECTION_ID()")
            .fetch_one(&mut connection)
            .await
            .map_err(|_| {
                SqlReadError::new(
                    "QUERY_CANCEL_UNAVAILABLE",
                    "No se pudo preparar la cancelación segura de esta consulta.",
                )
            })?;
        let mut active = registry.lock().map_err(|_| {
            SqlReadError::new(
                "QUERY_CANCEL_UNAVAILABLE",
                "No se pudo registrar de forma segura esta consulta.",
            )
        })?;
        if active.contains_key(&query_id) {
            return Err(SqlReadError::new(
                "DUPLICATE_QUERY_ID",
                "Esta consulta ya está registrada; vuelve a intentarlo.",
            ));
        }
        active.insert(query_id.clone(), (connection_id, thread_id));
        drop(active);
        Some(ActiveReadGuard { registry, query_id })
    } else {
        None
    };
    let escaped_database = database.replace('`', "``");
    let use_sql = format!("USE `{escaped_database}`");
    connection.execute(use_sql.as_str()).await.map_err(|_| {
        SqlReadError::new(
            "QUERY_FAILED",
            "No se pudo seleccionar la base de datos de destino.",
        )
    })?;
    connection
        .execute("START TRANSACTION READ ONLY")
        .await
        .map_err(|_| {
            SqlReadError::new(
                "READ_ONLY_UNAVAILABLE",
                "El servidor no pudo garantizar una transacción de solo lectura.",
            )
        })?;
    let bounded_sql = if direct_page_query {
        format!("{sql} LIMIT {} OFFSET {offset}", MAX_RESULT_ROWS + 1)
    } else {
        format!(
            "SELECT * FROM ({}) AS `_dbsual_result` LIMIT {} OFFSET {}",
            sql.trim().trim_end_matches(';'),
            MAX_RESULT_ROWS + 1,
            offset
        )
    };
    let columns = (&mut connection)
        .describe(&bounded_sql)
        .await
        .map_err(|_| {
            SqlReadError::new(
                "QUERY_FAILED",
                "El servidor rechazó la consulta de lectura.",
            )
        })?
        .columns()
        .iter()
        .map(|column| column.name().to_owned())
        .collect();
    let query_timeout = std::time::Duration::from_secs(15);
    let fetched = if let Some(value) = bind_value {
        tokio::time::timeout(
            query_timeout,
            sqlx::query(&bounded_sql)
                .bind(value)
                .fetch_all(&mut connection),
        )
        .await
    } else {
        tokio::time::timeout(
            query_timeout,
            sqlx::raw_sql(&bounded_sql).fetch_all(&mut connection),
        )
        .await
    };
    let fetched = match fetched {
        Ok(result) => result,
        Err(_) => {
            let _ = connection.close().await;
            return Err(SqlReadError::new(
                "QUERY_TIMEOUT",
                "La consulta superó el límite de 15 segundos.",
            ));
        }
    };
    let rollback = connection.execute("ROLLBACK").await;
    if rollback.is_err() {
        let _ = connection.close().await;
        return Err(SqlReadError::new(
            "QUERY_STATE_UNKNOWN",
            "La consulta terminó, pero no se pudo confirmar el cierre de la transacción.",
        ));
    }
    let mut rows = fetched.map_err(|error| {
        if is_mysql_error_number(&error, 1317) {
            SqlReadError::new("QUERY_CANCELLED", "La consulta fue cancelada en MySQL.")
        } else {
            SqlReadError::new(
                "QUERY_FAILED",
                "El servidor rechazó la consulta de lectura.",
            )
        }
    })?;
    let mut has_more = rows.len() > MAX_RESULT_ROWS;
    rows.truncate(MAX_RESULT_ROWS);
    let rows_in_page = rows.len();
    let mut total_bytes = 0usize;
    let mut values = Vec::with_capacity(rows.len());
    for (row_index, row) in rows.into_iter().enumerate() {
        if total_bytes >= MAX_RESULT_BYTES && !values.is_empty() {
            has_more = true;
            break;
        }
        let mut row_values = Vec::with_capacity(row.len());
        for index in 0..row.len() {
            let value = row.try_get_raw(index).map_err(|_| {
                SqlReadError::new("QUERY_FAILED", "No se pudo leer una celda del resultado.")
            })?;
            if matches!(
                row.columns()[index]
                    .type_info()
                    .name()
                    .to_ascii_uppercase()
                    .as_str(),
                "BINARY"
                    | "VARBINARY"
                    | "TINYBLOB"
                    | "BLOB"
                    | "MEDIUMBLOB"
                    | "LONGBLOB"
                    | "GEOMETRY"
            ) {
                return Err(SqlReadError::new(
                    "UNSUPPORTED_RESULT_TYPE",
                    "El resultado contiene datos binarios que el editor aún no puede mostrar.",
                ));
            }
            let cell = if value.is_null() {
                None
            } else {
                let bytes = value.as_bytes().map_err(|_| {
                    SqlReadError::new(
                        "UNSUPPORTED_RESULT_TYPE",
                        "El resultado contiene un tipo que DBSUAL aún no puede mostrar.",
                    )
                })?;
                total_bytes = total_bytes.saturating_add(bytes.len());
                Some(String::from_utf8(bytes.to_vec()).map_err(|_| {
                    SqlReadError::new(
                        "UNSUPPORTED_RESULT_ENCODING",
                        "El resultado incluye texto que no se puede mostrar con seguridad.",
                    )
                })?)
            };
            row_values.push(cell);
        }
        values.push(row_values);
        if total_bytes >= MAX_RESULT_BYTES {
            has_more |= row_index + 1 < rows_in_page;
            break;
        }
    }
    let candidate_offset = offset.saturating_add(values.len() as u64);
    let next_offset =
        (has_more && candidate_offset <= MAX_RESULT_OFFSET).then_some(candidate_offset);
    Ok(SqlReadResult {
        columns,
        returned_rows: values.len(),
        rows: values,
        has_more,
        next_offset,
        elapsed_ms: started.elapsed().as_millis(),
        total_rows: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnect_cancels_only_queries_bound_to_that_connection() {
        let registry = Mutex::new(HashMap::from([
            ("query-a".to_owned(), ("connection-a".to_owned(), 11)),
            ("query-b".to_owned(), ("connection-b".to_owned(), 12)),
            ("query-c".to_owned(), ("connection-a".to_owned(), 13)),
        ]));
        assert_eq!(
            connection_query_threads(&registry, "connection-a"),
            [11, 13]
        );
        assert_eq!(connection_query_threads(&registry, "connection-b"), [12]);
        assert!(connection_query_threads(&registry, "closed-connection").is_empty());
    }

    #[test]
    fn allows_select_with_literals_and_trailing_semicolon() {
        assert!(validate_read_query("SELECT 'UPDATE; DROP', `select` FROM items;").is_ok());
        assert!(validate_read_query("SELECT * FROM `order``items`").is_ok());
        assert!(validate_read_query("SELECT 1 /* harmless comment */").is_ok());
    }

    #[test]
    fn rejects_mutations_multi_statements_and_uncertain_syntax() {
        for sql in [
            "UPDATE items SET value=1",
            "SELECT 1; DELETE FROM items",
            "SELECT * FROM items INTO OUTFILE '/tmp/x'",
            "SELECT GET_LOCK('x', 1)",
            "SELECT /*!50000 SLEEP(5) */ 1",
            "WITH c AS (SELECT 1) SELECT * FROM c",
            "SELECT @x := 1",
            "-- comment only",
        ] {
            assert!(validate_read_query(sql).is_err(), "accepted: {sql}");
        }
    }

    #[test]
    fn table_page_options_allow_bound_text_and_reject_untrusted_modes() {
        let safe_target = (
            "connection",
            "app",
            "items",
            "123e4567-e89b-12d3-a456-426614174000",
        );
        let filter = TablePageOptions {
            sort_column: Some("display`name".into()),
            sort_direction: Some("desc".into()),
            filter_column: Some("payload".into()),
            filter_mode: Some("contains".into()),
            filter_value: Some("' OR 1=1 --".into()),
        };
        assert!(validate_table_page_request(
            safe_target.0,
            safe_target.1,
            safe_target.2,
            &filter,
            0,
            safe_target.3,
        )
        .is_ok());
        assert_eq!(quote_mysql_identifier("display`name"), "`display``name`");

        let mut bad_direction = filter.clone();
        bad_direction.sort_direction = Some("desc; DROP TABLE items".into());
        assert_eq!(
            validate_table_page_request(
                safe_target.0,
                safe_target.1,
                safe_target.2,
                &bad_direction,
                0,
                safe_target.3,
            )
            .unwrap_err()
            .code,
            "INVALID_TABLE_QUERY_OPTIONS"
        );

        let mut incomplete_filter = filter;
        incomplete_filter.filter_value = None;
        assert_eq!(
            validate_table_page_request(
                safe_target.0,
                safe_target.1,
                safe_target.2,
                &incomplete_filter,
                0,
                safe_target.3,
            )
            .unwrap_err()
            .code,
            "INVALID_TABLE_QUERY_OPTIONS"
        );
    }

    #[test]
    fn prepared_change_accepts_one_non_read_statement_and_rejects_reads_or_batches() {
        for sql in [
            "UPDATE customers SET note='a;b' WHERE id=1;",
            "/* reviewed */ DELETE FROM customers WHERE id=7 -- end\n",
            "INSERT INTO customers(name) VALUES ('Ada')",
        ] {
            assert!(is_single_change_plan(sql), "rejected: {sql}");
        }
        for sql in [
            "SELECT * FROM customers",
            "WITH c AS (SELECT 1) SELECT * FROM c",
            "UPDATE customers SET active=0; DROP DATABASE app",
            "UPDATE customers SET active=0; /* comment */ DELETE FROM customers",
            "/*! UPDATE customers SET active=0 */",
            "-- comments only",
        ] {
            assert!(!is_single_change_plan(sql), "accepted: {sql}");
        }
    }

    #[test]
    fn csv_export_escapes_null_marker_collisions_and_leading_backslashes() {
        assert_eq!(escape_csv_cell("\\N", "\\N"), "\\\\N");
        assert_eq!(escape_csv_cell("\\path", "\\N"), "\\\\path");
        assert_eq!(escape_csv_cell("plain text", "\\N"), "plain text");
        assert_eq!(escape_csv_cell("NULL", "NULL"), "\\NULL");
    }

    #[test]
    fn result_offset_is_bounded_and_accepts_the_limit() {
        assert!(validate_result_offset(MAX_RESULT_OFFSET).is_ok());
        assert_eq!(
            validate_result_offset(MAX_RESULT_OFFSET + 1)
                .unwrap_err()
                .code,
            "INVALID_RESULT_OFFSET"
        );
    }

    #[test]
    fn blocking_worker_can_drive_the_non_send_sqlx_future() {
        let result = tauri::async_runtime::block_on(async {
            tauri::async_runtime::spawn_blocking(|| tauri::async_runtime::block_on(async { 42_u8 }))
                .await
        });
        assert_eq!(result.expect("blocking worker completed"), 42);
    }
}
