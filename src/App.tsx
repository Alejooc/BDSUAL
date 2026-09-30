import { lazy, Suspense, useEffect, useState } from 'react'
import { isTauri } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { Panel, PanelGroup, PanelResizeHandle } from 'react-resizable-panels'
import { Activity, AlertCircle, Check, ChevronDown, ChevronRight, ChevronsDown, ChevronsLeft, CircleHelp, Clock3, Code2, Database, GitBranch, Home, Layers3, Plus, Settings, ShieldCheck, Sparkles, Table2, Workflow, X } from 'lucide-react'
import { type IpcError, type Section } from './ipc'
import { sectionLabels, useAppStore } from './store'
import BrandMark from './BrandMark'
import { ContextMenuHost, openContextMenu } from './ContextMenu'
import ConnectionWorkspace from './ConnectionWorkspace'
import TableDataView, { type TableDataTarget } from './TableDataView'
import SettingsPage from './SettingsPage'
const SqlEditor = lazy(() => import('./SqlEditor'))

const sections: { id: Section; icon: typeof Database }[] = [{ id: 'explorer', icon: Database }, { id: 'changes', icon: GitBranch }, { id: 'history', icon: Clock3 }]

function ActivityBar({ settingsActive }: { settingsActive: boolean }) {
  const { preferences, update } = useAppStore()
  return <aside className="activity-bar" aria-label="Navegación principal">
    <div className="brand-mark" title="DBSUAL"><span className="brand-glyph"><BrandMark /></span></div>
    <div className="activity-items">{sections.map(({ id, icon: Icon }) => <button key={id} className={`activity-button ${preferences.section === id ? 'selected' : ''}`} aria-label={sectionLabels[id]} aria-pressed={preferences.section === id} onClick={() => update({ section: id })}><Icon size={19} strokeWidth={1.7} /><span className="activity-label">{sectionLabels[id]}</span></button>)}</div>
    <div className="activity-bottom"><button className="activity-button" aria-label="Ayuda disponible próximamente" disabled><CircleHelp size={19} strokeWidth={1.7} /></button><button className="activity-button" aria-label="Restablecer disposición" title="Restablecer disposición" onClick={() => void useAppStore.getState().reset()}><ChevronsDown size={19} strokeWidth={1.7} /></button><button className={`activity-button ${settingsActive ? 'selected' : ''}`} aria-label="Configuración" title="Configuración" aria-pressed={settingsActive} onClick={() => window.dispatchEvent(new Event('dbsual:open-settings'))}><Settings size={19} strokeWidth={1.7} /></button><div className="avatar">DB</div></div>
  </aside>
}

function Sidebar() {
  const section = useAppStore((state) => state.preferences.section)
  return <ConnectionWorkspace section={section} />
}

function Welcome({ onOpenSql }: { onOpenSql: () => void }) {
  return <div className="welcome-scroll"><section className="welcome">
    <div className="eyebrow"><span className="eyebrow-line" /> ESPACIO DE TRABAJO</div>
    <div className="welcome-title-wrap"><div className="welcome-emblem"><BrandMark title="DBSUAL" /></div><div><h1>Tu trabajo,<br /><span>en orden.</span></h1><p className="welcome-lede">Un espacio claro para explorar, entender y controlar los cambios de tus bases de datos.</p></div></div>
    <div className="welcome-rule"><span>EMPIEZA POR AQUÍ</span><i /></div>
    <div className="quick-grid">
      <article className="quick-card"><div className="quick-icon indigo"><Database size={18} /></div><div><h2>Conecta una base de datos</h2><p>Agrega una conexión para comenzar a explorar.</p></div><button className="round-arrow" aria-label="Agregar conexión" onClick={() => { useAppStore.getState().update({ section: 'explorer', sidebarCollapsed: false }); requestAnimationFrame(() => window.dispatchEvent(new Event('dbsual:new-connection'))) }}><Plus size={16} /></button></article>
      <article className="quick-card"><div className="quick-icon mint"><GitBranch size={18} /></div><div><h2>Prepara un cambio</h2><p>Revisa cada modificación antes de aplicarla.</p></div><button className="round-arrow" aria-label="Ver cambios" disabled><ChevronRight size={16} /></button></article>
      <article className="quick-card"><div className="quick-icon indigo"><Code2 size={18} /></div><div><h2>Consulta con SQL</h2><p>Ejecuta consultas de lectura en una base MySQL.</p></div><button className="round-arrow" aria-label="Abrir editor SQL" onClick={onOpenSql}><ChevronRight size={16} /></button></article>
    </div>
    <div className="welcome-note"><div className="note-icon"><ShieldCheck size={17} /></div><div><strong>Tu información permanece local</strong><p>DBSUAL guarda la configuración de la interfaz en este dispositivo. No se guardan consultas ni resultados en esta entrega.</p></div></div>
    <div className="welcome-bottom"><span><Sparkles size={13} /> UNA BASE SÓLIDA PARA TRABAJAR CON CONFIANZA</span><span>DBSUAL <b>·</b> INICIO</span></div>
  </section></div>
}

function BottomPanel() { const update = useAppStore((s) => s.update); return <div className="bottom-panel"><div className="bottom-tabs"><span className="bottom-tab active"><Activity size={14} /> SALIDA</span><div className="bottom-tools"><button className="icon-button subtle" title="Contraer panel inferior" onClick={() => update({ bottomCollapsed: true })}><X size={14} /></button></div></div><div className="bottom-body"><div className="bottom-empty-icon"><Workflow size={16} /></div><span>La salida de operaciones aparecerá aquí.</span></div></div> }

function StatusBar() { const error = useAppStore((s) => s.error); return <footer className="status-bar"><div className="status-left"><span className="status-item"><span className="status-dot" /> {error ? 'Revisar almacenamiento local' : 'Almacenamiento local listo'}</span><span className="status-item dim"><GitBranch size={12} /> Sin proyecto</span></div><div className="status-right"><span className="status-item dim"><Code2 size={12} /> DBSUAL</span><span className="status-item dim"><Check size={12} /> {error ? 'Requiere atención' : 'Todo al día'}</span></div></footer> }

export default function App() {
  const { preferences, update, booting, error, clearError, layoutEpoch, reportError } = useAppStore()
  const [closeBlocked, setCloseBlocked] = useState(false)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [sqlOpen, setSqlOpen] = useState(false)
  const [sqlLoaded, setSqlLoaded] = useState(false)
  const [tableTargets, setTableTargets] = useState<TableDataTarget[]>([])
  const [activeTableId, setActiveTableId] = useState<string | null>(null)
  const session = useAppStore((state) => state.session)
  const setSession = useAppStore((state) => state.setSession)
  useEffect(() => {
    const openSettings = () => setSettingsOpen(true)
    window.addEventListener('dbsual:open-settings', openSettings)
    return () => window.removeEventListener('dbsual:open-settings', openSettings)
  }, [])
  useEffect(() => {
    document.documentElement.dataset.theme = preferences.theme
    document.documentElement.dataset.fontScale = preferences.fontScale
  }, [preferences.theme, preferences.fontScale])
  useEffect(() => {
    if (booting) return
    const hasQuery = session.tabs.some((tab) => tab.id === 'query')
    setSqlLoaded(hasQuery)
    setSqlOpen(hasQuery && session.activeTab === 'query')
  }, [booting, session])
  const activateTab = (tab: 'welcome' | 'query') => {
    const newTab = tab === 'welcome' ? { id: 'welcome' as const, kind: 'welcome' as const } : { id: 'query' as const, kind: 'query' as const }
    const tabs = session.tabs.some((item) => item.id === tab) ? session.tabs : [...session.tabs, newTab]
    setSession({ version: 1, tabs, activeTab: tab })
    if (tab === 'query') { setSqlLoaded(true); setSqlOpen(true) } else setSqlOpen(false)
  }
  const openSql = () => activateTab('query')
  const closeWorkTab = (tab: 'welcome' | 'query') => {
    const tabs = session.tabs.filter((item) => item.id !== tab)
    const activeTab = session.activeTab === tab ? (tabs[0]?.id ?? 'none') : session.activeTab
    setSession({ version: 1, tabs, activeTab })
    if (tab === 'query') setSqlOpen(false)
  }
  useEffect(() => {
    const openTable = (event: Event) => {
      const detail = (event as CustomEvent<Omit<TableDataTarget, 'id' | 'requestId'>>).detail
      if (!detail?.connectionId || !detail.database || !detail.table) return
      const id = JSON.stringify([detail.engine, detail.connectionId, detail.database, detail.schema ?? null, detail.table])
      setTableTargets((targets) => {
        if (targets.some((target) => target.id === id)) return targets
        return [...targets, { ...detail, id, requestId: crypto.randomUUID() }]
      })
      setActiveTableId(id)
      setSettingsOpen(false)
      setSqlOpen(false)
    }
    window.addEventListener('dbsual:open-table-data', openTable)
    return () => window.removeEventListener('dbsual:open-table-data', openTable)
  }, [])
  const closeTable = (id: string) => {
    const index = tableTargets.findIndex((target) => target.id === id)
    const remaining = tableTargets.filter((target) => target.id !== id)
    setTableTargets(remaining)
    if (activeTableId === id) {
      setActiveTableId(remaining[Math.max(0, index - 1)]?.id ?? null)
      setSqlOpen(!remaining.length && session.activeTab === 'query')
    }
  }
  useEffect(() => { void useAppStore.getState().boot() }, [])
  useEffect(() => {
    if (!isTauri()) return
    let active = true
    let unlisten: (() => void) | undefined
    void getCurrentWindow().onCloseRequested(async (event) => {
      event.preventDefault()
      try {
        if (useAppStore.getState().booting) {
          await getCurrentWindow().destroy()
          return
        }
        await useAppStore.getState().flush()
        await getCurrentWindow().destroy()
      } catch {
        if (active) setCloseBlocked(true)
      }
    }).then((off) => { if (active) unlisten = off; else off() }).catch((error: IpcError) => reportError({ code: 'WINDOW_ERROR', message: error.message ?? 'No se pudo preparar el cierre.' }))
    return () => { active = false; unlisten?.() }
  }, [reportError])
  const windowAction = (action: 'minimize' | 'toggleMaximize' | 'close') => {
    if (!isTauri()) return
    void getCurrentWindow()[action]().catch((error: Error) => reportError({ code: 'WINDOW_ERROR', message: error.message }))
  }
  const setSidebar = (size: number) => update({ sidebarWidth: Math.min(45, Math.max(15, Math.round(size))) })
  const setBottom = (size: number) => update({ bottomHeight: Math.min(60, Math.max(15, Math.round(size))) })
  if (booting) return <main className="app-shell"><div className="boot-indicator"><span className="spinner" /> Preparando espacio de trabajo…</div></main>
  return <main className="app-shell antialiased" onContextMenuCapture={(event) => event.preventDefault()}>
    <header className="titlebar" data-tauri-drag-region><div className="titlebar-app" data-tauri-drag-region><span className="titlebar-logo"><BrandMark /></span><span>DBSUAL</span><span className="title-sep">/</span><span className="title-current">Espacio de trabajo</span><button className="titlebar-home" aria-label="Abrir Inicio" title="Abrir Inicio" onClick={() => { setSettingsOpen(false); setActiveTableId(null); activateTab('welcome') }}><Home size={13} /> Inicio</button></div><div className="titlebar-status" data-tauri-drag-region><span className="live-dot" /> LOCAL <span className="title-version">0.1.0</span></div><div className="window-actions"><button aria-label="Minimizar" disabled={!isTauri()} onClick={() => windowAction('minimize')}>–</button><button aria-label="Maximizar" disabled={!isTauri()} onClick={() => windowAction('toggleMaximize')}>□</button><button className="close-window" aria-label="Cerrar" disabled={!isTauri()} onClick={() => windowAction('close')}>×</button></div></header>
    <div className="workspace"><ActivityBar settingsActive={settingsOpen} />
      <PanelGroup key={`main-${layoutEpoch}`} direction="horizontal" className="main-split" onLayout={(sizes) => { if (!preferences.sidebarCollapsed && sizes[0]) setSidebar(sizes[0]) }}>
        {!preferences.sidebarCollapsed && <Panel defaultSize={preferences.sidebarWidth} minSize={15} maxSize={45} className="side-panel"><Sidebar /></Panel>}
        {!preferences.sidebarCollapsed && <PanelResizeHandle className="resize-handle horizontal-handle" aria-label="Redimensionar panel lateral" />}
        <Panel minSize={30} className="content-panel">
          {preferences.sidebarCollapsed && <button className="restore-panel side-restore" onClick={() => update({ sidebarCollapsed: false })}><ChevronsLeft size={14} /> Mostrar panel</button>}
          <div className="tab-strip">{settingsOpen && <div className="table-data-tab-entry active-tab"><button className="tab active-tab"><span className="tab-glyph"><Settings size={14} /></span><span>Configuración</span></button><button className="table-data-tab-close" aria-label="Cerrar Configuración" onClick={() => setSettingsOpen(false)}><X size={12} /></button></div>}{session.tabs.some((tab) => tab.id === 'welcome') && <div className={`table-data-tab-entry ${session.activeTab === 'welcome' && !activeTableId && !settingsOpen ? 'active-tab' : ''}`}><button className={`tab ${session.activeTab === 'welcome' && !activeTableId && !settingsOpen ? 'active-tab' : ''}`} onContextMenu={(event) => openContextMenu(event, [{ label: 'Activar Inicio', action: () => { setSettingsOpen(false); setActiveTableId(null); activateTab('welcome') } }, { label: 'Cerrar pestaña', action: () => closeWorkTab('welcome') }])} onClick={() => { setSettingsOpen(false); setActiveTableId(null); activateTab('welcome') }}><span className="tab-glyph"><Layers3 size={14} /></span><span>Bienvenida</span></button><button className="table-data-tab-close" aria-label="Cerrar Bienvenida" onClick={() => closeWorkTab('welcome')}><X size={12} /></button></div>}{session.tabs.some((tab) => tab.id === 'query') && <div className={`table-data-tab-entry ${sqlOpen && !settingsOpen ? 'active-tab' : ''}`}><button className={`tab ${sqlOpen && !settingsOpen ? 'active-tab' : ''}`} onContextMenu={(event) => openContextMenu(event, [{ label: 'Activar Consulta SQL', action: () => { setSettingsOpen(false); setActiveTableId(null); openSql() } }, { label: 'Cerrar pestaña', action: () => closeWorkTab('query') }])} onClick={() => { setSettingsOpen(false); setActiveTableId(null); openSql() }}><span className="tab-glyph"><Code2 size={14} /></span><span>Consulta SQL</span></button><button className="table-data-tab-close" aria-label="Cerrar Consulta SQL" onClick={() => closeWorkTab('query')}><X size={12} /></button></div>}{tableTargets.map((target) => <div className={`table-data-tab-entry ${activeTableId === target.id && !settingsOpen ? 'active-tab' : ''}`} key={target.id}><button className={`tab table-data-tab ${activeTableId === target.id && !settingsOpen ? 'active-tab' : ''}`} onContextMenu={(event) => openContextMenu(event, [{ label: `Activar ${target.table}`, action: () => { setSettingsOpen(false); setActiveTableId(target.id); setSqlOpen(false) } }, { label: 'Cerrar pestaña', action: () => closeTable(target.id) }])} aria-label={`Activar tabla ${target.table} de ${target.database}`} onClick={() => { setSettingsOpen(false); setActiveTableId(target.id); setSqlOpen(false) }}><span className="tab-glyph"><Table2 size={14} /></span><span>{target.table}</span></button><button className="table-data-tab-close" aria-label={`Cerrar tabla ${target.table}`} onClick={() => closeTable(target.id)}><X size={12} /></button></div>)}<div className="tab-strip-space" /><button className="tab-action" aria-label="Nueva consulta SQL" title="Nueva consulta SQL" onClick={() => { setSettingsOpen(false); setActiveTableId(null); openSql() }}><Plus size={14} /></button></div>
          <PanelGroup key={`bottom-${layoutEpoch}-${settingsOpen}`} direction="vertical" className="content-split" onLayout={(sizes) => { if (!settingsOpen && !preferences.bottomCollapsed && sizes[1]) setBottom(sizes[1]) }}>
            <Panel minSize={settingsOpen ? 100 : 40} defaultSize={settingsOpen ? 100 : 72} className="center-panel">{settingsOpen ? <SettingsPage close={() => setSettingsOpen(false)} /> : <>{session.tabs.some((tab) => tab.id === 'welcome') && <div className="center-view" style={{ display: session.activeTab === 'welcome' && !activeTableId ? 'block' : 'none' }}><Welcome onOpenSql={openSql} /></div>}{session.tabs.length === 0 && !activeTableId && <div className="empty-workspace"><Layers3 size={22} /><h2>No hay pestañas abiertas</h2><p>Abre Inicio, crea una consulta o elige una tabla del explorador.</p><div><button className="small-outline" onClick={() => activateTab('welcome')}><Home size={14} /> Abrir Inicio</button><button className="small-outline" onClick={openSql}><Code2 size={14} /> Nueva consulta SQL</button><button className="small-outline" onClick={() => update({ section: 'explorer', sidebarCollapsed: false })}><Database size={14} /> Ir al Explorador</button></div></div>}{tableTargets.map((target) => <div className="center-view" key={target.id} style={{ display: activeTableId === target.id ? 'block' : 'none' }}><TableDataView target={target} close={() => closeTable(target.id)} active={!settingsOpen && activeTableId === target.id} /></div>)}{sqlLoaded && <div className="center-view" style={{ display: sqlOpen && !activeTableId ? 'block' : 'none' }}><Suspense fallback={<div className="boot-indicator"><span className="spinner" /> Preparando editor SQL…</div>}><SqlEditor /></Suspense></div>}</>}</Panel>{!settingsOpen && (preferences.bottomCollapsed ? <div className="collapsed-bottom"><span>Panel inferior contraído</span><button className="restore-panel" onClick={() => update({ bottomCollapsed: false })}><ChevronDown size={13} /> Mostrar salida</button></div> : <><PanelResizeHandle className="resize-handle vertical-handle" aria-label="Redimensionar panel inferior" /><Panel defaultSize={preferences.bottomHeight} minSize={15} maxSize={60} className="lower-panel"><BottomPanel /></Panel></>)}
          </PanelGroup>
        </Panel>
      </PanelGroup>
    </div>
    <StatusBar />
    <ContextMenuHost />
    {error && <div className="error-toast" role="alert"><AlertCircle size={17} /><div><strong>Requiere atención</strong><span>{error.message}</span><small>{error.code}</small><button className="retry-save" onClick={() => { void useAppStore.getState().flush().then(clearError).catch(() => undefined) }}>Reintentar guardado</button></div><button aria-label="Cerrar aviso" onClick={clearError}><X size={15} /></button></div>}
    {closeBlocked && <div className="close-options" role="alertdialog" aria-label="Error al guardar antes de cerrar"><p>No se pudo guardar la disposición antes de cerrar.</p><button onClick={() => { setCloseBlocked(false); windowAction('close') }}>Reintentar</button><button onClick={() => { void getCurrentWindow().destroy() }}>Cerrar sin guardar</button></div>}
  </main>
}
