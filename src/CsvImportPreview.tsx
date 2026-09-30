import { useMemo, useState } from 'react'
import { AlertCircle, FileUp, LockKeyhole, X } from 'lucide-react'
import './csv-import-preview.css'

type Delimiter = ',' | ';' | '\t'
type Preview = { headers: string[]; rows: string[][]; errors: string[] }
type PreviewCell = { kind: 'null' } | { kind: 'text'; value: string }
const MAX_BYTES = 5 * 1024 * 1024
const MAX_ROWS = 50_000

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

export default function CsvImportPreview({ table, columns, disabled }: { table: string; columns: string[]; disabled: boolean }) {
  const [open, setOpen] = useState(false)
  const [delimiter, setDelimiter] = useState<Delimiter>(',')
  const [nullMarker, setNullMarker] = useState('\\N')
  const [fileName, setFileName] = useState('')
  const [content, setContent] = useState('')
  const [error, setError] = useState('')
  const preview = useMemo(() => content ? parseCsv(content, delimiter) : null, [content, delimiter])
  const columnError = preview && columns.length
    ? preview.headers.filter((header) => !columns.some((column) => column.toLocaleLowerCase() === header.toLocaleLowerCase()))
    : []
  const decodedRows = preview?.rows.slice(0, 5).map((row) => row.map((value): PreviewCell => {
    if (value === nullMarker) return { kind: 'null' }
    if (value.startsWith('\\\\') || (nullMarker && value === `\\${nullMarker}`)) return { kind: 'text', value: value.slice(1) }
    return { kind: 'text', value }
  }))

  const selectFile = async (file?: File) => {
    setError(''); setContent(''); setFileName('')
    if (!file) return
    if (file.size > MAX_BYTES) { setError('El archivo supera el límite de 5 MiB.'); return }
    try {
      const text = new TextDecoder('utf-8', { fatal: true }).decode(await file.arrayBuffer())
      setContent(text); setFileName(file.name)
    } catch { setError('El archivo no es UTF-8 válido o no se pudo leer.') }
  }

  return <>
    <button type="button" className="secondary-action csv-import-open" disabled={disabled} onClick={() => setOpen(true)}><FileUp size={13} /> Vista previa CSV</button>
    {open && <div className="csv-import-backdrop"><section className="csv-import-dialog" role="dialog" aria-modal="true" aria-labelledby="csv-import-title">
      <header><div><h2 id="csv-import-title">Vista previa de importación CSV</h2><p>{table} · solo vista previa</p></div><button className="icon-button subtle" aria-label="Cerrar vista previa CSV" onClick={() => setOpen(false)}><X size={16} /></button></header>
      <label className="csv-import-field">Archivo CSV<input aria-label="Archivo CSV" type="file" accept=".csv,text/csv" onChange={(event) => void selectFile(event.currentTarget.files?.[0])} /></label>
      <div className="csv-import-options"><label>Separador<select aria-label="Separador CSV" value={delimiter} onChange={(event) => setDelimiter(event.target.value as Delimiter)}><option value=",">Coma (,)</option><option value=";">Punto y coma (;)</option><option value={"\t"}>Tabulación</option></select></label><label>Marcador NULL<input aria-label="Marcador NULL" value={nullMarker} maxLength={128} onChange={(event) => setNullMarker(event.target.value)} /></label></div>
      {fileName && <p className="csv-import-file">Archivo: {fileName}</p>}
      {error && <div className="csv-import-error" role="alert"><AlertCircle size={14} />{error}</div>}
      {preview && <div className="csv-import-results">
        {preview.errors.map((item) => <div className="csv-import-error" role="alert" key={item}><AlertCircle size={14} />{item}</div>)}
        {columnError.length > 0 && <div className="csv-import-error" role="alert">Columnas que no existen en «{table}»: {columnError.join(', ')}.</div>}
        {preview.errors.length === 0 && columnError.length === 0 && <p role="status">{preview.rows.length} filas válidas · {preview.headers.length} columnas reconocidas. NULL se mostrará como tal.</p>}
        <div className="csv-import-table-wrap"><table><thead><tr>{preview.headers.map((header, index) => <th key={`${header}-${index}`}>{header}</th>)}</tr></thead><tbody>{decodedRows?.map((row, rowIndex) => <tr key={rowIndex}>{row.map((cell, colIndex) => <td key={colIndex}>{cell.kind === 'null' ? <em>NULL</em> : cell.value}</td>)}</tr>)}</tbody></table></div>
      </div>}
      <div className="csv-import-safety"><LockKeyhole size={15} /><span>No hay un respaldo completo verificable para esta operación. La aplicación permanece bloqueada; esta vista previa no envía cambios a la base de datos.</span></div>
      <footer><button className="secondary-action" onClick={() => setOpen(false)}>Cerrar</button><button className="primary-action" disabled title="Requiere un respaldo completo verificable">Aplicar importación</button></footer>
    </section></div>}
  </>
}
