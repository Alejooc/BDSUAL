//! MySQL backup catalog inspection. This module reports exactly what the
//! configured connection can see; callers must not treat a partial inventory
//! as a complete backup.

use futures_util::TryStreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{
    mysql::{MySql, MySqlConnection, MySqlPool},
    Column, Connection, Executor, FromRow, QueryBuilder, Row, TypeInfo,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{File, OpenOptions},
    future::Future,
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
};
use uuid::Uuid;

const MYSQL_STREAM_FORMAT: &str = "dbsual-mysql-stream-v1";
const MARIADB_STREAM_FORMAT: &str = "dbsual-mariadb-stream-v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupObject {
    pub name: String,
    pub kind: String,
    pub engine: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupRoutine {
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupTrigger {
    pub name: String,
    pub table_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MysqlBackupInventory {
    pub coverage: String,
    pub server_version: String,
    pub database_name: String,
    pub character_set: String,
    pub collation: String,
    pub objects: Vec<BackupObject>,
    pub visible_tables_are_transactional: bool,
    pub routines: Vec<BackupRoutine>,
    pub triggers: Vec<BackupTrigger>,
    pub events: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupDefinition {
    pub name: String,
    pub kind: String,
    pub create_sql: String,
    #[serde(default)]
    pub dependencies: Vec<BackupObjectReference>,
    #[serde(default)]
    pub definer: Option<String>,
    #[serde(default)]
    pub sql_mode: Option<String>,
    #[serde(default)]
    pub character_set_client: Option<String>,
    #[serde(default)]
    pub collation_connection: Option<String>,
    #[serde(default)]
    pub database_collation: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupObjectReference {
    pub schema: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MysqlBackupInspection {
    pub inventory: MysqlBackupInventory,
    pub definitions: Vec<BackupDefinition>,
}

const STREAM_CHANNEL_CHUNKS: usize = 2;
const STREAM_CHUNK_BYTES: usize = 64 * 1024;
const MAX_TABLE_RECORD_BYTES: usize = 64 * 1024 * 1024;

fn supports_mysql_backup_lock(version: &str) -> bool {
    let mut components = version.split('-').next().unwrap_or_default().split('.');
    let Some(major) = components
        .next()
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return false;
    };
    let Some(minor) = components
        .next()
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return false;
    };
    let Some(patch) = components
        .next()
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return false;
    };
    (major, minor, patch) >= (8, 0, 3)
}

#[derive(Debug)]
pub enum MysqlBackupStreamError {
    UnsupportedServer(&'static str),
    Database(sqlx::Error),
    Vault(crate::vault::VaultError),
    Worker,
}

#[derive(Debug)]
pub enum MysqlTableRestoreError {
    InvalidStream,
    InvalidArtifact,
    IntegrityMismatch(String),
    InvalidTarget,
    DestinationExists,
    Database(sqlx::Error),
    IncompleteTarget,
    UnsupportedDefiner,
    Worker,
}

#[allow(dead_code)] // Se usa al conectar el artefacto autenticado con el restaurador.
struct HashingReader<R> {
    inner: R,
    digest: Sha256,
}

#[allow(dead_code)]
impl<R> HashingReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            digest: Sha256::new(),
        }
    }

    fn finish(self) -> (R, String) {
        let digest = self.digest.finalize();
        let hash = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        (self.inner, hash)
    }
}

#[allow(dead_code)]
impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let length = self.inner.read(buffer)?;
        self.digest.update(&buffer[..length]);
        Ok(length)
    }
}

#[allow(dead_code)] // Puente acotado entre descifrado síncrono y restauración asíncrona.
struct BlockingDuplexWriter {
    runtime: tokio::runtime::Handle,
    stream: tokio::io::DuplexStream,
    pending: Vec<u8>,
}

#[allow(dead_code)]
impl BlockingDuplexWriter {
    fn new(runtime: tokio::runtime::Handle, stream: tokio::io::DuplexStream) -> Self {
        Self {
            runtime,
            stream,
            pending: Vec::with_capacity(STREAM_CHUNK_BYTES),
        }
    }

    fn flush_pending(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let chunk = std::mem::take(&mut self.pending);
        self.pending = Vec::with_capacity(STREAM_CHUNK_BYTES);
        self.runtime
            .block_on(async { self.stream.write_all(&chunk).await })
    }
}

#[allow(dead_code)]
impl Write for BlockingDuplexWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let mut offset = 0;
        while offset < buffer.len() {
            let available = STREAM_CHUNK_BYTES.saturating_sub(self.pending.len());
            let length = available.min(buffer.len() - offset);
            self.pending
                .extend_from_slice(&buffer[offset..offset + length]);
            offset += length;
            if self.pending.len() == STREAM_CHUNK_BYTES {
                self.flush_pending()?;
            }
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_pending()
    }
}

#[allow(dead_code)] // Se activa al registrar el comando de restauración.
fn open_artifact_for_read(path: &Path) -> io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
        OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(path)
    }
    #[cfg(not(windows))]
    {
        OpenOptions::new().read(true).open(path)
    }
}

struct MysqlRestoreTable {
    name: String,
    create_sql: String,
    column_names: Vec<String>,
    column_types: Vec<String>,
    generated: Vec<bool>,
}

#[derive(FromRow)]
struct ForeignKeyColumn {
    table_name: String,
    constraint_name: String,
    referenced_table: String,
    column_name: String,
    referenced_column: String,
}

#[derive(Default)]
struct CanonicalRowSetDigest {
    rows: u64,
    sum: [u8; 32],
    xor: [u8; 32],
}

impl CanonicalRowSetDigest {
    fn update(&mut self, canonical_row: &[u8]) -> Result<(), MysqlTableRestoreError> {
        self.rows = self
            .rows
            .checked_add(1)
            .ok_or(MysqlTableRestoreError::InvalidStream)?;
        let mut hasher = Sha256::new();
        hasher.update(b"DBSUAL mysql canonical row v1\0");
        hasher.update(canonical_row);
        let row_hash = hasher.finalize();
        let mut carry = 0_u16;
        for index in (0..32).rev() {
            let value = self.sum[index] as u16 + row_hash[index] as u16 + carry;
            self.sum[index] = value as u8;
            carry = value >> 8;
            self.xor[index] ^= row_hash[index];
        }
        Ok(())
    }

    fn finish_hex(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"DBSUAL mysql canonical row set v1\0");
        hasher.update(self.rows.to_be_bytes());
        hasher.update(self.sum);
        hasher.update(self.xor);
        digest_hex(hasher.finalize().as_slice())
    }
}

fn digest_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

struct RestoreDestinationGuard {
    pool: MySqlPool,
    database: String,
    armed: bool,
}

impl RestoreDestinationGuard {
    fn new(pool: &MySqlPool, database: &str) -> Self {
        Self {
            pool: pool.clone(),
            database: database.to_owned(),
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RestoreDestinationGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let pool = self.pool.clone();
        let database = self.database.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let statement = format!("DROP DATABASE IF EXISTS {}", quote_identifier(&database));
                // Cancellation can drop the connection that was creating or
                // populating the destination at the same time this cleanup
                // task starts. Retry transient server/connection failures so
                // the provisional database is not left behind by that race.
                for attempt in 0..50 {
                    if sqlx::raw_sql(&statement).execute(&pool).await.is_ok() {
                        return;
                    }
                    if attempt < 49 {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }
            });
        }
    }
}

struct ChannelWriter {
    sender: Option<mpsc::Sender<io::Result<Vec<u8>>>>,
    pending: Option<Pin<Box<dyn Future<Output = io::Result<usize>> + Send>>>,
}

impl ChannelWriter {
    fn new(sender: mpsc::Sender<io::Result<Vec<u8>>>) -> Self {
        Self {
            sender: Some(sender),
            pending: None,
        }
    }

    async fn fail(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender
                .send(Err(io::Error::other("database backup stream failed")))
                .await;
        }
    }
}

impl AsyncWrite for ChannelWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.pending.is_none() {
            let sender = match self.sender.as_ref() {
                Some(sender) => sender.clone(),
                None => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "database backup stream is closed",
                    )))
                }
            };
            let length = buffer.len().min(STREAM_CHUNK_BYTES);
            let chunk = buffer[..length].to_vec();
            self.pending = Some(Box::pin(async move {
                sender.send(Ok(chunk)).await.map(|()| length).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "database backup consumer stopped",
                    )
                })
            }));
        }
        let result = self.pending.as_mut().unwrap().as_mut().poll(context);
        if result.is_ready() {
            self.pending = None;
        }
        result
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        if let Some(pending) = self.pending.as_mut() {
            match pending.as_mut().poll(context) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => {
                    self.pending = None;
                    return Poll::Ready(Err(error));
                }
                Poll::Ready(Ok(_)) => self.pending = None,
            }
        }
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.as_mut().poll_flush(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Ready(Ok(())) => {
                self.sender = None;
                Poll::Ready(Ok(()))
            }
        }
    }
}

struct ChannelReader {
    receiver: mpsc::Receiver<io::Result<Vec<u8>>>,
    current: Cursor<Vec<u8>>,
}

impl Read for ChannelReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        loop {
            if (self.current.position() as usize) < self.current.get_ref().len() {
                return self.current.read(output);
            }
            match self.receiver.blocking_recv() {
                Some(Ok(chunk)) => self.current = Cursor::new(chunk),
                Some(Err(error)) => return Err(error),
                None => return Ok(0),
            }
        }
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

/// Stream one visible base table as newline-delimited JSON. Values retain the
/// exact bytes returned by MySQL's text protocol; NULL has a separate JSON
/// representation. This is an internal wire format, not yet a restorable dump.
pub async fn stream_mysql_table_rows<W>(
    pool: &MySqlPool,
    inventory: &MysqlBackupInventory,
    table_name: &str,
    writer: &mut W,
) -> Result<u64, sqlx::Error>
where
    W: AsyncWrite + Unpin,
{
    let mut connection = pool.acquire().await?;
    stream_mysql_table_rows_on_connection(&mut connection, inventory, table_name, writer).await
}

async fn stream_mysql_table_rows_on_connection<W>(
    connection: &mut MySqlConnection,
    inventory: &MysqlBackupInventory,
    table_name: &str,
    writer: &mut W,
) -> Result<u64, sqlx::Error>
where
    W: AsyncWrite + Unpin,
{
    let table = inventory
        .objects
        .iter()
        .find(|object| object.name == table_name && object.kind == "BASE TABLE")
        .ok_or_else(|| {
            sqlx::Error::Protocol("backup table is not in the inspected inventory".into())
        })?;
    if table.engine.is_none() {
        return Err(sqlx::Error::Protocol(
            "backup table engine is unknown".into(),
        ));
    }
    let all_columns: Vec<String> = sqlx::query_scalar(
        "SELECT CAST(COLUMN_NAME AS CHAR) FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
    )
    .bind(&inventory.database_name)
    .bind(&table.name)
    .fetch_all(&mut *connection)
    .await?;
    if all_columns.is_empty() {
        return Err(sqlx::Error::Protocol(
            "backup table has no visible column metadata".into(),
        ));
    }
    // Naming columns explicitly also includes MySQL INVISIBLE columns, which
    // SELECT * omits even though they are part of the table definition.
    let projection = all_columns
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT {projection} FROM {}.{}",
        quote_identifier(&inventory.database_name),
        quote_identifier(&table.name)
    );
    let create_sql = sqlx::query(&format!(
        "SHOW CREATE TABLE {}.{}",
        quote_identifier(&inventory.database_name),
        quote_identifier(&table.name)
    ))
    .fetch_one(&mut *connection)
    .await
    .and_then(|row| create_statement_column(&row))?;
    let description = connection.describe(&sql).await?;
    let generated_columns: HashSet<String> = sqlx::query_scalar(
        "SELECT CAST(COLUMN_NAME AS CHAR) FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND EXTRA LIKE '%GENERATED%'",
    )
    .bind(&inventory.database_name)
    .bind(&table.name)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .collect();
    let columns: Vec<_> = description
        .columns()
        .iter()
        .map(|column| {
            serde_json::json!({
                "name": column.name(),
                "mysqlType": column.type_info().name(),
                "generated": generated_columns.contains(column.name()),
            })
        })
        .collect();
    let header = serde_json::to_vec(&serde_json::json!({
        "record": "table",
        "format": "dbsual-mysql-table-v1",
        "database": inventory.database_name,
        "table": table.name,
        "characterSet": inventory.character_set,
        "collation": inventory.collation,
        "createSql": create_sql,
        "valueEncoding": "hex-text-protocol",
        "columns": columns,
    }))
    .map_err(|error| sqlx::Error::Protocol(error.to_string().into()))?;
    writer.write_all(&header).await.map_err(sqlx::Error::Io)?;
    writer.write_all(b"\n").await.map_err(sqlx::Error::Io)?;

    let mut rows = sqlx::raw_sql(&sql).fetch(&mut *connection);
    let mut count = 0_u64;
    let mut row_set_digest = CanonicalRowSetDigest::default();
    while let Some(row) = rows.try_next().await? {
        let mut values = Vec::with_capacity(row.columns().len());
        for index in 0..row.columns().len() {
            let raw = row.try_get_raw(index)?;
            match raw.as_bytes() {
                Ok(bytes) => values.push(serde_json::json!({ "hex": encode_hex(bytes) })),
                Err(_) => values.push(serde_json::Value::Null),
            }
        }
        let encoded = serde_json::to_vec(&serde_json::json!({
            "record": "row",
            "values": values,
        }))
        .map_err(|error| sqlx::Error::Protocol(error.to_string().into()))?;
        row_set_digest.update(&encoded).map_err(|error| {
            sqlx::Error::Protocol(format!("invalid row count: {error:?}").into())
        })?;
        if encoded.len() > MAX_TABLE_RECORD_BYTES {
            return Err(sqlx::Error::Protocol(
                "backup row exceeds the maximum supported record size".into(),
            ));
        }
        writer.write_all(&encoded).await.map_err(sqlx::Error::Io)?;
        writer.write_all(b"\n").await.map_err(sqlx::Error::Io)?;
        count += 1;
    }
    let footer = serde_json::to_vec(&serde_json::json!({
        "record": "end",
        "rows": count,
        "rowSetSha256": row_set_digest.finish_hex(),
    }))
    .map_err(|error| sqlx::Error::Protocol(error.to_string().into()))?;
    writer.write_all(&footer).await.map_err(sqlx::Error::Io)?;
    writer.write_all(b"\n").await.map_err(sqlx::Error::Io)?;
    writer.flush().await.map_err(sqlx::Error::Io)?;
    Ok(count)
}

/// Restore one authenticated table stream into a new, empty database. The
/// caller must authenticate/decrypt the stream before passing it here. This
/// primitive is intentionally not registered as a Tauri command yet.
#[allow(dead_code)] // Internal recovery primitive; not exposed until authenticated artifact restore is wired end to end.
pub(crate) async fn restore_mysql_table_stream<R>(
    pool: &MySqlPool,
    target_database: &str,
    reader: &mut R,
) -> Result<u64, MysqlTableRestoreError>
where
    R: AsyncBufRead + Unpin,
{
    if !valid_restore_identifier(target_database) {
        return Err(MysqlTableRestoreError::InvalidTarget);
    }
    let header_line = read_bounded_line(reader, MAX_TABLE_RECORD_BYTES)
        .await
        .map_err(|_| MysqlTableRestoreError::InvalidStream)?
        .ok_or(MysqlTableRestoreError::InvalidStream)?;
    let header: serde_json::Value =
        serde_json::from_slice(&header_line).map_err(|_| MysqlTableRestoreError::InvalidStream)?;
    let table = parse_mysql_restore_table(&header)?;
    let character_set = restore_identifier_field(&header, "characterSet")?;
    let collation = restore_identifier_field(&header, "collation")?;

    let mut connection = pool
        .acquire()
        .await
        .map_err(MysqlTableRestoreError::Database)?
        .detach();
    let exists: Option<(Vec<u8>,)> =
        sqlx::query_as("SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?")
            .bind(target_database)
            .fetch_optional(&mut connection)
            .await
            .map_err(MysqlTableRestoreError::Database)?;
    if exists.is_some() {
        return Err(MysqlTableRestoreError::DestinationExists);
    }
    sqlx::raw_sql(&format!(
        "CREATE DATABASE {} CHARACTER SET {} COLLATE {}",
        quote_identifier(target_database),
        quote_identifier(&character_set),
        quote_identifier(&collation)
    ))
    .execute(&mut connection)
    .await
    .map_err(MysqlTableRestoreError::Database)?;
    let mut destination_guard = RestoreDestinationGuard::new(pool, target_database);

    let restore_result = async {
        sqlx::raw_sql(&format!("USE {}", quote_identifier(target_database)))
            .execute(&mut connection)
            .await
            .map_err(MysqlTableRestoreError::Database)?;
        sqlx::raw_sql(&table.create_sql)
            .execute(&mut connection)
            .await
            .map_err(MysqlTableRestoreError::Database)?;
        let (rows, expected_digest) =
            restore_mysql_table_rows(reader, &table, &mut connection).await?;
        if canonical_table_row_set_digest(&table, &mut connection).await? != expected_digest {
            return Err(MysqlTableRestoreError::IntegrityMismatch(table.name));
        }
        if read_bounded_line(reader, MAX_TABLE_RECORD_BYTES)
            .await
            .map_err(|_| MysqlTableRestoreError::InvalidStream)?
            .is_some()
        {
            return Err(MysqlTableRestoreError::InvalidStream);
        }
        Ok(rows)
    }
    .await;
    match restore_result {
        Ok(rows) => {
            destination_guard.disarm();
            Ok(rows)
        }
        Err(error) => {
            drop_restore_destination(&mut connection, target_database).await?;
            destination_guard.disarm();
            Err(error)
        }
    }
}

/// Restore table data and the supported view/trigger definitions from a
/// database stream into a new destination. Unsupported object types are
/// rejected before the destination is created.
#[allow(dead_code)]
pub(crate) async fn restore_mysql_database_table_stream<R>(
    pool: &MySqlPool,
    target_database: &str,
    reader: &mut R,
) -> Result<(u64, u64), MysqlTableRestoreError>
where
    R: AsyncBufRead + Unpin,
{
    restore_database_table_stream_with_format(pool, target_database, reader, MYSQL_STREAM_FORMAT)
        .await
}

/// Restore a MariaDB-family stream through the shared MySQL-protocol adapter.
/// The distinct format tag prevents a MySQL artifact from being accepted by
/// this entry point (and vice versa); callers still must verify server family.
#[allow(dead_code)]
pub(crate) async fn restore_mariadb_database_table_stream<R>(
    pool: &MySqlPool,
    target_database: &str,
    reader: &mut R,
) -> Result<(u64, u64), MysqlTableRestoreError>
where
    R: AsyncBufRead + Unpin,
{
    restore_database_table_stream_with_format(pool, target_database, reader, MARIADB_STREAM_FORMAT)
        .await
}

async fn restore_database_table_stream_with_format<R>(
    pool: &MySqlPool,
    target_database: &str,
    reader: &mut R,
    expected_format: &str,
) -> Result<(u64, u64), MysqlTableRestoreError>
where
    R: AsyncBufRead + Unpin,
{
    if !valid_restore_identifier(target_database) {
        return Err(MysqlTableRestoreError::InvalidTarget);
    }
    let header = read_restore_record(reader).await?;
    if header["record"] != "database"
        || header["format"] != expected_format
        || header["coverage"] != "visible_objects_only"
    {
        return Err(MysqlTableRestoreError::InvalidStream);
    }
    let source_database = restore_identifier_field(&header, "database")?;
    let character_set = restore_identifier_field(&header, "characterSet")?;
    let collation = restore_identifier_field(&header, "collation")?;
    let definition_count = header["objectCount"]
        .as_u64()
        .filter(|count| *count <= 100_000)
        .ok_or(MysqlTableRestoreError::InvalidStream)?;
    let mut expected_definitions = HashMap::with_capacity(definition_count as usize);
    let mut trigger_order = Vec::new();
    for _ in 0..definition_count {
        let definition = read_restore_record(reader).await?;
        if definition["record"] != "definition"
            || !restorable_definition_kind(definition["kind"].as_str().unwrap_or_default())
        {
            return Err(MysqlTableRestoreError::InvalidStream);
        }
        let name = restore_identifier_field(&definition, "name")?;
        let kind = definition["kind"].as_str().unwrap_or_default();
        let create_sql = definition["createSql"]
            .as_str()
            .filter(|sql| kind == "BASE TABLE" && valid_table_create_sql(sql, &name))
            .map(str::to_owned);
        let create_sql = if definition["kind"] == "BASE TABLE" {
            create_sql.ok_or(MysqlTableRestoreError::InvalidStream)?
        } else if kind == "TRIGGER" {
            let sql = definition["createSql"]
                .as_str()
                .filter(|sql| valid_trigger_create_sql(sql, &name, &source_database))
                .ok_or(MysqlTableRestoreError::InvalidStream)?;
            trigger_order.push(name.clone());
            sql.to_owned()
        } else {
            definition["createSql"]
                .as_str()
                .filter(|sql| valid_view_create_sql(sql, &name))
                .ok_or(MysqlTableRestoreError::InvalidStream)?
                .to_owned()
        };
        let dependencies: Vec<BackupObjectReference> = serde_json::from_value(
            definition
                .get("dependencies")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )
        .map_err(|_| MysqlTableRestoreError::InvalidStream)?;
        let definer = definition["definer"].as_str().map(str::to_owned);
        let sql_mode = definition["sqlMode"].as_str().map(str::to_owned);
        let character_set_client = definition["characterSetClient"].as_str().map(str::to_owned);
        let collation_connection = definition["collationConnection"]
            .as_str()
            .map(str::to_owned);
        let database_collation = definition["databaseCollation"].as_str().map(str::to_owned);
        if (matches!(kind, "VIEW" | "TRIGGER") && definer.is_none())
            || (kind == "TRIGGER"
                && (sql_mode.is_none()
                    || character_set_client.is_none()
                    || collation_connection.is_none()
                    || database_collation.is_none()))
            || (definition["kind"] == "BASE TABLE"
                && (!dependencies.is_empty()
                    || definer.is_some()
                    || sql_mode.is_some()
                    || character_set_client.is_some()
                    || collation_connection.is_some()
                    || database_collation.is_some()))
        {
            return Err(MysqlTableRestoreError::InvalidStream);
        }
        let parsed = BackupDefinition {
            name: name.clone(),
            kind: definition["kind"].as_str().unwrap().to_owned(),
            create_sql,
            dependencies,
            definer,
            sql_mode,
            character_set_client,
            collation_connection,
            database_collation,
        };
        if expected_definitions.insert(name, parsed).is_some() {
            return Err(MysqlTableRestoreError::InvalidStream);
        }
    }

    let view_order = plan_local_view_restore(&source_database, &expected_definitions)
        .ok_or(MysqlTableRestoreError::InvalidStream)?;
    let rewritten_views = view_order
        .iter()
        .map(|name| {
            let definition = expected_definitions.get(name)?;
            rewrite_local_view_schema(&definition.create_sql, &source_database, target_database)
                .map(|sql| (name.clone(), sql))
        })
        .collect::<Option<HashMap<_, _>>>()
        .ok_or(MysqlTableRestoreError::InvalidStream)?;
    let rewritten_triggers = trigger_order
        .iter()
        .map(|name| {
            let definition = expected_definitions.get(name)?;
            rewrite_local_view_schema(&definition.create_sql, &source_database, target_database)
                .map(|sql| (name.clone(), sql))
        })
        .collect::<Option<HashMap<_, _>>>()
        .ok_or(MysqlTableRestoreError::InvalidStream)?;

    let mut connection = pool
        .acquire()
        .await
        .map_err(MysqlTableRestoreError::Database)?
        .detach();
    let target_server_version: String = sqlx::query_scalar("SELECT VERSION()")
        .fetch_one(&mut connection)
        .await
        .map_err(MysqlTableRestoreError::Database)?;
    let target_is_mariadb = target_server_version
        .to_ascii_lowercase()
        .contains("mariadb");
    if expected_format == MARIADB_STREAM_FORMAT {
        let source_version = header["serverVersion"]
            .as_str()
            .ok_or(MysqlTableRestoreError::InvalidStream)?;
        if !target_is_mariadb
            || mariadb_backup_branch(source_version).is_none()
            || mariadb_backup_branch(source_version)
                != mariadb_backup_branch(&target_server_version)
        {
            return Err(MysqlTableRestoreError::InvalidStream);
        }
    } else if target_is_mariadb {
        return Err(MysqlTableRestoreError::InvalidStream);
    }
    for object_name in view_order.iter().chain(trigger_order.iter()) {
        let view = expected_definitions
            .get(object_name)
            .ok_or(MysqlTableRestoreError::InvalidStream)?;
        let definer = view
            .definer
            .as_deref()
            .ok_or(MysqlTableRestoreError::UnsupportedDefiner)?;
        if !mysql_definer_is_available(&mut connection, definer).await {
            return Err(MysqlTableRestoreError::UnsupportedDefiner);
        }
    }
    ensure_restore_destination_absent(&mut connection, target_database).await?;
    create_restore_destination(&mut connection, target_database, &character_set, &collation)
        .await?;
    let mut destination_guard = RestoreDestinationGuard::new(pool, target_database);

    let restore_result = async {
        sqlx::raw_sql(&format!("USE {}", quote_identifier(target_database)))
            .execute(&mut connection)
            .await
            .map_err(MysqlTableRestoreError::Database)?;
        let mut table_count = 0_u64;
        let mut total_rows = 0_u64;
        let mut deferred_foreign_keys = Vec::new();
        let mut expected_table_definitions = Vec::new();
        loop {
            let record = read_restore_record(reader).await?;
            match record["record"].as_str() {
                Some("table") => {
                    if record["database"].as_str() != Some(source_database.as_str()) {
                        return Err(MysqlTableRestoreError::InvalidStream);
                    }
                    let table = parse_mysql_restore_table(&record)?;
                    let Some(expected) = expected_definitions.get(&table.name) else {
                        return Err(MysqlTableRestoreError::InvalidStream);
                    };
                    if expected.kind != "BASE TABLE"
                        || expected.create_sql != table.create_sql
                        || !expected.dependencies.is_empty()
                        || expected.definer.is_some()
                    {
                        return Err(MysqlTableRestoreError::InvalidStream);
                    }
                    expected_definitions.remove(&table.name);
                    let (create_sql, foreign_keys) =
                        split_mysql_create_table_foreign_keys(&table.create_sql)
                            .ok_or(MysqlTableRestoreError::InvalidStream)?;
                    expected_table_definitions.push((table.name.clone(), table.create_sql.clone()));
                    sqlx::raw_sql(&create_sql)
                        .execute(&mut connection)
                        .await
                        .map_err(MysqlTableRestoreError::Database)?;
                    deferred_foreign_keys.extend(
                        foreign_keys
                            .into_iter()
                            .map(|foreign_key| (table.name.clone(), foreign_key)),
                    );
                    let (rows, expected_digest) =
                        restore_mysql_table_rows(reader, &table, &mut connection).await?;
                    if canonical_table_row_set_digest(&table, &mut connection).await?
                        != expected_digest
                    {
                        return Err(MysqlTableRestoreError::IntegrityMismatch(table.name));
                    }
                    table_count = table_count
                        .checked_add(1)
                        .ok_or(MysqlTableRestoreError::InvalidStream)?;
                    total_rows = total_rows
                        .checked_add(rows)
                        .ok_or(MysqlTableRestoreError::InvalidStream)?;
                }
                Some("database_end") => {
                    if record["streamComplete"] != true
                        || record["tables"].as_u64() != Some(table_count)
                        || record["rows"].as_u64() != Some(total_rows)
                        || expected_definitions.values().any(|definition| {
                            !matches!(definition.kind.as_str(), "VIEW" | "TRIGGER")
                        })
                        || read_bounded_line(reader, MAX_TABLE_RECORD_BYTES)
                            .await
                            .map_err(|_| MysqlTableRestoreError::InvalidStream)?
                            .is_some()
                    {
                        return Err(MysqlTableRestoreError::InvalidStream);
                    }
                    for (table, foreign_key) in &deferred_foreign_keys {
                        sqlx::raw_sql(&format!(
                            "ALTER TABLE {} ADD {}",
                            quote_identifier(table),
                            foreign_key
                        ))
                        .execute(&mut connection)
                        .await
                        .map_err(MysqlTableRestoreError::Database)?;
                    }
                    for (table, expected_create_sql) in &expected_table_definitions {
                        let row =
                            sqlx::query(&format!("SHOW CREATE TABLE {}", quote_identifier(table)))
                                .fetch_one(&mut connection)
                                .await
                                .map_err(MysqlTableRestoreError::Database)?;
                        if create_statement_column(&row)
                            .map_err(MysqlTableRestoreError::Database)?
                            != *expected_create_sql
                        {
                            return Err(MysqlTableRestoreError::IntegrityMismatch(table.clone()));
                        }
                    }
                    for view_name in &view_order {
                        let rewritten = rewritten_views
                            .get(view_name)
                            .ok_or(MysqlTableRestoreError::InvalidStream)?;
                        sqlx::raw_sql(&rewritten)
                            .execute(&mut connection)
                            .await
                            .map_err(MysqlTableRestoreError::Database)?;
                        let row = sqlx::query(&format!(
                            "SHOW CREATE VIEW {}",
                            quote_identifier(view_name)
                        ))
                        .fetch_one(&mut connection)
                        .await
                        .map_err(MysqlTableRestoreError::Database)?;
                        let actual = create_statement_column(&row)
                            .map_err(MysqlTableRestoreError::Database)?;
                        let normalized_expected =
                            strip_local_view_schema(rewritten, target_database)
                                .ok_or(MysqlTableRestoreError::InvalidStream)?;
                        if actual != normalized_expected {
                            return Err(MysqlTableRestoreError::IntegrityMismatch(
                                view_name.clone(),
                            ));
                        }
                    }
                    for trigger_name in &trigger_order {
                        let trigger = expected_definitions
                            .get(trigger_name)
                            .ok_or(MysqlTableRestoreError::InvalidStream)?;
                        set_trigger_creation_context(&mut connection, trigger).await?;
                        let rewritten = rewritten_triggers
                            .get(trigger_name)
                            .ok_or(MysqlTableRestoreError::InvalidStream)?;
                        sqlx::raw_sql(rewritten)
                            .execute(&mut connection)
                            .await
                            .map_err(MysqlTableRestoreError::Database)?;
                        let row = sqlx::query(&format!(
                            "SHOW CREATE TRIGGER {}",
                            quote_identifier(trigger_name)
                        ))
                        .fetch_one(&mut connection)
                        .await
                        .map_err(MysqlTableRestoreError::Database)?;
                        let actual = create_statement_column(&row)
                            .map_err(MysqlTableRestoreError::Database)?;
                        let normalized_expected =
                            strip_local_view_schema(rewritten, target_database)
                                .ok_or(MysqlTableRestoreError::InvalidStream)?;
                        let normalized_actual = strip_local_view_schema(&actual, target_database)
                            .ok_or(MysqlTableRestoreError::InvalidStream)?;
                        if normalized_actual != normalized_expected {
                            return Err(MysqlTableRestoreError::IntegrityMismatch(
                                trigger_name.clone(),
                            ));
                        }
                    }
                    validate_restore_foreign_keys(&mut connection, target_database).await?;
                    sqlx::raw_sql("SET SESSION FOREIGN_KEY_CHECKS = 1")
                        .execute(&mut connection)
                        .await
                        .map_err(MysqlTableRestoreError::Database)?;
                    return Ok((table_count, total_rows));
                }
                _ => return Err(MysqlTableRestoreError::InvalidStream),
            }
        }
    }
    .await;
    match restore_result {
        Ok(result) => {
            destination_guard.disarm();
            Ok(result)
        }
        Err(error) => {
            drop_restore_destination(&mut connection, target_database).await?;
            destination_guard.disarm();
            Err(error)
        }
    }
}

fn split_mysql_create_table_foreign_keys(sql: &str) -> Option<(String, Vec<String>)> {
    let open = sql.find(" (")? + 2;
    let bytes = sql.as_bytes();
    let mut depth = 1_i32;
    let mut quote = None;
    let mut escaped = false;
    let mut item_start = open;
    let mut close = None;
    let mut items = Vec::new();
    for index in open..bytes.len() {
        let byte = bytes[index];
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' && q != b'`' {
                escaped = true;
            } else if byte == q {
                if bytes.get(index + 1) == Some(&q) {
                    continue;
                }
                quote = None;
            }
            continue;
        }
        if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
        } else if byte == b'(' {
            depth += 1;
        } else if byte == b')' {
            depth -= 1;
            if depth == 0 {
                items.push(sql[item_start..index].trim());
                close = Some(index);
                break;
            }
        } else if byte == b',' && depth == 1 {
            items.push(sql[item_start..index].trim());
            item_start = index + 1;
        }
    }
    if quote.is_some() || close.is_none() || items.iter().any(|item| item.is_empty()) {
        return None;
    }
    let close = close?;
    let mut table_items = Vec::new();
    let mut foreign_keys = Vec::new();
    for item in items {
        let upper = item.to_ascii_uppercase();
        if upper.starts_with("FOREIGN KEY")
            || (upper.starts_with("CONSTRAINT ") && upper.contains(" FOREIGN KEY"))
        {
            foreign_keys.push(item.to_owned());
        } else {
            table_items.push(item.to_owned());
        }
    }
    if table_items.is_empty() {
        return None;
    }
    let create_sql = format!(
        "{}{}{}",
        &sql[..open],
        table_items.join(", "),
        &sql[close..]
    );
    Some((create_sql, foreign_keys))
}

fn restorable_definition_kind(kind: &str) -> bool {
    matches!(kind, "BASE TABLE" | "VIEW" | "TRIGGER")
}

fn plan_local_view_restore(
    source_database: &str,
    definitions: &HashMap<String, BackupDefinition>,
) -> Option<Vec<String>> {
    let mut views = BTreeMap::<String, Vec<String>>::new();
    for (name, definition) in definitions {
        match definition.kind.as_str() {
            "BASE TABLE" if definition.dependencies.is_empty() && definition.definer.is_none() => {}
            "VIEW" => {
                if definition.definer.as_deref().is_none_or(str::is_empty) {
                    return None;
                }
                let mut dependencies = Vec::new();
                for dependency in &definition.dependencies {
                    if dependency.schema != source_database
                        || !definitions.contains_key(&dependency.name)
                    {
                        return None;
                    }
                    if definitions[&dependency.name].kind == "VIEW" {
                        dependencies.push(dependency.name.clone());
                    } else if definitions[&dependency.name].kind != "BASE TABLE" {
                        return None;
                    }
                }
                views.insert(name.clone(), dependencies);
            }
            "TRIGGER"
                if definition.dependencies.is_empty()
                    && definition
                        .definer
                        .as_deref()
                        .is_some_and(|value| !value.is_empty()) =>
            {
                let Some((schema, table)) = trigger_target_table(&definition.create_sql) else {
                    return None;
                };
                if schema
                    .as_deref()
                    .is_some_and(|schema| schema != source_database)
                    || !definitions
                        .get(&table)
                        .is_some_and(|definition| definition.kind == "BASE TABLE")
                    || !valid_trigger_create_sql(&definition.create_sql, name, source_database)
                {
                    return None;
                }
            }
            _ => return None,
        }
    }

    let mut ready = views
        .iter()
        .filter(|(_, dependencies)| dependencies.is_empty())
        .map(|(name, _)| name.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let mut ordered = Vec::with_capacity(views.len());
    while let Some(name) = ready.pop_first() {
        ordered.push(name.clone());
        for (candidate, dependencies) in &views {
            if !ordered.contains(candidate)
                && dependencies
                    .iter()
                    .all(|dependency| ordered.contains(dependency))
                && !ready.contains(candidate)
            {
                ready.insert(candidate.clone());
            }
        }
    }
    (ordered.len() == views.len()).then_some(ordered)
}

fn valid_view_create_sql(sql: &str, view_name: &str) -> bool {
    let upper = sql.to_ascii_uppercase();
    sql.len() <= MAX_TABLE_RECORD_BYTES
        && sql.starts_with("CREATE ")
        && upper.contains(" VIEW ")
        && upper.contains(" AS ")
        && !sql.contains('\0')
        && sql.contains(&quote_identifier(view_name))
}

fn valid_trigger_create_sql(sql: &str, trigger_name: &str, source_database: &str) -> bool {
    let normalized = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    let upper = normalized.to_ascii_uppercase();
    sql.len() <= MAX_TABLE_RECORD_BYTES
        && upper.starts_with("CREATE ")
        && upper.contains(" TRIGGER ")
        && upper.contains(" FOR EACH ROW ")
        && !upper.contains(" FOLLOWS ")
        && !upper.contains(" PRECEDES ")
        && !upper.contains(" BEGIN ")
        && !sql.contains(';')
        && !sql.contains('\0')
        && trigger_name_matches(sql, trigger_name)
        && trigger_uses_only_local_qualifiers(sql, source_database)
        && trigger_target_table(sql).is_some()
}

fn trigger_name_matches(sql: &str, expected_name: &str) -> bool {
    let upper = sql.to_ascii_uppercase();
    let Some(trigger_start) = upper.find(" TRIGGER ").map(|index| index + 9) else {
        return false;
    };
    let tail = &sql[trigger_start..];
    let tail_upper = &upper[trigger_start..];
    let event_start = [" BEFORE ", " AFTER "]
        .iter()
        .filter_map(|keyword| tail_upper.find(keyword))
        .min();
    let Some(event_start) = event_start else {
        return false;
    };
    let qualified_name = tail[..event_start].trim();
    let name = qualified_name.rsplit('.').next().unwrap_or_default().trim();
    let name = name
        .strip_prefix('`')
        .and_then(|value| value.strip_suffix('`'))
        .unwrap_or(name)
        .replace("``", "`");
    name == expected_name
}

fn trigger_uses_only_local_qualifiers(sql: &str, source_database: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' => {
                index += 1;
                let mut closed = false;
                while index < bytes.len() {
                    if bytes[index] == b'\\' {
                        index = (index + 2).min(bytes.len());
                    } else if bytes[index] == b'\'' {
                        if bytes.get(index + 1) == Some(&b'\'') {
                            index += 2;
                        } else {
                            index += 1;
                            closed = true;
                            break;
                        }
                    } else {
                        index += 1;
                    }
                }
                if !closed {
                    return false;
                }
            }
            b'"' => return false,
            b'`' => {
                let Some((identifier, rest)) = parse_quoted_identifier(&sql[index..]) else {
                    return false;
                };
                let consumed = sql[index..].len() - rest.len();
                index += consumed;
                let after = bytes[index..]
                    .iter()
                    .position(|byte| !byte.is_ascii_whitespace())
                    .map_or(bytes.len(), |offset| index + offset);
                if bytes.get(after) == Some(&b'.') && identifier != source_database {
                    return false;
                }
            }
            byte if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$') => {
                let start = index;
                index += 1;
                while bytes
                    .get(index)
                    .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$'))
                {
                    index += 1;
                }
                let token = &sql[start..index];
                let after = bytes[index..]
                    .iter()
                    .position(|byte| !byte.is_ascii_whitespace())
                    .map_or(bytes.len(), |offset| index + offset);
                if bytes.get(after) == Some(&b'.')
                    && !token.eq_ignore_ascii_case("NEW")
                    && !token.eq_ignore_ascii_case("OLD")
                {
                    return false;
                }
            }
            _ => index += 1,
        }
    }
    true
}

fn trigger_target_table(sql: &str) -> Option<(Option<String>, String)> {
    let upper = sql.to_ascii_uppercase();
    let on = upper.find(" ON ")? + 4;
    let (first, rest) = parse_identifier_prefix(&sql[on..])?;
    let rest = rest.trim_start();
    if let Some(rest) = rest.strip_prefix('.') {
        let (table, _) = parse_identifier_prefix(rest.trim_start())?;
        Some((Some(first), table))
    } else {
        Some((None, first))
    }
}

fn parse_identifier_prefix(value: &str) -> Option<(String, &str)> {
    if value.starts_with('`') {
        return parse_quoted_identifier(value);
    }
    let end = value
        .bytes()
        .position(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')))
        .unwrap_or(value.len());
    if end == 0 {
        return None;
    }
    let name = &value[..end];
    if !name
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$'))
    {
        return None;
    }
    Some((name.to_owned(), &value[end..]))
}

fn parse_quoted_identifier(value: &str) -> Option<(String, &str)> {
    let bytes = value.as_bytes();
    if bytes.first() != Some(&b'`') {
        return None;
    }
    let mut identifier = Vec::new();
    let mut index = 1;
    while index < bytes.len() {
        if bytes[index] == b'`' {
            if bytes.get(index + 1) == Some(&b'`') {
                identifier.push(b'`');
                index += 2;
            } else {
                return Some((String::from_utf8(identifier).ok()?, &value[index + 1..]));
            }
        } else {
            identifier.push(bytes[index]);
            index += 1;
        }
    }
    None
}

async fn set_trigger_creation_context(
    connection: &mut MySqlConnection,
    trigger: &BackupDefinition,
) -> Result<(), MysqlTableRestoreError> {
    let sql_mode = trigger
        .sql_mode
        .as_deref()
        .filter(|value| value.len() <= 16_384 && !value.chars().any(char::is_control))
        .ok_or(MysqlTableRestoreError::InvalidStream)?;
    let character_set = trigger
        .character_set_client
        .as_deref()
        .filter(|value| {
            !value.is_empty()
                && value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        })
        .ok_or(MysqlTableRestoreError::InvalidStream)?;
    let collation = trigger
        .collation_connection
        .as_deref()
        .filter(|value| {
            !value.is_empty()
                && value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        })
        .ok_or(MysqlTableRestoreError::InvalidStream)?;
    sqlx::query("SET SESSION sql_mode = ?")
        .bind(sql_mode)
        .execute(&mut *connection)
        .await
        .map_err(MysqlTableRestoreError::Database)?;
    sqlx::query("SET SESSION character_set_client = ?")
        .bind(character_set)
        .execute(&mut *connection)
        .await
        .map_err(MysqlTableRestoreError::Database)?;
    sqlx::query("SET SESSION collation_connection = ?")
        .bind(collation)
        .execute(&mut *connection)
        .await
        .map_err(MysqlTableRestoreError::Database)?;
    Ok(())
}

fn rewrite_local_view_schema(sql: &str, source: &str, target: &str) -> Option<String> {
    transform_local_view_schema(sql, source, Some(target))
}

fn strip_local_view_schema(sql: &str, schema: &str) -> Option<String> {
    transform_local_view_schema(sql, schema, None)
}

fn transform_local_view_schema(sql: &str, source: &str, target: Option<&str>) -> Option<String> {
    let bytes = sql.as_bytes();
    let mut output = Vec::with_capacity(sql.len());
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        match bytes[index] {
            // MySQL may treat double quotes as strings or identifiers
            // depending on SQL_MODE. Refuse the ambiguous form.
            b'"' => return None,
            b'\'' => {
                let quote = bytes[index];
                index += 1;
                let mut closed = false;
                while index < bytes.len() {
                    if bytes[index] == b'\\' {
                        index = (index + 2).min(bytes.len());
                    } else if bytes[index] == quote {
                        if bytes.get(index + 1) == Some(&quote) {
                            index += 2;
                        } else {
                            index += 1;
                            closed = true;
                            break;
                        }
                    } else {
                        index += 1;
                    }
                }
                if !closed {
                    return None;
                }
                output.extend_from_slice(&bytes[start..index]);
            }
            b'`' => {
                index += 1;
                let mut identifier = Vec::new();
                let mut closed = false;
                while index < bytes.len() {
                    if bytes[index] == b'`' {
                        if bytes.get(index + 1) == Some(&b'`') {
                            identifier.push(b'`');
                            index += 2;
                        } else {
                            index += 1;
                            closed = true;
                            break;
                        }
                    } else {
                        identifier.push(bytes[index]);
                        index += 1;
                    }
                }
                if !closed {
                    return None;
                }
                let mut after = index;
                while bytes.get(after).is_some_and(u8::is_ascii_whitespace) {
                    after += 1;
                }
                if identifier == source.as_bytes() && bytes.get(after) == Some(&b'.') {
                    if let Some(target) = target {
                        output.extend_from_slice(quote_identifier(target).as_bytes());
                    } else {
                        index = after + 1;
                    }
                } else {
                    output.extend_from_slice(&bytes[start..index]);
                }
            }
            b'#' => {
                return None;
            }
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                return None;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                return None;
            }
            b';' => return None,
            byte if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$') => {
                index += 1;
                while bytes
                    .get(index)
                    .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$'))
                {
                    index += 1;
                }
                let token = &bytes[start..index];
                let mut after = index;
                while bytes.get(after).is_some_and(u8::is_ascii_whitespace) {
                    after += 1;
                }
                if token.eq_ignore_ascii_case(source.as_bytes()) && bytes.get(after) == Some(&b'.')
                {
                    return None;
                }
                if bytes.get(after) == Some(&b'.')
                    && !token.eq_ignore_ascii_case(b"NEW")
                    && !token.eq_ignore_ascii_case(b"OLD")
                {
                    return None;
                }
                output.extend_from_slice(token);
            }
            _ => {
                output.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8(output).ok()
}

async fn mysql_definer_is_available(connection: &mut MySqlConnection, definer: &str) -> bool {
    let current_user: Result<String, sqlx::Error> = sqlx::query_scalar("SELECT CURRENT_USER()")
        .fetch_one(connection)
        .await;
    // Requiring the exact active account proves existence and avoids needing
    // SET_USER_ID/SUPER to install a view with somebody else's definer.
    current_user.is_ok_and(|current_user| current_user == definer)
}

async fn validate_restore_foreign_keys(
    connection: &mut MySqlConnection,
    database: &str,
) -> Result<(), MysqlTableRestoreError> {
    let foreign_keys = sqlx::query_as::<_, ForeignKeyColumn>(
        "SELECT CAST(TABLE_NAME AS CHAR) AS table_name,
                CAST(CONSTRAINT_NAME AS CHAR) AS constraint_name,
                CAST(REFERENCED_TABLE_NAME AS CHAR) AS referenced_table,
                CAST(COLUMN_NAME AS CHAR) AS column_name,
                CAST(REFERENCED_COLUMN_NAME AS CHAR) AS referenced_column
         FROM information_schema.KEY_COLUMN_USAGE
         WHERE TABLE_SCHEMA = ? AND REFERENCED_TABLE_SCHEMA = ?
           AND REFERENCED_TABLE_NAME IS NOT NULL
         ORDER BY TABLE_NAME, CONSTRAINT_NAME, ORDINAL_POSITION",
    )
    .bind(database)
    .bind(database)
    .fetch_all(&mut *connection)
    .await
    .map_err(MysqlTableRestoreError::Database)?;
    let mut grouped: BTreeMap<(String, String, String), Vec<(String, String)>> = BTreeMap::new();
    for column in foreign_keys {
        grouped
            .entry((
                column.table_name,
                column.constraint_name,
                column.referenced_table,
            ))
            .or_default()
            .push((column.column_name, column.referenced_column));
    }
    for ((table, _constraint, referenced_table), columns) in grouped {
        let not_null = columns
            .iter()
            .map(|(child, _)| format!("c.{} IS NOT NULL", quote_identifier(child)))
            .collect::<Vec<_>>()
            .join(" AND ");
        let equality = columns
            .iter()
            .map(|(child, parent)| {
                format!(
                    "c.{} = p.{}",
                    quote_identifier(child),
                    quote_identifier(parent)
                )
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        let query = format!(
            "SELECT EXISTS(SELECT 1 FROM {} c WHERE {not_null} AND NOT EXISTS (SELECT 1 FROM {} p WHERE {equality}))",
            quote_identifier(&table),
            quote_identifier(&referenced_table)
        );
        let has_orphans: i64 = sqlx::query_scalar(&query)
            .fetch_one(&mut *connection)
            .await
            .map_err(MysqlTableRestoreError::Database)?;
        if has_orphans != 0 {
            return Err(MysqlTableRestoreError::InvalidStream);
        }
    }
    Ok(())
}

/// Authenticate an encrypted table-only snapshot completely before creating a
/// destination, then decrypt it again through a bounded pipe for restoration.
/// The caller supplies metadata loaded from DBSUAL's local recovery-point row.
#[allow(dead_code)] // Se conecta a IPC cuando exista el flujo de puntos de recuperación.
pub(crate) async fn restore_mysql_encrypted_table_snapshot(
    pool: &MySqlPool,
    target_database: &str,
    artifact_path: PathBuf,
    expected_vault_id: Uuid,
    expected_artifact_id: Uuid,
    expected_ciphertext_sha256: &str,
    expected_encrypted_bytes: u64,
) -> Result<(u64, u64), MysqlTableRestoreError> {
    restore_encrypted_table_snapshot_with_format(
        pool,
        target_database,
        artifact_path,
        expected_vault_id,
        expected_artifact_id,
        expected_ciphertext_sha256,
        expected_encrypted_bytes,
        MYSQL_STREAM_FORMAT,
    )
    .await
}

#[allow(dead_code)]
pub(crate) async fn restore_mariadb_encrypted_table_snapshot(
    pool: &MySqlPool,
    target_database: &str,
    artifact_path: PathBuf,
    expected_vault_id: Uuid,
    expected_artifact_id: Uuid,
    expected_ciphertext_sha256: &str,
    expected_encrypted_bytes: u64,
) -> Result<(u64, u64), MysqlTableRestoreError> {
    restore_encrypted_table_snapshot_with_format(
        pool,
        target_database,
        artifact_path,
        expected_vault_id,
        expected_artifact_id,
        expected_ciphertext_sha256,
        expected_encrypted_bytes,
        MARIADB_STREAM_FORMAT,
    )
    .await
}

async fn restore_encrypted_table_snapshot_with_format(
    pool: &MySqlPool,
    target_database: &str,
    artifact_path: PathBuf,
    expected_vault_id: Uuid,
    expected_artifact_id: Uuid,
    expected_ciphertext_sha256: &str,
    expected_encrypted_bytes: u64,
    expected_stream_format: &str,
) -> Result<(u64, u64), MysqlTableRestoreError> {
    if !valid_restore_identifier(target_database)
        || expected_ciphertext_sha256.len() != 64
        || !expected_ciphertext_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || expected_encrypted_bytes == 0
    {
        return Err(MysqlTableRestoreError::InvalidTarget);
    }

    let expected_hash = expected_ciphertext_sha256.to_ascii_lowercase();
    let first_pass_hash = expected_hash.clone();
    let (mut artifact_file, verified_info) = tokio::task::spawn_blocking(move || {
        let file = open_artifact_for_read(&artifact_path)
            .map_err(|_| MysqlTableRestoreError::InvalidArtifact)?;
        let file_size = file
            .metadata()
            .map_err(|_| MysqlTableRestoreError::InvalidArtifact)?
            .len();
        if file_size != expected_encrypted_bytes {
            return Err(MysqlTableRestoreError::InvalidArtifact);
        }
        let mut reader = HashingReader::new(file);
        let info = crate::vault::verify_artifact_from_keyring(&mut reader)
            .map_err(|_| MysqlTableRestoreError::InvalidArtifact)?;
        let (mut file, digest) = reader.finish();
        if info.vault_id != expected_vault_id
            || info.artifact_id != expected_artifact_id
            || digest != first_pass_hash
        {
            return Err(MysqlTableRestoreError::InvalidArtifact);
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|_| MysqlTableRestoreError::InvalidArtifact)?;
        Ok((file, info))
    })
    .await
    .map_err(|_| MysqlTableRestoreError::Worker)??;

    let (read_end, write_end) = tokio::io::duplex(STREAM_CHUNK_BYTES * STREAM_CHANNEL_CHUNKS);
    let runtime = tokio::runtime::Handle::current();
    let second_pass = tokio::task::spawn_blocking(move || {
        let mut reader = HashingReader::new(&mut artifact_file);
        let mut writer = BlockingDuplexWriter::new(runtime, write_end);
        let decrypt_result = crate::vault::decrypt_artifact_from_keyring(&mut reader, &mut writer);
        let (_file, digest) = reader.finish();
        let authenticated = decrypt_result.is_ok_and(|info| {
            info == verified_info
                && info.vault_id == expected_vault_id
                && info.artifact_id == expected_artifact_id
                && digest == expected_hash
        });
        if !authenticated {
            // The parser checks for EOF after the database footer. This record
            // forces rollback if the second authentication pass fails.
            let _ = writer.write_all(b"\n{}\n");
            let _ = writer.flush();
            return Err(MysqlTableRestoreError::InvalidArtifact);
        }
        writer
            .flush()
            .map_err(|_| MysqlTableRestoreError::InvalidArtifact)?;
        Ok(())
    });

    let mut plaintext_reader = tokio::io::BufReader::new(read_end);
    let restore_result = restore_database_table_stream_with_format(
        pool,
        target_database,
        &mut plaintext_reader,
        expected_stream_format,
    )
    .await;
    // If parsing fails early, close the bounded pipe before joining the
    // decryptor so its writer cannot wait forever for a reader that exited.
    drop(plaintext_reader);
    let decrypt_result = match second_pass.await {
        Ok(result) => result,
        Err(_) => Err(MysqlTableRestoreError::Worker),
    };

    match (restore_result, decrypt_result) {
        (Ok(restored), Ok(())) => Ok(restored),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => {
            let mut connection = pool
                .acquire()
                .await
                .map_err(|_| MysqlTableRestoreError::IncompleteTarget)?
                .detach();
            drop_restore_destination(&mut connection, target_database).await?;
            Err(error)
        }
    }
}

fn parse_mysql_restore_table(
    header: &serde_json::Value,
) -> Result<MysqlRestoreTable, MysqlTableRestoreError> {
    if header["record"] != "table" || header["format"] != "dbsual-mysql-table-v1" {
        return Err(MysqlTableRestoreError::InvalidStream);
    }
    restore_identifier_field(header, "database")?;
    restore_identifier_field(header, "characterSet")?;
    restore_identifier_field(header, "collation")?;
    let name = restore_identifier_field(header, "table")?;
    let create_sql = header["createSql"]
        .as_str()
        .filter(|sql| valid_table_create_sql(sql, &name))
        .ok_or(MysqlTableRestoreError::InvalidStream)?
        .to_owned();
    let columns = header["columns"]
        .as_array()
        .filter(|columns| !columns.is_empty() && columns.len() <= 4096)
        .ok_or(MysqlTableRestoreError::InvalidStream)?;
    let mut column_names = Vec::with_capacity(columns.len());
    let mut column_types = Vec::with_capacity(columns.len());
    let mut generated = Vec::with_capacity(columns.len());
    for column in columns {
        let name = column["name"]
            .as_str()
            .filter(|name| !name.is_empty() && name.len() <= 255)
            .ok_or(MysqlTableRestoreError::InvalidStream)?;
        let mysql_type = column["mysqlType"]
            .as_str()
            .ok_or(MysqlTableRestoreError::InvalidStream)?
            .to_ascii_uppercase();
        if mysql_type == "GEOMETRY" {
            return Err(MysqlTableRestoreError::InvalidStream);
        }
        column_names.push(name.to_owned());
        column_types.push(mysql_type);
        generated.push(
            column["generated"]
                .as_bool()
                .ok_or(MysqlTableRestoreError::InvalidStream)?,
        );
    }
    Ok(MysqlRestoreTable {
        name,
        create_sql,
        column_names,
        column_types,
        generated,
    })
}

async fn restore_mysql_table_rows<R>(
    reader: &mut R,
    table: &MysqlRestoreTable,
    connection: &mut MySqlConnection,
) -> Result<(u64, String), MysqlTableRestoreError>
where
    R: AsyncBufRead + Unpin,
{
    let mut row_set_digest = CanonicalRowSetDigest::default();
    loop {
        let (record, raw_line) = read_restore_record_with_line(reader).await?;
        match record["record"].as_str() {
            Some("row") => {
                let canonical_row = raw_line.strip_suffix(b"\n").unwrap_or(&raw_line);
                row_set_digest.update(canonical_row)?;
                let values = record["values"]
                    .as_array()
                    .filter(|values| values.len() == table.column_names.len())
                    .ok_or(MysqlTableRestoreError::InvalidStream)?;
                let writable_indices = (0..table.column_names.len())
                    .filter(|index| !table.generated[*index])
                    .collect::<Vec<_>>();
                let mut query = QueryBuilder::<MySql>::new("INSERT INTO ");
                query.push(quote_identifier(&table.name));
                if writable_indices.is_empty() {
                    query.push(" () VALUES ()");
                } else {
                    query.push(" (");
                    for (position, index) in writable_indices.iter().enumerate() {
                        if position > 0 {
                            query.push(", ");
                        }
                        query.push(quote_identifier(&table.column_names[*index]));
                    }
                    query.push(") VALUES (");
                    for (position, index) in writable_indices.iter().enumerate() {
                        if position > 0 {
                            query.push(", ");
                        }
                        if values[*index].is_null() {
                            query.push_bind(Option::<String>::None);
                            continue;
                        }
                        let hex = values[*index]["hex"]
                            .as_str()
                            .ok_or(MysqlTableRestoreError::InvalidStream)?;
                        let bytes = decode_hex(hex).ok_or(MysqlTableRestoreError::InvalidStream)?;
                        if is_mysql_binary_type(&table.column_types[*index]) {
                            query.push_bind(Some(bytes));
                        } else {
                            let text = String::from_utf8(bytes)
                                .map_err(|_| MysqlTableRestoreError::InvalidStream)?;
                            query.push_bind(Some(text));
                        }
                    }
                    query.push(")");
                }
                query
                    .build()
                    .execute(&mut *connection)
                    .await
                    .map_err(MysqlTableRestoreError::Database)?;
            }
            Some("end") => {
                let expected = record["rows"]
                    .as_u64()
                    .ok_or(MysqlTableRestoreError::InvalidStream)?;
                let expected_digest = record["rowSetSha256"]
                    .as_str()
                    .filter(|hash| is_sha256_hex(hash))
                    .ok_or(MysqlTableRestoreError::InvalidStream)?
                    .to_ascii_lowercase();
                if expected != row_set_digest.rows || expected_digest != row_set_digest.finish_hex()
                {
                    return Err(MysqlTableRestoreError::InvalidStream);
                }
                let actual: i64 = sqlx::query_scalar(&format!(
                    "SELECT COUNT(*) FROM {}",
                    quote_identifier(&table.name)
                ))
                .fetch_one(&mut *connection)
                .await
                .map_err(MysqlTableRestoreError::Database)?;
                if u64::try_from(actual).ok() != Some(expected) {
                    return Err(MysqlTableRestoreError::InvalidStream);
                }
                return Ok((expected, expected_digest));
            }
            _ => return Err(MysqlTableRestoreError::InvalidStream),
        }
    }
}

async fn canonical_table_row_set_digest(
    table: &MysqlRestoreTable,
    connection: &mut MySqlConnection,
) -> Result<String, MysqlTableRestoreError> {
    let projection = table
        .column_names
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let query = format!("SELECT {projection} FROM {}", quote_identifier(&table.name));
    let mut rows = sqlx::raw_sql(&query).fetch(&mut *connection);
    let mut digest = CanonicalRowSetDigest::default();
    while let Some(row) = rows
        .try_next()
        .await
        .map_err(MysqlTableRestoreError::Database)?
    {
        if row.columns().len() != table.column_names.len() {
            return Err(MysqlTableRestoreError::InvalidStream);
        }
        let mut values = Vec::with_capacity(row.columns().len());
        for index in 0..row.columns().len() {
            let raw = row
                .try_get_raw(index)
                .map_err(MysqlTableRestoreError::Database)?;
            match raw.as_bytes() {
                Ok(bytes) => values.push(serde_json::json!({ "hex": encode_hex(bytes) })),
                Err(_) => values.push(serde_json::Value::Null),
            }
        }
        let encoded = serde_json::to_vec(&serde_json::json!({
            "record": "row",
            "values": values,
        }))
        .map_err(|_| MysqlTableRestoreError::InvalidStream)?;
        digest.update(&encoded)?;
    }
    Ok(digest.finish_hex())
}

async fn read_restore_record<R>(reader: &mut R) -> Result<serde_json::Value, MysqlTableRestoreError>
where
    R: AsyncBufRead + Unpin,
{
    read_restore_record_with_line(reader)
        .await
        .map(|(record, _)| record)
}

async fn read_restore_record_with_line<R>(
    reader: &mut R,
) -> Result<(serde_json::Value, Vec<u8>), MysqlTableRestoreError>
where
    R: AsyncBufRead + Unpin,
{
    let line = read_bounded_line(reader, MAX_TABLE_RECORD_BYTES)
        .await
        .map_err(|_| MysqlTableRestoreError::InvalidStream)?
        .ok_or(MysqlTableRestoreError::InvalidStream)?;
    let record =
        serde_json::from_slice(&line).map_err(|_| MysqlTableRestoreError::InvalidStream)?;
    Ok((record, line))
}

fn restore_identifier_field(
    record: &serde_json::Value,
    field: &str,
) -> Result<String, MysqlTableRestoreError> {
    record[field]
        .as_str()
        .filter(|value| valid_restore_identifier(value))
        .map(str::to_owned)
        .ok_or(MysqlTableRestoreError::InvalidStream)
}

async fn ensure_restore_destination_absent(
    connection: &mut MySqlConnection,
    target_database: &str,
) -> Result<(), MysqlTableRestoreError> {
    let exists: Option<(Vec<u8>,)> =
        sqlx::query_as("SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?")
            .bind(target_database)
            .fetch_optional(&mut *connection)
            .await
            .map_err(MysqlTableRestoreError::Database)?;
    if exists.is_some() {
        return Err(MysqlTableRestoreError::DestinationExists);
    }
    Ok(())
}

async fn create_restore_destination(
    connection: &mut MySqlConnection,
    target_database: &str,
    character_set: &str,
    collation: &str,
) -> Result<(), MysqlTableRestoreError> {
    sqlx::raw_sql(&format!(
        "CREATE DATABASE {} CHARACTER SET {} COLLATE {}",
        quote_identifier(target_database),
        quote_identifier(character_set),
        quote_identifier(collation)
    ))
    .execute(&mut *connection)
    .await
    .map_err(MysqlTableRestoreError::Database)?;
    Ok(())
}

async fn drop_restore_destination(
    connection: &mut MySqlConnection,
    target_database: &str,
) -> Result<(), MysqlTableRestoreError> {
    if sqlx::raw_sql(&format!(
        "DROP DATABASE {}",
        quote_identifier(target_database)
    ))
    .execute(&mut *connection)
    .await
    .is_err()
    {
        return Err(MysqlTableRestoreError::IncompleteTarget);
    }
    Ok(())
}

fn valid_restore_identifier(identifier: &str) -> bool {
    !identifier.is_empty()
        && identifier.len() <= 64
        && identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$'))
}

async fn read_bounded_line<R>(reader: &mut R, limit: usize) -> io::Result<Option<Vec<u8>>>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok((!line.is_empty()).then_some(line));
        }
        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if line.len().saturating_add(consumed) > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "record exceeds the supported size",
            ));
        }
        let has_newline = available[consumed - 1] == b'\n';
        line.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if has_newline {
            return Ok(Some(line));
        }
    }
}

fn valid_table_create_sql(sql: &str, table_name: &str) -> bool {
    let prefix = format!("CREATE TABLE {} (", quote_identifier(table_name));
    let upper = sql.to_ascii_uppercase();
    // MySQL emits this executable version comment for INVISIBLE columns. It is
    // the only comment form accepted from server-generated table DDL.
    let without_invisible_version_comment = upper.replace("/*!80023 INVISIBLE */", "");
    let has_innodb_table_option = upper
        .rfind(") ENGINE=")
        .is_some_and(|index| upper[index..].starts_with(") ENGINE=INNODB"));
    sql.len() <= MAX_TABLE_RECORD_BYTES
        && sql.starts_with(&prefix)
        && !sql.contains(';')
        && !sql.contains('.')
        && !sql.contains('#')
        && !sql.contains("--")
        && !without_invisible_version_comment.contains("/*")
        && !upper.contains("DATA DIRECTORY")
        && !upper.contains("INDEX DIRECTORY")
        && !upper.contains("TABLESPACE")
        && has_innodb_table_option
}

fn is_mysql_binary_type(mysql_type: &str) -> bool {
    matches!(
        mysql_type,
        "BINARY" | "VARBINARY" | "TINYBLOB" | "BLOB" | "MEDIUMBLOB" | "LONGBLOB" | "BIT"
    )
}

fn decode_hex(encoded: &str) -> Option<Vec<u8>> {
    if encoded.len() % 2 != 0 {
        return None;
    }
    encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |byte| match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                _ => None,
            };
            Some((digit(pair[0])? << 4) | digit(pair[1])?)
        })
        .collect()
}

fn quote_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}

#[derive(Debug, PartialEq, Eq)]
enum ViewSqlToken {
    Word(String),
    Identifier(String),
    Dot,
    Comma,
    LeftParen,
    RightParen,
    Other,
}

/// Extract direct table/view names after FROM and JOIN from a view query.
/// The parser ignores comments and string literals and fails closed on derived
/// tables and comma joins, which cannot be reconstructed safely here.
fn view_query_dependencies(
    query: &str,
    default_schema: &str,
) -> Option<Vec<BackupObjectReference>> {
    let tokens = tokenize_view_query(query)?;
    let mut dependencies = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let ViewSqlToken::Word(keyword) = token else {
            continue;
        };
        if !matches!(keyword.to_ascii_lowercase().as_str(), "from" | "join") {
            continue;
        }
        let first = tokens.get(index + 1)?;
        if matches!(first, ViewSqlToken::LeftParen) {
            return None;
        }
        let first_name = view_token_identifier(first)?;
        let (schema, name, consumed) = if matches!(tokens.get(index + 2), Some(ViewSqlToken::Dot)) {
            let second_name = view_token_identifier(tokens.get(index + 3)?)?;
            (first_name, second_name, 4)
        } else {
            (default_schema.to_owned(), first_name, 2)
        };
        if schema.is_empty() || name.is_empty() {
            return None;
        }
        dependencies.push(BackupObjectReference { schema, name });

        // Do not silently omit a comma-separated relation in this FROM/JOIN
        // segment. Function argument commas are nested and remain supported.
        let mut depth = 0_i32;
        for following in tokens.iter().skip(index + consumed) {
            match following {
                ViewSqlToken::LeftParen => depth += 1,
                ViewSqlToken::RightParen => depth = (depth - 1).max(0),
                ViewSqlToken::Comma if depth == 0 => return None,
                ViewSqlToken::Word(word)
                    if depth == 0
                        && matches!(
                            word.to_ascii_lowercase().as_str(),
                            "join"
                                | "where"
                                | "on"
                                | "using"
                                | "group"
                                | "order"
                                | "having"
                                | "limit"
                                | "union"
                                | "window"
                        ) =>
                {
                    break;
                }
                _ => {}
            }
        }
    }
    if dependencies.is_empty() {
        return None;
    }
    dependencies
        .sort_by(|left, right| (&left.schema, &left.name).cmp(&(&right.schema, &right.name)));
    dependencies.dedup();
    Some(dependencies)
}

fn view_token_identifier(token: &ViewSqlToken) -> Option<String> {
    match token {
        ViewSqlToken::Word(value) | ViewSqlToken::Identifier(value) => Some(value.clone()),
        _ => None,
    }
}

fn tokenize_view_query(query: &str) -> Option<Vec<ViewSqlToken>> {
    let bytes = query.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if byte == b'#' || (byte == b'-' && bytes.get(index + 1) == Some(&b'-')) {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index += 2;
            let end = query[index..].find("*/")? + index;
            index = end + 2;
            continue;
        }
        if byte == b'\'' || byte == b'"' {
            let quote = byte;
            index += 1;
            let mut closed = false;
            while index < bytes.len() {
                if bytes[index] == b'\\' {
                    index = (index + 2).min(bytes.len());
                } else if bytes[index] == quote {
                    if bytes.get(index + 1) == Some(&quote) {
                        index += 2;
                    } else {
                        index += 1;
                        closed = true;
                        break;
                    }
                } else {
                    index += 1;
                }
            }
            if !closed {
                return None;
            }
            tokens.push(ViewSqlToken::Other);
            continue;
        }
        if byte == b'`' {
            index += 1;
            let mut identifier = Vec::new();
            let mut closed = false;
            while index < bytes.len() {
                if bytes[index] == b'`' {
                    if bytes.get(index + 1) == Some(&b'`') {
                        identifier.push(b'`');
                        index += 2;
                    } else {
                        index += 1;
                        closed = true;
                        break;
                    }
                } else {
                    identifier.push(bytes[index]);
                    index += 1;
                }
            }
            if !closed {
                return None;
            }
            tokens.push(ViewSqlToken::Identifier(
                String::from_utf8(identifier).ok()?,
            ));
            continue;
        }
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$') {
            let start = index;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'_' | b'$'))
            {
                index += 1;
            }
            tokens.push(ViewSqlToken::Word(
                std::str::from_utf8(&bytes[start..index]).ok()?.to_owned(),
            ));
            continue;
        }
        tokens.push(match byte {
            b'.' => ViewSqlToken::Dot,
            b',' => ViewSqlToken::Comma,
            b'(' => ViewSqlToken::LeftParen,
            b')' => ViewSqlToken::RightParen,
            _ => ViewSqlToken::Other,
        });
        index += 1;
    }
    Some(tokens)
}

fn create_statement_column(row: &sqlx::mysql::MySqlRow) -> Result<String, sqlx::Error> {
    let index = row
        .columns()
        .iter()
        .position(|column| {
            column.name().starts_with("Create ") || column.name() == "SQL Original Statement"
        })
        .ok_or_else(|| sqlx::Error::ColumnNotFound("CREATE statement".into()))?;
    row.try_get(index)
}

/// Read server-rendered DDL for every inventoried object. Names are re-quoted
/// before being sent back to MySQL; the SQL text is returned verbatim for the
/// later portable serializer.
pub async fn read_mysql_definitions(
    pool: &MySqlPool,
    inventory: &MysqlBackupInventory,
) -> Result<Vec<BackupDefinition>, sqlx::Error> {
    let mut connection = pool.acquire().await?;
    read_mysql_definitions_on_connection(&mut connection, inventory).await
}

async fn read_mysql_definitions_on_connection(
    connection: &mut MySqlConnection,
    inventory: &MysqlBackupInventory,
) -> Result<Vec<BackupDefinition>, sqlx::Error> {
    let database = quote_identifier(&inventory.database_name);
    let mut definitions = Vec::new();
    let mut dependencies_by_view = HashMap::<String, Vec<BackupObjectReference>>::new();
    if inventory
        .server_version
        .to_ascii_lowercase()
        .contains("mariadb")
    {
        let view_definitions: Vec<(String, String)> = sqlx::query_as(
            "SELECT CAST(TABLE_NAME AS CHAR), CAST(VIEW_DEFINITION AS CHAR) \
             FROM information_schema.VIEWS WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME",
        )
        .bind(&inventory.database_name)
        .fetch_all(&mut *connection)
        .await?;
        for (view, query) in view_definitions {
            let dependencies = view_query_dependencies(&query, &inventory.database_name)
                .ok_or_else(|| {
                    sqlx::Error::Protocol(
                        format!("view dependencies are not safely understood for {view}").into(),
                    )
                })?;
            dependencies_by_view.insert(view, dependencies);
        }
    } else {
        let view_dependencies: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT CAST(VIEW_NAME AS CHAR), CAST(TABLE_SCHEMA AS CHAR), CAST(TABLE_NAME AS CHAR) \
             FROM information_schema.VIEW_TABLE_USAGE WHERE VIEW_SCHEMA = ? ORDER BY VIEW_NAME, TABLE_SCHEMA, TABLE_NAME",
        )
        .bind(&inventory.database_name)
        .fetch_all(&mut *connection)
        .await?;
        for (view, schema, name) in view_dependencies {
            dependencies_by_view
                .entry(view)
                .or_default()
                .push(BackupObjectReference { schema, name });
        }
    }
    let view_context_rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT CAST(TABLE_NAME AS CHAR), CAST(DEFINER AS CHAR), \
                CAST(CHARACTER_SET_CLIENT AS CHAR), CAST(COLLATION_CONNECTION AS CHAR) \
         FROM information_schema.VIEWS WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME",
    )
    .bind(&inventory.database_name)
    .fetch_all(&mut *connection)
    .await?;
    let view_context: HashMap<String, (String, String, String)> = view_context_rows
        .into_iter()
        .map(|(name, definer, charset, collation)| (name, (definer, charset, collation)))
        .collect();
    let routine_context: HashMap<String, (String, String, String, String, String)> = sqlx::query_as::<_, (String, String, String, String, String, String)>(
        "SELECT CAST(ROUTINE_NAME AS CHAR), CAST(DEFINER AS CHAR), CAST(SQL_MODE AS CHAR), \
                CAST(CHARACTER_SET_CLIENT AS CHAR), CAST(COLLATION_CONNECTION AS CHAR), CAST(DATABASE_COLLATION AS CHAR) \
         FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA = ? ORDER BY ROUTINE_NAME, ROUTINE_TYPE",
    )
    .bind(&inventory.database_name)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|(name, definer, sql_mode, charset, collation, db_collation)| {
        (name, (definer, sql_mode, charset, collation, db_collation))
    })
    .collect();
    let trigger_context: HashMap<String, (String, String, String, String, String)> = sqlx::query_as::<_, (String, String, String, String, String, String)>(
        "SELECT CAST(TRIGGER_NAME AS CHAR), CAST(DEFINER AS CHAR), CAST(SQL_MODE AS CHAR), \
                CAST(CHARACTER_SET_CLIENT AS CHAR), CAST(COLLATION_CONNECTION AS CHAR), CAST(DATABASE_COLLATION AS CHAR) \
         FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA = ? ORDER BY TRIGGER_NAME",
    )
    .bind(&inventory.database_name)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|(name, definer, sql_mode, charset, collation, db_collation)| {
        (name, (definer, sql_mode, charset, collation, db_collation))
    })
    .collect();
    let event_context: HashMap<String, (String, String, String, String, String)> = sqlx::query_as::<_, (String, String, String, String, String, String)>(
        "SELECT CAST(EVENT_NAME AS CHAR), CAST(DEFINER AS CHAR), CAST(SQL_MODE AS CHAR), \
                CAST(CHARACTER_SET_CLIENT AS CHAR), CAST(COLLATION_CONNECTION AS CHAR), CAST(DATABASE_COLLATION AS CHAR) \
         FROM information_schema.EVENTS WHERE EVENT_SCHEMA = ? ORDER BY EVENT_NAME",
    )
    .bind(&inventory.database_name)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|(name, definer, sql_mode, charset, collation, db_collation)| {
        (name, (definer, sql_mode, charset, collation, db_collation))
    })
    .collect();
    for object in &inventory.objects {
        let name = quote_identifier(&object.name);
        let command = if object.kind == "VIEW" {
            format!("SHOW CREATE VIEW {database}.{name}")
        } else {
            format!("SHOW CREATE TABLE {database}.{name}")
        };
        let row = sqlx::query(&command).fetch_one(&mut *connection).await?;
        let view_context = if object.kind == "VIEW" {
            Some(view_context.get(&object.name).ok_or_else(|| {
                sqlx::Error::Protocol("view execution context is unavailable".into())
            })?)
        } else {
            None
        };
        definitions.push(BackupDefinition {
            name: object.name.clone(),
            kind: object.kind.clone(),
            create_sql: create_statement_column(&row)?,
            dependencies: if object.kind == "VIEW" {
                dependencies_by_view
                    .remove(&object.name)
                    .unwrap_or_default()
            } else {
                Vec::new()
            },
            definer: view_context.map(|context| context.0.clone()),
            sql_mode: None,
            character_set_client: view_context.map(|context| context.1.clone()),
            collation_connection: view_context.map(|context| context.2.clone()),
            database_collation: None,
        });
    }
    for routine in &inventory.routines {
        let command = format!(
            "SHOW CREATE {} {database}.{}",
            routine.kind,
            quote_identifier(&routine.name)
        );
        let row = sqlx::query(&command).fetch_one(&mut *connection).await?;
        let context = routine_context.get(&routine.name).ok_or_else(|| {
            sqlx::Error::Protocol("routine execution context is unavailable".into())
        })?;
        definitions.push(BackupDefinition {
            name: routine.name.clone(),
            kind: routine.kind.clone(),
            create_sql: create_statement_column(&row)?,
            dependencies: Vec::new(),
            definer: Some(context.0.clone()),
            sql_mode: Some(context.1.clone()),
            character_set_client: Some(context.2.clone()),
            collation_connection: Some(context.3.clone()),
            database_collation: Some(context.4.clone()),
        });
    }
    for trigger in &inventory.triggers {
        let command = format!(
            "SHOW CREATE TRIGGER {database}.{}",
            quote_identifier(&trigger.name)
        );
        let row = sqlx::query(&command).fetch_one(&mut *connection).await?;
        let context = trigger_context.get(&trigger.name).ok_or_else(|| {
            sqlx::Error::Protocol("trigger execution context is unavailable".into())
        })?;
        definitions.push(BackupDefinition {
            name: trigger.name.clone(),
            kind: "TRIGGER".into(),
            create_sql: create_statement_column(&row)?,
            dependencies: Vec::new(),
            definer: Some(context.0.clone()),
            sql_mode: Some(context.1.clone()),
            character_set_client: Some(context.2.clone()),
            collation_connection: Some(context.3.clone()),
            database_collation: Some(context.4.clone()),
        });
    }
    for event in &inventory.events {
        let command = format!("SHOW CREATE EVENT {database}.{}", quote_identifier(event));
        let row = sqlx::query(&command).fetch_one(&mut *connection).await?;
        let context = event_context.get(event).ok_or_else(|| {
            sqlx::Error::Protocol("event execution context is unavailable".into())
        })?;
        definitions.push(BackupDefinition {
            name: event.clone(),
            kind: "EVENT".into(),
            create_sql: create_statement_column(&row)?,
            dependencies: Vec::new(),
            definer: Some(context.0.clone()),
            sql_mode: Some(context.1.clone()),
            character_set_client: Some(context.2.clone()),
            collation_connection: Some(context.3.clone()),
            database_collation: Some(context.4.clone()),
        });
    }
    Ok(definitions)
}

async fn write_record<W: AsyncWrite + Unpin>(
    writer: &mut W,
    record: serde_json::Value,
) -> Result<(), sqlx::Error> {
    let encoded = serde_json::to_vec(&record)
        .map_err(|error| sqlx::Error::Protocol(error.to_string().into()))?;
    writer.write_all(&encoded).await.map_err(sqlx::Error::Io)?;
    writer.write_all(b"\n").await.map_err(sqlx::Error::Io)
}

/// Compose inventory, server-rendered definitions, and streamed table rows.
/// This content stream is not a protected recovery point until consistency,
/// permissions, and restoration are verified by the provider.
pub async fn stream_mysql_database<W>(
    connection: &mut MySqlConnection,
    inspection: &MysqlBackupInspection,
    writer: &mut W,
) -> Result<u64, sqlx::Error>
where
    W: AsyncWrite + Unpin,
{
    stream_database_with_format(connection, inspection, writer, MYSQL_STREAM_FORMAT).await
}

#[allow(dead_code)]
pub(crate) async fn stream_mariadb_database<W>(
    connection: &mut MySqlConnection,
    inspection: &MysqlBackupInspection,
    writer: &mut W,
) -> Result<u64, sqlx::Error>
where
    W: AsyncWrite + Unpin,
{
    stream_database_with_format(connection, inspection, writer, MARIADB_STREAM_FORMAT).await
}

/// Serialize using the explicit provider format. MySQL and MariaDB remain
/// incompatible at the artifact boundary even though SQLx shares a protocol
/// adapter for their supported table and row operations.
async fn stream_database_with_format<W>(
    connection: &mut MySqlConnection,
    inspection: &MysqlBackupInspection,
    writer: &mut W,
    format: &str,
) -> Result<u64, sqlx::Error>
where
    W: AsyncWrite + Unpin,
{
    let inventory = &inspection.inventory;
    if !inventory.visible_tables_are_transactional {
        return Err(sqlx::Error::Protocol(
            "backup contains a visible non-InnoDB table".into(),
        ));
    }
    write_record(
        writer,
        serde_json::json!({
            "record": "database",
            "format": format,
            "coverage": inventory.coverage,
            "serverVersion": inventory.server_version,
            "database": inventory.database_name,
            "characterSet": inventory.character_set,
            "collation": inventory.collation,
            "objectCount": inspection.definitions.len(),
        }),
    )
    .await?;
    for definition in &inspection.definitions {
        write_record(
            writer,
            serde_json::json!({
                "record": "definition",
                "kind": definition.kind,
                "name": definition.name,
                "createSql": definition.create_sql,
                "dependencies": definition.dependencies,
                "definer": definition.definer,
                "sqlMode": definition.sql_mode,
                "characterSetClient": definition.character_set_client,
                "collationConnection": definition.collation_connection,
                "databaseCollation": definition.database_collation,
            }),
        )
        .await?;
    }

    let mut table_count = 0_u64;
    let mut row_count = 0_u64;
    for table in inventory
        .objects
        .iter()
        .filter(|object| object.kind == "BASE TABLE")
    {
        let rows =
            stream_mysql_table_rows_on_connection(connection, inventory, &table.name, writer)
                .await?;
        row_count = row_count
            .checked_add(rows)
            .ok_or_else(|| sqlx::Error::Protocol("backup row count overflow".into()))?;
        table_count += 1;
    }
    write_record(
        writer,
        serde_json::json!({
            "record": "database_end",
            "streamComplete": true,
            "tables": table_count,
            "rows": row_count,
        }),
    )
    .await?;
    writer.flush().await.map_err(sqlx::Error::Io)?;
    Ok(row_count)
}

/// Connect the async MySQL reader to the authenticated artifact writer through
/// a bounded channel. Producer errors are forwarded as reader errors so the
/// vault discards its provisional ciphertext instead of publishing a partial.
pub async fn encrypt_mysql_database_stream(
    pool: &MySqlPool,
    database_name: &str,
    directory: PathBuf,
    master_key: crate::vault::MasterKey,
    recovery: crate::vault::RecoveryEnvelope,
) -> Result<crate::vault::PublishedArtifactInfo, MysqlBackupStreamError> {
    encrypt_mysql_database_stream_inner(pool, database_name, directory, master_key, recovery, false)
        .await
}

/// Capture only database layouts that the current recovery-point restorer can
/// reconstruct. This check occurs in the same locked snapshot as serialization.
pub async fn encrypt_mysql_table_recovery_snapshot(
    pool: &MySqlPool,
    database_name: &str,
    directory: PathBuf,
    master_key: crate::vault::MasterKey,
    recovery: crate::vault::RecoveryEnvelope,
) -> Result<crate::vault::PublishedArtifactInfo, MysqlBackupStreamError> {
    encrypt_mysql_database_stream_inner(pool, database_name, directory, master_key, recovery, true)
        .await
}

/// Capture the currently restorable MariaDB subset with MariaDB's own
/// BACKUP STAGE protocol and artifact format. This remains a partial recovery
/// point; callers must not use it to authorize destructive operations.
pub(crate) async fn encrypt_mariadb_table_recovery_snapshot(
    pool: &MySqlPool,
    database_name: &str,
    directory: PathBuf,
    master_key: crate::vault::MasterKey,
    recovery: crate::vault::RecoveryEnvelope,
) -> Result<crate::vault::PublishedArtifactInfo, MysqlBackupStreamError> {
    let mut stage = pool
        .acquire()
        .await
        .map_err(MysqlBackupStreamError::Database)?
        .detach();
    let server_version: String = sqlx::query_scalar("SELECT VERSION()")
        .fetch_one(&mut stage)
        .await
        .map_err(MysqlBackupStreamError::Database)?;
    if !supports_mariadb_backup_stage(&server_version) {
        return Err(MysqlBackupStreamError::UnsupportedServer(
            "El respaldo MariaDB requiere MariaDB 10.6, 10.11 o 11.4.",
        ));
    }

    sqlx::raw_sql("SET SESSION lock_wait_timeout = 10")
        .execute(&mut stage)
        .await
        .map_err(MysqlBackupStreamError::Database)?;
    sqlx::raw_sql("BACKUP STAGE START")
        .execute(&mut stage)
        .await
        .map_err(MysqlBackupStreamError::Database)?;
    let mut stage_active = true;
    for statement in [
        "BACKUP STAGE FLUSH",
        "BACKUP STAGE BLOCK_DDL",
        "BACKUP STAGE BLOCK_COMMIT",
    ] {
        if let Err(error) = sqlx::raw_sql(statement).execute(&mut stage).await {
            let _ = end_mariadb_backup_stage(&mut stage, &mut stage_active).await;
            let _ = stage.close().await;
            return Err(MysqlBackupStreamError::Database(error));
        }
    }

    let mut snapshot = match pool.acquire().await {
        Ok(connection) => connection.detach(),
        Err(error) => {
            let _ = end_mariadb_backup_stage(&mut stage, &mut stage_active).await;
            let _ = stage.close().await;
            return Err(MysqlBackupStreamError::Database(error));
        }
    };
    let inspection = async {
        sqlx::raw_sql("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut snapshot)
            .await?;
        sqlx::raw_sql("START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY")
            .execute(&mut snapshot)
            .await?;
        let inventory = inspect_mysql_database_on_connection(&mut snapshot, database_name).await?;
        if !supports_mariadb_backup_stage(&inventory.server_version) {
            return Err(sqlx::Error::Protocol(
                "MariaDB backup provider rejected a non-supported server family or version".into(),
            ));
        }
        let definitions = read_mysql_definitions_on_connection(&mut snapshot, &inventory).await?;
        let inspection = MysqlBackupInspection {
            inventory,
            definitions,
        };
        validate_table_recovery_snapshot(&mut snapshot, database_name, &inspection).await?;
        hold_mariadb_snapshot_metadata_locks(&mut snapshot, &inspection.inventory).await?;
        Ok::<_, sqlx::Error>(inspection)
    }
    .await;
    let inspection = match inspection {
        Ok(inspection) => inspection,
        Err(error) => {
            let _ = end_mariadb_backup_stage(&mut stage, &mut stage_active).await;
            let _ = sqlx::raw_sql("ROLLBACK").execute(&mut snapshot).await;
            let _ = stage.close().await;
            let _ = snapshot.close().await;
            return Err(MysqlBackupStreamError::Database(error));
        }
    };

    if let Err(error) = end_mariadb_backup_stage(&mut stage, &mut stage_active).await {
        // An ambiguous END must not publish a point. Closing this dedicated
        // session is the final server-side release for BACKUP STAGE.
        let _ = stage.close().await;
        let _ = sqlx::raw_sql("ROLLBACK").execute(&mut snapshot).await;
        let _ = snapshot.close().await;
        return Err(MysqlBackupStreamError::Database(error));
    }
    stage
        .close()
        .await
        .map_err(MysqlBackupStreamError::Database)?;

    let vault_id = recovery.vault_id();
    let (sender, receiver) = mpsc::channel(STREAM_CHANNEL_CHUNKS);
    let encrypt_task = tokio::task::spawn_blocking(move || {
        let mut reader = ChannelReader {
            receiver,
            current: Cursor::new(Vec::new()),
        };
        crate::vault::encrypt_artifact_reader_to_directory(
            Path::new(&directory),
            &mut reader,
            vault_id,
            &master_key,
            &recovery,
        )
    });
    let mut writer = ChannelWriter::new(sender);
    if let Err(error) = stream_mariadb_database(&mut snapshot, &inspection, &mut writer).await {
        writer.fail().await;
        drop(writer);
        let _ = sqlx::raw_sql("ROLLBACK").execute(&mut snapshot).await;
        let _ = snapshot.close().await;
        return match encrypt_task.await {
            Ok(Err(error)) => Err(MysqlBackupStreamError::Vault(error)),
            Err(_) => Err(MysqlBackupStreamError::Worker),
            Ok(Ok(_)) => Err(MysqlBackupStreamError::Database(error)),
        };
    }
    if let Err(error) = sqlx::raw_sql("COMMIT").execute(&mut snapshot).await {
        writer.fail().await;
        drop(writer);
        let _ = snapshot.close().await;
        let _ = encrypt_task.await;
        return Err(MysqlBackupStreamError::Database(error));
    }
    snapshot
        .close()
        .await
        .map_err(MysqlBackupStreamError::Database)?;
    if let Err(error) = writer.shutdown().await {
        drop(writer);
        let _ = encrypt_task.await;
        return Err(MysqlBackupStreamError::Database(sqlx::Error::Io(error)));
    }
    drop(writer);
    match encrypt_task.await {
        Ok(Ok(info)) => Ok(info),
        Ok(Err(error)) => Err(MysqlBackupStreamError::Vault(error)),
        Err(_) => Err(MysqlBackupStreamError::Worker),
    }
}

pub(crate) fn supports_mariadb_backup_stage(version: &str) -> bool {
    mariadb_backup_branch(version).is_some()
}

pub(crate) fn mariadb_backup_branch(version: &str) -> Option<(u64, u64)> {
    let lower = version.to_ascii_lowercase();
    let Some((numeric_version, _)) = lower.split_once("-mariadb") else {
        return None;
    };
    let components = numeric_version
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>();
    let Ok(components) = components else {
        return None;
    };
    if components.len() >= 3
        && matches!((components[0], components[1]), (10, 6) | (10, 11) | (11, 4))
    {
        Some((components[0], components[1]))
    } else {
        None
    }
}

async fn end_mariadb_backup_stage(
    stage: &mut MySqlConnection,
    stage_active: &mut bool,
) -> Result<(), sqlx::Error> {
    if *stage_active {
        sqlx::raw_sql("BACKUP STAGE END")
            .execute(&mut *stage)
            .await?;
        *stage_active = false;
    }
    Ok(())
}

async fn validate_table_recovery_snapshot(
    connection: &mut MySqlConnection,
    database_name: &str,
    inspection: &MysqlBackupInspection,
) -> Result<(), sqlx::Error> {
    if inspection
        .inventory
        .objects
        .iter()
        .any(|object| !matches!(object.kind.as_str(), "BASE TABLE" | "VIEW"))
        || !inspection.inventory.routines.is_empty()
        || !inspection.inventory.events.is_empty()
    {
        return Err(sqlx::Error::Protocol(
            "recovery snapshot contains unsupported object kinds".into(),
        ));
    }
    let definitions = inspection
        .definitions
        .iter()
        .map(|definition| (definition.name.clone(), definition.clone()))
        .collect::<HashMap<_, _>>();
    if plan_local_view_restore(database_name, &definitions).is_none() {
        return Err(sqlx::Error::Protocol(
            "recovery snapshot contains unsupported view dependencies".into(),
        ));
    }
    if inspection.definitions.iter().any(|definition| {
        definition.kind == "TRIGGER"
            && rewrite_local_view_schema(&definition.create_sql, database_name, database_name)
                .is_none()
    }) {
        return Err(sqlx::Error::Protocol(
            "recovery snapshot contains an unsupported trigger definition".into(),
        ));
    }
    let has_external_foreign_key: i64 = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM information_schema.KEY_COLUMN_USAGE
            WHERE TABLE_SCHEMA = ? AND REFERENCED_TABLE_NAME IS NOT NULL
              AND REFERENCED_TABLE_SCHEMA <> ?
        )",
    )
    .bind(database_name)
    .bind(database_name)
    .fetch_one(&mut *connection)
    .await?;
    if has_external_foreign_key != 0 {
        return Err(sqlx::Error::Protocol(
            "recovery snapshot contains a foreign key outside the database".into(),
        ));
    }
    if !inspection.inventory.visible_tables_are_transactional {
        return Err(sqlx::Error::Protocol(
            "backup contains a visible non-InnoDB table".into(),
        ));
    }
    Ok(())
}

async fn hold_mariadb_snapshot_metadata_locks(
    connection: &mut MySqlConnection,
    inventory: &MysqlBackupInventory,
) -> Result<(), sqlx::Error> {
    for object in inventory
        .objects
        .iter()
        .filter(|object| matches!(object.kind.as_str(), "BASE TABLE" | "VIEW"))
    {
        let query = format!(
            "SELECT * FROM {}.{} LIMIT 0",
            quote_identifier(&inventory.database_name),
            quote_identifier(&object.name)
        );
        sqlx::query(&query).fetch_all(&mut *connection).await?;
    }
    Ok(())
}

async fn encrypt_mysql_database_stream_inner(
    pool: &MySqlPool,
    database_name: &str,
    directory: PathBuf,
    master_key: crate::vault::MasterKey,
    recovery: crate::vault::RecoveryEnvelope,
    table_recovery_only: bool,
) -> Result<crate::vault::PublishedArtifactInfo, MysqlBackupStreamError> {
    let vault_id = recovery.vault_id();
    let mut connection = pool
        .acquire()
        .await
        .map_err(MysqlBackupStreamError::Database)?;
    let server_version: String = sqlx::query_scalar("SELECT VERSION()")
        .fetch_one(&mut *connection)
        .await
        .map_err(MysqlBackupStreamError::Database)?;
    if server_version.to_ascii_lowercase().contains("mariadb") {
        return Err(MysqlBackupStreamError::UnsupportedServer(
            "MariaDB requiere su propio proveedor de backup.",
        ));
    }
    if !supports_mysql_backup_lock(&server_version) {
        return Err(MysqlBackupStreamError::UnsupportedServer(
            "La captura protegida requiere MySQL 8.0.3 o posterior.",
        ));
    }
    let prior_lock_wait: u64 = sqlx::query_scalar("SELECT @@SESSION.lock_wait_timeout")
        .fetch_one(&mut *connection)
        .await
        .map_err(MysqlBackupStreamError::Database)?;
    sqlx::raw_sql("SET SESSION lock_wait_timeout = 10")
        .execute(&mut *connection)
        .await
        .map_err(MysqlBackupStreamError::Database)?;
    let lock_result = sqlx::raw_sql("LOCK INSTANCE FOR BACKUP")
        .execute(&mut *connection)
        .await;
    let restore_timeout = sqlx::raw_sql(&format!(
        "SET SESSION lock_wait_timeout = {prior_lock_wait}"
    ))
    .execute(&mut *connection)
    .await;
    if let Err(error) = lock_result {
        if restore_timeout.is_err() {
            let _ = connection.close().await;
        }
        return Err(MysqlBackupStreamError::Database(error));
    }
    if let Err(error) = restore_timeout {
        let _ = sqlx::raw_sql("UNLOCK INSTANCE")
            .execute(&mut *connection)
            .await;
        let _ = connection.close().await;
        return Err(MysqlBackupStreamError::Database(error));
    }
    let inspection = async {
        sqlx::raw_sql("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *connection)
            .await?;
        sqlx::raw_sql("START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY")
            .execute(&mut *connection)
            .await?;
        let inventory =
            inspect_mysql_database_on_connection(&mut connection, database_name).await?;
        if inventory
            .server_version
            .to_ascii_lowercase()
            .contains("mariadb")
        {
            return Err(sqlx::Error::Protocol(
                "MySQL backup provider cannot capture MariaDB".into(),
            ));
        }
        let definitions = read_mysql_definitions_on_connection(&mut connection, &inventory).await?;
        Ok::<_, sqlx::Error>(MysqlBackupInspection {
            inventory,
            definitions,
        })
    }
    .await;
    let inspection = match inspection {
        Ok(inspection) => inspection,
        Err(error) => {
            if rollback_and_unlock(&mut connection).await.is_err() {
                let _ = connection.close().await;
            }
            return Err(MysqlBackupStreamError::Database(error));
        }
    };

    if table_recovery_only
        && (inspection
            .inventory
            .objects
            .iter()
            .any(|object| !matches!(object.kind.as_str(), "BASE TABLE" | "VIEW"))
            || !inspection.inventory.routines.is_empty()
            || !inspection.inventory.events.is_empty())
    {
        if rollback_and_unlock(&mut connection).await.is_err() {
            let _ = connection.close().await;
        }
        return Err(MysqlBackupStreamError::UnsupportedServer(
            "La captura recuperable admite tablas, vistas y triggers simples; rutinas y eventos aún no están soportados.",
        ));
    }
    if table_recovery_only {
        let definitions = inspection
            .definitions
            .iter()
            .map(|definition| (definition.name.clone(), definition.clone()))
            .collect::<HashMap<_, _>>();
        if plan_local_view_restore(database_name, &definitions).is_none() {
            if rollback_and_unlock(&mut connection).await.is_err() {
                let _ = connection.close().await;
            }
            return Err(MysqlBackupStreamError::UnsupportedServer(
                "La captura recuperable requiere vistas locales con dependencias completas y sin ciclos.",
            ));
        }
        if inspection.definitions.iter().any(|definition| {
            definition.kind == "TRIGGER"
                && rewrite_local_view_schema(&definition.create_sql, database_name, database_name)
                    .is_none()
        }) {
            if rollback_and_unlock(&mut connection).await.is_err() {
                let _ = connection.close().await;
            }
            return Err(MysqlBackupStreamError::UnsupportedServer(
                "La captura recuperable solo admite triggers simples con referencias locales.",
            ));
        }
    }
    if table_recovery_only {
        let external_foreign_key_check = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(
                SELECT 1 FROM information_schema.KEY_COLUMN_USAGE
                WHERE TABLE_SCHEMA = ? AND REFERENCED_TABLE_NAME IS NOT NULL
                  AND REFERENCED_TABLE_SCHEMA <> ?
            )",
        )
        .bind(database_name)
        .bind(database_name)
        .fetch_one(&mut *connection)
        .await;
        let has_external_foreign_key = match external_foreign_key_check {
            Ok(result) => result,
            Err(error) => {
                if rollback_and_unlock(&mut connection).await.is_err() {
                    let _ = connection.close().await;
                }
                return Err(MysqlBackupStreamError::Database(error));
            }
        };
        if has_external_foreign_key != 0 {
            if rollback_and_unlock(&mut connection).await.is_err() {
                let _ = connection.close().await;
            }
            return Err(MysqlBackupStreamError::UnsupportedServer(
                "La base contiene claves foráneas hacia otra base, fuera de este punto de recuperación.",
            ));
        }
    }

    let (sender, receiver) = mpsc::channel(STREAM_CHANNEL_CHUNKS);
    let encrypt_task = tokio::task::spawn_blocking(move || {
        let mut reader = ChannelReader {
            receiver,
            current: Cursor::new(Vec::new()),
        };
        crate::vault::encrypt_artifact_reader_to_directory(
            Path::new(&directory),
            &mut reader,
            vault_id,
            &master_key,
            &recovery,
        )
    });

    let mut writer = ChannelWriter::new(sender);
    let stream_result = stream_mysql_database(&mut connection, &inspection, &mut writer).await;
    if let Err(error) = stream_result {
        writer.fail().await;
        drop(writer);
        if rollback_and_unlock(&mut connection).await.is_err() {
            let _ = connection.close().await;
        }
        match encrypt_task.await {
            Ok(Err(error)) => return Err(MysqlBackupStreamError::Vault(error)),
            Err(_) => return Err(MysqlBackupStreamError::Worker),
            Ok(Ok(_)) => return Err(MysqlBackupStreamError::Database(error)),
        }
    }
    if let Err(error) = sqlx::raw_sql("COMMIT").execute(&mut *connection).await {
        writer.fail().await;
        drop(writer);
        if rollback_and_unlock(&mut connection).await.is_err() {
            let _ = connection.close().await;
        }
        let _ = encrypt_task.await;
        return Err(MysqlBackupStreamError::Database(error));
    }
    if let Err(error) = sqlx::raw_sql("UNLOCK INSTANCE")
        .execute(&mut *connection)
        .await
    {
        writer.fail().await;
        drop(writer);
        let _ = connection.close().await;
        let _ = encrypt_task.await;
        return Err(MysqlBackupStreamError::Database(error));
    }
    writer
        .shutdown()
        .await
        .map_err(|error| MysqlBackupStreamError::Database(sqlx::Error::Io(error)))?;
    drop(writer);
    match encrypt_task.await {
        Ok(Ok(info)) => Ok(info),
        Ok(Err(error)) => Err(MysqlBackupStreamError::Vault(error)),
        Err(_) => Err(MysqlBackupStreamError::Worker),
    }
}

async fn rollback_and_unlock(connection: &mut MySqlConnection) -> Result<(), sqlx::Error> {
    let _ = sqlx::raw_sql("ROLLBACK").execute(&mut *connection).await?;
    sqlx::raw_sql("UNLOCK INSTANCE")
        .execute(&mut *connection)
        .await?;
    Ok(())
}

#[derive(FromRow)]
struct ObjectRow {
    name: String,
    kind: String,
    engine: Option<String>,
}

#[derive(FromRow)]
struct RoutineRow {
    name: String,
    kind: String,
}

#[derive(FromRow)]
struct TriggerRow {
    name: String,
    table_name: String,
}

pub async fn inspect_mysql_database(
    pool: &MySqlPool,
    database: &str,
) -> Result<MysqlBackupInventory, sqlx::Error> {
    let mut connection = pool.acquire().await?;
    inspect_mysql_database_on_connection(&mut connection, database).await
}

async fn inspect_mysql_database_on_connection(
    connection: &mut MySqlConnection,
    database: &str,
) -> Result<MysqlBackupInventory, sqlx::Error> {
    let server_version: String = sqlx::query_scalar("SELECT VERSION()")
        .fetch_one(&mut *connection)
        .await?;
    let (character_set, collation): (String, String) = sqlx::query_as(
        "SELECT DEFAULT_CHARACTER_SET_NAME, DEFAULT_COLLATION_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
    )
    .bind(database)
    .fetch_one(&mut *connection)
    .await?;
    let objects: Vec<BackupObject> = sqlx::query_as::<_, ObjectRow>(
        "SELECT CAST(TABLE_NAME AS CHAR) AS name, CAST(TABLE_TYPE AS CHAR) AS kind, CAST(ENGINE AS CHAR) AS engine FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME",
    )
    .bind(database)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|row| BackupObject {
        name: row.name,
        kind: row.kind,
        engine: row.engine,
    })
    .collect();
    let routines: Vec<BackupRoutine> = sqlx::query_as::<_, RoutineRow>(
        "SELECT CAST(ROUTINE_NAME AS CHAR) AS name, CAST(ROUTINE_TYPE AS CHAR) AS kind FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA = ? ORDER BY ROUTINE_NAME, ROUTINE_TYPE",
    )
    .bind(database)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|row| BackupRoutine {
        name: row.name,
        kind: row.kind,
    })
    .collect();
    let triggers: Vec<BackupTrigger> = sqlx::query_as::<_, TriggerRow>(
        "SELECT CAST(TRIGGER_NAME AS CHAR) AS name, CAST(EVENT_OBJECT_TABLE AS CHAR) AS table_name FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA = ? ORDER BY EVENT_OBJECT_TABLE, ACTION_TIMING, EVENT_MANIPULATION, ACTION_ORDER",
    )
    .bind(database)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|row| BackupTrigger {
        name: row.name,
        table_name: row.table_name,
    })
    .collect();
    let events: Vec<String> = sqlx::query_scalar(
        "SELECT CAST(EVENT_NAME AS CHAR) FROM information_schema.EVENTS WHERE EVENT_SCHEMA = ? ORDER BY EVENT_NAME",
    )
    .bind(database)
    .fetch_all(&mut *connection)
    .await?;

    let visible_tables_are_transactional = objects
        .iter()
        .filter(|object| object.kind == "BASE TABLE")
        .all(|object| {
            object
                .engine
                .as_deref()
                .is_some_and(|engine| engine.eq_ignore_ascii_case("InnoDB"))
        });
    Ok(MysqlBackupInventory {
        coverage: "visible_objects_only".into(),
        server_version,
        database_name: database.to_owned(),
        character_set,
        collation,
        objects,
        visible_tables_are_transactional,
        routines,
        triggers,
        events,
    })
}

#[cfg(test)]
mod version_tests {
    use super::{
        encrypt_mariadb_table_recovery_snapshot, plan_local_view_restore, read_bounded_line,
        restorable_definition_kind, restore_database_table_stream_with_format,
        restore_mariadb_encrypted_table_snapshot, rewrite_local_view_schema,
        split_mysql_create_table_foreign_keys, strip_local_view_schema,
        supports_mariadb_backup_stage, supports_mysql_backup_lock, trigger_name_matches,
        trigger_target_table, trigger_uses_only_local_qualifiers, valid_table_create_sql,
        valid_trigger_create_sql, view_query_dependencies, BackupDefinition, BackupObjectReference,
        CanonicalRowSetDigest, MARIADB_STREAM_FORMAT, MYSQL_STREAM_FORMAT,
    };
    use std::collections::HashMap;

    #[tokio::test]
    async fn restore_provider_rejects_the_other_engine_stream_format_before_connecting() {
        use sqlx::mysql::MySqlPoolOptions;
        use tokio::io::AsyncWriteExt;

        let pool = MySqlPoolOptions::new()
            .connect_lazy("mysql://dbsual:unused@127.0.0.1/dbsual")
            .expect("create a pool that is not connected until queried");
        for (expected, actual) in [
            (MYSQL_STREAM_FORMAT, MARIADB_STREAM_FORMAT),
            (MARIADB_STREAM_FORMAT, MYSQL_STREAM_FORMAT),
        ] {
            let (reader, mut writer) = tokio::io::duplex(512);
            writer
                .write_all(
                    serde_json::to_string(&serde_json::json!({
                        "record": "database",
                        "format": actual,
                        "coverage": "visible_objects_only",
                    }))
                    .unwrap()
                    .as_bytes(),
                )
                .await
                .unwrap();
            writer.write_all(b"\n").await.unwrap();
            drop(writer);

            let result = restore_database_table_stream_with_format(
                &pool,
                "restore_target",
                &mut tokio::io::BufReader::new(reader),
                expected,
            )
            .await;
            assert!(matches!(
                result,
                Err(super::MysqlTableRestoreError::InvalidStream)
            ));
        }
    }

    fn restore_definition(
        name: &str,
        kind: &str,
        dependencies: Vec<BackupObjectReference>,
    ) -> BackupDefinition {
        BackupDefinition {
            name: name.into(),
            kind: kind.into(),
            create_sql: String::new(),
            dependencies,
            definer: (kind == "VIEW").then(|| "dbsual@%".into()),
            sql_mode: None,
            character_set_client: None,
            collation_connection: None,
            database_collation: None,
        }
    }

    #[tokio::test]
    #[ignore = "requiere DBSUAL_MARIADB_TEST_PASSWORD y una instancia MariaDB desechable"]
    // Sonda técnica; no captura ni publica un artefacto de respaldo.
    async fn mariadb_backup_stage_can_release_commit_block_after_snapshot() {
        use std::time::Duration;

        let pool = crate::connections::connect_mariadb_test_from_env()
            .await
            .expect("connect to disposable MariaDB server through application path");
        let database = format!("dbsual_stage_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE `{database}`"))
            .execute(&pool)
            .await
            .expect("create disposable backup-stage schema");
        sqlx::query(&format!(
            "CREATE TABLE `{database}`.snapshot_rows (id INT PRIMARY KEY) ENGINE=InnoDB"
        ))
        .execute(&pool)
        .await
        .expect("create transactional snapshot fixture");
        sqlx::query(&format!(
            "INSERT INTO `{database}`.snapshot_rows VALUES (1)"
        ))
        .execute(&pool)
        .await
        .expect("insert initial snapshot row");

        let test_result: Result<(), String> = async {
            let mut stage = pool.acquire().await.map_err(|error| error.to_string())?;
            let mut snapshot = pool.acquire().await.map_err(|error| error.to_string())?;
            let mut stage_open = false;
            let snapshot_result: Result<(), String> = async {
                for statement in [
                    "BACKUP STAGE START",
                    "BACKUP STAGE FLUSH",
                    "BACKUP STAGE BLOCK_DDL",
                    "BACKUP STAGE BLOCK_COMMIT",
                ] {
                    sqlx::raw_sql(statement)
                        .execute(&mut *stage)
                        .await
                        .map_err(|error| format!("{statement}: {error}"))?;
                    if statement == "BACKUP STAGE START" {
                        stage_open = true;
                    }
                }
                for statement in [
                    "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ",
                    "START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY",
                ] {
                    sqlx::raw_sql(statement)
                        .execute(&mut *snapshot)
                        .await
                        .map_err(|error| format!("{statement}: {error}"))?;
                }
                let before: i64 = sqlx::query_scalar(&format!(
                    "SELECT COUNT(*) FROM `{database}`.snapshot_rows"
                ))
                .fetch_one(&mut *snapshot)
                .await
                .map_err(|error| error.to_string())?;
                if before != 1 {
                    return Err(format!("initial consistent snapshot returned {before} rows"));
                }

                let concurrent_pool = pool.clone();
                let writer_sql = format!("INSERT INTO `{database}`.snapshot_rows VALUES (2)");
                let writer = tokio::spawn(async move {
                    sqlx::query(&writer_sql)
                        .execute(&concurrent_pool)
                        .await
                });
                tokio::time::sleep(Duration::from_millis(250)).await;
                if writer.is_finished() {
                    let _ = sqlx::raw_sql("BACKUP STAGE END")
                        .execute(&mut *stage)
                        .await;
                    stage_open = false;
                    let _ = writer.await;
                    return Err("BLOCK_COMMIT did not hold a concurrent InnoDB commit".into());
                }

                sqlx::raw_sql("BACKUP STAGE END")
                    .execute(&mut *stage)
                    .await
                    .map_err(|error| format!("BACKUP STAGE END: {error}"))?;
                stage_open = false;
                tokio::time::timeout(Duration::from_secs(5), writer)
                    .await
                    .map_err(|_| "writer remained blocked after BACKUP STAGE END".to_owned())?
                    .map_err(|error| error.to_string())?
                    .map_err(|error| error.to_string())?;

                let after: i64 = sqlx::query_scalar(&format!(
                    "SELECT COUNT(*) FROM `{database}`.snapshot_rows"
                ))
                .fetch_one(&mut *snapshot)
                .await
                .map_err(|error| error.to_string())?;
                if after != 1 {
                    return Err(format!(
                        "the read transaction lost its original snapshot after stage end ({after} rows)"
                    ));
                }
                let ddl_pool = pool.clone();
                let ddl_sql = format!(
                    "ALTER TABLE `{database}`.snapshot_rows ADD COLUMN note VARCHAR(20) NULL"
                );
                let ddl = tokio::spawn(async move {
                    sqlx::query(&ddl_sql).execute(&ddl_pool).await
                });
                tokio::time::sleep(Duration::from_millis(250)).await;
                if ddl.is_finished() {
                    let _ = sqlx::raw_sql("COMMIT")
                        .execute(&mut *snapshot)
                        .await;
                    let _ = ddl.await;
                    return Err("a concurrent ALTER passed an active table snapshot".into());
                }
                sqlx::raw_sql("COMMIT")
                    .execute(&mut *snapshot)
                    .await
                    .map_err(|error| error.to_string())?;
                tokio::time::timeout(Duration::from_secs(5), ddl)
                    .await
                    .map_err(|_| "DDL remained blocked after the snapshot transaction ended".to_owned())?
                    .map_err(|error| error.to_string())?
                    .map_err(|error| error.to_string())?;
                let committed: i64 = sqlx::query_scalar(&format!(
                    "SELECT COUNT(*) FROM `{database}`.snapshot_rows"
                ))
                .fetch_one(&pool)
                .await
                .map_err(|error| error.to_string())?;
                if committed != 2 {
                    return Err(format!("expected the concurrent row after COMMIT, got {committed}"));
                }
                Ok(())
            }
            .await;
            if stage_open {
                let _ = sqlx::raw_sql("BACKUP STAGE END")
                    .execute(&mut *stage)
                    .await;
                let _ = sqlx::raw_sql("ROLLBACK")
                    .execute(&mut *snapshot)
                    .await;
                let _ = stage.close().await;
                let _ = snapshot.close().await;
            }
            snapshot_result
        }
        .await;

        let cleanup = sqlx::query(&format!("DROP DATABASE `{database}`"))
            .execute(&pool)
            .await;
        if let Err(error) = cleanup {
            panic!("failed to remove disposable backup-stage schema: {error}");
        }
        test_result.expect("MariaDB snapshot/lock behavior must match the tested assumptions");
    }

    #[test]
    fn mariadb_backup_provider_is_limited_to_the_supported_server_families() {
        for version in ["10.6.28-MariaDB", "10.11.19-MariaDB", "11.4.13-MariaDB"] {
            assert!(supports_mariadb_backup_stage(version), "{version}");
        }
        for version in [
            "10.5.27-MariaDB",
            "11.5.2-MariaDB",
            "8.4.11 MySQL Community Server",
            "10.6.28-compatible-mariadb-proxy",
        ] {
            assert!(!supports_mariadb_backup_stage(version), "{version}");
        }
    }

    #[test]
    fn mariadb_view_dependencies_and_schema_qualified_trigger_names_are_parsed() {
        let dependencies = view_query_dependencies(
            "select `d`.`t`.`id` from `d`.`t` where `d`.`t`.`label` <> 'FROM fake'",
            "d",
        )
        .unwrap();
        assert_eq!(
            dependencies,
            [BackupObjectReference {
                schema: "d".into(),
                name: "t".into(),
            }]
        );
        assert!(view_query_dependencies("SELECT * FROM (SELECT * FROM t) q", "d").is_none());
        assert!(view_query_dependencies("SELECT * FROM t, u", "d").is_none());

        let trigger = "CREATE DEFINER=`root`@`%` TRIGGER `d`.child_audit\n AFTER INSERT ON `d`.child_rows FOR EACH ROW\n INSERT INTO `d`.audit_rows (id) VALUES (NEW.id)";
        assert!(trigger_name_matches(trigger, "child_audit"));
        assert!(trigger_uses_only_local_qualifiers(trigger, "d"));
        assert_eq!(
            trigger_target_table(trigger),
            Some((Some("d".into()), "child_rows".into()))
        );
        assert!(valid_trigger_create_sql(trigger, "child_audit", "d"));
    }

    #[tokio::test]
    #[ignore = "requiere DBSUAL_MARIADB_TEST_PASSWORD y MariaDB 10.6, 10.11 o 11.4 desechable"]
    async fn mariadb_recovery_snapshot_round_trips_to_a_new_database() {
        let pool = crate::connections::connect_mariadb_test_from_env()
            .await
            .expect("connect to disposable MariaDB server through application path");
        let database = format!("dbsual_recovery_{}", uuid::Uuid::new_v4().simple());
        let restored = format!("{database}_restore");
        let artifact_dir = std::env::temp_dir().join(format!(
            "dbsual-mariadb-recovery-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let vault = crate::vault::create_vault().expect("create ephemeral test vault");
        let vault_id = vault.vault_id;
        crate::vault::store_master_key(vault_id, &vault.master_key)
            .expect("store ephemeral test key in Windows credential store");

        let outcome: Result<(), String> = async {
            sqlx::query(&format!("CREATE DATABASE `{database}`"))
                .execute(&pool)
                .await
                .map_err(|error| format!("create source database: {error}"))?;
            sqlx::query(&format!(
                "CREATE TABLE `{database}`.parent_rows (
                    id INT PRIMARY KEY,
                    label VARCHAR(80) NOT NULL
                ) ENGINE=InnoDB"
            ))
            .execute(&pool)
            .await
            .map_err(|error| format!("create parent table: {error}"))?;
            sqlx::query(&format!(
                "CREATE TABLE `{database}`.child_rows (
                    id INT PRIMARY KEY,
                    parent_id INT NOT NULL,
                    description VARCHAR(80) NOT NULL,
                    payload VARBINARY(16) NOT NULL,
                    optional_text VARCHAR(32) NULL,
                    CONSTRAINT child_parent_fk FOREIGN KEY (parent_id)
                        REFERENCES parent_rows (id)
                ) ENGINE=InnoDB"
            ))
            .execute(&pool)
            .await
            .map_err(|error| format!("create child table: {error}"))?;
            sqlx::query(&format!(
                "CREATE TABLE `{database}`.audit_rows (id INT NOT NULL) ENGINE=InnoDB"
            ))
            .execute(&pool)
            .await
            .map_err(|error| format!("create audit table: {error}"))?;
            sqlx::query(&format!(
                "INSERT INTO `{database}`.parent_rows VALUES (1, 'área')"
            ))
            .execute(&pool)
            .await
            .map_err(|error| format!("insert parent row: {error}"))?;
            sqlx::query(&format!(
                "INSERT INTO `{database}`.child_rows VALUES (?, ?, ?, ?, ?)"
            ))
            .bind(1_i32)
            .bind(1_i32)
            .bind("niño 🧪")
            .bind(vec![0_u8, 255, 42])
            .bind(Option::<String>::None)
            .execute(&pool)
            .await
            .map_err(|error| format!("insert binary/Unicode fixture: {error}"))?;
            sqlx::query(&format!(
                "CREATE VIEW `{database}`.child_view AS
                 SELECT id, description FROM `{database}`.child_rows"
            ))
            .execute(&pool)
            .await
            .map_err(|error| format!("create view: {error}"))?;
            sqlx::query(&format!(
                "CREATE TRIGGER `{database}`.child_audit
                 AFTER INSERT ON `{database}`.child_rows FOR EACH ROW
                 INSERT INTO `{database}`.audit_rows (id) VALUES (NEW.id)"
            ))
            .execute(&pool)
            .await
            .map_err(|error| format!("create trigger: {error}"))?;

            let (master_key, recovery) = vault.artifact_material();
            let artifact = encrypt_mariadb_table_recovery_snapshot(
                &pool,
                &database,
                artifact_dir.clone(),
                master_key,
                recovery,
            )
            .await
            .map_err(|error| format!("capture recovery point: {error:?}"))?;
            let restored_counts = restore_mariadb_encrypted_table_snapshot(
                &pool,
                &restored,
                artifact_dir.join(format!("{}.dbsual-artifact", artifact.artifact.artifact_id)),
                vault_id,
                artifact.artifact.artifact_id,
                &artifact.ciphertext_sha256,
                artifact.encrypted_bytes,
            )
            .await
            .map_err(|error| format!("restore recovery point: {error:?}"))?;
            if restored_counts != (3, 2) {
                return Err(format!(
                    "expected 3 tables and 2 rows, got {restored_counts:?}"
                ));
            }
            let restored_row: (String, Vec<u8>, Option<String>) = sqlx::query_as(&format!(
                "SELECT description, payload, optional_text
                 FROM `{restored}`.child_rows WHERE id = 1"
            ))
            .fetch_one(&pool)
            .await
            .map_err(|error| format!("read restored row: {error}"))?;
            if restored_row != ("niño 🧪".into(), vec![0, 255, 42], None) {
                return Err(format!("restored row differs: {restored_row:?}"));
            }
            let view_value: String = sqlx::query_scalar(&format!(
                "SELECT description FROM `{restored}`.child_view WHERE id = 1"
            ))
            .fetch_one(&pool)
            .await
            .map_err(|error| format!("read restored view: {error}"))?;
            if view_value != "niño 🧪" {
                return Err("restored view returned different data".into());
            }
            let audit_count: i64 =
                sqlx::query_scalar(&format!("SELECT COUNT(*) FROM `{restored}`.audit_rows"))
                    .fetch_one(&pool)
                    .await
                    .map_err(|error| format!("check trigger effects during restore: {error}"))?;
            if audit_count != 0 {
                return Err("the trigger ran while restoring table rows".into());
            }
            sqlx::query(&format!(
                "INSERT INTO `{restored}`.child_rows
                 (id, parent_id, description, payload, optional_text)
                 VALUES (2, 1, 'after restore', X'01', '')"
            ))
            .execute(&pool)
            .await
            .map_err(|error| format!("exercise restored trigger: {error}"))?;
            let audit_count: i64 = sqlx::query_scalar(&format!(
                "SELECT COUNT(*) FROM `{restored}`.audit_rows WHERE id = 2"
            ))
            .fetch_one(&pool)
            .await
            .map_err(|error| format!("verify restored trigger: {error}"))?;
            if audit_count != 1 {
                return Err("the restored trigger did not run after restoration".into());
            }
            Ok(())
        }
        .await;

        let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS `{restored}`"))
            .execute(&pool)
            .await;
        let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS `{database}`"))
            .execute(&pool)
            .await;
        let artifact_cleanup = std::fs::remove_dir_all(&artifact_dir);
        if artifact_cleanup
            .as_ref()
            .is_err_and(|error| error.kind() != std::io::ErrorKind::NotFound)
        {
            panic!("failed to remove temporary artifact directory: {artifact_cleanup:?}");
        }
        crate::vault::remove_master_key(vault_id)
            .expect("remove ephemeral test key from Windows credential store");
        outcome.expect("MariaDB recovery snapshot must round-trip");
    }

    #[test]
    fn view_restore_plan_is_topological_and_rejects_unsupported_dependencies() {
        let definitions = HashMap::from([
            (
                "base".into(),
                restore_definition("base", "BASE TABLE", vec![]),
            ),
            (
                "dependent".into(),
                restore_definition(
                    "dependent",
                    "VIEW",
                    vec![BackupObjectReference {
                        schema: "source_db".into(),
                        name: "first".into(),
                    }],
                ),
            ),
            (
                "first".into(),
                restore_definition(
                    "first",
                    "VIEW",
                    vec![BackupObjectReference {
                        schema: "source_db".into(),
                        name: "base".into(),
                    }],
                ),
            ),
        ]);
        let order = plan_local_view_restore("source_db", &definitions).unwrap();
        assert_eq!(order, ["first", "dependent"]);

        let mut cross_schema = definitions.clone();
        cross_schema.get_mut("first").unwrap().dependencies[0].schema = "other_db".into();
        assert!(plan_local_view_restore("source_db", &cross_schema).is_none());

        let mut missing = definitions.clone();
        missing.get_mut("first").unwrap().dependencies[0].name = "missing".into();
        assert!(plan_local_view_restore("source_db", &missing).is_none());

        let mut cycle = definitions.clone();
        cycle.get_mut("first").unwrap().dependencies[0].name = "dependent".into();
        assert!(plan_local_view_restore("source_db", &cycle).is_none());
    }

    #[test]
    fn view_schema_rewrite_changes_only_qualified_identifiers() {
        let original = "CREATE DEFINER=`dbsual`@`%` SQL SECURITY DEFINER VIEW `source_db`.`v` AS select 'source_db.t' as label from `source_db`.`t`";
        let restored = rewrite_local_view_schema(original, "source_db", "restore_db").unwrap();
        assert!(restored.contains("VIEW `restore_db`.`v`"));
        assert!(restored.contains("from `restore_db`.`t`"));
        assert!(restored.contains("'source_db.t'"));
        assert_eq!(
            strip_local_view_schema(&restored, "restore_db").unwrap(),
            "CREATE DEFINER=`dbsual`@`%` SQL SECURITY DEFINER VIEW `v` AS select 'source_db.t' as label from `t`"
        );
        assert!(rewrite_local_view_schema(
            "CREATE VIEW source_db.v AS SELECT * FROM source_db.t",
            "source_db",
            "restore_db"
        )
        .is_none());
        assert!(rewrite_local_view_schema(
            "CREATE VIEW `source_db`.`v` AS SELECT * FROM \"source_db\".\"t\"",
            "source_db",
            "restore_db"
        )
        .is_none());
        assert!(rewrite_local_view_schema(
            "CREATE VIEW `source_db`.`v` AS SELECT 1; DROP DATABASE `source_db`",
            "source_db",
            "restore_db"
        )
        .is_none());
    }

    #[test]
    fn backup_definition_serialization_preserves_execution_context() {
        let definition = BackupDefinition {
            name: "worker".into(),
            kind: "PROCEDURE".into(),
            create_sql: "CREATE PROCEDURE `db`.`worker`() SELECT 1".into(),
            dependencies: Vec::new(),
            definer: Some("dbsual@%".into()),
            sql_mode: Some("STRICT_TRANS_TABLES".into()),
            character_set_client: Some("utf8mb4".into()),
            collation_connection: Some("utf8mb4_0900_ai_ci".into()),
            database_collation: Some("utf8mb4_0900_ai_ci".into()),
        };
        let encoded = serde_json::to_vec(&definition).unwrap();
        let decoded: BackupDefinition = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, definition);
    }

    #[test]
    fn restore_preflight_rejects_unsupported_programmable_object_kinds() {
        assert!(restorable_definition_kind("BASE TABLE"));
        assert!(restorable_definition_kind("VIEW"));
        assert!(restorable_definition_kind("TRIGGER"));
        // A procedure whose body appears to be one SELECT is not enough to
        // establish read-only behavior: SELECT can call side-effecting
        // functions, write files or variables, lock rows, or read other schemas.
        for (kind, ddl) in [
            ("PROCEDURE", "CREATE PROCEDURE `worker`() SELECT 1"),
            ("PROCEDURE", "CREATE PROCEDURE `worker`() SELECT SLEEP(30)"),
            (
                "PROCEDURE",
                "CREATE PROCEDURE `worker`() SELECT @saved := 1",
            ),
            (
                "PROCEDURE",
                "CREATE PROCEDURE `worker`() SELECT `id` FROM `items` FOR UPDATE",
            ),
            (
                "PROCEDURE",
                "CREATE PROCEDURE `worker`() SELECT `id` FROM `other_db`.`items`",
            ),
            (
                "PROCEDURE",
                "CREATE PROCEDURE `worker`() SELECT secret INTO OUTFILE '/tmp/out'",
            ),
            (
                "FUNCTION",
                "CREATE FUNCTION `read_value`() RETURNS INT RETURN 1",
            ),
            ("EVENT", "CREATE EVENT `scheduled` DO SELECT 1"),
        ] {
            if kind == "PROCEDURE" {
                assert!(ddl.contains("SELECT"));
            }
            assert!(
                !restorable_definition_kind(kind),
                "{kind} must fail before target creation"
            );
        }
    }

    #[test]
    fn trigger_restore_preflight_accepts_only_simple_local_trigger_ddl() {
        let simple = "CREATE DEFINER=`dbsual`@`%` TRIGGER `audit_insert` AFTER INSERT ON `source_db`.`items` FOR EACH ROW INSERT INTO `source_db`.`audit` (`item_id`) VALUES (NEW.`id`)";
        assert!(valid_trigger_create_sql(
            simple,
            "audit_insert",
            "source_db"
        ));
        assert_eq!(
            trigger_target_table(simple),
            Some((Some("source_db".into()), "items".into()))
        );
        assert!(!valid_trigger_create_sql(
            "CREATE TRIGGER `audit_insert` AFTER INSERT ON `items` FOR EACH ROW BEGIN INSERT INTO `audit` VALUES (NEW.`id`); END",
            "audit_insert",
            "source_db"
        ));
        assert!(!valid_trigger_create_sql(
            "CREATE TRIGGER `different` AFTER INSERT ON `items` FOR EACH ROW SET @x = NEW.`id`",
            "audit_insert",
            "source_db"
        ));
        assert!(!valid_trigger_create_sql(
            "CREATE TRIGGER `audit_insert` AFTER INSERT ON `items` FOR EACH ROW INSERT INTO `external_db`.`audit` VALUES (NEW.id)",
            "audit_insert",
            "source_db"
        ));
        let server_ddl = "CREATE DEFINER=`root`@`%` TRIGGER `parent_audit` AFTER INSERT ON `a_parent` FOR EACH ROW INSERT INTO `source_db`.audit_log (parent_id) VALUES (NEW.id)";
        assert!(valid_trigger_create_sql(
            server_ddl,
            "parent_audit",
            "source_db"
        ));
        let rewritten = rewrite_local_view_schema(server_ddl, "source_db", "restore_db").unwrap();
        assert!(rewritten.contains("`restore_db`.audit_log"));
        assert!(strip_local_view_schema(&rewritten, "restore_db").is_some());
    }

    #[test]
    fn canonical_row_set_digest_ignores_read_order_and_preserves_duplicates() {
        let mut first = CanonicalRowSetDigest::default();
        first.update(br#"{"id":1}"#).unwrap();
        first.update(br#"{"id":2}"#).unwrap();

        let mut reordered = CanonicalRowSetDigest::default();
        reordered.update(br#"{"id":2}"#).unwrap();
        reordered.update(br#"{"id":1}"#).unwrap();
        assert_eq!(first.finish_hex(), reordered.finish_hex());

        let mut duplicated = CanonicalRowSetDigest::default();
        duplicated.update(br#"{"id":1}"#).unwrap();
        duplicated.update(br#"{"id":2}"#).unwrap();
        duplicated.update(br#"{"id":2}"#).unwrap();
        assert_ne!(first.finish_hex(), duplicated.finish_hex());
    }

    #[test]
    fn backup_lock_version_gate_matches_mysql_support_floor() {
        assert!(!supports_mysql_backup_lock("8.0.2"));
        assert!(supports_mysql_backup_lock("8.0.3"));
        assert!(supports_mysql_backup_lock("8.0.36-commercial"));
        assert!(supports_mysql_backup_lock("8.4.11"));
        assert!(supports_mysql_backup_lock("9.0.0"));
        assert!(!supports_mysql_backup_lock("not-a-version"));
        assert!(!supports_mysql_backup_lock("8.0"));
    }

    #[test]
    fn restore_ddl_must_match_a_single_conservative_table_definition() {
        assert!(valid_table_create_sql(
            "CREATE TABLE `sample` (`id` int NOT NULL) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
            "sample"
        ));
        assert!(!valid_table_create_sql(
            "CREATE TABLE `other` (`id` int) ENGINE=InnoDB",
            "sample"
        ));
        assert!(!valid_table_create_sql(
            "CREATE TABLE `sample` (`id` int); DROP DATABASE `other`",
            "sample"
        ));
        assert!(!valid_table_create_sql(
            "CREATE TABLE `sample` (`id` int REFERENCES `otherdb`.`t` (`id`)) ENGINE=InnoDB",
            "sample"
        ));
        assert!(!valid_table_create_sql(
            "CREATE TABLE `sample` (`id` int) ENGINE=InnoDB DATA DIRECTORY='/tmp/x'",
            "sample"
        ));
        assert!(!valid_table_create_sql(
            "CREATE TABLE `sample` (`id` int DEFAULT 'ENGINE=InnoDB') ENGINE=MEMORY",
            "sample"
        ));
        assert!(valid_table_create_sql(
            "CREATE TABLE `sample` (`id` int, `hidden` varchar(24) /*!80023 INVISIBLE */) ENGINE=InnoDB",
            "sample"
        ));
        assert!(!valid_table_create_sql(
            "CREATE TABLE `sample` (`id` int /*!80023 DROP DATABASE other */) ENGINE=InnoDB",
            "sample"
        ));
    }

    #[test]
    fn restore_defers_foreign_keys_until_all_tables_exist() {
        let (sql, foreign_keys) = split_mysql_create_table_foreign_keys(
            "CREATE TABLE `a` (`id` int NOT NULL, `label` varchar(20) DEFAULT 'a,b', CONSTRAINT `fk` FOREIGN KEY (`other_id`) REFERENCES `b` (`id`)) ENGINE=InnoDB",
        ).unwrap();
        assert!(sql.contains("`label` varchar(20) DEFAULT 'a,b'"));
        assert!(!sql.contains("FOREIGN KEY"));
        assert_eq!(foreign_keys.len(), 1);
        assert!(foreign_keys[0].starts_with("CONSTRAINT `fk` FOREIGN KEY"));

        let (sql, foreign_keys) = split_mysql_create_table_foreign_keys(
            "CREATE TABLE `simple` (`id` int NOT NULL, KEY `idx` (`id`)) ENGINE=InnoDB",
        )
        .unwrap();
        assert!(sql.contains("KEY `idx`"));
        assert!(foreign_keys.is_empty());
    }

    #[tokio::test]
    async fn bounded_line_reader_rejects_oversized_input_before_accumulating_it() {
        let mut reader = tokio::io::BufReader::new(std::io::Cursor::new(b"12345\n"));
        assert!(read_bounded_line(&mut reader, 6).await.unwrap().is_some());
        let mut reader = tokio::io::BufReader::new(std::io::Cursor::new(b"123456\n"));
        assert!(read_bounded_line(&mut reader, 5).await.is_err());
    }
}
