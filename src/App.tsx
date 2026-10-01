import { lazy, Suspense, useEffect, useRef, useState } from 'react'
import { isTauri } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { Panel, PanelGroup, PanelResizeHandle } from 'react-resizable-panels'
import { AlertCircle, Check, ChevronRight, ChevronsDown, ChevronsLeft, CircleHelp, Clock3, Code2, Database, GitBranch, Home, Layers3, Pin, Plus, Settings, ShieldCheck, Sparkles, Table2, X } from 'lucide-react'
import { type IpcError, type Section } from './ipc'
import { sectionLabels, useAppStore } from './store'
import BrandMark from './BrandMark'
import { ContextMenuHost, openContextMenu } from './ContextMenu'
import ConnectionWorkspace from './ConnectionWorkspace'
import TableDataView, { type TableDataTarget } from './TableDataView'
import TableStructureView, { type StructureTarget } from './TableStructureView'
import type { TableDataRefreshRequest } from './workspaceEvents'
import SettingsPage from './SettingsPage'
import { isLightBackground } from './theme'
const SqlEditor = lazy(() => import('./SqlEditor'))

const sections: { id: Section; icon: typeof Database }[] = [{ id: 'explorer', icon: Database }, { id: 'changes', icon: GitBranch }, { id: 'history', icon: Clock3 }]

function ActivityBar({ settingsActive, onOpenSql }: { settingsActive: boolean; onOpenSql: () => void }) {
  const { preferences, update } = useAppStore()
  return <aside className="activity-bar" aria-label="Navegación principal">
    <div className="activity-items">{sections.map(({ id, icon: Icon }) => <button key={id} className={`activity-button ${preferences.section === id ? 'selected' : ''}`} aria-label={sectionLabels[id]} aria-pressed={preferences.section === id} onClick={() => update({ section: id })}><Icon size={19} strokeWidth={1.7} /><span className="activity-label">{sectionLabels[id]}</span></button>)}<button className="activity-button" aria-label="Nueva consulta SQL" title="Nueva consulta SQL" onClick={onOpenSql}><Code2 size={19} strokeWidth={1.7} /><span className="activity-label">Nueva consulta SQL</span></button></div>
    <div className="activity-bottom"><button className="activity-button" aria-label="Ayuda disponible próximamente" disabled><CircleHelp size={19} strokeWidth={1.7} /></button><button className="activity-button" aria-label="Restablecer disposición" title="Restablecer disposición" onClick={() => void useAppStore.getState().reset()}><ChevronsDown size={19} strokeWidth={1.7} /></button><button className={`activity-button ${settingsActive ? 'selected' : ''}`} aria-label="Configuración" title="Configuración" aria-pressed={settingsActive} onClick={() => window.dispatchEvent(new Event('dbsual:open-settings'))}><Settings size={19} strokeWidth={1.7} /></button></div>
  </aside>
}

function Sidebar({ onRefreshTableData }: { onRefreshTableData: (request: TableDataRefreshRequest) => void }) {
  const section = useAppStore((state) => state.preferences.section)
  return <ConnectionWorkspace section={section} onRefreshTableData={onRefreshTableData} />
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


function StatusBar() { const error = useAppStore((s) => s.error); return <footer className="status-bar"><div className="status-left"><span className="status-item"><span className="status-dot" /> {error ? 'Revisar almacenamiento local' : 'Almacenamiento local listo'}</span><span className="status-item dim"><GitBranch size={12} /> Sin proyecto</span></div><div className="status-right"><span className="status-item dim"><Code2 size={12} /> DBSUAL</span><span className="status-item dim"><Check size={12} /> {error ? 'Requiere atención' : 'Todo al día'}</span></div></footer> }

export default function App() {
  const { preferences, update, booting, error, clearError, layoutEpoch, reportError } = useAppStore()
  const [closeBlocked, setCloseBlocked] = useState(false)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [sqlOpen, setSqlOpen] = useState(false)
  const [sqlLoaded, setSqlLoaded] = useState(false)
  const [tableTargets, setTableTargets] = useState<TableDataTarget[]>([])
  const [structureTargets, setStructureTargets] = useState<Array<StructureTarget & { id: string }>>([])
  const [activeStructureId, setActiveStructureId] = useState<string | null>(null)
  const [draggedTableTab, setDraggedTableTab] = useState<string | null>(null)
  const [dropTableTab, setDropTableTab] = useState<string | null>(null)
  const tabDragRef = useRef<{ id: string; x: number; y: number; active: boolean } | null>(null)
  const tabStripRef = useRef<HTMLDivElement>(null)
  const tabRailDragRef = useRef<{ pointerX: number; scrollLeft: number } | null>(null)
  const [tabRail, setTabRail] = useState({ visible: false, width: 0, left: 0 })
  const updateTabRail = () => {
    const strip = tabStripRef.current
    if (!strip) return
    const maxScroll = strip.scrollWidth - strip.clientWidth
    if (maxScroll <= 1) { setTabRail((rail) => rail.visible ? { visible: false, width: 0, left: 0 } : rail); return }
    const width = Math.max(18, strip.clientWidth * strip.clientWidth / strip.scrollWidth)
    const left = strip.scrollLeft / maxScroll * (strip.clientWidth - width)
    setTabRail((rail) => rail.visible && Math.abs(rail.width - width) < 0.5 && Math.abs(rail.left - left) < 0.5 ? rail : { visible: true, width, left })
  }
  const [tableRefreshRequests, setTableRefreshRequests] = useState<Record<string, number>>({})
  const [activeTableId, setActiveTableId] = useState<string | null>(null)
  const session = useAppStore((state) => state.session)
  const setSession = useAppStore((state) => state.setSession)
  useEffect(() => {
    const openSettings = () => { setActiveStructureId(null); setSettingsOpen(true) }
    window.addEventListener('dbsual:open-settings', openSettings)
    return () => window.removeEventListener('dbsual:open-settings', openSettings)
  }, [])
  useEffect(() => {
    const openStructure = (event: Event) => {
      const detail = (event as CustomEvent<StructureTarget>).detail
      if (!detail?.connectionId || !detail.database || !detail.table) return
      const id = JSON.stringify([detail.connectionId, detail.database, detail.table])
      setStructureTargets((targets) => targets.some((target) => target.id === id) ? targets : [...targets, { ...detail, id }])
      setActiveStructureId(id)
      setActiveTableId(null)
      setSettingsOpen(false)
      setSqlOpen(false)
    }
    window.addEventListener('dbsual:open-table-structure', openStructure)
    return () => window.removeEventListener('dbsual:open-table-structure', openStructure)
  }, [])
  const closeStructure = (id: string) => {
    const index = structureTargets.findIndex((target) => target.id === id)
    const remaining = structureTargets.filter((target) => target.id !== id)
    setStructureTargets(remaining)
    if (activeStructureId === id) {
      const next = remaining[Math.max(0, index - 1)]?.id ?? null
      setActiveStructureId(next)
      if (!next) {
        const fallbackTable = tableTargets.at(-1)
        setActiveTableId(fallbackTable?.id ?? null)
        setSqlOpen(!fallbackTable && session.activeTab === 'query')
      }
    }
  }
  useEffect(() => {
    const root = document.documentElement
    root.dataset.theme = preferences.theme
    const colorTokens = ['background', 'sidebar', 'surface', 'elevated', 'border', 'text', 'muted', 'accent'] as const
    colorTokens.forEach((token) => root.style.removeProperty(`--theme-${token}`))
    if (preferences.theme === 'custom') {
      colorTokens.forEach((token) => root.style.setProperty(`--theme-${token}`, preferences.customColors[token]))
      const background = preferences.customColors.background
      root.style.colorScheme = isLightBackground(background) ? 'light' : 'dark'
    } else root.style.removeProperty('color-scheme')
    document.documentElement.dataset.fontScale = preferences.fontScale
  }, [preferences.theme, preferences.customColors, preferences.fontScale])
  useEffect(() => {
    const strip = tabStripRef.current
    if (!strip) return
    updateTabRail()
    const observer = new ResizeObserver(updateTabRail)
    observer.observe(strip)
    const content = strip.firstElementChild
    if (content) observer.observe(content)
    return () => observer.disconnect()
  }, [tableTargets, session.tabs, settingsOpen])
  useEffect(() => {
    if (booting) return
    const hasQuery = session.tabs.some((tab) => tab.id === 'query')
    setSqlLoaded(hasQuery)
    setSqlOpen(hasQuery && session.activeTab === 'query')
  }, [booting, session])
  const activateTab = (tab: 'welcome' | 'query') => {
    setActiveStructureId(null)
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
      setActiveStructureId(null)
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
  const toggleTablePin = (id: string) => {
    setTableTargets((targets) => {
      const updated = targets.map((target) => target.id === id ? { ...target, isPinned: !target.isPinned } : target)
      return [...updated.filter((target) => target.isPinned), ...updated.filter((target) => !target.isPinned)]
    })
  }
  const moveTableTab = (fromId: string, toId: string, after = false) => {
    setTableTargets((targets) => {
      const fromIndex = targets.findIndex((target) => target.id === fromId)
      const toIndex = targets.findIndex((target) => target.id === toId)
      if (fromIndex < 0 || toIndex < 0 || fromIndex === toIndex || Boolean(targets[fromIndex].isPinned) !== Boolean(targets[toIndex].isPinned)) return targets
      const next = [...targets]
      const [moving] = next.splice(fromIndex, 1)
      const targetIndex = next.findIndex((target) => target.id === toId)
      next.splice(targetIndex + (after ? 1 : 0), 0, moving)
      return next
    })
  }
  const refreshMatchingTables = (request: TableDataRefreshRequest) => {
    setTableRefreshRequests((current) => {
      const next = { ...current }
      let changed = false
      for (const target of tableTargets) {
        if (target.connectionId !== request.connectionId || target.database !== request.database || target.table !== request.table) continue
        next[target.id] = (current[target.id] ?? 0) + 1
        changed = true
      }
      return changed ? next : current
    })
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
  if (booting) return <main className="app-shell"><div className="boot-indicator"><span className="spinner" /> Preparando espacio de trabajo…</div></main>
  return <main className="app-shell antialiased" onContextMenuCapture={(event) => event.preventDefault()}>
    <header className="titlebar" data-tauri-drag-region><div className="titlebar-app" data-tauri-drag-region><span className="titlebar-logo"><BrandMark /></span><span>DBSUAL</span></div><div className="window-actions"><button aria-label="Minimizar" disabled={!isTauri()} onClick={() => windowAction('minimize')}>–</button><button aria-label="Maximizar" disabled={!isTauri()} onClick={() => windowAction('toggleMaximize')}><span className="maximize-glyph" aria-hidden="true" /></button><button className="close-window" aria-label="Cerrar" disabled={!isTauri()} onClick={() => windowAction('close')}>×</button></div></header>
    <div className="workspace"><ActivityBar settingsActive={settingsOpen} onOpenSql={() => { setSettingsOpen(false); setActiveTableId(null); openSql() }} />
      <PanelGroup key={`main-${layoutEpoch}`} direction="horizontal" className="main-split" onLayout={(sizes) => { if (!preferences.sidebarCollapsed && sizes[0]) setSidebar(sizes[0]) }}>
        {!preferences.sidebarCollapsed && <Panel defaultSize={preferences.sidebarWidth} minSize={15} maxSize={45} className="side-panel"><Sidebar onRefreshTableData={refreshMatchingTables} /></Panel>}
        {!preferences.sidebarCollapsed && <PanelResizeHandle className="resize-handle horizontal-handle" aria-label="Redimensionar panel lateral" />}
        <Panel minSize={30} className="content-panel">
          {preferences.sidebarCollapsed && <button className="restore-panel side-restore" onClick={() => update({ sidebarCollapsed: false })}><ChevronsLeft size={14} /> Mostrar panel</button>}
          <div className="tab-strip"><div ref={tabStripRef} className="tab-strip-scroll" onScroll={updateTabRail} onWheel={(event) => { const strip = event.currentTarget; if (strip.scrollWidth <= strip.clientWidth) return; if (Math.abs(event.deltaY) > Math.abs(event.deltaX)) { strip.scrollLeft += event.deltaY * 2.5; event.preventDefault() } }} onPointerMove={(event) => { const drag = tabDragRef.current; if (!drag) return; if (!drag.active && Math.hypot(event.clientX - drag.x, event.clientY - drag.y) > 5) { drag.active = true; setDraggedTableTab(drag.id) } if (!drag.active) return; const entry = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>("[data-table-tab-id]"); const target = tableTargets.find((item) => item.id === entry?.dataset.tableTabId); const dragged = tableTargets.find((item) => item.id === drag.id); setDropTableTab(target && dragged && Boolean(target.isPinned) === Boolean(dragged.isPinned) ? target.id : null) }} onPointerUp={(event) => { const drag = tabDragRef.current; if (!drag) return; if (drag.active) { const entry = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>("[data-table-tab-id]"); const targetId = entry?.dataset.tableTabId; if (targetId) { const rect = entry.getBoundingClientRect(); moveTableTab(drag.id, targetId, event.clientX >= rect.left + rect.width / 2) } } tabDragRef.current = null; setDraggedTableTab(null); setDropTableTab(null) }} onPointerCancel={() => { tabDragRef.current = null; setDraggedTableTab(null); setDropTableTab(null) }}>{settingsOpen && <div className="table-data-tab-entry active-tab"><button className="tab active-tab"><span className="tab-glyph"><Settings size={14} /></span><span>Configuración</span></button><button className="table-data-tab-close" aria-label="Cerrar Configuración" onClick={() => setSettingsOpen(false)}><X size={12} /></button></div>}{session.tabs.some((tab) => tab.id === 'welcome') && <div className={`table-data-tab-entry ${session.activeTab === 'welcome' && !activeTableId && !settingsOpen ? 'active-tab' : ''}`}><button className={`tab ${session.activeTab === 'welcome' && !activeTableId && !settingsOpen ? 'active-tab' : ''}`} onContextMenu={(event) => openContextMenu(event, [{ label: 'Activar Inicio', action: () => { setSettingsOpen(false); setActiveTableId(null); activateTab('welcome') } }, { label: 'Cerrar pestaña', action: () => closeWorkTab('welcome') }])} onClick={() => { setSettingsOpen(false); setActiveTableId(null); activateTab('welcome') }}><span className="tab-glyph"><Layers3 size={14} /></span><span>Bienvenida</span></button><button className="table-data-tab-close" aria-label="Cerrar Bienvenida" onClick={() => closeWorkTab('welcome')}><X size={12} /></button></div>}{session.tabs.some((tab) => tab.id === 'query') && <div className={`table-data-tab-entry ${sqlOpen && !settingsOpen ? 'active-tab' : ''}`}><button className={`tab ${sqlOpen && !settingsOpen ? 'active-tab' : ''}`} onContextMenu={(event) => openContextMenu(event, [{ label: 'Activar Consulta SQL', action: () => { setSettingsOpen(false); setActiveTableId(null); openSql() } }, { label: 'Cerrar pestaña', action: () => closeWorkTab('query') }])} onClick={() => { setSettingsOpen(false); setActiveTableId(null); openSql() }}><span className="tab-glyph"><Code2 size={14} /></span><span>Consulta SQL</span></button><button className="table-data-tab-close" aria-label="Cerrar Consulta SQL" onClick={() => closeWorkTab('query')}><X size={12} /></button></div>}{structureTargets.map((target) => <div className={`table-data-tab-entry ${activeStructureId === target.id && !settingsOpen ? 'active-tab' : ''}`} key={target.id}><button className={`tab table-data-tab ${activeStructureId === target.id && !settingsOpen ? 'active-tab' : ''}`} aria-label={`Activar estructura ${target.table}`} onClick={() => { setSettingsOpen(false); setActiveTableId(null); setActiveStructureId(target.id); setSqlOpen(false) }}><span className="tab-glyph"><Database size={14} /></span><span>Estructura · {target.table}</span></button><button className="table-data-tab-close" aria-label={`Cerrar estructura ${target.table}`} onClick={() => closeStructure(target.id)}><X size={12} /></button></div>)}{tableTargets.map((target) => <div className={`table-data-tab-entry ${activeTableId === target.id && !settingsOpen ? 'active-tab' : ''}${draggedTableTab === target.id ? ' dragging' : ''}${dropTableTab === target.id && draggedTableTab !== target.id ? ' drop-target' : ''}`} key={target.id} data-table-tab-id={target.id} onPointerDown={(event) => { if (event.button !== 0 || (event.target as HTMLElement).closest(".table-data-tab-close")) return; tabDragRef.current = { id: target.id, x: event.clientX, y: event.clientY, active: false } }}><button className={`tab table-data-tab ${activeTableId === target.id && !settingsOpen ? 'active-tab' : ''}`} onContextMenu={(event) => openContextMenu(event, [{ label: target.isPinned ? 'Desfijar pestaña' : 'Fijar pestaña', action: () => toggleTablePin(target.id) }, { label: `Activar ${target.table}`, action: () => { setSettingsOpen(false); setActiveTableId(target.id); setActiveStructureId(null); setSqlOpen(false) } }, { label: 'Cerrar pestaña', action: () => closeTable(target.id) }])} aria-label={`Activar tabla ${target.table} de ${target.database}`} onClick={() => { setSettingsOpen(false); setActiveTableId(target.id); setActiveStructureId(null); setSqlOpen(false) }}><span className="tab-glyph"><Table2 size={14} /></span><span>{target.table}</span>{target.isPinned && <Pin className="tab-pin-icon" size={11} aria-label="Pestaña fijada" />} </button><button className="table-data-tab-close" aria-label={`Cerrar tabla ${target.table}`} onClick={() => closeTable(target.id)}><X size={12} /></button></div>)}</div><div className="tab-scroll-rail" aria-hidden="true" style={{ display: tabRail.visible ? undefined : "none" }}><div className="tab-scroll-thumb" style={{ width: tabRail.width, transform: `translateX(${tabRail.left}px)` }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation(); tabRailDragRef.current = { pointerX: event.clientX, scrollLeft: tabStripRef.current?.scrollLeft ?? 0 }; event.currentTarget.setPointerCapture(event.pointerId) }} onPointerMove={(event) => { const start = tabRailDragRef.current; const strip = tabStripRef.current; if (!start || !strip) return; const travel = strip.clientWidth - tabRail.width; if (travel > 0) strip.scrollLeft = start.scrollLeft + (event.clientX - start.pointerX) * (strip.scrollWidth - strip.clientWidth) / travel }} onPointerUp={() => { tabRailDragRef.current = null }} onPointerCancel={() => { tabRailDragRef.current = null }} /></div>
          </div><div className="content-split">
            <div className="center-panel">{settingsOpen ? <SettingsPage close={() => setSettingsOpen(false)} /> : <>{session.tabs.some((tab) => tab.id === 'welcome') && <div className="center-view" style={{ display: session.activeTab === 'welcome' && !activeTableId && !activeStructureId ? 'block' : 'none' }}><Welcome onOpenSql={openSql} /></div>}{session.tabs.length === 0 && !activeTableId && !activeStructureId && <div className="empty-workspace"><Layers3 size={22} /><h2>No hay pestañas abiertas</h2><p>Abre Inicio, crea una consulta o elige una tabla del explorador.</p><div><button className="small-outline" onClick={() => activateTab('welcome')}><Home size={14} /> Abrir Inicio</button><button className="small-outline" onClick={openSql}><Code2 size={14} /> Nueva consulta SQL</button><button className="small-outline" onClick={() => update({ section: 'explorer', sidebarCollapsed: false })}><Database size={14} /> Ir al Explorador</button></div></div>}{structureTargets.map((target) => <div className="center-view" key={target.id} style={{ display: activeStructureId === target.id ? 'block' : 'none' }}><TableStructureView target={target} /></div>)}{tableTargets.map((target) => <div className="center-view" key={target.id} style={{ display: activeTableId === target.id && !activeStructureId ? 'block' : 'none' }}><TableDataView target={target} close={() => closeTable(target.id)} active={!settingsOpen && activeTableId === target.id} refreshRequest={tableRefreshRequests[target.id]} /></div>)}{sqlLoaded && <div className="center-view" style={{ display: sqlOpen && !activeTableId && !activeStructureId ? 'block' : 'none' }}><Suspense fallback={<div className="boot-indicator"><span className="spinner" /> Preparando editor SQL…</div>}><SqlEditor /></Suspense></div>}</>}</div></div>
        </Panel>
      </PanelGroup>
    </div>
    <StatusBar />
    <ContextMenuHost />
    {error && <div className="error-toast" role="alert"><AlertCircle size={17} /><div><strong>Requiere atención</strong><span>{error.message}</span><small>{error.code}</small><button className="retry-save" onClick={() => { void useAppStore.getState().flush().then(clearError).catch(() => undefined) }}>Reintentar guardado</button></div><button aria-label="Cerrar aviso" onClick={clearError}><X size={15} /></button></div>}
    {closeBlocked && <div className="close-options" role="alertdialog" aria-label="Error al guardar antes de cerrar"><p>No se pudo guardar la disposición antes de cerrar.</p><button onClick={() => { setCloseBlocked(false); windowAction('close') }}>Reintentar</button><button onClick={() => { void getCurrentWindow().destroy() }}>Cerrar sin guardar</button></div>}
  </main>
}
