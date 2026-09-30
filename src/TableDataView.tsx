import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type KeyboardEvent as ReactKeyboardEvent } from 'react'
import { isTauri } from '@tauri-apps/api/core'
import { AlertCircle, ArrowDownUp, ArrowLeft, Check, Database, Filter, LoaderCircle, Pencil, RefreshCw, Search, X } from 'lucide-react'
import { ipc, type ColumnMetadata, type DatabaseEngine, type IpcError, type MysqlRowUpdatePreview, type SqlReadResult, type TablePageOptions } from './ipc'
import './table-data.css'
import { openContextMenu } from './ContextMenu'
import CsvImportPreview from './CsvImportPreview'

export type TableDataTarget = {
  id: string
  engine: DatabaseEngine
  connectionId: string
  database: string
  schema?: string | null
  table: string
  requestId: string
}

function messageOf(error: unknown) {
  return error && typeof error === 'object' && 'message' in error
    ? String((error as IpcError).message)
    : 'No se pudieron cargar las filas de la tabla.'
}

const MIN_COLUMN_WIDTH = 90
const MAX_COLUMN_WIDTH = 640
const columnKey = (name: string, index: number) => `${index}:${name}`
const initialColumnWidth = (name: string) => Math.min(320, Math.max(112, name.length * 8 + 48))

function ColumnResizeHandle({ name, width, resize }: { name: string; width: number; resize: (width: number) => void }) {
  const drag = useRef<{ startX: number; startWidth: number } | null>(null)
  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    event.preventDefault()
    event.stopPropagation()
    drag.current = { startX: event.clientX, startWidth: width }
    event.currentTarget.setPointerCapture(event.pointerId)
  }
  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!drag.current) return
    resize(Math.max(MIN_COLUMN_WIDTH, Math.min(MAX_COLUMN_WIDTH, drag.current.startWidth + event.clientX - drag.current.startX)))
  }
  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
      event.preventDefault()
      resize(Math.max(MIN_COLUMN_WIDTH, Math.min(MAX_COLUMN_WIDTH, width + (event.key === 'ArrowRight' ? 12 : -12))))
    } else if (event.key === 'Home' || event.key === 'End') {
      event.preventDefault()
      resize(event.key === 'Home' ? MIN_COLUMN_WIDTH : MAX_COLUMN_WIDTH)
    }
  }
  return <div className="table-data-col-resizer" role="separator" aria-orientation="vertical" aria-label={`Cambiar ancho de ${name}`} aria-valuemin={MIN_COLUMN_WIDTH} aria-valuemax={MAX_COLUMN_WIDTH} aria-valuenow={Math.round(width)} tabIndex={0} onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={() => { drag.current = null }} onPointerCancel={() => { drag.current = null }} onLostPointerCapture={() => { drag.current = null }} onKeyDown={onKeyDown} onClick={(event) => event.stopPropagation()} />
}

function GridCellValue({ value, column, expandable, expanded, toggle }: { value: string | null; column: string; expandable: boolean; expanded: boolean; toggle: () => void }) {
  if (value === null) return <span className="sql-null">NULL</span>
  if (!expandable) return <span>{value}</span>
  return <button type="button" className={`table-data-cell-value ${expanded ? 'expanded' : 'clamped'}`} aria-label={`Valor de ${column}${expanded ? ', expandido. Pulse para contraer.' : ', truncado. Pulse para ver completo.'}`} aria-expanded={expanded} title={expanded ? `Contraer ${column}` : `Ver el valor completo de ${column}`} onClick={toggle} onKeyDown={(event) => { if (event.key === 'Escape' && expanded) { event.stopPropagation(); toggle() } }}>{value}</button>
}

export default function TableDataView({ target, close, active = true }: { target: TableDataTarget; close: () => void; active?: boolean }) {
  const [result, setResult] = useState<SqlReadResult | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [sortColumn, setSortColumn] = useState('')
  const [sortDirection, setSortDirection] = useState<'asc' | 'desc'>('asc')
  const [filterColumn, setFilterColumn] = useState('')
  const [filterMode, setFilterMode] = useState<'equals' | 'contains'>('contains')
  const [filterDraft, setFilterDraft] = useState('')
  const [activeFilter, setActiveFilter] = useState<{ column: string; mode: 'equals' | 'contains'; value: string } | null>(null)
  const [columnMetadata, setColumnMetadata] = useState<ColumnMetadata[]>([])
  const [columnWidths, setColumnWidths] = useState<Record<string, number>>({})
  const [expandedCell, setExpandedCell] = useState<{ rowIndex: number; columnIndex: number } | null>(null)
  const [editing, setEditing] = useState<{ rowIndex: number; columnName: string; oldValue: string | null } | null>(null)
  const [editValue, setEditValue] = useState('')
  const [editAsNull, setEditAsNull] = useState(false)
  const [rowUpdate, setRowUpdate] = useState<MysqlRowUpdatePreview | null>(null)
  const [deferredReview, setDeferredReview] = useState<MysqlRowUpdatePreview | null>(null)
  const [appliedUpdate, setAppliedUpdate] = useState<MysqlRowUpdatePreview | null>(null)
  const [revertPrepared, setRevertPrepared] = useState(false)
  const [reviewConfirmed, setReviewConfirmed] = useState(false)
  const [updateBusy, setUpdateBusy] = useState(false)
  const [updateError, setUpdateError] = useState('')
  const [insertDraft, setInsertDraft] = useState<Record<string, string> | null>(null)
  const [insertNulls, setInsertNulls] = useState<string[]>([])
  const activeQueryId = useRef('')
  const epoch = useRef(0)
  const optionsRef = useRef<TablePageOptions>({ sortColumn: null, sortDirection: null, filterColumn: null, filterMode: null, filterValue: null })

  useEffect(() => {
    if (!result) return
    setColumnWidths((current) => {
      let changed = false
      const next = { ...current }
      result.columns.forEach((column, index) => {
        const key = columnKey(column, index)
        if (next[key] === undefined) { next[key] = initialColumnWidth(column); changed = true }
      })
      return changed ? next : current
    })
  }, [result?.columns])

  useEffect(() => { if (!active) setExpandedCell(null) }, [active])

  const load = async (offset: number, append: boolean, options = optionsRef.current) => {
    if (!append) setExpandedCell(null)
    const previousQueryId = activeQueryId.current
    if (previousQueryId && !['postgresql', 'sqlite'].includes(target.engine)) void ipc.cancelReadQuery(previousQueryId).catch(() => undefined)
    const queryId = crypto.randomUUID()
    const currentEpoch = ++epoch.current
    activeQueryId.current = queryId
    setBusy(true)
    setError('')
    try {
      const page = target.engine === 'postgresql'
        ? await ipc.readPostgresTablePage(target.connectionId, target.database, target.schema ?? '', target.table, options, offset)
        : target.engine === 'sqlite'
          ? await ipc.readSqliteTablePage(target.connectionId, target.database, target.table, options, offset)
          : await ipc.readTablePage(target.connectionId, target.database, target.table, options, offset, queryId)
      if (epoch.current !== currentEpoch) return
      if (target.engine === 'mysql' && offset === 0) {
        try {
          const metadata = await ipc.listColumns(target.connectionId, target.database, target.table, 'table', target.schema)
          if (epoch.current === currentEpoch) setColumnMetadata(metadata)
        } catch { if (epoch.current === currentEpoch) setColumnMetadata([]) }
      }
      if (!filterColumn && page.columns[0]) setFilterColumn(page.columns[0])
      setResult((current) => append && current ? {
        ...page,
        rows: [...current.rows, ...page.rows],
        returnedRows: current.returnedRows + page.returnedRows,
        elapsedMs: current.elapsedMs + page.elapsedMs,
      } : page)
    } catch (cause) {
      if (epoch.current === currentEpoch) setError(messageOf(cause))
    } finally {
      if (epoch.current === currentEpoch) {
        activeQueryId.current = ''
        setBusy(false)
      }
    }
  }

  useEffect(() => {
    optionsRef.current = { sortColumn: null, sortDirection: null, filterColumn: null, filterMode: null, filterValue: null }
    setSortColumn('')
    setSortDirection('asc')
    setFilterColumn('')
    setActiveFilter(null)
    setFilterDraft('')
    setResult(null)
    setColumnMetadata([])
    setEditing(null)
    setRowUpdate(null)
    setDeferredReview(null)
    setAppliedUpdate(null)
    setRevertPrepared(false)
    setReviewConfirmed(false)
    setUpdateError('')
    void load(0, false)
    return () => {
      epoch.current += 1
      const queryId = activeQueryId.current
      if (queryId && !['postgresql', 'sqlite'].includes(target.engine)) void ipc.cancelReadQuery(queryId).catch(() => undefined)
    }
  }, [target.requestId])

  const beginCellEdit = (rowIndex: number, columnName: string, oldValue: string | null) => {
    if (target.engine !== 'mysql' || !isTauri() || rowUpdate || deferredReview || updateBusy) return
    const metadata = columnMetadata.find((column) => column.name === columnName)
    if (!metadata || metadata.isPrimaryKey || !metadata.primaryKeyAvailable) return
    setEditing({ rowIndex, columnName, oldValue })
    setEditValue(oldValue ?? '')
    setEditAsNull(oldValue === null)
    setUpdateError('')
  }

  const prepareRowUpdate = async () => {
    if (!editing || !result || target.engine !== 'mysql') return
    const primaryColumns = columnMetadata.filter((column) => column.isPrimaryKey)
    const row = result.rows[editing.rowIndex]
    if (!primaryColumns.length || !row) {
      setUpdateError('No se pudo identificar esta fila con una clave primaria. Permanece en solo lectura.')
      return
    }
    const primaryKey = primaryColumns.map((column) => ({ column: column.name, value: row[result.columns.indexOf(column.name)] ?? null }))
    if (primaryKey.some((part) => part.value === null)) {
      setUpdateError('La clave primaria de esta fila no se pudo leer. Permanece en solo lectura.')
      return
    }
    setUpdateBusy(true)
    setUpdateError('')
    try {
      const preview = await ipc.prepareMysqlRowUpdate(target.connectionId, target.database, target.table, primaryKey, editing.columnName, editAsNull ? null : editValue)
      setRowUpdate(preview)
      setReviewConfirmed(false)
    } catch (cause) {
      setUpdateError(messageOf(cause))
    } finally { setUpdateBusy(false) }
  }

  const beginInsertRow = () => {
    if (target.engine !== 'mysql' || !isTauri() || !result || rowUpdate || updateBusy) return
    setInsertDraft(Object.fromEntries(result.columns.map((column) => [column, ''])))
    setInsertNulls([])
    setUpdateError('')
  }

  const prepareRowInsert = async () => {
    if (!insertDraft) return
    setUpdateBusy(true); setUpdateError('')
    try {
      const values = Object.entries(insertDraft).map(([column, value]) => ({ column, value: insertNulls.includes(column) ? null : value }))
      const preview = await ipc.prepareMysqlRowInsert(target.connectionId, target.database, target.table, values)
      setInsertDraft(null); setRowUpdate(preview); setReviewConfirmed(false)
    } catch (cause) { setUpdateError(messageOf(cause)) }
    finally { setUpdateBusy(false) }
  }

  const confirmRowUpdate = async () => {
    if (!rowUpdate) return
    setUpdateBusy(true)
    setUpdateError('')
    try {
      await ipc.confirmHistoryRevision(rowUpdate.revision.id)
      setReviewConfirmed(true)
    } catch (cause) { setUpdateError(messageOf(cause)) }
    finally { setUpdateBusy(false) }
  }

  const applyRowUpdate = async () => {
    if (!rowUpdate || !reviewConfirmed) return
    setUpdateBusy(true)
    setUpdateError('')
    try {
      const applied = rowUpdate
      await ipc.applyMysqlRowUpdate(rowUpdate.revision.id)
      setRowUpdate(null)
      setDeferredReview(null)
      setReviewConfirmed(false)
      setEditing(null)
      setAppliedUpdate(applied)
      setRevertPrepared(false)
      await load(0, false)
    } catch (cause) { setUpdateError(messageOf(cause)) }
    finally { setUpdateBusy(false) }
  }

  const prepareRowRevert = async () => {
    if (!appliedUpdate || updateBusy || rowUpdate || deferredReview) return
    setUpdateBusy(true)
    setUpdateError('')
    try {
      const compensatingReview = await ipc.prepareMysqlRowRevert(appliedUpdate.revision.id)
      setRowUpdate(compensatingReview)
      setReviewConfirmed(false)
      setRevertPrepared(true)
    } catch (cause) { setUpdateError(messageOf(cause)) }
    finally { setUpdateBusy(false) }
  }

  const closeReview = () => {
    // Preparing or confirming changes only the local history. Keep the review reachable in this grid.
    if (rowUpdate) setDeferredReview(rowUpdate)
    setRowUpdate(null)
    setReviewConfirmed(false)
    setEditing(null)
    setUpdateError('')
  }

  useEffect(() => {
    const onDisconnecting = (event: Event) => {
      const detail = (event as CustomEvent<{ connectionId?: string }>).detail
      if (detail?.connectionId !== target.connectionId) return
      epoch.current += 1
      activeQueryId.current = ''
      setResult(null)
      setBusy(false)
      setError('La conexión se está cerrando. Se limpiaron las filas de esta tabla.')
    }
    window.addEventListener('dbsual:connection-disconnecting', onDisconnecting)
    return () => window.removeEventListener('dbsual:connection-disconnecting', onDisconnecting)
  }, [target.connectionId])

  return <section className="table-data-workspace" aria-label={`Datos de ${target.table}`}>
    <header className="table-data-header">
      <div className="table-data-title"><Database size={15} /><span><strong>{target.table}</strong><small>{target.database}{target.schema ? ` · ${target.schema}` : ''} · {target.engine === 'mysql' ? 'Edición protegida disponible para filas identificables' : 'Solo lectura'}</small></span></div>
      <div className="table-data-actions">
        {target.engine === 'mysql' && isTauri() && <button className="secondary-action" onClick={beginInsertRow} disabled={busy || !result || !columnMetadata.some((column) => column.isPrimaryKey)}>Añadir fila</button>}
        {target.engine === 'mysql' && <CsvImportPreview table={target.table} columns={result?.columns ?? []} disabled={busy || !result} />}
        <button className="icon-button subtle" onClick={() => void load(0, false)} disabled={busy} title="Actualizar filas" aria-label="Actualizar filas"><RefreshCw size={14} /></button>
        <button className="icon-button subtle" onClick={close} title="Cerrar tabla" aria-label="Cerrar tabla"><ArrowLeft size={15} /></button>
      </div>
    </header>
    <div className="table-data-notice">Las filas se cargan desde la base de datos en tramos. Puedes ordenar y filtrar el conjunto completo. Cada cambio MySQL pasa por preparar, revisar, confirmar y aplicar en el historial.</div>
    {target.engine === 'mysql' && !isTauri() && <div className="table-data-notice" role="status">La edición protegida y el historial requieren el núcleo de DBSUAL; en la vista previa solo puedes consultar.</div>}
    {error && <div className="sql-error" role="alert"><AlertCircle size={15} /><span>{error}</span></div>}
    {busy && !result && <div className="table-data-empty"><LoaderCircle size={15} className="spin" /> Cargando datos…</div>}
    {!busy && !error && !result && <div className="table-data-empty">No hay filas para mostrar.</div>}
    {result && <>
      <div className="table-data-count">{result.returnedRows} filas cargadas{result.hasMore ? ' · hay más filas' : ''}{sortColumn ? ` · orden ${sortColumn} ${sortDirection === 'asc' ? 'ascendente' : 'descendente'}` : ''}{activeFilter ? ` · filtro en ${activeFilter.column}` : ''} · {result.elapsedMs} ms</div>
      {result.columns.length > 0 && <form className="table-data-filter-bar" onSubmit={(event) => {
        event.preventDefault()
        if (!filterColumn) return
        const nextFilter = { column: filterColumn, mode: filterMode, value: filterDraft }
        const options: TablePageOptions = { ...optionsRef.current, filterColumn: nextFilter.column, filterMode: nextFilter.mode, filterValue: nextFilter.value }
        optionsRef.current = options
        setActiveFilter(nextFilter)
        void load(0, false, options)
      }}>
        <Filter size={13} />
        <select aria-label="Columna para filtrar" value={filterColumn || result.columns[0]} onChange={(event) => setFilterColumn(event.target.value)}>
          {result.columns.map((column) => <option key={column} value={column}>{column}</option>)}
        </select>
        <select aria-label="Tipo de filtro" value={filterMode} onChange={(event) => setFilterMode(event.target.value as 'equals' | 'contains')}>
          <option value="contains">contiene</option><option value="equals">es igual a</option>
        </select>
        <input aria-label="Valor del filtro" value={filterDraft} onChange={(event) => setFilterDraft(event.target.value)} placeholder="Filtrar filas…" maxLength={4096} />
        <button className="icon-button tiny" type="submit" disabled={busy || !filterColumn}><Search size={13} /> Aplicar</button>
        {activeFilter && <button className="icon-button tiny" type="button" disabled={busy} onClick={() => {
          const options: TablePageOptions = { ...optionsRef.current, filterColumn: null, filterMode: null, filterValue: null }
          optionsRef.current = options
          setActiveFilter(null)
          setFilterDraft('')
          void load(0, false, options)
        }}><X size={13} /> Limpiar filtro</button>}
      </form>}
      <div className="sql-result-table-wrap table-data-grid" aria-busy={busy} onContextMenu={(event) => openContextMenu(event, [{ label: 'Actualizar filas', disabled: busy, action: () => void load(0, false) }, { label: 'Cerrar pestaña de datos', action: close }])}><table className="sql-result-table table-data-result-table"><colgroup>{result.columns.map((column, index) => <col key={columnKey(column, index)} style={{ width: columnWidths[columnKey(column, index)] ?? initialColumnWidth(column) }} />)}</colgroup><thead><tr>{result.columns.map((column, index) => <th key={`${column}-${index}`}><button className="table-data-sort" type="button" onClick={() => {
        const direction = sortColumn === column ? (sortDirection === 'asc' ? 'desc' : 'asc') : 'asc'
        const options: TablePageOptions = { ...optionsRef.current, sortColumn: column, sortDirection: direction }
        optionsRef.current = options
        setSortColumn(column)
        setSortDirection(direction)
        void load(0, false, options)
      }} aria-label={`Ordenar por ${column}`} title={`Ordenar por ${column}`}><span>{column}</span><ArrowDownUp size={12} />{sortColumn === column && <small>{sortDirection === 'asc' ? '↑' : '↓'}</small>}</button><ColumnResizeHandle name={column} width={columnWidths[columnKey(column, index)] ?? initialColumnWidth(column)} resize={(width) => setColumnWidths((current) => ({ ...current, [columnKey(column, index)]: width }))} /></th>)}</tr></thead><tbody>{result.rows.map((row, rowIndex) => <tr key={rowIndex}>{row.map((value, cellIndex) => {
        const column = result.columns[cellIndex]
        const primaryKey = columnMetadata.some((meta) => meta.name === column && meta.isPrimaryKey)
        const canEdit = target.engine === 'mysql' && isTauri() && !rowUpdate && !deferredReview && !primaryKey && columnMetadata.some((meta) => meta.name === column && meta.primaryKeyAvailable) && Boolean(columnMetadata.some((meta) => meta.isPrimaryKey))
        const expanded = expandedCell?.rowIndex === rowIndex && expandedCell.columnIndex === cellIndex
        const cellWidth = columnWidths[columnKey(column, cellIndex)] ?? initialColumnWidth(column)
        const expandable = value !== null && (value.includes('\n') || value.length > Math.max(36, Math.floor((cellWidth - 24) / 7) * 3))
        return <td key={cellIndex} title={value ?? 'NULL'}>{editing?.rowIndex === rowIndex && editing.columnName === column ? <div className="table-data-cell-edit"><input aria-label={`Nuevo valor para ${column}`} value={editValue} disabled={editAsNull || updateBusy} onChange={(event) => setEditValue(event.target.value)} onKeyDown={(event) => { if (event.key === 'Escape') setEditing(null); if (event.key === 'Enter') void prepareRowUpdate() }} /><label><input type="checkbox" checked={editAsNull} disabled={updateBusy} onChange={(event) => setEditAsNull(event.target.checked)} /> NULL</label><button type="button" aria-label={`Revisar cambio de ${column}`} disabled={updateBusy} onClick={() => void prepareRowUpdate()}><Check size={12} /></button><button type="button" aria-label="Cancelar edición" disabled={updateBusy} onClick={() => setEditing(null)}><X size={12} /></button></div> : <><GridCellValue value={value} column={column} expandable={expandable} expanded={expanded} toggle={() => setExpandedCell(expanded ? null : { rowIndex, columnIndex: cellIndex })} />{canEdit && <button type="button" className="table-data-cell-edit-button" aria-label={`Editar ${column}, fila ${rowIndex + 1}`} title={`Editar ${column}`} onClick={() => beginCellEdit(rowIndex, column, value)}><Pencil size={11} /></button>}</>}</td>
      })}</tr>)}</tbody></table>{busy && <div className="table-data-loading-overlay" role="status"><LoaderCircle size={15} className="spin" /> Cargando datos…</div>}</div>
      {result.nextOffset !== null && <div className="table-data-footer"><span>Se muestran filas por páginas. Las escrituras concurrentes pueden mover filas entre páginas.</span><button className="sql-load-more" onClick={() => void load(result.nextOffset!, true)} disabled={busy}>{busy ? 'Cargando…' : 'Cargar más filas'}</button></div>}
    </>}
    {updateError && <div className="sql-error table-data-edit-error" role="alert"><AlertCircle size={15} /><span>{updateError}</span></div>}
    {deferredReview && !rowUpdate && <div className="table-data-applied" role="status"><span>{deferredReview.revision.status === 'confirmed' ? 'La revisión confirmada sigue pendiente; no se descartó ni aplicó.' : 'El borrador sigue guardado en el historial; no se descartó.'}</span><button type="button" className="secondary-action" onClick={() => { setRowUpdate(deferredReview); setReviewConfirmed(deferredReview.revision.status === 'confirmed'); setDeferredReview(null) }}>Reabrir revisión</button></div>}
    {appliedUpdate && !rowUpdate && !deferredReview && <div className="table-data-applied" role="status"><span>{revertPrepared ? 'La revisión compensatoria quedó guardada en el historial. No se modificó la base al prepararla.' : `Revisión ${appliedUpdate.revision.revisionNumber} aplicada. Puedes preparar una revisión compensatoria${appliedUpdate.operation === 'delete' ? ' adicional' : ''}.`}</span>{!revertPrepared && appliedUpdate.operation !== 'delete' && <button type="button" className="secondary-action" disabled={updateBusy} onClick={() => void prepareRowRevert()}>{updateBusy ? 'Preparando…' : 'Preparar reversión'}</button>}</div>}
    {insertDraft && <div className="table-data-review-backdrop"><section className="table-data-review" role="dialog" aria-modal="true" aria-labelledby="row-insert-title"><header><div><h2 id="row-insert-title">Preparar fila nueva</h2><p>{target.database}.{target.table} · aún no se ha modificado la base</p></div><button className="icon-button subtle" aria-label="Cerrar inserción" onClick={() => setInsertDraft(null)}><X size={16} /></button></header><p className="table-data-review-copy">Indica un valor para cada columna, incluida la clave primaria. Marca NULL cuando corresponda. La inserción solo está disponible para tablas InnoDB sin triggers ni claves foráneas entrantes.</p><div className="row-insert-fields">{Object.entries(insertDraft).map(([column, value]) => <label key={column}><span>{column}{columnMetadata.find((entry) => entry.name === column)?.isPrimaryKey ? ' · clave primaria' : ''}</span><input aria-label={`Valor para ${column}`} value={value} disabled={insertNulls.includes(column) || updateBusy} onChange={(event) => setInsertDraft((current) => current ? { ...current, [column]: event.target.value } : current)} /><span className="row-insert-options"><input type="checkbox" aria-label={`Valor NULL para ${column}`} checked={insertNulls.includes(column)} onChange={(event) => setInsertNulls((current) => event.target.checked ? [...current, column] : current.filter((item) => item !== column))} /> NULL</span></label>)}</div>{updateError && <div className="sql-error" role="alert"><AlertCircle size={15} /><span>{updateError}</span></div>}<footer><button className="secondary-action" onClick={() => setInsertDraft(null)}>Cancelar</button><button className="primary-action" disabled={updateBusy} onClick={() => void prepareRowInsert()}>{updateBusy ? 'Preparando…' : 'Revisar fila'}</button></footer></section></div>}
    {rowUpdate && <div className="table-data-review-backdrop"><section className="table-data-review" role="dialog" aria-modal="true" aria-labelledby="row-update-title">
      <header><div><h2 id="row-update-title">{rowUpdate.operation === 'insert' ? 'Revisar fila nueva' : rowUpdate.operation === 'delete' ? 'Revisar reversión de inserción' : rowUpdate.revision.message === 'Revertir edición de fila' ? 'Revisar reversión compensatoria' : 'Revisar cambio de fila'}</h2><p>Revisión {rowUpdate.revision.revisionNumber} · {target.database}.{target.table}</p></div><button className="icon-button subtle" aria-label="Cerrar revisión" disabled={updateBusy} onClick={closeReview}><X size={16} /></button></header>
      <p className="table-data-review-copy">{rowUpdate.operation === 'insert' ? 'La preparación y la confirmación solo guardan la fila en el historial. La base no se modifica hasta que elijas Aplicar fila.' : rowUpdate.operation === 'delete' ? 'Esta nueva revisión propone quitar la fila insertada, solo si conserva todos sus valores y no hay efectos externos. Confirmar no la elimina.' : rowUpdate.revision.message === 'Revertir edición de fila' ? 'Esto prepara una nueva revisión con el valor anterior. No cambia la base ni elimina la revisión original. Confirma y aplica esta revisión por separado.' : <>La preparación y la confirmación solo guardan este cambio en el historial. La base no se modifica hasta que elijas <strong>Aplicar cambio</strong>.</>}</p>
      {rowUpdate.operation === 'insert' || rowUpdate.operation === 'delete' ? <dl><dt>Clave</dt><dd>{rowUpdate.primaryKey.map((key) => `${key.column} = ${key.value ?? 'NULL'}`).join(', ')}</dd><dt>Valores</dt><dd>{(rowUpdate.values ?? []).map((value) => `${value.column} = ${value.value ?? 'NULL'}`).join(' · ')}</dd></dl> : <dl><dt>Fila</dt><dd>{rowUpdate.primaryKey.map((key) => `${key.column} = ${key.value ?? 'NULL'}`).join(', ')}</dd><dt>Columna</dt><dd>{rowUpdate.columnName}</dd><dt>Valor actual</dt><dd>{rowUpdate.oldValue === null ? <span className="sql-null">NULL</span> : rowUpdate.oldValue}</dd><dt>Valor propuesto</dt><dd>{rowUpdate.newValue === null ? <span className="sql-null">NULL</span> : rowUpdate.newValue}</dd></dl>}
      {reviewConfirmed && <div className="table-data-confirmed" role="status">Revisión confirmada y guardada en el historial. Aún no se ha aplicado.</div>}
      {updateError && <div className="sql-error" role="alert"><AlertCircle size={15} /><span>{updateError}</span></div>}
      <footer>{!reviewConfirmed ? <><button className="secondary-action" disabled={updateBusy} onClick={closeReview}>Cerrar · conservar borrador</button><button className="primary-action" disabled={updateBusy} onClick={() => void confirmRowUpdate()}>{updateBusy ? 'Guardando…' : 'Confirmar revisión'}</button></> : <><button className="secondary-action" disabled={updateBusy} onClick={closeReview}>Seguir después · volver a abrir</button><button className="primary-action" disabled={updateBusy} onClick={() => void applyRowUpdate()}>{updateBusy ? 'Aplicando…' : rowUpdate.operation === 'insert' ? 'Aplicar fila' : rowUpdate.operation === 'delete' ? 'Aplicar reversión' : rowUpdate.revision.message === 'Revertir edición de fila' ? 'Aplicar reversión' : 'Aplicar cambio'}</button></>}</footer>
    </section></div>}
  </section>
}
