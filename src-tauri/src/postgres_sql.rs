//! Lecturas PostgreSQL acotadas para el editor SQL.

use std::time::Instant;

use futures_util::TryStreamExt;
use serde::Serialize;
use serde_json::Value;
use sqlx::{Column, Executor, Row};
use tauri::State;

const MAX_QUERY_BYTES: usize = 64 * 1024;
const MAX_ROWS: usize = 200;
const MAX_RESULT_BYTES: usize = 1024 * 1024;
const MAX_OFFSET: u64 = 1_000_000;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostgresReadError {
    pub code: &'static str,
    pub message: &'static str,
}

impl PostgresReadError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostgresReadResult {
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
    returned_rows: usize,
    has_more: bool,
    next_offset: Option<u64>,
    elapsed_ms: u128,
}

#[tauri::command]
pub async fn execute_postgres_read_query(
    state: State<'_, crate::AppState>,
    connection_id: String,
    database: String,
    sql: String,
    offset: u64,
    query_id: String,
) -> Result<PostgresReadResult, PostgresReadError> {
    validate_request(&connection_id, &database, &sql, offset, &query_id)?;
    let active = state.active_postgres.lock().await;
    let connection = active.get(&connection_id).cloned().ok_or_else(|| {
        PostgresReadError::new(
            "CONNECTION_CLOSED",
            "Abre la conexión antes de ejecutar SQL.",
        )
    })?;
    drop(active);
    let connection = connection
        .connect_to_database(&database)
        .await
        .map_err(|_| connection_failed())?;
    let started = Instant::now();
    let result = execute(&connection, &sql, offset).await;
    connection.pool().close().await;
    result.map(|(columns, rows, has_more)| {
        let returned_rows = rows.len();
        PostgresReadResult {
            columns,
            rows,
            returned_rows,
            has_more,
            next_offset: (has_more && offset.saturating_add(returned_rows as u64) <= MAX_OFFSET)
                .then_some(offset.saturating_add(returned_rows as u64)),
            elapsed_ms: started.elapsed().as_millis(),
        }
    })
}

fn validate_request(
    connection_id: &str,
    database: &str,
    sql: &str,
    offset: u64,
    query_id: &str,
) -> Result<(), PostgresReadError> {
    if connection_id.is_empty()
        || connection_id.len() > 128
        || database.is_empty()
        || database.len() > 255
        || database.chars().any(char::is_control)
        || uuid::Uuid::parse_str(query_id).is_err()
    {
        return Err(PostgresReadError::new(
            "INVALID_READ_TARGET",
            "El destino de la consulta no es válido.",
        ));
    }
    if offset > MAX_OFFSET {
        return Err(PostgresReadError::new(
            "INVALID_RESULT_OFFSET",
            "La paginación superó el límite permitido.",
        ));
    }
    if sql.len() > MAX_QUERY_BYTES || !is_single_select(sql) {
        return Err(PostgresReadError::new(
            "INVALID_READ_QUERY",
            "La lectura requiere una sola sentencia SELECT de hasta 64 KiB.",
        ));
    }
    Ok(())
}

/// Conservative outer statement check. PostgreSQL's read-only transaction
/// remains the server-side guard against writes hidden in expressions/CTEs.
fn is_single_select(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut words = Vec::new();
    let mut ended = false;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if byte == b'-' && bytes.get(i + 1) == Some(&b'-') {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if byte == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            let mut depth = 1usize;
            while i < bytes.len() && depth > 0 {
                if bytes.get(i..i + 2) == Some(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes.get(i..i + 2) == Some(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if depth != 0 {
                return false;
            }
            continue;
        }
        if byte == b'\'' || byte == b'"' {
            let quote = byte;
            i += 1;
            let mut closed = false;
            while i < bytes.len() {
                if bytes[i] == quote {
                    if bytes.get(i + 1) == Some(&quote) {
                        i += 2;
                    } else {
                        i += 1;
                        closed = true;
                        break;
                    }
                } else if quote == b'\'' && bytes[i] == b'\\' {
                    i = (i + 2).min(bytes.len());
                } else {
                    i += 1;
                }
            }
            if !closed {
                return false;
            }
            continue;
        }
        if byte == b'$' {
            let mut end = i + 1;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            if bytes.get(end) == Some(&b'$') {
                let delimiter = &bytes[i..=end];
                let Some(relative) =
                    sql[end + 1..].find(std::str::from_utf8(delimiter).unwrap_or("\0"))
                else {
                    return false;
                };
                i = end + 1 + relative + delimiter.len();
                continue;
            }
        }
        if byte == b';' {
            if ended {
                return false;
            }
            ended = true;
            i += 1;
            continue;
        }
        if ended {
            return false;
        }
        if byte.is_ascii_alphabetic() || byte == b'_' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            words.push(sql[start..i].to_ascii_uppercase());
        } else {
            i += 1;
        }
    }
    words.first().is_some_and(|word| word == "SELECT") && !words.contains(&"INTO".to_owned())
}

async fn execute(
    connection: &crate::postgres::PostgresConnection,
    sql: &str,
    offset: u64,
) -> Result<(Vec<String>, Vec<Vec<Option<String>>>, bool), PostgresReadError> {
    let statement = sql.trim().trim_end_matches(';').trim();
    let describe_sql = format!("SELECT * FROM ({statement}) AS __dbsual_result LIMIT 0");
    let columns = connection
        .pool()
        .describe(&describe_sql)
        .await
        .map_err(|_| query_failed())?
        .columns()
        .iter()
        .map(|column| column.name().to_owned())
        .collect::<Vec<_>>();
    let mut unique_columns = std::collections::HashSet::new();
    if columns.iter().any(|column| !unique_columns.insert(column)) {
        return Err(PostgresReadError::new(
            "DUPLICATE_RESULT_COLUMNS",
            "La consulta devuelve nombres de columna repetidos; asigna alias únicos para ver los resultados.",
        ));
    }

    let mut transaction = connection
        .pool()
        .begin()
        .await
        .map_err(|_| query_failed())?;
    sqlx::query("SET TRANSACTION READ ONLY")
        .execute(&mut *transaction)
        .await
        .map_err(|_| {
            PostgresReadError::new(
                "READ_ONLY_UNAVAILABLE",
                "PostgreSQL no pudo garantizar una transacción de solo lectura.",
            )
        })?;
    sqlx::query("SET LOCAL statement_timeout = '15000ms'")
        .execute(&mut *transaction)
        .await
        .map_err(|_| query_failed())?;

    let bounded = format!(
        "SELECT to_jsonb(__dbsual_result) AS row_data FROM ({statement}) AS __dbsual_result LIMIT {} OFFSET {offset}",
        MAX_ROWS + 1
    );
    let mut stream = sqlx::query(&bounded).fetch(&mut *transaction);
    let mut rows = Vec::new();
    let mut bytes = 0usize;
    let mut has_more = false;
    while let Some(row) = stream.try_next().await.map_err(|_| query_failed())? {
        let value: Value = row.try_get("row_data").map_err(|_| query_failed())?;
        let Some(cells) = value.as_object() else {
            return Err(query_failed());
        };
        let values = columns
            .iter()
            .map(|column| match cells.get(column).unwrap_or(&Value::Null) {
                Value::Null => None,
                Value::String(text) => Some(text.clone()),
                other => Some(other.to_string()),
            })
            .collect::<Vec<_>>();
        let row_bytes = values.iter().flatten().map(String::len).sum::<usize>();
        if rows.len() == MAX_ROWS {
            has_more = true;
            break;
        }
        if bytes >= MAX_RESULT_BYTES && !rows.is_empty() {
            has_more = true;
            break;
        }
        bytes = bytes.saturating_add(row_bytes);
        rows.push(values);
        if bytes >= MAX_RESULT_BYTES {
            has_more = true;
            break;
        }
    }
    drop(stream);
    transaction.rollback().await.map_err(|_| query_failed())?;
    Ok((columns, rows, has_more))
}

fn connection_failed() -> PostgresReadError {
    PostgresReadError::new(
        "CONNECTION_FAILED",
        "No se pudo abrir la base PostgreSQL seleccionada.",
    )
}

fn query_failed() -> PostgresReadError {
    PostgresReadError::new(
        "QUERY_FAILED",
        "PostgreSQL no pudo completar la consulta de lectura.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_one_select_and_rejects_batches_and_select_into() {
        assert!(is_single_select("SELECT 1;"));
        assert!(is_single_select("/* hi */ SELECT $$a;b$$"));
        assert!(!is_single_select("UPDATE items SET n = 1"));
        assert!(!is_single_select("SELECT 1; DELETE FROM items"));
        assert!(!is_single_select("SELECT 1 INTO new_table"));
        assert!(!is_single_select(
            "SELECT 1; -- trailing second statement\n DELETE FROM items"
        ));
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::postgres::{connect, PostgresConnectConfig, PostgresTlsMode};

    #[tokio::test]
    #[ignore = "requiere PostgreSQL desechable y DBSUAL_POSTGRES_TEST_*"]
    async fn executes_bounded_select_inside_a_read_only_transaction() {
        let host = std::env::var("DBSUAL_POSTGRES_TEST_HOST").expect("host requerido");
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
        .expect("conexión de prueba");
        let schema = format!("dbsual_sql_{}", std::process::id());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(connection.pool())
            .await
            .expect("crear esquema de prueba");
        sqlx::query(&format!(
            "CREATE SEQUENCE {schema}.read_only_guard START WITH 7"
        ))
        .execute(connection.pool())
        .await
        .expect("crear secuencia de prueba");

        let (columns, rows, has_more) =
            execute(&connection, "SELECT 1 AS value, NULL::text AS missing", 0)
                .await
                .expect("SELECT PostgreSQL");
        assert_eq!(columns, ["value", "missing"]);
        assert_eq!(rows, [vec![Some("1".into()), None]]);
        assert!(!has_more);

        let (_, first_page, has_more) = execute(
            &connection,
            "SELECT generate_series(1, 205) AS n ORDER BY n",
            0,
        )
        .await
        .expect("primera página");
        assert_eq!(first_page.len(), 200);
        assert!(has_more);
        let (_, second_page, has_more) = execute(
            &connection,
            "SELECT generate_series(1, 205) AS n ORDER BY n",
            200,
        )
        .await
        .expect("segunda página");
        assert_eq!(second_page.len(), 5);
        assert!(!has_more);

        let write_attempt = execute(
            &connection,
            &format!("SELECT nextval('{schema}.read_only_guard')"),
            0,
        )
        .await;
        assert!(
            write_attempt.is_err(),
            "la transacción debe rechazar nextval"
        );
        let (sequence_value,): (i64,) =
            sqlx::query_as(&format!("SELECT last_value FROM {schema}.read_only_guard"))
                .fetch_one(connection.pool())
                .await
                .expect("secuencia todavía legible");
        assert_eq!(sequence_value, 7, "la lectura no debe avanzar la secuencia");

        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(connection.pool())
            .await
            .expect("limpiar esquema");
        connection.pool().close().await;
    }
}
