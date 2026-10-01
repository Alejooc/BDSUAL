import { useEffect, useState } from 'react'
import { AlertCircle, Database, KeyRound, LoaderCircle, RefreshCw } from 'lucide-react'
import { ipc, type ColumnMetadata, type DatabaseEngine, type IpcError, type TableStructure } from './ipc'

export type StructureTarget = { connectionId: string; connectionName: string; engine: DatabaseEngine; database: string; table: string }
type StructureSections = {
  columns: ColumnMetadata[] | null
  details: TableStructure | null
  columnError: string
  detailError: string
}

function messageOf(error: unknown) {
  return error && typeof error === 'object' && 'message' in error
    ? String((error as IpcError).message)
    : 'No se pudo consultar la estructura de la tabla.'
}

export default function TableStructureView({ target }: { target: StructureTarget }) {
  const [value, setValue] = useState<StructureSections | null>(null)
  const [loading, setLoading] = useState(false)
  const [refresh, setRefresh] = useState(0)

  useEffect(() => {
    let active = true
    setLoading(true)
    setValue({ columns: null, details: null, columnError: '', detailError: '' })
    void Promise.allSettled([
      ipc.listColumns(target.connectionId, target.database, target.table, 'table'),
      ipc.getTableStructure(target.connectionId, target.database, target.table),
    ]).then(([columns, details]) => {
      if (!active) return
      setValue({
        columns: columns.status === 'fulfilled' ? columns.value : null,
        details: details.status === 'fulfilled' ? details.value : null,
        columnError: columns.status === 'rejected' ? messageOf(columns.reason) : '',
        detailError: details.status === 'rejected' ? messageOf(details.reason) : '',
      })
    }).finally(() => {
      if (active) setLoading(false)
    })
    return () => { active = false }
  }, [target.connectionId, target.database, target.table, refresh])

  return <section className="table-structure-workspace" aria-labelledby="table-structure-title">
      <header className="table-structure-header"><div className="table-data-title"><Database size={17} /><span><h2 id="table-structure-title">Estructura · {target.table}</h2><small>{target.connectionName} · {target.database} · {target.engine === 'mariadb' ? 'MariaDB' : 'MySQL'} · solo lectura</small></span></div><button className="small-outline structure-refresh" disabled={loading} onClick={() => setRefresh((current) => current + 1)}><RefreshCw size={13} /> Actualizar estructura</button></header>
      <div className="structure-body">
        <div className="structure-section-heading"><span>Columnas</span><button className="icon-button subtle" disabled={loading} aria-label="Actualizar estructura" title="Actualizar estructura" onClick={() => setRefresh((current) => current + 1)}><RefreshCw size={13} /></button></div>
        {loading && <div className="table-data-empty structure-state"><LoaderCircle size={14} className="spin" /> Consultando estructura…</div>}
        {value?.columnError && <div className="sql-error" role="alert"><AlertCircle size={15} /><span>{value.columnError}</span></div>}
        {value?.columns && <div className="structure-table-wrap"><table className="structure-table"><thead><tr><th>Nombre</th><th>Tipo</th><th>Nulabilidad</th><th>Predeterminado</th></tr></thead><tbody>{value.columns.map((column) => <tr key={column.name}><td><span>{column.name}</span>{column.isPrimaryKey && <span className="structure-pk"><KeyRound size={10} /> PK</span>}</td><td>{column.dataType}</td><td>{column.isNullable ? 'NULL' : 'NOT NULL'}</td><td>{column.defaultAvailable ? column.defaultValue ?? '—' : 'No disponible'}</td></tr>)}</tbody></table></div>}
        <div className="structure-section-heading"><span>Índices</span><span>{value?.details?.indexes.length ?? '—'}</span></div>
        {value?.detailError && <div className="sql-error" role="alert"><AlertCircle size={15} /><span>{value.detailError}</span></div>}
        {value?.details && (value.details.indexes.length ? <div className="structure-card-list">{value.details.indexes.map((index) => <article className="structure-card" key={index.name}><strong>{index.name}</strong><span>{index.columns.join(', ')}</span><small>{index.name === 'PRIMARY' ? 'Clave primaria' : index.unique ? 'Único' : 'No único'} · {index.indexType}</small></article>)}</div> : <p className="structure-empty">No hay índices visibles.</p>)}
        <div className="structure-section-heading"><span>Restricciones</span><span>{value?.details?.constraints.length ?? '—'}</span></div>
        {value?.details && (value.details.constraints.length ? <div className="structure-card-list">{value.details.constraints.map((constraint) => <article className="structure-card" key={constraint.name}><strong>{constraint.name}</strong><span>{constraint.kind}{constraint.columns.length ? ` · ${constraint.columns.join(', ')}` : ''}</span>{constraint.referencedTable && <small>Referencia: {constraint.referencedDatabase ? `${constraint.referencedDatabase}.` : ''}{constraint.referencedTable} ({constraint.referencedColumns.join(', ')})</small>}</article>)}</div> : <p className="structure-empty">No hay restricciones visibles.</p>)}
      </div>
  </section>
}
