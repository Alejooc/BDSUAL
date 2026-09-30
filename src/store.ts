import { create } from 'zustand'
import { ipc, type IpcError, type Section, type Session, type UiPreferences } from './ipc'

type Store = { preferences: UiPreferences; session: Session; booting: boolean; error: IpcError | null; layoutEpoch: number; boot: () => Promise<void>; update: (patch: Partial<UiPreferences>) => void; setSession: (session: Session) => void; reset: () => Promise<void>; flush: () => Promise<void>; reportError: (error: IpcError) => void; clearError: () => void }
const initial: UiPreferences = { version: 1, section: 'explorer', sidebarWidth: 24, sidebarCollapsed: false, bottomHeight: 28, bottomCollapsed: false, theme: 'dark', fontScale: 'default', sqlFontSize: 14 }
const initialSession: Session = { version: 1, tabs: [{ id: 'welcome', kind: 'welcome' }], activeTab: 'welcome' }
let saveTimer: ReturnType<typeof setTimeout> | undefined
export const useAppStore = create<Store>((set, get) => ({
  preferences: initial, session: initialSession, booting: true, error: null, layoutEpoch: 0,
  boot: async () => { try { const result = await ipc.bootstrap(); if (result.contractVersion !== 1 || result.storageStatus !== 'ready') throw { code: 'CONTRACT_VERSION', message: 'Esta interfaz y el núcleo usan versiones incompatibles.' }; set({ preferences: result.preferences, session: result.session, booting: false }) } catch (error) { set({ booting: false, error: error as IpcError }) } },
  update: (patch) => {
    const preferences = { ...get().preferences, ...patch }
    set({ preferences, error: null })
    if (saveTimer) clearTimeout(saveTimer)
    saveTimer = setTimeout(() => { void ipc.saveUiPreferences(preferences).catch((error) => set({ error: error as IpcError })) }, 300)
  },
  setSession: (session) => set({ session }),
  reset: async () => { try { if (saveTimer) clearTimeout(saveTimer); const preferences = await ipc.resetUiPreferences(); set((state) => ({ preferences, error: null, layoutEpoch: state.layoutEpoch + 1 })) } catch (error) { set({ error: error as IpcError }) } },
  flush: async () => { if (saveTimer) clearTimeout(saveTimer); try { await ipc.saveUiPreferences(get().preferences); await ipc.saveSession(get().session) } catch (error) { set({ error: error as IpcError }); throw error } },
  reportError: (error) => set({ error }),
  clearError: () => set({ error: null }),
}))
export const sectionLabels: Record<Section, string> = { explorer: 'Explorador', changes: 'Cambios', history: 'Historial' }
