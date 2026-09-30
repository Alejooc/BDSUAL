import { useState } from 'react'
import { Eye, RotateCcw, Settings, ShieldCheck, Type } from 'lucide-react'
import type { UiPreferences } from './ipc'
import { useAppStore } from './store'
import VaultSetup from './VaultSetup'

type Category = 'appearance' | 'editor' | 'workspace' | 'security'
const categories: Array<{ id: Category; title: string; description: string; icon: typeof Eye }> = [
  { id: 'appearance', title: 'Apariencia', description: 'Tema y escala de la interfaz', icon: Eye },
  { id: 'editor', title: 'Editor', description: 'Tipografía de SQL', icon: Type },
  { id: 'workspace', title: 'Espacio de trabajo', description: 'Disposición de paneles', icon: Settings },
  { id: 'security', title: 'Seguridad y recuperación', description: 'Depósito cifrado y claves', icon: ShieldCheck },
]

function SelectSetting({ label, description, value, options, onChange }: { label: string; description: string; value: string; options: Array<[string, string]>; onChange: (value: string) => void }) {
  return <label className="settings-control"><span><strong>{label}</strong><small>{description}</small></span><select value={value} onChange={(event) => onChange(event.target.value)}>{options.map(([id, title]) => <option key={id} value={id}>{title}</option>)}</select></label>
}

export default function SettingsPage({ close }: { close: () => void }) {
  const { preferences, update, reset } = useAppStore()
  const [category, setCategory] = useState<Category>('appearance')
  const save = (patch: Partial<UiPreferences>) => update(patch)
  return <div className="settings-page">
    <aside className="settings-nav"><div className="settings-nav-title"><Settings size={17} /><span>Configuración</span></div><p>Personaliza DBSUAL para tu forma de trabajar.</p>{categories.map(({ id, title, description, icon: Icon }) => <button key={id} className={category === id ? 'selected' : ''} onClick={() => setCategory(id)}><Icon size={16} /><span><strong>{title}</strong><small>{description}</small></span></button>)}<button className="settings-reset" onClick={() => void reset()}><RotateCcw size={14} /> Restablecer preferencias</button></aside>
    <section className="settings-content"><header className="settings-page-heading"><div><span className="settings-eyebrow">PREFERENCIAS DE LA APLICACIÓN</span><h1>{categories.find((item) => item.id === category)?.title}</h1><p>{categories.find((item) => item.id === category)?.description}</p></div><button className="settings-close" onClick={close} aria-label="Cerrar Configuración">×</button></header>
      {category === 'appearance' && <div className="settings-section"><h2>Apariencia</h2><p className="settings-help">Ajusta el contraste y el tamaño general del texto.</p><SelectSetting label="Tema" description="Se aplica inmediatamente en toda la ventana." value={preferences.theme} options={[["dark", "Oscuro DBSUAL"], ["light", "Claro"], ["contrast", "Oscuro de alto contraste"]]} onChange={(value) => save({ theme: value as UiPreferences['theme'] })} /><SelectSetting label="Tamaño del texto" description="Escala etiquetas, controles y contenido de la interfaz." value={preferences.fontScale} options={[["compact", "Compacto"], ["default", "Estándar"], ["large", "Grande"]]} onChange={(value) => save({ fontScale: value as UiPreferences['fontScale'] })} /></div>}
      {category === 'editor' && <div className="settings-section"><h2>Editor SQL</h2><p className="settings-help">Preferencias para leer y escribir consultas.</p><SelectSetting label="Tamaño de fuente" description="Tamaño del texto en el editor SQL." value={String(preferences.sqlFontSize)} options={[["12", "12 px"], ["13", "13 px"], ["14", "14 px"], ["16", "16 px"], ["18", "18 px"], ["20", "20 px"]]} onChange={(value) => save({ sqlFontSize: Number(value) })} /></div>}
      {category === 'workspace' && <div className="settings-section"><h2>Disposición</h2><p className="settings-help">La posición y el tamaño de los paneles se guardan automáticamente.</p><div className="settings-readout"><span>Panel lateral</span><strong>{preferences.sidebarCollapsed ? 'Contraído' : `${preferences.sidebarWidth}% del ancho`}</strong></div><div className="settings-readout"><span>Panel inferior</span><strong>{preferences.bottomCollapsed ? 'Contraído' : `${preferences.bottomHeight}% de la altura`}</strong></div><button className="settings-secondary" onClick={() => update({ sidebarWidth: 24, sidebarCollapsed: false, bottomHeight: 28, bottomCollapsed: false })}>Restablecer disposición inicial</button></div>}
      {category === 'security' && <div className="settings-section"><h2>Recuperación y cifrado</h2><p className="settings-help">Administra el depósito local que protege los artefactos de recuperación.</p><VaultSetup /></div>}
    </section>
  </div>
}
