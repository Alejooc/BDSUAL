import { useEffect, useState } from 'react'
import { isTauri } from '@tauri-apps/api/core'
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog'
import { AlertCircle, KeyRound, LoaderCircle, ShieldCheck, X } from 'lucide-react'
import { ipc, type IpcError, type VaultStatus } from './ipc'

export default function VaultSetup() {
  const [status, setStatus] = useState<VaultStatus | null>(null)
  const [phrase, setPhrase] = useState<string | null>(null)
  const [confirmation, setConfirmation] = useState('')
  const [exportPath, setExportPath] = useState<string | null>(null)
  const [exportPassword, setExportPassword] = useState('')
  const [exportPasswordAgain, setExportPasswordAgain] = useState('')
  const [exported, setExported] = useState(false)
  const [imported, setImported] = useState(false)
  const [importFlow, setImportFlow] = useState(false)
  const [importPath, setImportPath] = useState<string | null>(null)
  const [importPassword, setImportPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<IpcError | null>(null)

  useEffect(() => {
    if (!isTauri()) return
    void ipc.getVaultStatus().then(setStatus).catch(setError)
  }, [])

  const begin = async () => {
    setBusy(true)
    setError(null)
    try {
      const setup = await ipc.beginVaultSetup()
      setPhrase(setup.recoveryPhrase)
      setConfirmation('')
      setImportFlow(false)
    } catch (cause) {
      setError(cause as IpcError)
    } finally {
      setBusy(false)
    }
  }

  const cancel = async () => {
    setBusy(true)
    try {
      await ipc.cancelVaultSetup()
      setPhrase(null)
      setConfirmation('')
      setImportFlow(false)
      setError(null)
    } catch (cause) {
      setError(cause as IpcError)
    } finally {
      setBusy(false)
    }
  }

  const confirm = async () => {
    if (!phrase) return
    setBusy(true)
    setError(null)
    try {
      const result = await ipc.confirmVaultSetup(confirmation.trim().split(/\s+/).join(' '))
      setStatus(result)
      setPhrase(null)
      setConfirmation('')
      setImported(importFlow)
      setImportFlow(false)
    } catch (cause) {
      setError(cause as IpcError)
    } finally {
      setBusy(false)
    }
  }

  const beginExport = async () => {
    setError(null)
    setExported(false)
    try {
      const path = await saveDialog({
        title: 'Guardar archivo de recuperación de DBSUAL',
        defaultPath: 'DBSUAL-recovery.dbsual-key',
        filters: [{ name: 'Archivo de clave DBSUAL', extensions: ['dbsual-key'] }],
      })
      if (path) {
        setExportPassword('')
        setExportPasswordAgain('')
        setExportPath(path)
      }
    } catch (cause) {
      setError(cause as IpcError)
    }
  }

  const exportKeyFile = async () => {
    if (!exportPath) return
    setBusy(true)
    setError(null)
    try {
      await ipc.exportRecoveryKeyFile(exportPath, exportPassword)
      setExportPath(null)
      setExportPassword('')
      setExportPasswordAgain('')
      setExported(true)
    } catch (cause) {
      setError(cause as IpcError)
    } finally {
      setBusy(false)
    }
  }

  const cancelExport = () => {
    setExportPath(null)
    setExportPassword('')
    setExportPasswordAgain('')
    setError(null)
  }

  const beginImport = async () => {
    setError(null)
    try {
      const path = await openDialog({
        title: 'Seleccionar archivo de recuperación de DBSUAL',
        multiple: false,
        directory: false,
        filters: [{ name: 'Archivo de clave DBSUAL', extensions: ['dbsual-key'] }],
      })
      if (typeof path === 'string') { setImportPassword(''); setImportPath(path) }
    } catch (cause) { setError(cause as IpcError) }
  }

  const importKeyFile = async () => {
    if (!importPath) return
    setBusy(true)
    setError(null)
    try {
      const setup = await ipc.importRecoveryKeyFile(importPath, importPassword)
      setPhrase(setup.recoveryPhrase)
      setConfirmation('')
      setImportFlow(true)
      setImportPath(null)
      setImportPassword('')
    } catch (cause) { setError(cause as IpcError) }
    finally { setBusy(false) }
  }

  const cancelImport = () => { setImportPath(null); setImportPassword(''); setError(null) }

  const phraseMatches = phrase && confirmation.trim().split(/\s+/).join(' ') === phrase
  return <>
    <div className="welcome-note vault-note">
      <div className="note-icon">{status?.configured ? <ShieldCheck size={17} /> : <KeyRound size={17} />}</div>
      <div className="vault-note-copy">
      <strong>{status?.configured ? 'Depósito de respaldos protegido' : 'Prepara tus respaldos protegidos'}</strong>
      <p>{status?.configured ? 'La clave maestra está guardada en el almacén seguro de Windows.' : 'Configura la frase o importa un archivo de recuperación antes de usar respaldos protegidos.'}</p>
      </div>
      {!status?.configured && <><button className="secondary-action vault-configure" disabled={!isTauri() || !status || busy} onClick={() => void begin()}>{busy ? <LoaderCircle size={13} className="spin" /> : <KeyRound size={13} />} Configurar frase</button><button className="secondary-action vault-configure" disabled={!isTauri() || !status || busy} onClick={() => void beginImport()}><KeyRound size={13} /> Importar archivo</button></>}
      {status?.configured && <button className="secondary-action vault-configure" disabled={!isTauri() || busy} onClick={() => void beginExport()}>{busy ? <LoaderCircle size={13} className="spin" /> : <KeyRound size={13} />} Exportar archivo de recuperación</button>}
      {!isTauri() && <span className="vault-unavailable">Disponible en la app de escritorio</span>}
    </div>
    {exported && <div className="test-success vault-inline-success" role="status"><ShieldCheck size={14} /><span>Archivo cifrado creado. Guárdalo junto con su contraseña fuera de este equipo.</span></div>}
    {imported && <div className="test-success vault-inline-success" role="status"><ShieldCheck size={14} /><span>Clave importada y guardada en el almacén seguro de Windows.</span></div>}
    {error && !phrase && !exportPath && !importPath && <div className="dialog-error vault-inline-error" role="alert"><AlertCircle size={15} /><span>{error.message}</span></div>}
    {phrase && <div className="modal-backdrop" role="presentation"><section className="confirm-dialog vault-dialog" role="dialog" aria-modal="true" aria-labelledby="vault-title">
      <header className="vault-dialog-heading"><div className="confirm-icon"><KeyRound size={18} /></div><button className="icon-button subtle" aria-label="Cerrar" disabled={busy} onClick={() => void cancel()}><X size={16} /></button></header>
      <h2 id="vault-title">{importFlow ? 'Crea una frase nueva para este depósito' : 'Guarda tu frase de recuperación'}</h2>
      <p>{importFlow ? 'El archivo abrió el depósito. Esta nueva frase recuperará la misma clave y los mismos respaldos. Guárdala fuera de este equipo; DBSUAL no podrá mostrarla de nuevo.' : 'Esta frase permite recuperar tus respaldos después de reinstalar Windows. Guárdala fuera de este equipo. DBSUAL no podrá mostrarla de nuevo.'}</p>
      <ol className="recovery-words">{phrase.split(' ').map((word, index) => <li key={`${index}-${word}`}><span>{index + 1}</span><strong>{word}</strong></li>)}</ol>
      <label className="field-label recovery-confirm-label">Escribe las 12 palabras para confirmar<input autoComplete="off" autoCapitalize="none" spellCheck={false} value={confirmation} onChange={(event) => setConfirmation(event.target.value)} placeholder="palabra palabra palabra…" /></label>
      {error && <div className="dialog-error" role="alert"><AlertCircle size={15} /><span>{error.message}</span></div>}
      <footer><button className="text-action" disabled={busy} onClick={() => void cancel()}>Cancelar</button><button className="primary-action" disabled={busy || !phraseMatches} onClick={() => void confirm()}>{busy ? <LoaderCircle size={14} className="spin" /> : <ShieldCheck size={14} />} Confirmar frase</button></footer>
    </section></div>}
    {exportPath && <div className="modal-backdrop" role="presentation"><section className="confirm-dialog vault-dialog" role="dialog" aria-modal="true" aria-labelledby="key-file-title">
      <header className="vault-dialog-heading"><div className="confirm-icon"><KeyRound size={18} /></div><button className="icon-button subtle" aria-label="Cerrar" disabled={busy} onClick={cancelExport}><X size={16} /></button></header>
      <h2 id="key-file-title">Protege el archivo de recuperación</h2>
      <p>El archivo no contiene la clave en claro. Necesitarás esta contraseña para usarlo después. DBSUAL no la guardará.</p>
      <div className="selected-key-path">{exportPath.split(/[\\/]/).pop()}</div>
      <label className="field-label recovery-confirm-label">Contraseña<input type="password" autoComplete="new-password" value={exportPassword} onChange={(event) => setExportPassword(event.target.value)} /></label>
      <label className="field-label recovery-confirm-label">Confirma la contraseña<input type="password" autoComplete="new-password" value={exportPasswordAgain} onChange={(event) => setExportPasswordAgain(event.target.value)} /></label>
      {error && <div className="dialog-error" role="alert"><AlertCircle size={15} /><span>{error.message}</span></div>}
      <footer><button className="text-action" disabled={busy} onClick={cancelExport}>Cancelar</button><button className="primary-action" disabled={busy || Array.from(exportPassword.trim()).length < 12 || exportPassword !== exportPasswordAgain} onClick={() => void exportKeyFile()}>{busy ? <LoaderCircle size={14} className="spin" /> : <ShieldCheck size={14} />} Guardar archivo cifrado</button></footer>
    </section></div>}
    {importPath && <div className="modal-backdrop" role="presentation"><section className="confirm-dialog vault-dialog" role="dialog" aria-modal="true" aria-labelledby="import-key-title">
      <header className="vault-dialog-heading"><div className="confirm-icon"><KeyRound size={18} /></div><button className="icon-button subtle" aria-label="Cerrar" disabled={busy} onClick={cancelImport}><X size={16} /></button></header>
      <h2 id="import-key-title">Importa tu archivo de recuperación</h2>
      <p>La clave se guardará en el almacén seguro de Windows. La contraseña solo se usa para abrir el archivo y no se conserva.</p>
      <div className="selected-key-path">{importPath.split(/[\\/]/).pop()}</div>
      <label className="field-label recovery-confirm-label">Contraseña del archivo<input type="password" autoComplete="current-password" value={importPassword} onChange={(event) => setImportPassword(event.target.value)} /></label>
      {error && <div className="dialog-error" role="alert"><AlertCircle size={15} /><span>{error.message}</span></div>}
      <footer><button className="text-action" disabled={busy} onClick={cancelImport}>Cancelar</button><button className="primary-action" disabled={busy || importPassword.length < 12} onClick={() => void importKeyFile()}>{busy ? <LoaderCircle size={14} className="spin" /> : <ShieldCheck size={14} />} Importar clave</button></footer>
    </section></div>}
  </>
}
