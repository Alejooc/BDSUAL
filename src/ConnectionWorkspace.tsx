import { useEffect, useRef, useState, type ReactNode } from 'react'
import { isTauri } from '@tauri-apps/api/core'
import { open, save } from '@tauri-apps/plugin-dialog'
import { Activity, AlertCircle, Check, ChevronDown, ChevronRight, Clock3, Database, Download, Eye, FileKey, GitBranch, LoaderCircle, LockKeyhole, MoreHorizontal, Plus, Plug, PlugZap, RefreshCw, ShieldCheck, SlidersHorizontal, Table2, Trash2, Unplug, Users, X } from 'lucide-react'
import { ipc, type ActiveConnection, type ColumnMetadata, type ConnectionInput, type DatabaseEngine, type DatabaseObject, type HistoryProject, type IpcError, type RecoveryPoint, type RevisionSummary, type SavedConnection, type Section, type ServerProcess, type ServerUser, type ServerVariable } from './ipc'
import { sectionLabels, useAppStore } from './store'
import TableStructureDialog, { type StructureTarget } from './TableStructureDialog'
import { openContextMenu } from './ContextMenu'

type ConnStatus = 'disconnected' | 'connecting' | 'connected' | 'error'
type NodeValue<T> = { value: T; error?: IpcError }
type CsvExportTarget = { connectionId: string; database: string; table: string }
type ServerSection = 'processes' | 'users' | 'variables'
type ServerSectionData = { processes?: NodeValue<ServerProcess[]>; users?: NodeValue<ServerUser[]>; variables?: NodeValue<ServerVariable[]> }
type Props = { section: Section }

const revisionStatusLabels: Record<string, string> = {
  draft: 'Borrador',
  confirmed: 'Confirmada sin aplicar',
  applying: 'Aplicándose',
  applied: 'Aplicada',
  failed: 'Fallida',
  failed_partial: 'Fallida parcialmente',
  uncertain: 'Resultado incierto',
  discarded: 'Descartada',
}

function revisionStatusLabel(status: string) {
  return revisionStatusLabels[status] ?? 'Estado no reconocido'
}

type RowChangePlanView = {
  tableName: string
  primaryKey: Array<{ column: string; value: string | null }>
  columnName: string
  oldValue: string | null
  newValue: string | null
}

function parseRowChangePlan(raw: string, revision: RevisionSummary): RowChangePlanView | null {
  if (!['Actualizar fila', 'Revertir edición de fila'].includes(revision.message)) return null
  try {
    const value: unknown = JSON.parse(raw)
    if (!value || typeof value !== 'object') return null
    const plan = value as Record<string, unknown>
    if (plan.version !== 1 || typeof plan.tableName !== 'string' || typeof plan.columnName !== 'string') return null
    if (!Array.isArray(plan.primaryKey) || plan.primaryKey.length === 0 || plan.primaryKey.length > 32) return null
    const primaryKey = plan.primaryKey.map((item) => {
      if (!item || typeof item !== 'object') throw new Error('Clave inválida')
      const key = item as Record<string, unknown>
      if (typeof key.column !== 'string' || (key.value !== null && typeof key.value !== 'string')) throw new Error('Clave inválida')
      return { column: key.column, value: key.value as string | null }
    })
    if ((plan.oldValue !== null && typeof plan.oldValue !== 'string') || (plan.newValue !== null && typeof plan.newValue !== 'string')) return null
    return { tableName: plan.tableName, primaryKey, columnName: plan.columnName, oldValue: plan.oldValue as string | null, newValue: plan.newValue as string | null }
  } catch {
    return null
  }
}

function displayCellValue(value: string | null) {
  return value === null ? 'NULL' : value === '' ? 'Cadena vacía' : value
}

function UncertainRevisionNotice() {
  return <div className="history-uncertain-warning" role="alert"><AlertCircle size={14} /><span><strong>Comprueba la base de datos antes de continuar.</strong> La aplicación se interrumpió y el resultado remoto no se pudo confirmar. DBSUAL no repetirá esta revisión automáticamente; no la apliques de nuevo. Inspecciona el destino y prepara otra revisión solo después de conocer su estado.</span></div>
}

function errorOf(error: unknown): IpcError { return typeof error === 'object' && error !== null && 'message' in error ? error as IpcError : { code: 'IPC_ERROR', message: 'No se pudo completar la operación.' } }
function NodeError({ error, retry }: { error: IpcError; retry: () => void }) { return <div className="node-error" role="alert"><AlertCircle size={12} /><span title={error.message}>{error.message}</span><button onClick={retry}>Reintentar</button></div> }
function ServerNode({ name, icon: Icon, expanded, loading, data, toggle, retry, children }: { name: string; icon: typeof Activity; expanded: boolean; loading: boolean; data?: NodeValue<unknown[]>; toggle: () => void; retry: () => void; children: ReactNode }) {
  return <div className="server-info-node"><div className="tree-object-row"><button className="node-disclosure" aria-label={`${expanded ? 'Contraer' : 'Expandir'} ${name}`} aria-expanded={expanded} onClick={toggle}>{expanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />}</button><Icon size={12} /><span>{name}</span>{expanded && <button className="icon-button tiny refresh-node" aria-label={`Actualizar ${name}`} disabled={loading} onClick={retry}><RefreshCw size={11} /></button>}</div>{expanded && <div className="server-info-rows">{loading && <div className="node-state"><LoaderCircle size={11} className="spin" /> Cargando {name.toLowerCase()}…</div>}{data?.error && <NodeError error={data.error} retry={retry} />}{!loading && data && !data.error && data.value.length === 0 && <div className="node-empty">No hay datos visibles para esta cuenta.</div>}{!data && !loading && <div className="node-empty">Aún no se ha consultado.</div>}{data && !data.error && children}</div>}</div>
}

function ConnectionDialog({ current, close, saved }: { current?: SavedConnection; close: () => void; saved: (editedId?: string) => Promise<void> }) {
  const [name, setName] = useState(current?.name ?? '')
  const [engine, setEngine] = useState<DatabaseEngine>(current?.engine ?? 'mysql')
  const [host, setHost] = useState(current?.host ?? '')
  const [port, setPort] = useState(String(current?.port ?? (engine === 'postgresql' ? 5432 : 3306)))
  const [user, setUser] = useState(current?.user ?? '')
  const [password, setPassword] = useState('')
  const [tls, setTls] = useState(current ? current.tlsMode === 'verifyIdentity' : true)
  const [caPath, setCaPath] = useState(current?.tlsCaPath ?? '')
  const [certPath, setCertPath] = useState(current?.tlsClientCertPath ?? '')
  const [keyPath, setKeyPath] = useState(current?.tlsClientKeyPath ?? '')
  const [sshEnabled, setSshEnabled] = useState(current?.sshEnabled ?? false)
  const [sshHost, setSshHost] = useState(current?.sshHost ?? '')
  const [sshPort, setSshPort] = useState(String(current?.sshPort ?? 22))
  const [sshUser, setSshUser] = useState(current?.sshUser ?? '')
  const [sshAuthMethod, setSshAuthMethod] = useState<'password' | 'privateKey' | 'agent'>(current?.sshAuthMethod ?? 'password')
  const [sshKeyPath, setSshKeyPath] = useState(current?.sshKeyPath ?? '')
  const [sshPassword, setSshPassword] = useState('')
  const [sshKeyPassphrase, setSshKeyPassphrase] = useState('')
  const [busy, setBusy] = useState<'save' | 'test' | null>(null)
  const [error, setError] = useState<IpcError | null>(null)
  const [tested, setTested] = useState<{ serverVersion: string; tlsActive: boolean } | null>(null)
  const [validation, setValidation] = useState<string | null>(null)
  const buildInput = (): ConnectionInput => ({
    ...(current ? { id: current.id } : {}), name: name.trim(), engine, host: host.trim(), port: engine === 'sqlite' ? 0 : Number(port), user: engine === 'sqlite' ? '' : user.trim(),
    ...(password ? { password } : {}), tlsMode: tls ? 'verifyIdentity' : 'disabled',
    ...(tls && caPath.trim() ? { tlsCaPath: caPath.trim() } : {}),
    ...(tls && certPath.trim() ? { tlsClientCertPath: certPath.trim() } : {}),
    ...(tls && keyPath.trim() ? { tlsClientKeyPath: keyPath.trim() } : {}),
    sshEnabled,
    ...(sshEnabled ? {
      sshHost: sshHost.trim(), sshPort: Number(sshPort), sshUser: sshUser.trim(), sshAuthMethod,
      ...(sshAuthMethod === 'privateKey' && sshKeyPath.trim() ? { sshKeyPath: sshKeyPath.trim() } : {}),
      ...(sshAuthMethod === 'password' && sshPassword ? { sshPassword } : {}),
      ...(sshAuthMethod === 'privateKey' && sshKeyPassphrase ? { sshKeyPassphrase } : {}),
    } : {}),
  })
  const validate = () => {
    if (!name.trim()) return 'Escribe un nombre para identificar la conexión.'
    if (engine === 'sqlite') return host.trim() ? null : 'Selecciona un archivo SQLite existente.'
    if (!host.trim()) return `Escribe el servidor ${engine === 'postgresql' ? 'PostgreSQL' : engine === 'mariadb' ? 'MariaDB' : 'MySQL'}.`
    if (!Number.isInteger(Number(port)) || Number(port) < 1 || Number(port) > 65535) return 'El puerto debe estar entre 1 y 65535.'
    if (!user.trim()) return `Escribe el usuario de ${engine === 'postgresql' ? 'PostgreSQL' : engine === 'mariadb' ? 'MariaDB' : 'MySQL'}.`
    if (engine === 'postgresql' && sshEnabled) return 'El túnel SSH para PostgreSQL aún no está disponible.'
    if (Boolean(certPath.trim()) !== Boolean(keyPath.trim())) return 'Para usar un certificado de cliente, indica tanto el certificado como su clave.'
    if (sshEnabled) {
      if (!sshHost.trim() || !sshUser.trim()) return 'Escribe el servidor y el usuario del túnel SSH.'
      if (!Number.isInteger(Number(sshPort)) || Number(sshPort) < 1 || Number(sshPort) > 65535) return 'El puerto SSH debe estar entre 1 y 65535.'
      if (sshAuthMethod === 'privateKey' && !sshKeyPath.trim()) return 'Indica la ruta del archivo de clave privada SSH.'
      const savedPasswordAvailable = current?.sshEnabled && current.sshAuthMethod === 'password' && sshAuthMethod === 'password'
      if (sshAuthMethod === 'password' && !sshPassword && !savedPasswordAvailable) return 'Escribe la contraseña SSH.'
    }
    return null
  }
  const run = async (mode: 'save' | 'test') => {
    const issue = validate(); setValidation(issue); setError(null); setTested(null)
    if (issue) return
    setBusy(mode)
    try {
      const input = buildInput()
      if (mode === 'test') {
        const result = await ipc.testConnection(input)
        setTested(result)
      } else {
        await ipc.saveConnection(input)
        setPassword('')
        await saved(current?.id)
        close()
      }
    } catch (cause) { setError(errorOf(cause)) }
    finally { setBusy(null) }
  }
  return <div className="modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget && !busy) close() }}><section className="connection-dialog" role="dialog" aria-modal="true" aria-labelledby="connection-dialog-title">
    <header className="dialog-heading"><div className="dialog-heading-icon"><Database size={18} /></div><div><h2 id="connection-dialog-title">{current ? 'Editar conexión' : 'Nueva conexión'}</h2><p>{engine === 'sqlite' ? 'SQLite · archivo local, acceso de solo lectura.' : `${engine === 'postgresql' ? 'PostgreSQL' : engine === 'mariadb' ? 'MariaDB' : 'MySQL'} · la contraseña se guarda en el almacén seguro de Windows.`}</p></div><button className="icon-button subtle" aria-label="Cerrar" disabled={Boolean(busy)} onClick={close}><X size={17} /></button></header>
    <form className="connection-form" onSubmit={(event) => { event.preventDefault(); void run('save') }}>
      <div className="form-section"><div className="form-section-title"><span>CONEXIÓN</span><span className="mysql-pill">{engine === 'sqlite' ? 'SQLITE' : engine === 'postgresql' ? 'POSTGRESQL' : engine === 'mariadb' ? 'MARIADB' : 'MYSQL'}</span></div>
        <label className="field-label">Motor<select aria-label="Motor de base de datos" value={engine} disabled={Boolean(current) || Boolean(busy)} onChange={(event) => { const next = event.target.value as DatabaseEngine; if (Number(port) === (engine === 'postgresql' ? 5432 : 3306)) setPort(next === 'postgresql' ? '5432' : '3306'); setPassword(''); setCaPath(''); setCertPath(''); setKeyPath(''); setSshEnabled(false); setSshPassword(''); setSshKeyPassphrase(''); setEngine(next); setTested(null); setError(null); setValidation(null) }}><option value="mysql">MySQL</option><option value="mariadb">MariaDB · 10.6, 10.11 y 11.4</option><option value="postgresql">PostgreSQL · conexión directa</option><option value="sqlite">SQLite · archivo local, solo lectura</option></select></label>
        {current && <p className="field-hint">El motor de una conexión guardada no se cambia; crea otra conexión para usar un motor distinto.</p>}
        <label className="field-label">Nombre de conexión<input autoFocus value={name} onChange={(event) => setName(event.target.value)} placeholder="Ej. Producción" maxLength={80} /></label>
        {engine === 'sqlite' ? <div className="field-row"><label className="field-label grow">Archivo de base de datos<input value={host} onChange={(event) => setHost(event.target.value)} placeholder="Selecciona un archivo .db o .sqlite" readOnly /></label><button type="button" className="secondary-action" disabled={Boolean(busy)} onClick={async () => { const selected = await open({ multiple: false, filters: [{ name: 'SQLite', extensions: ['db', 'sqlite', 'sqlite3'] }] }); if (typeof selected === 'string') setHost(selected) }}>Elegir archivo</button></div> : <><div className="field-row"><label className="field-label grow">Servidor<input value={host} onChange={(event) => setHost(event.target.value)} placeholder="db.ejemplo.com o 127.0.0.1" autoComplete="off" /></label><label className="field-label port-field">Puerto<input value={port} onChange={(event) => setPort(event.target.value)} inputMode="numeric" /></label></div>
        <div className="field-row"><label className="field-label grow">Usuario<input value={user} onChange={(event) => setUser(event.target.value)} autoComplete="username" /></label><label className="field-label grow">Contraseña<input type="password" value={password} onChange={(event) => setPassword(event.target.value)} placeholder={current ? 'Dejar vacía para conservarla' : 'Opcional'} autoComplete="new-password" /></label></div>
        {current && <p className="field-hint"><LockKeyhole size={12} /> La contraseña guardada no se muestra. Déjala vacía para conservarla o escribe otra para reemplazarla.</p>}</>}
      </div>
      {engine !== 'sqlite' && <div className="form-section tls-section"><div className="form-section-title"><span>SEGURIDAD DE TRANSPORTE</span><span className="optional-label">OPCIONAL</span></div>
        <label className="toggle-row"><input type="checkbox" checked={tls} onChange={(event) => setTls(event.target.checked)} /><span className="toggle-track" /><span><strong>Usar TLS y verificar la identidad del servidor</strong><small>Si la verificación falla, la conexión no continuará sin cifrado.</small></span><ShieldCheck size={17} className={tls ? 'toggle-accent' : ''} /></label>
        {tls && <div className="tls-fields"><label className="field-label">Certificado de autoridad propio <span>· si el servidor lo requiere</span><input value={caPath} onChange={(event) => setCaPath(event.target.value)} placeholder="Ruta del archivo CA (opcional)" /></label>{engine !== 'postgresql' ? <><div className="field-row"><label className="field-label grow">Certificado de cliente<input value={certPath} onChange={(event) => setCertPath(event.target.value)} placeholder="Ruta PEM (opcional)" /></label><label className="field-label grow">Clave privada<input value={keyPath} onChange={(event) => setKeyPath(event.target.value)} placeholder="Ruta PEM (opcional)" /></label></div><p className="field-hint"><FileKey size={12} /> Los archivos se usan desde su ubicación; DBSUAL no copia su contenido.</p></> : <p className="field-hint">El certificado de cliente PostgreSQL aún no está disponible en esta primera conexión.</p>}</div>}
        {engine !== 'postgresql' && <label className="toggle-row ssh-toggle"><input type="checkbox" checked={sshEnabled} onChange={(event) => setSshEnabled(event.target.checked)} /><span className="toggle-track" /><span><strong>Conectar mediante un túnel SSH</strong><small>TLS conserva la validación del nombre del servidor.</small></span><LockKeyhole size={16} className={sshEnabled ? 'toggle-accent' : ''} /></label>}
        {engine === 'postgresql' && <p className="field-hint">Esta primera conexión PostgreSQL es directa. TLS con verificación de identidad está disponible; el túnel SSH se habilitará después de validar el adaptador.</p>}
        {engine !== 'postgresql' && sshEnabled && <div className="tls-fields ssh-fields">
          <div className="field-row"><label className="field-label grow">Servidor SSH<input value={sshHost} onChange={(event) => setSshHost(event.target.value)} placeholder="bastion.ejemplo.com" /></label><label className="field-label port-field">Puerto<input value={sshPort} onChange={(event) => setSshPort(event.target.value)} inputMode="numeric" /></label></div>
          <div className="field-row"><label className="field-label grow">Usuario SSH<input value={sshUser} onChange={(event) => setSshUser(event.target.value)} autoComplete="username" /></label><label className="field-label grow">Autenticación<select value={sshAuthMethod} onChange={(event) => setSshAuthMethod(event.target.value as typeof sshAuthMethod)}><option value="password">Contraseña</option><option value="privateKey">Clave privada</option><option value="agent">Agente OpenSSH de Windows</option></select></label></div>
          {sshAuthMethod === 'password' && <label className="field-label">Contraseña SSH<input type="password" value={sshPassword} onChange={(event) => setSshPassword(event.target.value)} placeholder={current?.sshEnabled ? 'Dejar vacía para conservarla' : 'Contraseña del servidor SSH'} autoComplete="new-password" /></label>}
          {sshAuthMethod === 'privateKey' && <><label className="field-label">Archivo de clave privada SSH<input value={sshKeyPath} onChange={(event) => setSshKeyPath(event.target.value)} placeholder="Ruta local del archivo de clave" /></label><label className="field-label">Frase de paso <span>· opcional</span><input type="password" value={sshKeyPassphrase} onChange={(event) => setSshKeyPassphrase(event.target.value)} placeholder={current?.sshEnabled && current.sshAuthMethod === 'privateKey' ? 'Dejar vacía para conservarla' : 'Si la clave está protegida'} autoComplete="new-password" /></label></>}
          {sshAuthMethod === 'agent' && <p className="field-hint"><LockKeyhole size={12} /> DBSUAL usará el agente OpenSSH de Windows y no guardará claves.</p>}
          <p className="field-hint"><ShieldCheck size={12} /> La primera conexión muestra la huella SSH. Verifícala con una fuente confiable antes de aprobarla.</p>
        </div>}
      </div>}
      {(validation || error) && <div className="dialog-error" role="alert"><AlertCircle size={15} /><span>{validation ?? error?.message}</span>{error && <small>{error.code}</small>}</div>}
      {tested && <div className="test-success" role="status"><Check size={15} /><span>Prueba correcta · {engine === 'sqlite' ? 'SQLite' : engine === 'postgresql' ? 'PostgreSQL' : engine === 'mariadb' ? 'MariaDB' : 'MySQL'} {tested.serverVersion} · {engine === 'sqlite' ? 'solo lectura' : `TLS ${tested.tlsActive ? 'activo' : 'inactivo'}`}</span></div>}
      <footer className="dialog-actions"><button type="button" className="text-action" disabled={Boolean(busy)} onClick={close}>Cancelar</button><button type="button" className="secondary-action" disabled={Boolean(busy)} onClick={() => void run('test')}>{busy === 'test' ? <LoaderCircle className="spin" size={14} /> : <Plug size={14} />} Probar conexión</button><button type="submit" className="primary-action" disabled={Boolean(busy)}>{busy === 'save' ? <LoaderCircle className="spin" size={14} /> : <Check size={14} />} Guardar</button></footer>
    </form>
  </section></div>
}

export default function ConnectionWorkspace({ section }: Props) {
  const { update } = useAppStore()
  const [connections, setConnections] = useState<SavedConnection[]>([])
  const [statuses, setStatuses] = useState<Record<string, { state: ConnStatus; detail?: ActiveConnection | IpcError }>>({})
  const [listState, setListState] = useState<{ loading: boolean; error?: IpcError }>({ loading: true })
  const [dialog, setDialog] = useState<'new' | SavedConnection | null>(null)
  useEffect(() => { const open = () => setDialog('new'); window.addEventListener('dbsual:new-connection', open); return () => window.removeEventListener('dbsual:new-connection', open) }, [])
  const [removeTarget, setRemoveTarget] = useState<SavedConnection | null>(null)
  const [removing, setRemoving] = useState(false)
  const [pendingSshKey, setPendingSshKey] = useState<{ connection: SavedConnection; fingerprint: string; publicKey: string; typedFingerprint: string; changed: boolean; error?: string } | null>(null)
  const [expandedConnections, setExpandedConnections] = useState<Set<string>>(new Set())
  const [expandedDatabases, setExpandedDatabases] = useState<Set<string>>(new Set())
  const [expandedObjects, setExpandedObjects] = useState<Set<string>>(new Set())
  const [expandedServerSections, setExpandedServerSections] = useState<Set<string>>(new Set())
  const [serverInfo, setServerInfo] = useState<Record<string, ServerSectionData>>({})
  const [databases, setDatabases] = useState<Record<string, NodeValue<string[]>>>({})
  const [objects, setObjects] = useState<Record<string, NodeValue<DatabaseObject[]>>>({})
  const [columns, setColumns] = useState<Record<string, NodeValue<ColumnMetadata[]>>>({})
  const [loadingNodes, setLoadingNodes] = useState<Set<string>>(new Set())
  const [historyProjects, setHistoryProjects] = useState<HistoryProject[]>([])
  const [historyRevisions, setHistoryRevisions] = useState<Record<string, RevisionSummary[]>>({})
  const [recoveryPoints, setRecoveryPoints] = useState<Record<string, RecoveryPoint[]>>({})
  const [historyError, setHistoryError] = useState<string | null>(null)
  const [historyBusy, setHistoryBusy] = useState<string | null>(null)
  const [selectedPlan, setSelectedPlan] = useState<{ projectName: string; revision: RevisionSummary; sql: string; rowChange?: RowChangePlanView | null } | null>(null)
  const [createTarget, setCreateTarget] = useState<{ connectionId: string; name: string }>({ connectionId: '', name: '' })
  const [createDialog, setCreateDialog] = useState(false)
  const [createBusy, setCreateBusy] = useState(false)
  const [restoreTarget, setRestoreTarget] = useState<{ project: HistoryProject; point: RecoveryPoint } | null>(null)
  const [restoreName, setRestoreName] = useState('')
  const [csvTarget, setCsvTarget] = useState<CsvExportTarget | null>(null)
  const [structureTarget, setStructureTarget] = useState<StructureTarget | null>(null)
  const [csvDelimiter, setCsvDelimiter] = useState<'comma' | 'semicolon' | 'tab'>('comma')
  const [csvNullMarker, setCsvNullMarker] = useState('\\N')
  const [csvBusy, setCsvBusy] = useState(false)
  const [csvError, setCsvError] = useState('')
  const [csvNotice, setCsvNotice] = useState('')
  const connectionGeneration = useRef<Record<string, number>>({})
  const bumpGeneration = (id: string) => { connectionGeneration.current[id] = (connectionGeneration.current[id] ?? 0) + 1; return connectionGeneration.current[id] }
  const clearTreeFor = (id: string) => {
    const belongsToConnection = (key: string) => key.startsWith(`obj:${id}:`) || key.startsWith(`col:${id}:`)
    const without = <T,>(record: Record<string, T>) => Object.fromEntries(Object.entries(record).filter(([key]) => !belongsToConnection(key))) as Record<string, T>
    setExpandedConnections((old) => { const next = new Set(old); next.delete(id); return next })
    setExpandedDatabases((old) => new Set([...old].filter((key) => !belongsToConnection(key))))
    setExpandedObjects((old) => new Set([...old].filter((key) => !belongsToConnection(key))))
    setExpandedServerSections((old) => new Set([...old].filter((key) => !key.startsWith(`${id}:`))))
    setServerInfo((old) => { const next = { ...old }; delete next[id]; return next })
    setDatabases((old) => { const next = { ...old }; delete next[id]; return next })
    setObjects(without)
    setColumns(without)
  }

  const loadConnections = async () => { setListState({ loading: true }); try { setConnections(await ipc.listConnections()); setListState({ loading: false }) } catch (error) { setListState({ loading: false, error: errorOf(error) }) } }
  useEffect(() => { void loadConnections() }, [])
  const loadHistoryProjects = async (): Promise<HistoryProject[]> => { if (!isTauri()) return []; try { const projects = await ipc.listHistoryProjects(); setHistoryProjects(projects); setHistoryError(null); return projects } catch (error) { setHistoryError(errorOf(error).message); return [] } }
  const loadAllRevisions = async () => { const projects = await loadHistoryProjects(); try { const pairs = await Promise.all(projects.map(async (project) => [project.id, await ipc.listHistoryRevisions(project.id)] as const)); setHistoryRevisions(Object.fromEntries(pairs)) } catch (error) { setHistoryError(errorOf(error).message) } }
  useEffect(() => { if (isTauri() && (section === 'history' || section === 'changes')) { if (section === 'changes') void loadAllRevisions(); else void loadHistoryProjects() } }, [section])
  useEffect(() => { const refresh = () => void loadAllRevisions(); window.addEventListener('dbsual:history-changed', refresh); return () => window.removeEventListener('dbsual:history-changed', refresh) }, [])
  const startHistory = async (connectionId: string, databaseName: string) => { const key = `${connectionId}:${databaseName}`; setHistoryBusy(key); setHistoryError(null); try { const project = await ipc.createHistoryProject(connectionId, databaseName); setHistoryProjects((items) => [project, ...items.filter((item) => item.id !== project.id)]) } catch (error) { setHistoryError(errorOf(error).message) } finally { setHistoryBusy(null) } }
  const showRevisions = async (projectId: string) => { try { const [revisions, points] = await Promise.all([ipc.listHistoryRevisions(projectId), ipc.listHistoryRecoveryPoints(projectId)]); setHistoryRevisions((items) => ({ ...items, [projectId]: revisions })); setRecoveryPoints((items) => ({ ...items, [projectId]: points })); setHistoryError(null) } catch (error) { setHistoryError(errorOf(error).message) } }
  const captureRecoveryPoint = async (project: HistoryProject) => { const key = `backup:${project.id}`; setHistoryBusy(key); setHistoryError(null); try { const point = await ipc.captureRecoveryPoint(project.connectionId, project.databaseName); setRecoveryPoints((items) => ({ ...items, [project.id]: [point, ...(items[project.id] ?? [])] })) } catch (error) { setHistoryError(errorOf(error).message) } finally { setHistoryBusy(null) } }
  const restoreRecoveryPoint = async () => { if (!restoreTarget || !restoreName.trim()) return; const { project, point } = restoreTarget; setHistoryBusy(point.id); setHistoryError(null); try { const revision = await ipc.prepareRecoveryRestore(point.id, restoreName.trim()); setRestoreTarget(null); setRestoreName(''); await openPlan(project, revision); await loadAllRevisions() } catch (error) { setHistoryError(errorOf(error).message) } finally { setHistoryBusy(null) } }
  const openPlan = async (project: HistoryProject, revision: RevisionSummary) => { setHistoryBusy(revision.id); setHistoryError(null); try { const encryptedPlan = await ipc.readHistoryPlan(revision.id); const isRowChange = ['Actualizar fila', 'Revertir edición de fila'].includes(revision.message); const rowChange = isRowChange ? parseRowChangePlan(encryptedPlan, revision) : undefined; let sql = isRowChange ? '' : encryptedPlan; if (isRowChange && !rowChange) sql = 'No se pudo interpretar el plan de fila de forma segura. El contenido protegido no se muestra.'; if (revision.message.startsWith('Restaurar punto en ')) { const plan = JSON.parse(encryptedPlan) as { recoveryPointId: string; targetDatabase: string }; sql = `Punto de recuperación: ${plan.recoveryPointId}\nBase nueva: ${plan.targetDatabase}\n\nLa cobertura corresponde al punto seleccionado y puede ser parcial.` } setSelectedPlan({ projectName: project.databaseName, revision, sql, ...(isRowChange ? { rowChange } : {}) }) } catch (error) { setHistoryError(errorOf(error).message) } finally { setHistoryBusy(null) } }
  const confirmPlan = async () => { if (!selectedPlan) return; setHistoryBusy(selectedPlan.revision.id); try { await ipc.confirmHistoryRevision(selectedPlan.revision.id); setSelectedPlan(null); await loadAllRevisions() } catch (error) { setHistoryError(errorOf(error).message) } finally { setHistoryBusy(null) } }
  const prepareDatabase = async () => { if (!createTarget.connectionId || !createTarget.name.trim()) return; setCreateBusy(true); setHistoryError(null); try { const revision = await ipc.prepareCreateDatabase(createTarget.connectionId, createTarget.name.trim()); const projects = await loadHistoryProjects(); const project = projects.find((item) => item.id === revision.projectId); if (!project) throw new Error('No se encontró el proyecto creado en el historial.'); const sql = await ipc.readHistoryPlan(revision.id); setSelectedPlan({ projectName: project.databaseName, revision, sql }); setCreateDialog(false); window.dispatchEvent(new Event('dbsual:history-changed')) } catch (error) { setHistoryError(errorOf(error).message) } finally { setCreateBusy(false) } }
  const applyCreateDatabase = async () => { if (!selectedPlan || selectedPlan.revision.message !== 'Crear base de datos') return; const revision = selectedPlan.revision; setHistoryBusy(revision.id); setHistoryError(null); try { await ipc.applyCreateDatabase(revision.id); setSelectedPlan(null); const project = historyProjects.find((item) => item.id === revision.projectId); if (project) await loadDatabases(project.connectionId); await loadAllRevisions() } catch (error) { setHistoryError(errorOf(error).message); await loadAllRevisions() } finally { setHistoryBusy(null) } }
  const applyRecoveryRestore = async () => { if (!selectedPlan || !selectedPlan.revision.message.startsWith('Restaurar punto en ')) return; const revision = selectedPlan.revision; setHistoryBusy(revision.id); setHistoryError(null); try { const result = await ipc.restoreRecoveryPoint(revision.id); setSelectedPlan(null); setCsvNotice(`Base ${result.databaseName} restaurada · ${result.tablesRestored} tablas · ${result.rowsRestored} filas`); const project = historyProjects.find((item) => item.id === revision.projectId); if (project) await loadDatabases(project.connectionId); await loadAllRevisions() } catch (error) { const message = errorOf(error).message; setSelectedPlan(null); await loadAllRevisions(); setHistoryError(message) } finally { setHistoryBusy(null) } }
  const exportCsv = async () => {
    if (!csvTarget || csvBusy) return
    setCsvBusy(true); setCsvError('')
    try {
      const safeName = csvTarget.table.replace(/[<>:"/\\|?*]/g, '_')
      const path = await save({ title: 'Exportar tabla como CSV', defaultPath: `${safeName}.csv`, filters: [{ name: 'Archivo CSV', extensions: ['csv'] }] })
      if (!path) return
      const result = await ipc.exportTableCsv(csvTarget.connectionId, csvTarget.database, csvTarget.table, path, csvDelimiter, csvNullMarker)
      setCsvNotice(`CSV exportado · ${result.rowsWritten} filas · ${csvTarget.table}`)
      setCsvTarget(null)
    } catch (cause) { setCsvError(errorOf(cause).message) }
    finally { setCsvBusy(false) }
  }
  const beginNodeLoad = (key: string) => setLoadingNodes((old) => new Set(old).add(key))
  const finishNodeLoad = (key: string) => setLoadingNodes((old) => { const next = new Set(old); next.delete(key); return next })
  const loadDatabases = async (connectionId: string) => { const key = `db:${connectionId}`; const generation = connectionGeneration.current[connectionId] ?? 0; beginNodeLoad(key); setDatabases((old) => ({ ...old, [connectionId]: { value: old[connectionId]?.value ?? [] } })); try { const value = await ipc.listDatabases(connectionId); if (connectionGeneration.current[connectionId] === generation) setDatabases((old) => ({ ...old, [connectionId]: { value } })) } catch (error) { if (connectionGeneration.current[connectionId] === generation) setDatabases((old) => ({ ...old, [connectionId]: { value: old[connectionId]?.value ?? [], error: errorOf(error) } })) } finally { finishNodeLoad(key) } }
  const loadObjects = async (connectionId: string, database: string) => { const key = `obj:${connectionId}:${database}`; const generation = connectionGeneration.current[connectionId] ?? 0; beginNodeLoad(key); const nodeKey = key; setObjects((old) => ({ ...old, [nodeKey]: { value: old[nodeKey]?.value ?? [] } })); try { const value = await ipc.listDatabaseObjects(connectionId, database); if (connectionGeneration.current[connectionId] === generation) setObjects((old) => ({ ...old, [nodeKey]: { value } })) } catch (error) { if (connectionGeneration.current[connectionId] === generation) setObjects((old) => ({ ...old, [nodeKey]: { value: old[nodeKey]?.value ?? [], error: errorOf(error) } })) } finally { finishNodeLoad(key) } }
  const loadColumns = async (connectionId: string, database: string, item: DatabaseObject) => { const key = `col:${connectionId}:${database}:${item.schema ?? ''}:${item.kind}:${item.name}`; const generation = connectionGeneration.current[connectionId] ?? 0; beginNodeLoad(key); setColumns((old) => ({ ...old, [key]: { value: old[key]?.value ?? [] } })); try { const value = await ipc.listColumns(connectionId, database, item.name, item.kind, item.schema); if (connectionGeneration.current[connectionId] === generation) setColumns((old) => ({ ...old, [key]: { value } })) } catch (error) { if (connectionGeneration.current[connectionId] === generation) setColumns((old) => ({ ...old, [key]: { value: old[key]?.value ?? [], error: errorOf(error) } })) } finally { finishNodeLoad(key) } }
  const loadServerSection = async (connectionId: string, section: ServerSection) => {
    const key = `${connectionId}:${section}`
    const generation = connectionGeneration.current[connectionId] ?? 0
    beginNodeLoad(key)
    setServerInfo((old) => ({ ...old, [connectionId]: { ...old[connectionId], [section]: { value: old[connectionId]?.[section]?.value ?? [] } } }))
    try {
      const value = section === 'processes' ? await ipc.listServerProcesses(connectionId) : section === 'users' ? await ipc.listServerUsers(connectionId) : await ipc.listServerVariables(connectionId)
      if (connectionGeneration.current[connectionId] === generation) setServerInfo((old) => ({ ...old, [connectionId]: { ...old[connectionId], [section]: { value } } }))
    } catch (cause) {
      if (connectionGeneration.current[connectionId] === generation) setServerInfo((old) => ({ ...old, [connectionId]: { ...old[connectionId], [section]: { value: old[connectionId]?.[section]?.value ?? [], error: errorOf(cause) } } }))
    } finally { finishNodeLoad(key) }
  }

  const open = async (connection: SavedConnection, expandAfterConnect = false) => { const generation = bumpGeneration(connection.id); setStatuses((old) => ({ ...old, [connection.id]: { state: 'connecting' } })); try { const active = await ipc.openConnection(connection.id); if (connectionGeneration.current[connection.id] === generation) { setStatuses((old) => ({ ...old, [connection.id]: { state: 'connected', detail: active } })); if (expandAfterConnect) setExpandedConnections((old) => new Set(old).add(connection.id)); if (expandedConnections.has(connection.id) || expandAfterConnect) void loadDatabases(connection.id) } } catch (cause) { const error = errorOf(cause); if (connectionGeneration.current[connection.id] === generation) setStatuses((old) => ({ ...old, [connection.id]: { state: 'error', detail: error } })); if ((error.code === 'SSH_HOST_KEY_UNKNOWN' || error.code === 'SSH_HOST_KEY_CHANGED') && error.fingerprint && error.publicKey) setPendingSshKey({ connection, fingerprint: error.fingerprint, publicKey: error.publicKey, typedFingerprint: '', changed: error.code === 'SSH_HOST_KEY_CHANGED' }) } }
  const approveSshKey = async () => { if (!pendingSshKey || pendingSshKey.typedFingerprint.trim() !== pendingSshKey.fingerprint) return; const pending = pendingSshKey; try { await ipc.approveSshHostKey(pending.connection.id, pending.fingerprint, pending.publicKey); setPendingSshKey(null); await open(pending.connection) } catch (cause) { setPendingSshKey({ ...pending, error: errorOf(cause).message }) } }
  const disconnect = async (connection: SavedConnection) => { const generation = bumpGeneration(connection.id); window.dispatchEvent(new CustomEvent('dbsual:connection-disconnecting', { detail: { connectionId: connection.id } })); setStatuses((old) => ({ ...old, [connection.id]: { state: 'connecting' } })); try { await ipc.disconnectConnection(connection.id); if (connectionGeneration.current[connection.id] === generation) { setStatuses((old) => ({ ...old, [connection.id]: { state: 'disconnected' } })); clearTreeFor(connection.id) } } catch (error) { if (connectionGeneration.current[connection.id] === generation) setStatuses((old) => ({ ...old, [connection.id]: { state: 'error', detail: errorOf(error) } })) } }
  const removeConnection = async () => { if (!removeTarget) return; setRemoving(true); window.dispatchEvent(new CustomEvent('dbsual:connection-disconnecting', { detail: { connectionId: removeTarget.id } })); try { await ipc.removeConnection(removeTarget.id); bumpGeneration(removeTarget.id); clearTreeFor(removeTarget.id); setConnections((all) => all.filter((conn) => conn.id !== removeTarget.id)); setStatuses((all) => { const next = { ...all }; delete next[removeTarget.id]; return next }); setRemoveTarget(null) } catch (error) { setListState({ loading: false, error: errorOf(error) }) } finally { setRemoving(false) } }
  const saved = async (editedId?: string) => { if (editedId) { bumpGeneration(editedId); setStatuses((old) => ({ ...old, [editedId]: { state: 'disconnected' } })); clearTreeFor(editedId) } setDialog(null); await loadConnections() }
  const toggleConnection = (connection: SavedConnection) => { const expanded = new Set(expandedConnections); if (expanded.has(connection.id)) expanded.delete(connection.id); else { expanded.add(connection.id); if (statuses[connection.id]?.state === 'connected' && !databases[connection.id]) void loadDatabases(connection.id) } setExpandedConnections(expanded); }
  const toggleDatabase = (id: string, database: string) => { const key = `obj:${id}:${database}`; const expanded = new Set(expandedDatabases); if (expanded.has(key)) expanded.delete(key); else { expanded.add(key); if (!objects[key]) void loadObjects(id, database) } setExpandedDatabases(expanded) }
  const activateConnection = (connection: SavedConnection) => {
    const status = statuses[connection.id]?.state
    if (status === 'connected') {
      toggleConnection(connection)
    } else if (status !== 'connecting') void open(connection, true)
  }
  const toggleObject = (id: string, database: string, item: DatabaseObject) => { const key = `col:${id}:${database}:${item.schema ?? ''}:${item.kind}:${item.name}`; const expanded = new Set(expandedObjects); if (expanded.has(key)) expanded.delete(key); else { expanded.add(key); if (!columns[key]) void loadColumns(id, database, item) } setExpandedObjects(expanded) }
  const toggleServerSection = (id: string, section: ServerSection) => { const key = `${id}:${section}`; const expanded = new Set(expandedServerSections); if (expanded.has(key)) expanded.delete(key); else { expanded.add(key); if (!serverInfo[id]?.[section]) void loadServerSection(id, section) } setExpandedServerSections(expanded) }
  const emptyText = section === 'changes' ? 'No hay cambios preparados.' : 'Inicia el historial desde una base de datos conectada.'
  const changeRows = historyProjects.flatMap((project) => (historyRevisions[project.id] ?? []).map((revision) => ({ project, revision })))

  return <div className="sidebar">
    <div className="sidebar-heading"><span>{sectionLabels[section].toUpperCase()}</span><div className="sidebar-actions">{section === 'explorer' && <button className="icon-button subtle" title="Actualizar conexiones" disabled={listState.loading} onClick={() => void loadConnections()}><RefreshCw size={14} /></button>}<button className="icon-button subtle" title="Contraer panel lateral" onClick={() => update({ sidebarCollapsed: true })}><X size={14} /></button></div></div>
    {section === 'explorer' ? <div className="sidebar-content explorer-content"><div className="tree-row tree-title"><ChevronDown size={14} /><span>CONEXIONES</span><button className="icon-button tiny" title="Nueva conexión" onClick={() => setDialog('new')}><Plus size={15} /></button></div>
      {listState.loading && <div className="workspace-inline-state"><LoaderCircle className="spin" size={15} /> Cargando conexiones…</div>}
      {listState.error && <div className="list-error"><AlertCircle size={14} /><span>{listState.error.message}</span><button onClick={() => void loadConnections()}>Reintentar</button></div>}
      {!listState.loading && !listState.error && connections.length === 0 && <div className="empty-sidebar"><div className="empty-orbit"><Database size={18} /></div><p>Aún no hay conexiones</p><span>Guarda una conexión MySQL, MariaDB o PostgreSQL para explorar sus bases de datos.</span><button className="small-outline" onClick={() => setDialog('new')}><Plus size={14} /> Nueva conexión</button></div>}
      {connections.map((connection) => {
        const status = statuses[connection.id]?.state ?? 'disconnected'; const active = statuses[connection.id]?.detail as ActiveConnection | undefined
        return <div className="connection-tree" key={connection.id}>
          <div className={`connection-row ${status}`} onContextMenu={(event) => openContextMenu(event, [
            { label: status === 'connected' ? 'Desconectar' : 'Conectar', disabled: status === 'connecting', action: () => status === 'connected' ? void disconnect(connection) : void open(connection) },
            { label: 'Editar conexión', disabled: status === 'connecting', action: () => setDialog(connection) },
            { label: 'Quitar conexión…', disabled: status === 'connecting', action: () => setRemoveTarget(connection) },
          ])}>
            <button className="node-disclosure" aria-label={`${expandedConnections.has(connection.id) ? 'Contraer' : 'Expandir'} ${connection.name}`} aria-expanded={expandedConnections.has(connection.id)} disabled={status !== 'connected'} onClick={() => toggleConnection(connection)}>{expandedConnections.has(connection.id) ? <ChevronDown size={14} /> : <ChevronRight size={14} />}</button>
            <span className={`conn-dot ${status}`} title={status === 'connected' ? 'Conectada' : status === 'connecting' ? 'Conectando' : status === 'error' ? 'Error' : 'Desconectada'} />
            <button className="connection-main" title={`${connection.name} · ${connection.host}`} onClick={() => activateConnection(connection)}><strong>{connection.name}</strong><small>{connection.engine === 'sqlite' ? 'SQLite' : connection.engine === 'postgresql' ? 'PostgreSQL' : connection.engine === 'mariadb' ? 'MariaDB' : 'MySQL'} · {connection.engine === 'sqlite' ? connection.host : `${connection.user}@${connection.host}`}</small></button>
            {status === 'connected' && connection.engine === 'mysql' && <button className="connection-action more" title="Preparar creación de base de datos" onClick={() => { setCreateTarget({ connectionId: connection.id, name: '' }); setHistoryError(null); setCreateDialog(true) }}><Plus size={13} /></button>}{status === 'connecting' ? <LoaderCircle className="row-loading spin" size={13} /> : status === 'connected' ? <button className="connection-action" title="Desconectar" onClick={() => void disconnect(connection)}><Unplug size={13} /></button> : <button className="connection-action" title="Abrir conexión" onClick={() => void open(connection)}><PlugZap size={14} /></button>}
            <button className="connection-action more" aria-label={`Más opciones para ${connection.name}`} title="Más opciones" disabled={status === 'connecting'} onClick={(event) => openContextMenu(event, [{ label: 'Editar conexión', action: () => setDialog(connection) }, { label: 'Quitar conexión…', action: () => setRemoveTarget(connection) }])}><MoreHorizontal size={14} /></button>
          </div>
          {status === 'error' && <div className="connection-error" role="alert"><AlertCircle size={12} /><span>{(statuses[connection.id]?.detail as IpcError | undefined)?.message ?? 'No se pudo abrir la conexión.'}</span><button onClick={() => void open(connection)}>Reintentar</button></div>}
          {status === 'connected' && <div className="connection-meta"><span>{connection.engine === 'sqlite' ? 'SQLite' : connection.engine === 'postgresql' ? 'PostgreSQL' : connection.engine === 'mariadb' ? 'MariaDB' : 'MySQL'} {active?.serverVersion}</span><span>{connection.engine === 'sqlite' ? 'Solo lectura' : active?.tlsActive ? 'TLS activo' : 'TLS inactivo'}</span></div>}
          {expandedConnections.has(connection.id) && status === 'connected' && <div className="tree-children">
            {connection.engine !== 'postgresql' && <><ServerNode name="Procesos" icon={Activity} expanded={expandedServerSections.has(`${connection.id}:processes`)} loading={loadingNodes.has(`${connection.id}:processes`)} data={serverInfo[connection.id]?.processes} toggle={() => toggleServerSection(connection.id, 'processes')} retry={() => void loadServerSection(connection.id, 'processes')}>
              {serverInfo[connection.id]?.processes?.value.map((process) => <div className="server-info-row" key={process.id}><strong>#{process.id} · {process.user ?? 'usuario desconocido'}</strong><span>{process.host ?? 'host no visible'} · {process.database ?? 'sin base'} · {process.command ?? 'sin comando'}</span><small>{process.state ?? 'sin estado'} · {process.durationSeconds}s</small></div>)}
            </ServerNode>
            <ServerNode name="Usuarios" icon={Users} expanded={expandedServerSections.has(`${connection.id}:users`)} loading={loadingNodes.has(`${connection.id}:users`)} data={serverInfo[connection.id]?.users} toggle={() => toggleServerSection(connection.id, 'users')} retry={() => void loadServerSection(connection.id, 'users')}>
              {serverInfo[connection.id]?.users?.value.map((user) => <div className="server-info-row" key={`${user.name}@${user.host}`}><strong>{user.name}@{user.host}</strong><span>{user.authenticationPlugin ?? 'autenticación no visible'}</span><small>{user.locked === null ? 'Estado no disponible' : user.locked ? 'Cuenta bloqueada' : 'Cuenta habilitada'}</small></div>)}
            </ServerNode>
            <ServerNode name="Variables" icon={SlidersHorizontal} expanded={expandedServerSections.has(`${connection.id}:variables`)} loading={loadingNodes.has(`${connection.id}:variables`)} data={serverInfo[connection.id]?.variables} toggle={() => toggleServerSection(connection.id, 'variables')} retry={() => void loadServerSection(connection.id, 'variables')}>
              {serverInfo[connection.id]?.variables?.value.map((variable) => <div className="server-info-row variable-row" key={variable.name}><strong>{variable.name}</strong><span>{variable.value}</span><small>Alcance global</small></div>)}
            </ServerNode>
            </>}
            <div className="tree-row tree-title nested-title"><Database size={12} /><span>BASES DE DATOS</span><button className="icon-button tiny" title="Actualizar bases de datos" disabled={loadingNodes.has(`db:${connection.id}`)} onClick={() => void loadDatabases(connection.id)}><RefreshCw size={12} /></button></div>
            {loadingNodes.has(`db:${connection.id}`) && <div className="node-state"><LoaderCircle size={12} className="spin" /> Cargando bases…</div>}
            {databases[connection.id]?.error && <NodeError error={databases[connection.id].error!} retry={() => void loadDatabases(connection.id)} />}
            {!loadingNodes.has(`db:${connection.id}`) && databases[connection.id]?.value.length === 0 && !databases[connection.id]?.error && <div className="node-empty">No hay bases de datos visibles.</div>}
            {databases[connection.id]?.value.map((database) => { const dbKey = `obj:${connection.id}:${database}`; const opened = expandedDatabases.has(dbKey); const result = objects[dbKey]; const isLoading = loadingNodes.has(dbKey); const tables = result?.value.filter((item) => item.kind === 'table') ?? []; const views = result?.value.filter((item) => item.kind === 'view') ?? []; const schemas = [...new Set(result?.value.map((item) => item.schema).filter((schema): schema is string => Boolean(schema)))].sort()
              const hasHistory = historyProjects.some((project) => project.connectionId === connection.id && project.databaseName === database)
              const historyKey = `${connection.id}:${database}`
              return <div className="database-node" key={database}><div className="tree-object-row" onContextMenu={(event) => openContextMenu(event, [{ label: opened ? 'Contraer base de datos' : 'Explorar base de datos', action: () => toggleDatabase(connection.id, database) }, { label: 'Actualizar tablas y vistas', disabled: status !== 'connected', action: () => void loadObjects(connection.id, database) }, ...(connection.engine === 'mysql' && !hasHistory ? [{ label: 'Iniciar historial local', action: () => void startHistory(connection.id, database) }] : [])])}><button className="node-disclosure" aria-expanded={opened} aria-label={`${opened ? 'Contraer' : 'Expandir'} base ${database}`} onClick={() => toggleDatabase(connection.id, database)}>{opened ? <ChevronDown size={13} /> : <ChevronRight size={13} />}</button><Database size={13} className="db-icon" /><button className="tree-node-label" type="button" title={database} onClick={() => toggleDatabase(connection.id, database)}>{database}</button>{connection.engine === 'mysql' && !hasHistory && <button className="icon-button tiny refresh-node" title="Iniciar historial local para esta base" disabled={historyBusy === historyKey} onClick={() => void startHistory(connection.id, database)}>{historyBusy === historyKey ? <LoaderCircle size={11} className="spin" /> : <GitBranch size={11} />}</button>}{opened && <button className="icon-button tiny refresh-node" title="Actualizar objetos" onClick={() => void loadObjects(connection.id, database)}><RefreshCw size={11} /></button>}</div>
                {opened && <div className="object-children">
                  {isLoading && <div className="node-state"><LoaderCircle size={12} className="spin" /> Cargando objetos…</div>}
                  {result?.error && <NodeError error={result.error} retry={() => void loadObjects(connection.id, database)} />}
                  {!isLoading && result && !result.error && result.value.length === 0 && <div className="node-empty">No hay tablas ni vistas visibles.</div>}
                  {connection.engine === 'postgresql' && result && !result.error && schemas.map((schema) => <section className="object-group" key={schema}><div className="tree-row group-label"><Database size={11} /><span>{schema}</span></div>{(['table', 'view'] as const).map((kind) => { const items = result.value.filter((item) => item.schema === schema && item.kind === kind); return items.length ? <div className="object-group" key={kind}><div className="tree-row group-label">{kind === 'table' ? <Table2 size={11} /> : <Eye size={11} />}<span>{kind === 'table' ? 'TABLAS' : 'VISTAS'}</span></div>{items.map((item) => <ObjectTree key={`${schema}:${kind}:${item.name}`} connectionId={connection.id} database={database} item={item} expanded={expandedObjects} toggle={toggleObject} columns={columns} loading={loadingNodes} reload={loadColumns} onViewData={() => window.dispatchEvent(new CustomEvent('dbsual:open-table-data', { detail: { engine: 'postgresql', connectionId: connection.id, database, schema, table: item.name } }))} />)}</div> : null })}</section>)}
                  {connection.engine !== 'postgresql' && result && !result.error && tables.length > 0 && <div className="object-group"><div className="tree-row group-label"><Table2 size={11} /><span>TABLAS</span></div>{tables.map((item) => <ObjectTree key={`table:${item.name}`} connectionId={connection.id} database={database} item={item} expanded={expandedObjects} toggle={toggleObject} columns={columns} loading={loadingNodes} reload={loadColumns} onViewData={() => window.dispatchEvent(new CustomEvent('dbsual:open-table-data', { detail: { engine: connection.engine, connectionId: connection.id, database, schema: null, table: item.name } }))} onViewStructure={connection.engine === 'sqlite' ? undefined : () => setStructureTarget({ connectionId: connection.id, database, table: item.name })} onExport={connection.engine === 'sqlite' ? undefined : () => { setCsvNotice(''); setCsvError(''); setCsvTarget({ connectionId: connection.id, database, table: item.name }) }} />)}</div>}
                  {connection.engine !== 'postgresql' && result && !result.error && views.length > 0 && <div className="object-group"><div className="tree-row group-label"><Eye size={11} /><span>VISTAS</span></div>{views.map((item) => <ObjectTree key={`view:${item.name}`} connectionId={connection.id} database={database} item={item} expanded={expandedObjects} toggle={toggleObject} columns={columns} loading={loadingNodes} reload={loadColumns} />)}</div>}
                </div>}
              </div>
            })}
          </div>}
        </div>
      })}
      <div className="sidebar-divider"/><div className="tree-row tree-title muted"><Clock3 size={13} /><span>ELEMENTOS RECIENTES</span></div>
    </div> : <div className="sidebar-content">
      <div className="tree-row tree-title"><ChevronDown size={14} /><span>{section === 'changes' ? 'CAMBIOS PREPARADOS' : 'HISTORIAL LOCAL'}</span></div>
      {section === 'changes' ? <div className="history-list">
        {historyError && <div className="node-error" role="alert"><AlertCircle size={12} /><span>{historyError}</span><button onClick={() => void loadAllRevisions()}>Reintentar</button></div>}
        {changeRows.length === 0 && !historyError && <div className="empty-sidebar compact"><div className="empty-orbit"><GitBranch size={18} /></div><p>Sin cambios preparados</p><span>{emptyText}</span></div>}
        {changeRows.map(({ project, revision }) => <div className="history-change-entry" key={revision.id}><button className="history-change-button" disabled={historyBusy === revision.id} onClick={() => void openPlan(project, revision)}><GitBranch size={13} /><span><strong>Rev. {revision.revisionNumber} · {project.databaseName}</strong><small>{revisionStatusLabel(revision.status)} · {revision.operationCount} operación{revision.operationCount === 1 ? '' : 'es'}</small></span>{historyBusy === revision.id ? <LoaderCircle size={13} className="spin" /> : <ChevronRight size={13} />}</button>{revision.status === 'uncertain' && <UncertainRevisionNotice />}</div>)}
      </div> : <div className="history-list">{historyError && <div className="node-error" role="alert"><AlertCircle size={12} /><span>{historyError}</span><button onClick={() => void loadHistoryProjects()}>Reintentar</button></div>}{historyProjects.length === 0 && !historyError && <div className="empty-sidebar compact"><div className="empty-orbit"><Clock3 size={18} /></div><p>Sin historial todavía</p><span>{emptyText}</span></div>}{historyProjects.map((project) => { const connection = connections.find((item) => item.id === project.connectionId); const revisions = historyRevisions[project.id]; const points = recoveryPoints[project.id]; const busyKey = `backup:${project.id}`; const connected = connection && statuses[connection.id]?.state === 'connected'; return <section className="history-project" key={project.id}><button className="history-project-button" onClick={() => void showRevisions(project.id)}><Database size={13} /><span><strong>{project.databaseName}</strong><small>{connection?.name ?? 'Conexión quitada'} · {project.engine === 'mariadb' ? 'MariaDB' : 'MySQL'}{project.serverVersion ? ` ${project.serverVersion}` : ''}</small></span><ChevronRight size={13} /></button>{connection && connection.engine === project.engine && ['mysql', 'mariadb'].includes(project.engine) && <button className="history-revision-button" disabled={!connected || historyBusy === busyKey} title={!connected ? 'Abre la conexión para capturar el punto' : undefined} onClick={() => void captureRecoveryPoint(project)}><span>{historyBusy === busyKey ? 'Capturando respaldo…' : 'Capturar punto cifrado'}</span><small>Tablas, vistas y triggers simples · cobertura parcial</small></button>}{points?.map((point) => <div className="history-revision" key={point.id}><small>Respaldo · {point.verificationState} · {Math.max(1, Math.round(point.encryptedBytes / 1024))} KB</small><small>Cobertura: {point.coverage === 'visible_tables_only' ? 'tablas visibles' : point.coverage === 'visible_tables_and_views' ? 'tablas y vistas visibles' : point.coverage === 'visible_tables_views_triggers' ? 'tablas, vistas y triggers simples visibles' : point.coverage}</small>{connection && connection.engine === project.engine && ['visible_tables_only', 'visible_tables_and_views', 'visible_tables_views_triggers'].includes(point.coverage) && <button className="icon-button tiny" disabled={!connected} onClick={() => { setRestoreTarget({ project, point }); setRestoreName(`${project.databaseName}_restore`) }}>Restaurar en base nueva</button>}</div>)}{revisions && (revisions.length ? revisions.map((revision) => <div className="history-revision-entry" key={revision.id}><button className="history-revision-button" onClick={() => void openPlan(project, revision)}><span className="history-status">{revisionStatusLabel(revision.status)}</span><small>Rev. {revision.revisionNumber} · {revision.planSha256.slice(0, 12)}</small></button>{revision.status === 'uncertain' && <UncertainRevisionNotice />}</div>) : <div className="history-revision"><small>Aún no hay revisiones.</small></div>)}</section> })}</div>}
    </div>}
    <div className="sidebar-footer"><span className="status-dot" /> {connections.length ? `${connections.length} conexión${connections.length === 1 ? '' : 'es'} guardada${connections.length === 1 ? '' : 's'}` : 'Almacenamiento local listo'}</div>
    {csvNotice && <div className="csv-export-notice" role="status"><Check size={13} /><span>{csvNotice}</span><button className="icon-button tiny" aria-label="Cerrar aviso" onClick={() => setCsvNotice('')}><X size={12} /></button></div>}
    {structureTarget && <TableStructureDialog target={structureTarget} close={() => setStructureTarget(null)} />}
    {dialog && <ConnectionDialog key={dialog === 'new' ? 'new' : dialog.id} current={dialog === 'new' ? undefined : dialog} close={() => setDialog(null)} saved={saved} />}
    {pendingSshKey && <div className="modal-backdrop" role="presentation"><section className="confirm-dialog ssh-trust-dialog" role="alertdialog" aria-modal="true" aria-labelledby="ssh-trust-title"><div className="confirm-icon"><ShieldCheck size={18} /></div><h2 id="ssh-trust-title">{pendingSshKey.changed ? 'Cambió la clave del servidor SSH' : 'Verifica la clave del servidor SSH'}</h2><p>{pendingSshKey.changed ? `La clave guardada para «${pendingSshKey.connection.name}» no coincide. La conexión permanece bloqueada y DBSUAL no reemplazará la clave automáticamente.` : `Para «${pendingSshKey.connection.name}» compara esta huella con la que te proporcione tu administrador o el servidor por un canal confiable. DBSUAL no la aprobará automáticamente.`}</p><code className="ssh-fingerprint">{pendingSshKey.fingerprint}</code><details><summary>Clave pública observada</summary><code className="ssh-public-key">{pendingSshKey.publicKey}</code></details>{!pendingSshKey.changed && <label className="field-label">Escribe la huella verificada<input autoComplete="off" value={pendingSshKey.typedFingerprint} onChange={(event) => setPendingSshKey({ ...pendingSshKey, typedFingerprint: event.target.value })} placeholder="SHA256:…" /></label>}{pendingSshKey.error && <p className="confirm-error" role="alert">{pendingSshKey.error}</p>}<footer><button className="text-action" onClick={() => setPendingSshKey(null)}>Cerrar</button>{!pendingSshKey.changed && <button className="primary-action" disabled={pendingSshKey.typedFingerprint.trim() !== pendingSshKey.fingerprint} onClick={() => void approveSshKey()}><ShieldCheck size={14} /> Aprobar huella y conectar</button>}</footer></section></div>}
    {removeTarget && <div className="modal-backdrop" role="presentation"><section className="confirm-dialog" role="alertdialog" aria-modal="true" aria-labelledby="remove-title"><div className="confirm-icon"><Trash2 size={18} /></div><h2 id="remove-title">Quitar «{removeTarget.name}»</h2><p>Se quitará la configuración local y su credencial guardada. No se eliminará ninguna base de datos ni se harán cambios en el servidor.</p><footer><button className="text-action" disabled={removing} onClick={() => setRemoveTarget(null)}>Cancelar</button><button className="danger-action" disabled={removing} onClick={() => void removeConnection()}>{removing ? <LoaderCircle size={14} className="spin" /> : <Trash2 size={14} />} Quitar conexión</button></footer>{listState.error && <p className="confirm-error" role="alert">{listState.error.message}</p>}</section></div>}
    {selectedPlan && <div className="modal-backdrop" role="presentation"><section className="confirm-dialog plan-review-dialog" role="dialog" aria-modal="true" aria-labelledby="plan-review-title"><header className="dialog-heading"><div className="dialog-heading-icon"><GitBranch size={18} /></div><div><h2 id="plan-review-title">Revisión {selectedPlan.revision.revisionNumber} · {selectedPlan.projectName}</h2><p>El plan está cifrado en el almacenamiento local.</p></div><button className="icon-button subtle" aria-label="Cerrar revisión" onClick={() => setSelectedPlan(null)}><X size={17} /></button></header><div className="plan-review-meta"><span>Estado: {revisionStatusLabel(selectedPlan.revision.status)}</span><span>Hash: {selectedPlan.revision.planSha256}</span></div>{selectedPlan.revision.status === 'uncertain' && <UncertainRevisionNotice />}{selectedPlan.rowChange ? <div className="row-change-diff"><div className="row-change-target"><span>Tabla</span><strong>{selectedPlan.rowChange.tableName}</strong></div><div className="row-change-key"><span>Fila identificada por</span>{selectedPlan.rowChange.primaryKey.map((key) => <span className="row-key-value" key={key.column}><strong>{key.column}</strong><code>{displayCellValue(key.value)}</code></span>)}</div><div className="row-change-values"><div><span>Columna</span><strong>{selectedPlan.rowChange.columnName}</strong></div><div className="row-value-before"><span>Valor actual</span><code>{displayCellValue(selectedPlan.rowChange.oldValue)}</code></div><span className="row-change-arrow" aria-hidden="true">→</span><div className="row-value-after"><span>Valor propuesto</span><code>{displayCellValue(selectedPlan.rowChange.newValue)}</code></div></div></div> : <pre className="plan-review-sql">{selectedPlan.sql}</pre>}<p className="plan-review-note">{selectedPlan.revision.message === 'Crear base de datos' ? 'La creación no tiene datos previos que respaldar. Confirmar solo guarda la revisión; aplicar es una acción separada y vuelve a verificar el servidor y el nombre.' : selectedPlan.revision.message.startsWith('Restaurar punto en ') ? 'Confirmar guarda la restauración en el historial; no crea la base. Aplicar vuelve a verificar el punto y que el destino siga libre. La cobertura del respaldo es parcial y se restaura a una base nueva.' : 'Confirmar guarda esta revisión en el historial; no ejecuta SQL ni modifica la base. La aplicación requiere recuperación y verificación de conflictos.'}</p>{historyError && <div className="dialog-error" role="alert"><AlertCircle size={14} /><span>{historyError}</span></div>}<footer><button className="text-action" onClick={() => setSelectedPlan(null)}>Cerrar</button>{selectedPlan.revision.status === 'draft' && <button className="primary-action" disabled={historyBusy === selectedPlan.revision.id} onClick={() => void confirmPlan()}>{historyBusy === selectedPlan.revision.id ? <LoaderCircle size={14} className="spin" /> : <Check size={14} />} Confirmar revisión</button>}{selectedPlan.revision.message === 'Crear base de datos' && selectedPlan.revision.status === 'confirmed' && <button className="primary-action" disabled={historyBusy === selectedPlan.revision.id} onClick={() => void applyCreateDatabase()}>{historyBusy === selectedPlan.revision.id ? <LoaderCircle size={14} className="spin" /> : <Check size={14} />} Aplicar creación</button>}{selectedPlan.revision.message.startsWith('Restaurar punto en ') && selectedPlan.revision.status === 'confirmed' && <button className="primary-action" disabled={historyBusy === selectedPlan.revision.id} onClick={() => void applyRecoveryRestore()}>{historyBusy === selectedPlan.revision.id ? <LoaderCircle size={14} className="spin" /> : <Check size={14} />} Aplicar restauración</button>}</footer></section></div>}
    {createDialog && <div className="modal-backdrop" role="presentation"><section className="confirm-dialog" role="dialog" aria-modal="true" aria-labelledby="create-database-title"><header className="dialog-heading"><div className="dialog-heading-icon"><Database size={18} /></div><div><h2 id="create-database-title">Preparar una base nueva</h2><p>MySQL · se guardará como borrador cifrado</p></div><button className="icon-button subtle" aria-label="Cerrar" disabled={createBusy} onClick={() => setCreateDialog(false)}><X size={17} /></button></header><label className="field-label">Nombre<input autoFocus value={createTarget.name} maxLength={64} onChange={(event) => setCreateTarget((value) => ({ ...value, name: event.target.value }))} placeholder="mi_base" /></label><p className="plan-review-note">Usa letras, números, _ o $. El servidor y la base se volverán a verificar al aplicar. La base no se crea al preparar ni al confirmar.</p>{historyError && <div className="dialog-error" role="alert"><AlertCircle size={14} /><span>{historyError}</span></div>}<footer><button className="text-action" disabled={createBusy} onClick={() => setCreateDialog(false)}>Cancelar</button><button className="primary-action" disabled={createBusy || !createTarget.name.trim()} onClick={() => void prepareDatabase()}>{createBusy ? <LoaderCircle size={14} className="spin" /> : <GitBranch size={14} />} Preparar revisión</button></footer></section></div>}
    {restoreTarget && <div className="modal-backdrop" role="presentation"><section className="confirm-dialog" role="dialog" aria-modal="true" aria-labelledby="restore-database-title"><header className="dialog-heading"><div className="dialog-heading-icon"><Database size={18} /></div><div><h2 id="restore-database-title">Preparar restauración</h2><p>{restoreTarget.project.databaseName} · {restoreTarget.project.engine === 'mariadb' ? 'MariaDB' : 'MySQL'}</p></div><button className="icon-button subtle" aria-label="Cerrar restauración" onClick={() => setRestoreTarget(null)}><X size={17} /></button></header><label className="field-label">Nombre para la base nueva<input autoFocus value={restoreName} maxLength={64} onChange={(event) => setRestoreName(event.target.value)} /></label><p className="plan-review-note">Se creará una base nueva; las bases existentes no se reemplazan. Este respaldo incluye tablas, vistas y triggers simples visibles. No incluye rutinas ni eventos; las bases con referencias entre esquemas, triggers complejos o vistas no compatibles se rechazan. DBSUAL vuelve a autenticar el respaldo al aplicar; preparar y confirmar no crean la base.</p>{historyError && <div className="dialog-error" role="alert"><AlertCircle size={14} /><span>{historyError}</span></div>}<footer><button className="text-action" disabled={historyBusy === restoreTarget.point.id} onClick={() => setRestoreTarget(null)}>Cancelar</button><button className="primary-action" disabled={historyBusy === restoreTarget.point.id || !restoreName.trim()} onClick={() => void restoreRecoveryPoint()}>{historyBusy === restoreTarget.point.id ? <LoaderCircle size={14} className="spin" /> : <GitBranch size={14} />} Preparar revisión</button></footer></section></div>}
    {csvTarget && <div className="modal-backdrop" role="presentation"><section className="confirm-dialog csv-export-dialog" role="dialog" aria-modal="true" aria-labelledby="csv-export-title"><header className="dialog-heading"><div className="dialog-heading-icon"><Download size={18} /></div><div><h2 id="csv-export-title">Exportar «{csvTarget.table}»</h2><p>{csvTarget.database} · CSV UTF-8</p></div><button className="icon-button subtle" aria-label="Cerrar exportación" disabled={csvBusy} onClick={() => { setCsvTarget(null); setCsvError('') }}><X size={17} /></button></header><label className="field-label">Separador<select value={csvDelimiter} onChange={(event) => setCsvDelimiter(event.target.value as typeof csvDelimiter)}><option value="comma">Coma (,)</option><option value="semicolon">Punto y coma (;)</option><option value="tab">Tabulación</option></select></label><label className="field-label">Representación de valores NULL<input value={csvNullMarker} maxLength={128} onChange={(event) => setCsvNullMarker(event.target.value)} /></label><p className="plan-review-note">Se exportarán todas las filas visibles. Los datos binarios se escribirán como hexadecimal 0x… El archivo se publicará solo cuando termine la lectura; una ruta existente no se sobrescribe.</p>{csvError && <div className="dialog-error" role="alert"><AlertCircle size={14} /><span>{csvError}</span></div>}<footer><button className="text-action" disabled={csvBusy} onClick={() => setCsvTarget(null)}>Cancelar</button><button className="primary-action" disabled={csvBusy} onClick={() => void exportCsv()}>{csvBusy ? <LoaderCircle size={14} className="spin" /> : <Download size={14} />} Elegir destino y exportar</button></footer></section></div>}
  </div>
}

function ObjectTree({ connectionId, database, item, expanded, toggle, columns, loading, reload, onExport, onViewData, onViewStructure }: { connectionId: string; database: string; item: DatabaseObject; expanded: Set<string>; toggle: (id: string, db: string, item: DatabaseObject) => void; columns: Record<string, NodeValue<ColumnMetadata[]>>; loading: Set<string>; reload: (id: string, db: string, item: DatabaseObject) => Promise<void>; onExport?: () => void; onViewData?: () => void; onViewStructure?: () => void }) {
  const key = `col:${connectionId}:${database}:${item.schema ?? ''}:${item.kind}:${item.name}`; const opened = expanded.has(key); const result = columns[key]; const isLoading = loading.has(key)
  return <div className="object-node"><div className="tree-object-row column-parent" onContextMenu={(event) => openContextMenu(event, [
    ...(item.kind === 'table' && onViewData ? [
      { label: 'Ver datos', action: () => onViewData?.() },
      ...(onViewStructure ? [{ label: 'Ver estructura', action: () => onViewStructure() }] : []),
      ...(onExport ? [{ label: 'Exportar como CSV…', action: () => onExport() }] : []),
    ] : []),
    { label: opened ? 'Ocultar columnas' : 'Ver columnas', action: () => toggle(connectionId, database, item) },
    { label: 'Actualizar estructura', action: () => void reload(connectionId, database, item) },
  ])}><button className="node-disclosure" aria-expanded={opened} aria-label={`${opened ? 'Contraer' : 'Expandir'} ${item.kind} ${item.name}`} onClick={() => toggle(connectionId, database, item)}>{opened ? <ChevronDown size={12} /> : <ChevronRight size={12} />}</button>{item.kind === 'table' ? <Table2 size={12} className="table-icon" /> : <Eye size={12} className="view-icon" />}<button className="tree-node-label" type="button" title={item.name} onClick={item.kind === 'table' ? onViewData : undefined}>{item.name}</button>{item.kind === 'table' && onViewData && (onViewStructure || onExport) && <button className="icon-button tiny refresh-node" aria-label={`Más opciones para ${item.name}`} title="Más opciones" onClick={(event) => openContextMenu(event, [...(onViewStructure ? [{ label: 'Ver estructura', action: () => onViewStructure() }] : []), ...(onExport ? [{ label: 'Exportar como CSV…', action: () => onExport() }] : [])])}><MoreHorizontal size={12} /></button>}{opened && <button className="icon-button tiny refresh-node" aria-label="Actualizar columnas" onClick={() => void reload(connectionId, database, item)}><RefreshCw size={10} /></button>}</div>
    {opened && <div className="column-list">{isLoading && <div className="node-state"><LoaderCircle size={11} className="spin" /> Cargando columnas…</div>}{result?.error && <NodeError error={result.error} retry={() => void reload(connectionId, database, item)} />}{!isLoading && result && !result.error && result.value.length === 0 && <div className="node-empty">Sin columnas visibles.</div>}{result?.value.map((column) => { const defaultText = column.defaultAvailable ? (column.defaultValue === null ? 'Sin valor predeterminado' : `Predeterminado: ${column.defaultValue}`) : 'Valor predeterminado no disponible'; const primaryText = column.primaryKeyAvailable ? (column.isPrimaryKey ? 'Clave primaria' : 'No es clave primaria') : 'Metadato de clave primaria no disponible'; return <div className="column-row" key={column.name} title={`${column.name} · ${column.dataType}${column.isNullable ? ' · permite NULL' : ' · no permite NULL'} · ${primaryText} · ${defaultText}`}><span className="column-bullet" /><span className="column-name">{column.name}</span>{column.primaryKeyAvailable && column.isPrimaryKey && <span className="pk-tag" title="Clave primaria">PK</span>}{!column.primaryKeyAvailable && <span className="pk-tag unknown" title="Metadato de clave primaria no disponible">?</span>}<span className="column-type">{column.dataType}</span><span className={`null-tag ${column.isNullable ? 'yes' : ''}`}>{column.isNullable ? 'NULL' : 'NOT NULL'}</span></div> })}</div>}
  </div>
}
