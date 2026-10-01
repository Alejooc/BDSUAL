//! Lectura paginada de tablas PostgreSQL para la cuadrícula de solo lectura.

use std::time::{Duration, Instant};

use crate::postgres::quote_ident;
use serde::Serialize;
use sqlx::Row;
use tauri::State;

const MAX_ROWS: usize = 200;
const MAX_BYTES: usize = 1024 * 1024;
const MAX_OFFSET: u64 = 1_000_000;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableReadError {
    pub code: &'static str,
    pub message: &'static str,
}

impl TableReadError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableReadResult {
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
    returned_rows: usize,
    has_more: bool,
    next_offset: Option<u64>,
    elapsed_ms: u128,
    total_rows: Option<u64>,
}

#[tauri::command]
pub async fn read_postgres_table_page(
    state: State<'_, crate::AppState>,
    connection_id: String,
    database: String,
    schema: String,
    table: String,
    options: crate::sql_editor::TablePageOptions,
    offset: u64,
) -> Result<TableReadResult, TableReadError> {
    validate(&connection_id, &database, &schema, &table, &options, offset)?;
    let active = state.active_postgres.lock().await;
    let connection = active.get(&connection_id).cloned().ok_or_else(|| {
        TableReadError::new(
            "CONNECTION_CLOSED",
            "Abre la conexión antes de consultar la tabla.",
        )
    })?;
    drop(active);
    let connection = connection
        .connect_to_database(&database)
        .await
        .map_err(|_| {
            TableReadError::new(
                "CONNECTION_FAILED",
                "No se pudo abrir la base PostgreSQL seleccionada.",
            )
        })?;
    let result = read_page(&connection, &schema, &table, options, offset).await;
    connection.pool().close().await;
    result
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 255 && !value.chars().any(char::is_control)
}

fn validate(
    connection_id: &str,
    database: &str,
    schema: &str,
    table: &str,
    options: &crate::sql_editor::TablePageOptions,
    offset: u64,
) -> Result<(), TableReadError> {
    if connection_id.is_empty()
        || connection_id.len() > 128
        || !valid_identifier(database)
        || !valid_identifier(schema)
        || !valid_identifier(table)
    {
        return Err(TableReadError::new(
            "INVALID_QUERY_TARGET",
            "El destino de la tabla no es válido.",
        ));
    }
    if offset > MAX_OFFSET {
        return Err(TableReadError::new(
            "INVALID_RESULT_OFFSET",
            "La paginación superó el límite permitido.",
        ));
    }
    match (&options.sort_column, &options.sort_direction) {
        (Some(column), Some(direction))
            if valid_identifier(column) && matches!(direction.as_str(), "asc" | "desc") => {}
        (None, None) => {}
        _ => {
            return Err(TableReadError::new(
                "INVALID_TABLE_QUERY_OPTIONS",
                "La columna o dirección de orden no es válida.",
            ))
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
            return Err(TableReadError::new(
                "INVALID_TABLE_QUERY_OPTIONS",
                "La columna o el filtro no es válido.",
            ))
        }
    }
    Ok(())
}

async fn read_page(
    connection: &crate::postgres::PostgresConnection,
    schema: &str,
    table: &str,
    options: crate::sql_editor::TablePageOptions,
    offset: u64,
) -> Result<TableReadResult, TableReadError> {
    let started = Instant::now();
    let pool = connection.pool();
    let object: Option<String> = sqlx::query_scalar(
        "SELECT c.relkind::text FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2 AND c.relkind IN ('r','p','v','m','f')"
    ).bind(schema).bind(table).fetch_optional(pool).await.map_err(|_| TableReadError::new("QUERY_FAILED", "No se pudo validar la tabla PostgreSQL."))?;
    if object.is_none() {
        return Err(TableReadError::new(
            "OBJECT_NOT_FOUND",
            "La tabla ya no existe o no es visible en este esquema.",
        ));
    }

    let columns: Vec<String> = sqlx::query_scalar("SELECT column_name FROM information_schema.columns WHERE table_schema=$1 AND table_name=$2 ORDER BY ordinal_position")
        .bind(schema).bind(table).fetch_all(pool).await.map_err(|_| TableReadError::new("QUERY_FAILED", "No se pudieron validar las columnas."))?;
    if columns.is_empty() {
        return Err(TableReadError::new(
            "OBJECT_NOT_FOUND",
            "La tabla ya no existe o no es visible en este esquema.",
        ));
    }
    for requested in [options.sort_column.as_ref(), options.filter_column.as_ref()]
        .into_iter()
        .flatten()
    {
        if !columns.iter().any(|name| name == requested) {
            return Err(TableReadError::new(
                "COLUMN_NOT_FOUND",
                "La columna seleccionada ya no existe en esta tabla.",
            ));
        }
    }
    let primary_keys: Vec<String> = if matches!(object.as_deref(), Some("r" | "p")) {
        sqlx::query_scalar("SELECT a.attname FROM pg_catalog.pg_index i JOIN LATERAL unnest(i.indkey) WITH ORDINALITY k(attnum, ord) ON true JOIN pg_catalog.pg_attribute a ON a.attrelid=i.indrelid AND a.attnum=k.attnum WHERE i.indrelid=pg_catalog.to_regclass(format('%I.%I', $1, $2)) AND i.indisprimary ORDER BY k.ord")
            .bind(schema).bind(table).fetch_all(pool).await.map_err(|_| TableReadError::new("QUERY_FAILED", "No se pudo preparar el orden estable."))?
    } else {
        Vec::new()
    };

    let projection = columns
        .iter()
        .map(|c| {
            format!(
                "{}.{}::text AS {}",
                quote_ident("_dbsual_source"),
                quote_ident(c),
                quote_ident(c)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut builder = sqlx::QueryBuilder::<sqlx::Postgres>::new(format!(
        "SELECT {projection} FROM {}.{} AS {}",
        quote_ident(schema),
        quote_ident(table),
        quote_ident("_dbsual_source")
    ));
    if let (Some(column), Some(mode), Some(value)) = (
        &options.filter_column,
        &options.filter_mode,
        &options.filter_value,
    ) {
        builder.push(" WHERE ");
        if mode == "equals" {
            builder
                .push(quote_ident("_dbsual_source"))
                .push(".")
                .push(quote_ident(column))
                .push("::text = ")
                .push_bind(value);
        } else {
            builder
                .push("strpos(")
                .push(quote_ident("_dbsual_source"))
                .push(".")
                .push(quote_ident(column))
                .push("::text, ")
                .push_bind(value)
                .push(") > 0");
        }
    }
    let total_rows = if offset == 0 {
        let mut count_sql = format!(
            "SELECT COUNT(*) FROM {}.{} AS {}",
            quote_ident(schema),
            quote_ident(table),
            quote_ident("_dbsual_source")
        );
        let count = if let (Some(column), Some(mode), Some(value)) = (
            options.filter_column.as_ref(),
            options.filter_mode.as_ref(),
            options.filter_value.as_ref(),
        ) {
            if mode == "equals" {
                count_sql.push_str(&format!(
                    " WHERE {}.{}::text = $1",
                    quote_ident("_dbsual_source"),
                    quote_ident(column)
                ));
            } else {
                count_sql.push_str(&format!(
                    " WHERE strpos({}.{}::text, $1) > 0",
                    quote_ident("_dbsual_source"),
                    quote_ident(column)
                ));
            }
            sqlx::query_scalar::<_, i64>(&count_sql)
                .bind(value)
                .fetch_one(pool)
                .await
        } else {
            sqlx::query_scalar::<_, i64>(&count_sql)
                .fetch_one(pool)
                .await
        }
        .map_err(|_| {
            TableReadError::new(
                "QUERY_FAILED",
                "No se pudo contar los registros de la tabla PostgreSQL.",
            )
        })?;
        Some(count as u64)
    } else {
        None
    };
    let mut order = Vec::<(String, String)>::new();
    if let (Some(column), Some(direction)) = (&options.sort_column, &options.sort_direction) {
        order.push((
            column.clone(),
            if direction == "desc" { "DESC" } else { "ASC" }.into(),
        ));
    }
    for key in primary_keys {
        if !order.iter().any(|(name, _)| name == &key) {
            order.push((key, "ASC".into()));
        }
    }
    if !order.is_empty() {
        builder.push(" ORDER BY ");
        for (index, (column, direction)) in order.iter().enumerate() {
            if index > 0 {
                builder.push(", ");
            }
            builder
                .push(quote_ident("_dbsual_source"))
                .push(".")
                .push(quote_ident(column))
                .push(" ")
                .push(direction);
        }
    }
    builder
        .push(" LIMIT ")
        .push_bind((MAX_ROWS + 1) as i64)
        .push(" OFFSET ")
        .push_bind(offset as i64);

    let mut tx = pool
        .begin()
        .await
        .map_err(|_| TableReadError::new("QUERY_FAILED", "No se pudo iniciar la lectura."))?;
    sqlx::query("SET TRANSACTION READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(|_| {
            TableReadError::new(
                "READ_ONLY_UNAVAILABLE",
                "PostgreSQL no pudo garantizar una lectura de solo lectura.",
            )
        })?;
    sqlx::query("SET LOCAL statement_timeout = '15s'")
        .execute(&mut *tx)
        .await
        .map_err(|_| {
            TableReadError::new(
                "QUERY_FAILED",
                "No se pudo limitar la duración de la lectura.",
            )
        })?;
    let rows = tokio::time::timeout(Duration::from_secs(16), builder.build().fetch_all(&mut *tx))
        .await
        .map_err(|_| {
            TableReadError::new(
                "QUERY_TIMEOUT",
                "La lectura superó el límite de 15 segundos.",
            )
        })?
        .map_err(|_| {
            TableReadError::new("QUERY_FAILED", "PostgreSQL rechazó la lectura de la tabla.")
        })?;
    tx.rollback().await.map_err(|_| {
        TableReadError::new(
            "QUERY_STATE_UNKNOWN",
            "No se pudo cerrar con seguridad la transacción de lectura.",
        )
    })?;
    let has_more = rows.len() > MAX_ROWS;
    let rows = rows.into_iter().take(MAX_ROWS).collect::<Vec<_>>();
    let mut values = Vec::new();
    let mut bytes = 0usize;
    for row in rows {
        let mut cells = Vec::with_capacity(columns.len());
        for (index, _) in columns.iter().enumerate() {
            let value: Option<String> = row.try_get(index).map_err(|_| {
                TableReadError::new(
                    "UNSUPPORTED_RESULT_TYPE",
                    "Una columna tiene un tipo que no se puede mostrar de forma segura.",
                )
            })?;
            if let Some(text) = &value {
                bytes = bytes.saturating_add(text.len());
            }
            cells.push(value);
        }
        values.push(cells);
        if bytes >= MAX_BYTES {
            break;
        }
    }
    let returned_rows = values.len();
    let has_more = has_more || returned_rows < MAX_ROWS && returned_rows > 0 && bytes >= MAX_BYTES;
    Ok(TableReadResult {
        columns,
        rows: values,
        returned_rows,
        has_more,
        next_offset: (has_more && offset.saturating_add(returned_rows as u64) <= MAX_OFFSET)
            .then_some(offset.saturating_add(returned_rows as u64)),
        elapsed_ms: started.elapsed().as_millis(),
        total_rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_postgres_identifiers_and_rejects_bad_options() {
        assert_eq!(crate::postgres::quote_ident("odd\"name"), "\"odd\"\"name\"");
        let options = crate::sql_editor::TablePageOptions {
            sort_column: Some("id".into()),
            sort_direction: Some("asc; DROP".into()),
            filter_column: None,
            filter_mode: None,
            filter_value: None,
        };
        assert_eq!(
            validate("c", "db", "public", "items", &options, 0)
                .unwrap_err()
                .code,
            "INVALID_TABLE_QUERY_OPTIONS"
        );
    }

    #[tokio::test]
    #[ignore = "requiere PostgreSQL desechable y DBSUAL_POSTGRES_TEST_*"]
    async fn reads_the_qualified_table_with_bound_filters_and_nulls() {
        let connection = crate::postgres::connect(crate::postgres::PostgresConnectConfig {
            host: std::env::var("DBSUAL_POSTGRES_TEST_HOST").expect("host requerido"),
            port: std::env::var("DBSUAL_POSTGRES_TEST_PORT")
                .expect("puerto requerido")
                .parse()
                .expect("puerto válido"),
            username: std::env::var("DBSUAL_POSTGRES_TEST_USER").expect("usuario requerido"),
            password: std::env::var("DBSUAL_POSTGRES_TEST_PASSWORD").ok(),
            database: Some("postgres".into()),
            tls_mode: crate::postgres::PostgresTlsMode::Disabled,
            tls_ca_path: None,
        })
        .await
        .expect("conexión desechable");
        let suffix = std::process::id();
        let schema_a = format!("dbsual_grid_a_{suffix}");
        let schema_b = format!("dbsual_grid_b_{suffix}");
        let setup = async {
            sqlx::query(&format!("CREATE SCHEMA {schema_a}"))
                .execute(connection.pool())
                .await?;
            sqlx::query(&format!("CREATE SCHEMA {schema_b}"))
                .execute(connection.pool())
                .await?;
            for schema in [&schema_a, &schema_b] {
                sqlx::query(&format!(
                    "CREATE TABLE {schema}.same_name (id integer PRIMARY KEY, label text NULL)"
                ))
                .execute(connection.pool())
                .await?;
            }
            sqlx::query(&format!(
                "INSERT INTO {schema_a}.same_name VALUES (1, 'alpha'), (2, NULL), (3, 'a%_\\\\b')"
            ))
            .execute(connection.pool())
            .await?;
            sqlx::query(&format!(
                "INSERT INTO {schema_b}.same_name VALUES (1, 'other')"
            ))
            .execute(connection.pool())
            .await?;
            Ok::<(), sqlx::Error>(())
        }
        .await;
        setup.expect("preparar tablas homónimas");
        let options = crate::sql_editor::TablePageOptions {
            sort_column: Some("id".into()),
            sort_direction: Some("asc".into()),
            filter_column: None,
            filter_mode: None,
            filter_value: None,
        };
        let first = read_page(&connection, &schema_a, "same_name", options.clone(), 0)
            .await
            .expect("tabla esquema A");
        assert_eq!(first.rows[0], [Some("1".into()), Some("alpha".into())]);
        assert_eq!(first.rows[1], [Some("2".into()), None]);
        assert_eq!(first.returned_rows, 3);
        let filtered = read_page(
            &connection,
            &schema_a,
            "same_name",
            crate::sql_editor::TablePageOptions {
                sort_column: None,
                sort_direction: None,
                filter_column: Some("label".into()),
                filter_mode: Some("contains".into()),
                filter_value: Some("%_\\\\".into()),
            },
            0,
        )
        .await
        .expect("filtro literal parametrizado");
        assert_eq!(filtered.returned_rows, 1);
        assert_eq!(filtered.rows[0][0].as_deref(), Some("3"));
        let second = read_page(&connection, &schema_b, "same_name", options, 0)
            .await
            .expect("tabla esquema B");
        assert_eq!(second.rows[0][1].as_deref(), Some("other"));
        sqlx::query(&format!("DROP SCHEMA {schema_a} CASCADE"))
            .execute(connection.pool())
            .await
            .expect("limpiar esquema A");
        sqlx::query(&format!("DROP SCHEMA {schema_b} CASCADE"))
            .execute(connection.pool())
            .await
            .expect("limpiar esquema B");
        connection.pool().close().await;
    }
}
