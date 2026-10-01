import { useState } from 'react'
import { Eye, RotateCcw, Settings, ShieldCheck, Type } from 'lucide-react'
import type { ThemeName, UiPreferences } from './ipc'
import { useAppStore } from './store'
import VaultSetup from './VaultSetup'

type Category = 'appearance' | 'editor' | 'workspace' | 'security'
const categories: Array<{ id: Category; title: string; description: string; icon: typeof Eye }> = [
  { id: 'appearance', title: 'Apariencia', description: 'Temas, colores y escala de la interfaz', icon: Eye },
  { id: 'editor', title: 'Editor', description: 'Tipografía de SQL', icon: Type },
  { id: 'workspace', title: 'Espacio de trabajo', description: 'Ancho del panel lateral', icon: Settings },
  { id: 'security', title: 'Seguridad y recuperación', description: 'Depósito cifrado y claves', icon: ShieldCheck },
]
const themes: Array<{ id: ThemeName; label: string; colors: string[] }> = [
  { id: 'dark', label: 'Oscuro DBSUAL', colors: ['#111316', '#17191d', '#7778ec'] },
  { id: 'light', label: 'Claro', colors: ['#f4f6fa', '#e8ebf1', '#5355c8'] },
  { id: 'contrast', label: 'Alto contraste', colors: ['#090a0c', '#111318', '#f4d35e'] },
  { id: 'midnight', label: 'Medianoche', colors: ['#0b1020', '#111a2d', '#78a9ff'] },
  { id: 'nord', label: 'Nórdico', colors: ['#2e3440', '#3b4252', '#88c0d0'] },
  { id: 'forest', label: 'Bosque', colors: ['#101a17', '#17251f', '#79c99e'] },
  { id: 'custom', label: 'Personalizado', colors: ['#111316', '#24262c', '#a6a7ff'] },
]
const colorLabels: Array<[keyof UiPreferences['customColors'], string]> = [
  ['background', 'Fondo principal'], ['sidebar', 'Panel lateral'], ['surface', 'Superficies'], ['elevated', 'Controles y elementos elevados'],
  ['border', 'Bordes'], ['text', 'Texto principal'], ['muted', 'Texto secundario'], ['accent', 'Color de énfasis'],
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
      {category === 'appearance' && <div className="settings-section"><h2>Apariencia</h2><p className="settings-help">Elige una paleta completa o personaliza los colores de la interfaz. Los cambios se aplican al instante y se guardan en este dispositivo.</p><div className="appearance-theme-grid" role="group" aria-label="Temas de color">{themes.map((theme) => <button type="button" key={theme.id} className={`appearance-theme-card ${preferences.theme === theme.id ? 'selected' : ''}`} aria-pressed={preferences.theme === theme.id} onClick={() => save({ theme: theme.id })}><span className="appearance-theme-swatches" aria-hidden="true">{theme.colors.map((color) => <i key={color} style={{ backgroundColor: color }} />)}</span><span>{theme.label}</span></button>)}</div>{preferences.theme === 'custom' && <fieldset className="appearance-color-editor"><legend>Colores personalizados</legend>{colorLabels.map(([token, label]) => <label className="appearance-color-field" key={token}><span>{label}</span><input type="color" aria-label={label} value={preferences.customColors[token]} onChange={(event) => save({ theme: 'custom', customColors: { ...preferences.customColors, [token]: event.target.value.toUpperCase() } })} /><code>{preferences.customColors[token]}</code></label>)}</fieldset>}<SelectSetting label="Tamaño del texto" description="Escala etiquetas, controles y contenido de la interfaz." value={preferences.fontScale} options={[["compact", "Compacto"], ["default", "Estándar"], ["large", "Grande"]]} onChange={(value) => save({ fontScale: value as UiPreferences['fontScale'] })} /></div>}
      {category === 'editor' && <div className="settings-section"><h2>Editor SQL</h2><p className="settings-help">Preferencias para leer y escribir consultas.</p><SelectSetting label="Tamaño de fuente" description="Tamaño del texto en el editor SQL." value={String(preferences.sqlFontSize)} options={[["12", "12 px"], ["13", "13 px"], ["14", "14 px"], ["16", "16 px"], ["18", "18 px"], ["20", "20 px"]]} onChange={(value) => save({ sqlFontSize: Number(value) })} /></div>}
      {category === 'workspace' && <div className="settings-section"><h2>Disposición</h2><p className="settings-help">El ancho del panel lateral se guarda automáticamente.</p><div className="settings-readout"><span>Panel lateral</span><strong>{preferences.sidebarCollapsed ? 'Contraído' : `${preferences.sidebarWidth}% del ancho`}</strong></div><button className="settings-secondary" onClick={() => update({ sidebarWidth: 24, sidebarCollapsed: false })}>Restablecer disposición inicial</button></div>}
      {category === 'security' && <div className="settings-section"><h2>Recuperación y cifrado</h2><p className="settings-help">Administra el depósito local que protege los artefactos de recuperación.</p><VaultSetup /></div>}
    </section>
  </div>
}
