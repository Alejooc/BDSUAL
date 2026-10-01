import { invoke, isTauri } from '@tauri-apps/api/core'

export type Section = 'explorer' | 'changes' | 'history'
export type ThemeName = 'dark' | 'light' | 'contrast' | 'midnight' | 'nord' | 'forest' | 'custom'
export type ThemeColors = { background: string; sidebar: string; surface: string; elevated: string; border: string; text: string; muted: string; accent: string }
export type UiPreferences = { version: 1; section: Section; sidebarWidth: number; sidebarCollapsed: boolean; bottomHeight: number; bottomCollapsed: boolean; theme: ThemeName; customColors: ThemeColors; fontScale: 'compact' | 'default' | 'large'; sqlFontSize: number }
export type SessionTab = { id: 'welcome'; kind: 'welcome' } | { id: 'query'; kind: 'query' }
export type Session = { version: 1; tabs: SessionTab[]; activeTab: SessionTab['id'] | 'none' }
export type Bootstrap = { contractVersion: 1; storageStatus: 'ready'; preferences: UiPreferences; session: Session }
export type IpcError = { code: string; message: string; fingerprint?: string; publicKey?: string }
export type TlsMode = 'disabled' | 'verifyIdentity'
export type DatabaseEngine = 'mysql' | 'mariadb' | 'postgresql' | 'sqlite'
export type ConnectionInput = {
  id?: string; name: string; engine: DatabaseEngine; host: string; port: number; user: string; password?: string;
  tlsMode: TlsMode; tlsCaPath?: string; tlsClientCertPath?: string; tlsClientKeyPath?: string;
  sshEnabled: boolean; sshHost?: string; sshPort?: number; sshUser?: string; sshAuthMethod?: 'password' | 'privateKey' | 'agent';
  sshKeyPath?: string; sshAgentPipe?: string; sshPassword?: string; sshKeyPassphrase?: string
}
export type SavedConnection = { id: string; name: string; engine: DatabaseEngine; host: string; port: number; user: string; tlsMode: TlsMode; tlsCaPath?: string | null; tlsClientCertPath?: string | null; tlsClientKeyPath?: string | null; sshEnabled: boolean; sshHost?: string | null; sshPort?: number | null; sshUser?: string | null; sshAuthMethod?: 'password' | 'privateKey' | 'agent' | null; sshKeyPath?: string | null; sshAgentPipe?: string | null }
export type ActiveConnection = { id: string; state: 'connected'; serverVersion: string; tlsActive: boolean }
export type ConnectionTest = { serverVersion: string; tlsActive: boolean }
export type DatabaseObject = { name: string; kind: 'table' | 'view'; schema?: string | null }
export type ServerProcess = { id: number; user: string | null; host: string | null; database: string | null; command: string | null; state: string | null; durationSeconds: number }
  export type ServerUser = { name: string; host: string; authenticationPlugin: string | null; locked: boolean | null }
export type ServerVariable = { name: string; value: string; scope: 'global' }
export type ColumnMetadata = { name: string; dataType: string; isNullable: boolean; isPrimaryKey: boolean; primaryKeyAvailable: boolean; defaultValue: string | null; defaultAvailable: boolean }
export type TableStructure = { indexes: Array<{ name: string; unique: boolean; indexType: string; columns: string[] }>; constraints: Array<{ name: string; kind: string; columns: string[]; referencedDatabase: string | null; referencedTable: string | null; referencedColumns: string[] }> }
export type SqlReadResult = { columns: string[]; rows: Array<Array<string | null>>; returnedRows: number; totalRows?: number | null; hasMore: boolean; nextOffset: number | null; elapsedMs: number }
export type TablePageOptions = { sortColumn: string | null; sortDirection: 'asc' | 'desc' | null; filterColumn: string | null; filterMode: 'equals' | 'contains' | null; filterValue: string | null }
export type CsvExportResult = { rowsWritten: number }
export type VaultStatus = { configured: boolean }
export type VaultSetup = { recoveryPhrase: string }
export type HistoryProject = { id: string; connectionId: string; databaseName: string; engine: string; serverVersion: string | null; createdAtMs: number }
export type RecoveryPoint = { id: string; projectId: string; engine: string; serverVersion: string | null; artifactId: string; ciphertextSha256: string; encryptedBytes: number; plaintextBytes: number; coverage: 'visible_tables_only' | 'visible_tables_and_views' | 'visible_tables_views_triggers' | 'complete'; verificationState: string; protectedRevisionId: string | null; createdAtMs: number }
export type RecoveryRestoreSummary = { databaseName: string; tablesRestored: number; rowsRestored: number }
export type RevisionSummary = { id: string; projectId: string; revisionNumber: number; message: string; status: string; recoveryState: string; planSha256: string; artifactId: string; operationCount: number; createdAtMs: number; confirmedAtMs: number | null }
export type MysqlRowUpdateValue = { column: string; value: string | null }
export type MysqlRowUpdatePreview = { revision: RevisionSummary; primaryKey: MysqlRowUpdateValue[]; columnName: string; oldValue: string | null; newValue: string | null; operation?: 'update' | 'insert' | 'delete'; values?: MysqlRowUpdateValue[] }
export type MysqlCsvImportPreview = { revision: RevisionSummary; headers: string[]; rowCount: number; sampleRows: Array<Array<string | null>> }
export type MysqlBackupInventory = { coverage: 'visible_objects_only'; serverVersion: string; databaseName: string; characterSet: string; collation: string; visibleTablesAreTransactional: boolean; objects: Array<{ name: string; kind: string; engine: string | null }>; routines: Array<{ name: string; kind: string }>; triggers: Array<{ name: string; tableName: string }>; events: string[] }
export type MysqlBackupInspection = { inventory: MysqlBackupInventory; definitions: Array<{ name: string; kind: string; createSql: string }> }

const defaults: Bootstrap = {
  contractVersion: 1, storageStatus: 'ready',
  preferences: { version: 1, section: 'explorer', sidebarWidth: 24, sidebarCollapsed: false, bottomHeight: 28, bottomCollapsed: false, theme: 'dark', customColors: { background: '#111316', sidebar: '#17191d', surface: '#15171b', elevated: '#1d2025', border: '#292c32', text: '#e4e6e9', muted: '#858992', accent: '#7778ec' }, fontScale: 'default', sqlFontSize: 14 },
  session: { version: 1, tabs: [{ id: 'welcome', kind: 'welcome' }], activeTab: 'welcome' },
}
let browserState = structuredClone(defaults)
function normalizeError(error: unknown): IpcError {
  if (typeof error === 'object' && error !== null && 'code' in error && 'message' in error) return error as IpcError
  return { code: 'IPC_ERROR', message: error instanceof Error ? error.message : 'No se pudo completar la operación.' }
}
async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    if (isTauri()) return await invoke<T>(command, args)
    if (command === 'bootstrap') return structuredClone(browserState) as T
    if (command === 'save_ui_preferences') browserState.preferences = structuredClone(args?.preferences as UiPreferences)
    if (command === 'save_session') browserState.session = structuredClone(args?.session as Session)
    if (command === 'reset_ui_preferences') browserState.preferences = structuredClone(defaults.preferences)
    return structuredClone(command === 'reset_ui_preferences' ? browserState.preferences : undefined) as T
  } catch (error) { throw normalizeError(error) }
}
async function backend<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    if (!isTauri()) throw { code: 'CORE_UNAVAILABLE', message: 'El núcleo de DBSUAL solo está disponible en la aplicación de escritorio.' }
    return await invoke<T>(command, args)
  } catch (error) { throw normalizeError(error) }
}
export const ipc = {
  bootstrap: () => call<Bootstrap>('bootstrap'),
  saveUiPreferences: (preferences: UiPreferences) => call<void>('save_ui_preferences', { preferences }),
  saveSession: (session: Session) => call<void>('save_session', { session }),
  resetUiPreferences: () => call<UiPreferences>('reset_ui_preferences'),
  listConnections: () => isTauri() ? backend<SavedConnection[]>('list_connections') : Promise.resolve([]),
  saveConnection: (input: ConnectionInput) => backend<SavedConnection>('save_connection', { input }),
  testConnection: (input: ConnectionInput) => backend<ConnectionTest>('test_connection', { input }),
  openConnection: (id: string) => backend<ActiveConnection>('open_connection', { id }),
  approveSshHostKey: (connectionId: string, fingerprint: string, publicKey: string) => backend<void>('approve_ssh_host_key', { connectionId, fingerprint, publicKey }),
  disconnectConnection: (id: string) => backend<{ id: string; state: 'disconnected' }>('disconnect_connection', { id }),
  removeConnection: (id: string) => backend<void>('remove_connection', { id }),
  listDatabases: (connectionId: string) => backend<string[]>('list_databases', { connectionId }),
  listServerProcesses: (connectionId: string) => backend<ServerProcess[]>('list_server_processes', { connectionId }),
  listServerUsers: (connectionId: string) => backend<ServerUser[]>('list_server_users', { connectionId }),
  listServerVariables: (connectionId: string) => backend<ServerVariable[]>('list_server_variables', { connectionId }),
  listDatabaseObjects: (connectionId: string, database: string) => backend<DatabaseObject[]>('list_database_objects', { connectionId, database }),
  listColumns: (connectionId: string, database: string, objectName: string, objectType: DatabaseObject['kind'], objectSchema?: string | null) => backend<ColumnMetadata[]>('list_columns', { connectionId, database, objectName, objectType, objectSchema: objectSchema ?? null }),
  getTableStructure: (connectionId: string, database: string, table: string) => backend<TableStructure>('get_table_structure', { connectionId, database, table }),
  executeReadQuery: (connectionId: string, database: string, sql: string, offset = 0, queryId: string = crypto.randomUUID()) => backend<SqlReadResult>('execute_read_query', { connectionId, database, sql, offset, queryId }),
  executePostgresReadQuery: (connectionId: string, database: string, sql: string, offset = 0, queryId: string = crypto.randomUUID()) => backend<SqlReadResult>('execute_postgres_read_query', { connectionId, database, sql, offset, queryId }),
  executeSqliteReadQuery: (connectionId: string, database: string, sql: string, offset = 0) => backend<SqlReadResult>('execute_sqlite_read_query', { connectionId, database, sql, offset }),
  readTablePage: (connectionId: string, database: string, table: string, options: TablePageOptions, offset = 0, queryId = crypto.randomUUID()) => backend<SqlReadResult>('read_table_page', { connectionId, database, table, options, offset, queryId }),
  readPostgresTablePage: (connectionId: string, database: string, schema: string, table: string, options: TablePageOptions, offset = 0) => backend<SqlReadResult>('read_postgres_table_page', { connectionId, database, schema, table, options, offset }),
  readSqliteTablePage: (connectionId: string, database: string, table: string, options: TablePageOptions, offset = 0) => backend<SqlReadResult>('read_sqlite_table_page', { connectionId, database, table, options, offset }),
  cancelReadQuery: (queryId: string) => backend<void>('cancel_read_query', { queryId }),
  exportTableCsv: (connectionId: string, database: string, table: string, path: string, delimiter: 'comma' | 'semicolon' | 'tab', nullMarker: string) => backend<CsvExportResult>('export_table_csv', { connectionId, database, table, path, delimiter, nullMarker }),
  getVaultStatus: () => backend<VaultStatus>('get_vault_status'),
  beginVaultSetup: () => backend<VaultSetup>('begin_vault_setup'),
  confirmVaultSetup: (recoveryPhrase: string) => backend<VaultStatus>('confirm_vault_setup', { recoveryPhrase }),
  cancelVaultSetup: () => backend<void>('cancel_vault_setup'),
  exportRecoveryKeyFile: (path: string, password: string) => backend<void>('export_recovery_key_file', { path, password }),
  importRecoveryKeyFile: (path: string, password: string) => backend<VaultSetup>('import_recovery_key_file', { path, password }),
  listHistoryProjects: () => backend<HistoryProject[]>('list_history_projects'),
  listHistoryRecoveryPoints: (projectId: string) => backend<RecoveryPoint[]>('list_history_recovery_points', { projectId }),
  captureRecoveryPoint: (connectionId: string, databaseName: string) => backend<RecoveryPoint>('capture_recovery_point', { connectionId, databaseName }),
  prepareRecoveryRestore: (recoveryPointId: string, targetDatabase: string) => backend<RevisionSummary>('prepare_recovery_restore', { recoveryPointId, targetDatabase }),
  restoreRecoveryPoint: (revisionId: string) => backend<RecoveryRestoreSummary>('restore_recovery_point', { revisionId }),
  listHistoryRevisions: (projectId: string) => backend<RevisionSummary[]>('list_history_revisions', { projectId }),
  confirmHistoryRevision: (revisionId: string) => backend<void>('confirm_history_revision', { revisionId }),
  readHistoryPlan: (revisionId: string) => backend<string>('read_history_plan', { revisionId }),
  createHistoryProject: (connectionId: string, databaseName: string) => backend<HistoryProject>('create_history_project', { connectionId, databaseName }),
  prepareSqlDraft: (connectionId: string, databaseName: string, sql: string) => backend<RevisionSummary>('prepare_sql_draft', { connectionId, databaseName, sql }),
  prepareMysqlRowUpdate: (connectionId: string, databaseName: string, tableName: string, primaryKey: MysqlRowUpdateValue[], columnName: string, newValue: string | null) => backend<MysqlRowUpdatePreview>('prepare_mysql_row_update', { connectionId, databaseName, tableName, primaryKey, columnName, newValue }),
  prepareMysqlRowInsert: (connectionId: string, databaseName: string, tableName: string, values: MysqlRowUpdateValue[]) => backend<MysqlRowUpdatePreview>('prepare_mysql_row_insert', { connectionId, databaseName, tableName, values }),
  prepareMysqlRowRevert: (revisionId: string) => backend<MysqlRowUpdatePreview>('prepare_mysql_row_revert', { revisionId }),
  prepareMysqlCsvImport: (connectionId: string, databaseName: string, tableName: string, csvText: string, delimiter: 'comma' | 'semicolon' | 'tab', nullMarker: string) => backend<MysqlCsvImportPreview>('prepare_mysql_csv_import', { connectionId, databaseName, tableName, csvText, delimiter, nullMarker }),
  prepareMysqlCsvImportRevert: (revisionId: string) => backend<MysqlCsvImportPreview>('prepare_mysql_csv_import_revert', { revisionId }),
  applyMysqlRowUpdate: (revisionId: string) => backend<void>('apply_mysql_row_update', { revisionId }),
  prepareCreateDatabase: (connectionId: string, databaseName: string) => backend<RevisionSummary>('prepare_create_database', { connectionId, databaseName }),
  applyCreateDatabase: (revisionId: string) => backend<void>('apply_create_database', { revisionId }),
  inspectMysqlBackupCatalog: (connectionId: string, databaseName: string) => backend<MysqlBackupInspection>('inspect_mysql_backup_catalog', { connectionId, databaseName }),
}
