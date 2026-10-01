import { beforeEach, describe, expect, it, vi } from 'vitest'

const ipcMocks = vi.hoisted(() => ({
  resetUiPreferences: vi.fn(),
  saveUiPreferences: vi.fn(),
  saveSession: vi.fn(),
  bootstrap: vi.fn(),
}))

vi.mock('./ipc', () => ({ ipc: ipcMocks }))

import { useAppStore } from './store'

const initialPreferences = {
  version: 1 as const,
  section: 'explorer' as const,
  sidebarWidth: 24,
  sidebarCollapsed: false,
  bottomHeight: 28,
  bottomCollapsed: false,
  theme: 'dark' as const,
  customColors: { background: '#111316', sidebar: '#17191d', surface: '#15171b', elevated: '#1d2025', border: '#292c32', text: '#e4e6e9', muted: '#858992', accent: '#7778ec' },
  fontScale: 'default' as const,
  sqlFontSize: 14,
}
const initialSession = { version: 1 as const, tabs: [{ id: 'welcome' as const, kind: 'welcome' as const }], activeTab: 'welcome' as const }

describe('estado de preferencias', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.clearAllMocks()
    ipcMocks.resetUiPreferences.mockResolvedValue(initialPreferences)
    ipcMocks.saveUiPreferences.mockResolvedValue(undefined)
    ipcMocks.saveSession.mockResolvedValue(undefined)
    useAppStore.setState({ preferences: initialPreferences, session: initialSession, error: null, layoutEpoch: 0 })
  })

  it('guarda el último diseño después de que termina la edición', async () => {
    useAppStore.getState().update({ section: 'history' })
    useAppStore.getState().update({ sidebarWidth: 31 })

    expect(useAppStore.getState().preferences).toMatchObject({ section: 'history', sidebarWidth: 31 })
    expect(ipcMocks.saveUiPreferences).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(300)

    expect(ipcMocks.saveUiPreferences).toHaveBeenCalledTimes(1)
    expect(ipcMocks.saveUiPreferences).toHaveBeenCalledWith({ ...initialPreferences, section: 'history', sidebarWidth: 31 })
  })

  it('restablece el diseño y actualiza el panel', async () => {
    useAppStore.setState({ preferences: { ...initialPreferences, sidebarWidth: 38 }, layoutEpoch: 2 })

    await useAppStore.getState().reset()

    expect(useAppStore.getState().preferences).toEqual(initialPreferences)
    expect(useAppStore.getState().layoutEpoch).toBe(3)
    expect(ipcMocks.resetUiPreferences).toHaveBeenCalledOnce()
  })

  it('restaura la pestaña activa y guarda solo metadatos de sesión al cerrar', async () => {
    const session = { version: 1 as const, tabs: [{ id: 'welcome' as const, kind: 'welcome' as const }, { id: 'query' as const, kind: 'query' as const }], activeTab: 'query' as const }
    ipcMocks.bootstrap.mockResolvedValue({ contractVersion: 1, storageStatus: 'ready', preferences: initialPreferences, session })

    await useAppStore.getState().boot()
    expect(useAppStore.getState().session).toEqual(session)

    useAppStore.getState().setSession(session)

    await useAppStore.getState().flush()

    expect(ipcMocks.saveSession).toHaveBeenCalledWith(session)
  })
})
