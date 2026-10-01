import { useMemo, useState } from 'react'
import { AlertCircle, Check, FileUp, LoaderCircle, LockKeyhole, X } from 'lucide-react'
import { isTauri } from '@tauri-apps/api/core'
import { ipc, type IpcError, type MysqlCsvImportPreview } from './ipc'
import './csv-import-preview.css'

type Delimiter = ',' | ';' | '\t'
type Preview = { headers: string[]; rows: string[][]; errors: string[] }
type PreviewCell = { kind: 'null' } | { kind: 'text'; value: string }
const MAX_BYTES = 5 * 1024 * 1024
const MAX_ROWS = 50_000
// Se habilita al completar el recorrido de escritura por IPC nativo en Windows.
const MYSQL_CSV_IMPORT_ENABLED = false

function parseCsv(text: string, delimiter: Delimiter): Preview {
  const rows: string[][] = []
  let row: string[] = []
  let cell = ''
  let quoted = false
  for (let i = 0; i < text.length; i++) {
    const char = text[i]
    if (quoted) {
      if (char === '"' && text[i + 1] === '"') { cell += '"'; i++ }
      else if (char === '"') quoted = false
      else cell += char
    } else if (char === '"' && cell.length === 0) quoted = true
    else if (char === delimiter) { row.push(cell); cell = '' }
    else if (char === '\n' || char === '\r') {
      if (char === '\r' && text[i + 1] === '\n') i++
      row.push(cell); cell = ''
      if (row.some((value) => value.length > 0)) rows.push(row)
      row = []
    } else cell += char
  }
  if (quoted) return { headers: [], rows: [], errors: ['Hay un campo entre comillas sin cerrar.'] }
  row.push(cell)
  if (row.some((value) => value.length > 0)) rows.push(row)
  if (!rows.length) return { headers: [], rows: [], errors: ['El archivo no contiene encabezados ni filas.'] }
  const [headers, ...data] = rows
  const errors: string[] = []
  if (headers.length > 512 || headers.some((value) => !value.trim() || value.trim() !== value)) errors.push('Los encabezados deben tener nombre, sin espacios al inicio/final y máximo 512 columnas.')
  if (new Set(headers.map((value) => value.toLocaleLowerCase())).size !== headers.length) errors.push('Hay encabezados repetidos; cada columna debe ser única.')
  if (!data.length) errors.push('El archivo no contiene filas de datos.')
  if (data.length > MAX_ROWS) errors.push(`El archivo supera el límite de ${MAX_ROWS.toLocaleString()} filas.`)
  const wrongWidth = data.findIndex((item) => item.length !== headers.length)
  if (wrongWidth >= 0) errors.push(`La fila ${wrongWidth + 2} tiene ${data[wrongWidth].length} campos; se esperaban ${headers.length}.`)
  return { headers, rows: data, errors }
}

export default function CsvImportPreview({ connectionId, database, table, columns, disabled, onApplied }: { connectionId: string; database: string; table: string; columns: string[]; disabled: boolean; onApplied: () => void }) {
  const [open, setOpen] = useState(false)
  const [delimiter, setDelimiter] = useState<Delimiter>(',')
  const [nullMarker, setNullMarker] = useState('\\N')
  const [fileName, setFileName] = useState('')
  const [content, setContent] = useState('')
  const [error, setError] = useState('')
  const [operationError, setOperationError] = useState('')
  const [busy, setBusy] = useState(false)
  const [prepared, setPrepared] = useState<MysqlCsvImportPreview | null>(null)
  const [confirmed, setConfirmed] = useState(false)
  const [applied, setApplied] = useState(false)
  const [reverted, setReverted] = useState(false)
  const [revertPrepared, setRevertPrepared] = useState<MysqlCsvImportPreview | null>(null)
  const [revertConfirmed, setRevertConfirmed] = useState(false)
  const preview = useMemo(() => content ? parseCsv(content, delimiter) : null, [content, delimiter])
  const columnError = preview && columns.length
    ? preview.headers.filter((header) => !columns.some((column) => column === header)).concat(columns.filter((column) => !preview.headers.includes(column)))
    : []
  const decodedRows = preview?.rows.slice(0, 5).map((row) => row.map((value): PreviewCell => {
    if (value === nullMarker) return { kind: 'null' }
    if (value.startsWith('\\\\') || (nullMarker && value === `\\${nullMarker}`)) return { kind: 'text', value: value.slice(1) }
    return { kind: 'text', value }
  }))

  const selectFile = async (file?: File) => {
    setError(''); setOperationError(''); setContent(''); setFileName(''); setPrepared(null); setConfirmed(false); setApplied(false); setReverted(false); setRevertPrepared(null); setRevertConfirmed(false)
    if (!file) return
    if (file.size > MAX_BYTES) { setError('El archivo supera el límite de 5 MiB.'); return }
    try {
      const text = new TextDecoder('utf-8', { fatal: true }).decode(await file.arrayBuffer())
      setContent(text); setFileName(file.name)
    } catch { setError('El archivo no es UTF-8 válido o no se pudo leer.') }
  }

  const run = async (action: () => Promise<void>) => {
    setBusy(true); setOperationError('')
    try { await action() } catch (caught) { setOperationError(caught && typeof caught === 'object' && 'message' in caught ? String((caught as IpcError).message) : 'No se pudo completar la operación.') }
    finally { setBusy(false) }
  }
  const prepareImport = () => run(async () => {
    if (!content || !preview || preview.errors.length || columnError.length || !isTauri()) throw new Error('La importación requiere el núcleo de escritorio y un CSV válido.')
    const preparedPlan = await ipc.prepareMysqlCsvImport(connectionId, database, table, content, delimiter === ',' ? 'comma' : delimiter === ';' ? 'semicolon' : 'tab', nullMarker)
    setPrepared(preparedPlan)
  })
  const confirmImport = () => run(async () => { if (!prepared) return; await ipc.confirmHistoryRevision(prepared.revision.id); setConfirmed(true) })
  const applyImport = () => run(async () => { if (!prepared || !confirmed) return; await ipc.applyMysqlRowUpdate(prepared.revision.id); setApplied(true); onApplied() })
  const prepareRevert = () => run(async () => { if (!prepared || !applied) return; setRevertPrepared(await ipc.prepareMysqlCsvImportRevert(prepared.revision.id)) })
  const confirmRevert = () => run(async () => { if (!revertPrepared) return; await ipc.confirmHistoryRevision(revertPrepared.revision.id); setRevertConfirmed(true) })
  const applyRevert = () => run(async () => { if (!revertPrepared || !revertConfirmed) return; await ipc.applyMysqlRowUpdate(revertPrepared.revision.id); setApplied(false); setReverted(true); onApplied() })

  return <>
    <button type="button" className="secondary-action csv-import-open" disabled={disabled} onClick={() => setOpen(true)}><FileUp size={13} /> Vista previa CSV</button>
    {open && <div className="csv-import-backdrop"><section className="csv-import-dialog" role="dialog" aria-modal="true" aria-labelledby="csv-import-title">
      <header><div><h2 id="csv-import-title">Vista previa de importación CSV</h2><p>{table} · solo vista previa</p></div><button className="icon-button subtle" aria-label="Cerrar vista previa CSV" onClick={() => setOpen(false)}><X size={16} /></button></header>
      <label className="csv-import-field">Archivo CSV<input aria-label="Archivo CSV" type="file" accept=".csv,text/csv" disabled={busy || Boolean(prepared)} onChange={(event) => void selectFile(event.currentTarget.files?.[0])} /></label>
      <div className="csv-import-options"><label>Separador<select aria-label="Separador CSV" value={delimiter} disabled={busy || Boolean(prepared)} onChange={(event) => setDelimiter(event.target.value as Delimiter)}><option value=",">Coma (,)</option><option value=";">Punto y coma (;)</option><option value={"\t"}>Tabulación</option></select></label><label>Marcador NULL<input aria-label="Marcador NULL" value={nullMarker} maxLength={128} disabled={busy || Boolean(prepared)} onChange={(event) => setNullMarker(event.target.value)} /></label></div>
      {fileName && <p className="csv-import-file">Archivo: {fileName}</p>}
      {error && <div className="csv-import-error" role="alert"><AlertCircle size={14} />{error}</div>}
      {preview && <div className="csv-import-results">
        {preview.errors.map((item) => <div className="csv-import-error" role="alert" key={item}><AlertCircle size={14} />{item}</div>)}
        {columnError.length > 0 && <div className="csv-import-error" role="alert">Columnas que no existen en «{table}»: {columnError.join(', ')}.</div>}
        {preview.errors.length === 0 && columnError.length === 0 && <p role="status">{preview.rows.length} filas válidas · {preview.headers.length} columnas reconocidas. NULL se mostrará como tal.</p>}
        <div className="csv-import-table-wrap"><table><thead><tr>{preview.headers.map((header, index) => <th key={`${header}-${index}`}>{header}</th>)}</tr></thead><tbody>{decodedRows?.map((row, rowIndex) => <tr key={rowIndex}>{row.map((cell, colIndex) => <td key={colIndex}>{cell.kind === 'null' ? <em>NULL</em> : cell.value}</td>)}</tr>)}</tbody></table></div>
      </div>}
      {operationError && <div className="csv-import-error" role="alert"><AlertCircle size={14} />{operationError}</div>}
      {prepared && <div className="csv-import-safety"><Check size={15} /><span>Revisión {prepared.revision.revisionNumber} preparada · {prepared.rowCount} filas. Preparar no modifica la base; revisa el destino y confirma para habilitar la aplicación.</span></div>}
      {applied && <div className="csv-import-safety"><Check size={15} /><span>El lote quedó aplicado. Puedes preparar una reversión; solo se permitirá si todas las filas conservan sus valores.</span></div>}
      {!isTauri() && <div className="csv-import-safety"><LockKeyhole size={15} /><span>La importación solo funciona en la aplicación de escritorio. Esta vista previa web no envía cambios a la base.</span></div>}
      {isTauri() && <div className="csv-import-safety"><LockKeyhole size={15} /><span>El core transaccional y la compensación pasaron MySQL 8.0.46. La acción se habilitará después de comprobar este recorrido desde la ventana Tauri nativa de Windows.</span></div>}
      <footer>
        <button className="secondary-action" onClick={() => setOpen(false)} disabled={busy}>Cerrar</button>
        {!prepared && <button className="primary-action" onClick={() => void prepareImport()} disabled={busy || disabled || !MYSQL_CSV_IMPORT_ENABLED || !isTauri() || !content || !preview || preview.errors.length > 0 || columnError.length > 0}>{busy ? <LoaderCircle size={13} className="spin" /> : null}Preparar importación</button>}
        {prepared && !confirmed && !applied && !reverted && <button className="primary-action" onClick={() => void confirmImport()} disabled={busy}>{busy ? <LoaderCircle size={13} className="spin" /> : null}Confirmar revisión</button>}
        {prepared && confirmed && !applied && !reverted && <button className="primary-action" onClick={() => void applyImport()} disabled={busy}>{busy ? <LoaderCircle size={13} className="spin" /> : null}Aplicar a la base</button>}
        {reverted && <span role="status">La reversión quedó aplicada.</span>}
        {applied && !revertPrepared && <button className="secondary-action" onClick={() => void prepareRevert()} disabled={busy}>Preparar reversión</button>}
        {revertPrepared && !revertConfirmed && <button className="primary-action" onClick={() => void confirmRevert()} disabled={busy}>Confirmar reversión</button>}
        {revertPrepared && revertConfirmed && <button className="primary-action" onClick={() => void applyRevert()} disabled={busy}>Aplicar reversión</button>}
      </footer>
    </section></div>}
  </>
}
