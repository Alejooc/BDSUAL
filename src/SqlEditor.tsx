import { useEffect, useMemo, useRef, useState } from 'react'
import Editor, { loader, type OnMount } from '@monaco-editor/react'
import * as monaco from '../node_modules/monaco-editor/esm/vs/editor/editor.api.js'
import editorWorker from '../node_modules/monaco-editor/esm/vs/editor/editor.worker?worker'
import { AlertCircle, Check, Database, GitBranch, Play, RefreshCw, Square } from 'lucide-react'
import { ipc, type IpcError, type SavedConnection, type SqlReadResult } from './ipc'
import { useAppStore } from './store'

const runtime = self as typeof self & { MonacoEnvironment?: { getWorker: (_moduleId: string, label: string) => Worker } }
runtime.MonacoEnvironment = { getWorker: () => new editorWorker() }
loader.config({ monaco })
monaco.languages.register({ id: 'sql' })
monaco.languages.setMonarchTokensProvider('sql', {
  ignoreCase: true,
  keywords: ['select', 'from', 'where', 'and', 'or', 'not', 'null', 'is', 'in', 'like', 'between', 'order', 'by', 'group', 'having', 'limit', 'offset', 'as', 'distinct', 'join', 'inner', 'left', 'right', 'outer', 'on', 'union', 'all', 'case', 'when', 'then', 'else', 'end', 'asc', 'desc', 'true', 'false'],
  tokenizer: {
    root: [
      [/--.*$/, 'comment'],
      [/\/\*/, 'comment', '@comment'],
      [/'([^']|'')*'/, 'string'],
      [/[0-9]+(\.[0-9]+)?/, 'number'],
      [/[a-zA-Z_][\w$]*/, { cases: { '@keywords': 'keyword', '@default': 'identifier' } }],
      [/[()]/, '@brackets'],
      [/[;,\.]/, 'delimiter'],
      [/[=><!~?:+\-*%&|^\/]+/, 'operator'],
    ],
    comment: [[/[^/*]+/, 'comment'], [/\*\//, 'comment', '@pop'], [/[/*]/, 'comment']],
  },
})

function messageOf(error: unknown) {
  return error && typeof error === 'object' && 'message' in error ? String((error as IpcError).message) : 'No se pudo ejecutar la consulta.'
}
function codeOf(error: unknown) { return error && typeof error === 'object' && 'code' in error ? String((error as IpcError).code) : '' }

export default function SqlEditor() {
  const sqlFontSize = useAppStore((state) => state.preferences.sqlFontSize)
  const { update } = useAppStore()
  const [connections, setConnections] = useState<SavedConnection[]>([])
  const [connectionId, setConnectionId] = useState('')
  const [databases, setDatabases] = useState<string[]>([])
  const [database, setDatabase] = useState('')
  const [query, setQuery] = useState('SELECT 1;')
  const [result, setResult] = useState<SqlReadResult | null>(null)
  const [resultQuery, setResultQuery] = useState('')
  const [resultEditorValue, setResultEditorValue] = useState('')
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const [busy, setBusy] = useState(false)
  const [activeQueryId, setActiveQueryId] = useState('')
  const [cancelBusy, setCancelBusy] = useState(false)
  const [connecting, setConnecting] = useState(false)
  const editorRef = useRef<Parameters<OnMount>[0] | null>(null)
  const queryEpoch = useRef(0)
  const selectedConnection = useMemo(() => connections.find((item) => item.id === connectionId), [connections, connectionId])
  const runRead = (id: string, targetDatabase: string, sql: string, offset: number, queryId: string) => selectedConnection?.engine === 'postgresql'
    ? ipc.executePostgresReadQuery(id, targetDatabase, sql, offset, queryId)
    : selectedConnection?.engine === 'sqlite'
      ? ipc.executeSqliteReadQuery(id, targetDatabase, sql, offset)
      : ipc.executeReadQuery(id, targetDatabase, sql, offset, queryId)

  useEffect(() => { void ipc.listConnections().then((value) => setConnections(value)).catch((cause) => setError(messageOf(cause))) }, [])

  useEffect(() => {
    const onDisconnecting = (event: Event) => {
      const detail = (event as CustomEvent<{ connectionId?: string }>).detail
      if (!detail || detail.connectionId !== connectionId) return
      queryEpoch.current += 1
      setResult(null)
      setActiveQueryId('')
      setBusy(false)
      setError('La conexión se está cerrando. Se descartaron los resultados pendientes.')
    }
    window.addEventListener('dbsual:connection-disconnecting', onDisconnecting)
    return () => window.removeEventListener('dbsual:connection-disconnecting', onDisconnecting)
  }, [connectionId])

  const connect = async () => {
    if (!connectionId) return
    setConnecting(true); setError(''); setDatabases([]); setDatabase(''); setResult(null)
    try {
      await ipc.openConnection(connectionId)
      const names = await ipc.listDatabases(connectionId)
      setDatabases(names)
      if (names.length) setDatabase(names[0])
    } catch (cause) { setError(messageOf(cause)) } finally { setConnecting(false) }
  }

  const execute = async () => {
    if (!connectionId || !database || !query.trim()) return
    const selection = editorRef.current?.getSelection()
    const selectedText = selection && !selection.isEmpty() ? editorRef.current?.getModel()?.getValueInRange(selection) : ''
    const sql = selectedText?.trim() ? selectedText : query
    const queryId = crypto.randomUUID()
    const epoch = queryEpoch.current
    setBusy(true); setError(''); setNotice(''); setResult(null); setResultQuery(sql); setResultEditorValue(query)
    setActiveQueryId(queryId)
    try { const value = await runRead(connectionId, database, sql, 0, queryId); if (queryEpoch.current === epoch) { setResult(value); setError(''); setNotice('') } }
    catch (cause) { if (queryEpoch.current === epoch) { if (codeOf(cause) === 'QUERY_CANCELLED') setNotice('MySQL confirmó la cancelación de la consulta.'); else setError(messageOf(cause)) } }
    finally { if (queryEpoch.current === epoch) { setBusy(false); setActiveQueryId('') } }
  }

  const loadMore = async () => {
    if (!result || result.nextOffset === null || !connectionId || !database) return
    const queryId = crypto.randomUUID()
    const epoch = queryEpoch.current
    setBusy(true); setError('')
    setActiveQueryId(queryId)
    try {
      const page = await runRead(connectionId, database, resultQuery, result.nextOffset, queryId)
      if (queryEpoch.current === epoch) {
        setError(''); setNotice('')
        setResult((current) => current ? {
          ...page,
          rows: [...current.rows, ...page.rows],
          returnedRows: current.returnedRows + page.returnedRows,
          elapsedMs: current.elapsedMs + page.elapsedMs,
        } : page)
      }
    } catch (cause) { if (queryEpoch.current === epoch) { if (codeOf(cause) === 'QUERY_CANCELLED') setNotice('MySQL confirmó la cancelación de la consulta.'); else setError(messageOf(cause)) } }
    finally { if (queryEpoch.current === epoch) { setBusy(false); setActiveQueryId('') } }
  }

  const cancel = async () => {
    if (!activeQueryId) return
    setCancelBusy(true); setError('')
    try {
      await ipc.cancelReadQuery(activeQueryId)
      setNotice('MySQL recibió la solicitud. Se confirmará al detener la lectura.')
    } catch (cause) {
      if (codeOf(cause) === 'QUERY_NOT_ACTIVE') setNotice('La consulta terminó antes de recibir la cancelación.')
      else setError(messageOf(cause))
    } finally { setCancelBusy(false) }
  }

  const prepareChange = async () => {
    if (!connectionId || !database || !query.trim()) return
    setBusy(true); setError(''); setNotice('')
    try {
      const revision = await ipc.prepareSqlDraft(connectionId, database, query)
      setNotice(`Borrador ${revision.revisionNumber} cifrado y guardado en Cambios preparados.`)
      window.dispatchEvent(new Event('dbsual:history-changed'))
      update({ section: 'changes' })
    } catch (cause) { setError(messageOf(cause)) }
    finally { setBusy(false) }
  }

  return <section className="sql-workspace" aria-label="Editor SQL">
    <div className="sql-toolbar">
      <div className="sql-target"><Database size={14} /><span>Destino</span>
        <select aria-label="Conexión para la consulta" value={connectionId} onChange={(event) => { setConnectionId(event.target.value); setDatabases([]); setDatabase(''); setResult(null); setResultQuery(''); setResultEditorValue(''); setError('') }}>
          <option value="">Elegir conexión</option>
          {connections.filter((item) => item.engine === 'mysql' || item.engine === 'mariadb' || item.engine === 'postgresql' || item.engine === 'sqlite').map((item) => <option key={item.id} value={item.id}>{item.name} · {item.engine === 'mariadb' ? 'MariaDB' : item.engine === 'postgresql' ? 'PostgreSQL' : item.engine === 'sqlite' ? 'SQLite' : 'MySQL'} · {item.host}</option>)}
        </select>
        {connectionId && <button className="sql-connect" onClick={() => void connect()} disabled={connecting}>{connecting ? <><RefreshCw size={12} className="spin" /> Conectando</> : 'Conectar / actualizar'}</button>}
        <select aria-label="Base de datos de destino" value={database} disabled={!databases.length} onChange={(event) => { setDatabase(event.target.value); setResult(null); setResultQuery(''); setResultEditorValue('') }}><option value="">Elegir base</option>{databases.map((name) => <option key={name} value={name}>{name}</option>)}</select>
      </div>
      <button className="sql-prepare" onClick={() => void prepareChange()} disabled={busy || !connectionId || !database || !query.trim() || selectedConnection?.engine === 'postgresql' || selectedConnection?.engine === 'sqlite'} title={selectedConnection?.engine === 'postgresql' ? 'La preparación de cambios PostgreSQL aún no está disponible' : selectedConnection?.engine === 'sqlite' ? 'SQLite está disponible en modo de solo lectura' : 'Guardar SQL cifrado como borrador para revisión'}><GitBranch size={13} /> Preparar cambio</button>{busy && activeQueryId && selectedConnection?.engine !== 'postgresql' && selectedConnection?.engine !== 'sqlite' && <button className="sql-cancel" onClick={() => void cancel()} disabled={cancelBusy} title="Solicitar a MySQL la cancelación de esta consulta"><Square size={12} fill="currentColor" /> {cancelBusy ? 'Cancelando…' : 'Cancelar'}</button>}<button className="sql-run" onClick={() => void execute()} disabled={busy || !connectionId || !database || !query.trim()} title="Ejecutar la selección o toda la consulta"><Play size={13} fill="currentColor" /> {busy ? 'Ejecutando…' : 'Ejecutar lectura'}</button>
    </div>
    <div className="sql-context"><span>{selectedConnection ? `${selectedConnection.name}  ›  ${database || 'Selecciona una base'}` : 'Selecciona una conexión y una base de datos'}</span><span>Solo SELECT · lectura protegida</span></div>
    <div className="sql-monaco"><Editor height="100%" language="sql" theme="vs-dark" value={query} onChange={(value) => setQuery(value ?? '')} onMount={(editor) => { editorRef.current = editor }} options={{ minimap: { enabled: false }, lineNumbers: 'on', fontSize: sqlFontSize, tabSize: 2, scrollBeyondLastLine: false, wordWrap: 'on', automaticLayout: true, renderLineHighlight: 'line', padding: { top: 12 } }} /></div>
    <div className="sql-results">
      <div className="sql-results-heading"><span>RESULTADOS</span>{result && <span><Check size={12} /> {result.returnedRows} filas · {result.elapsedMs} ms{result.hasMore ? ' · hay más filas' : ''}</span>}</div>
      {error && <div className="sql-error" role="alert"><AlertCircle size={15} /><span>{error}</span></div>}
      {notice && <div className="sql-prepared-notice" role="status"><Check size={14} /><span>{notice}</span></div>}
      {!error && !result && <div className="sql-result-empty">Los resultados de la última consulta aparecerán aquí. El primer lote muestra hasta 200 filas.</div>}
      {result && <div className="sql-result-table-wrap"><table className="sql-result-table"><thead><tr>{result.columns.map((column, index) => <th key={`${column}-${index}`}>{column}</th>)}</tr></thead><tbody>{result.rows.map((row, rowIndex) => <tr key={rowIndex}>{row.map((value, cellIndex) => <td key={cellIndex} title={value ?? 'NULL'}>{value === null ? <span className="sql-null">NULL</span> : value}</td>)}</tr>)}</tbody></table>{result.nextOffset !== null && <div className="sql-more-note"><span>Se muestran {result.returnedRows} filas. La consulta vuelve a ejecutarse por tramo y los cambios concurrentes pueden mover filas entre páginas.</span><button className="sql-load-more" onClick={() => void loadMore()} disabled={busy || query.trim() !== resultEditorValue.trim()}>{busy ? 'Cargando…' : 'Cargar más filas'}</button>{query.trim() !== resultEditorValue.trim() && <small>Ejecuta la consulta actual para cargar más resultados.</small>}</div>}</div>}
    </div>
  </section>
}
