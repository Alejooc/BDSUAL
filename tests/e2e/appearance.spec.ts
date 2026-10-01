import { expect, test } from '@playwright/test'

test('aplica y conserva temas y colores personalizados en preferencias', async ({ page }) => {
  await page.addInitScript(() => {
    const preferences = {
      version: 1,
      section: 'explorer',
      sidebarWidth: 24,
      sidebarCollapsed: false,
      bottomHeight: 28,
      bottomCollapsed: false,
      theme: 'dark',
      customColors: {
        background: '#111316', sidebar: '#17191D', surface: '#15171B', elevated: '#1D2025',
        border: '#292C32', text: '#E4E6E9', muted: '#858992', accent: '#7778EC',
      },
      fontScale: 'default',
      sqlFontSize: 14,
    }
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = []
    const w = window as Window & {
      isTauri?: boolean
      __appearanceCalls?: typeof calls
      __TAURI_INTERNALS__?: {
        metadata: { currentWindow: { label: string } }
        transformCallback: (callback: (...args: unknown[]) => unknown) => string
        invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>
      }
    }
    w.isTauri = true
    w.__appearanceCalls = calls
    ;(window as Window & { __TAURI_EVENT_PLUGIN_INTERNALS__?: { unregisterListener: (event: string, id: number) => void } }).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined }
    w.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' } },
      transformCallback: (callback) => {
        const key = `__tauri_callback_${Date.now()}_${Math.random()}`
        ;(window as unknown as Record<string, unknown>)[key] = callback
        return key
      },
      invoke: async (command, args = {}) => {
        calls.push({ command, args })
        if (command === 'plugin:event|listen') return 1
        if (command === 'plugin:event|unlisten') return undefined
        if (command === 'bootstrap') return {
          contractVersion: 1,
          storageStatus: 'ready',
          preferences: structuredClone(preferences),
          session: { version: 1, tabs: [{ id: 'welcome', kind: 'welcome' }], activeTab: 'welcome' },
        }
        if (command === 'save_ui_preferences') {
          Object.assign(preferences, structuredClone(args.preferences))
          return undefined
        }
        if (command === 'save_session') return undefined
        if (command === 'list_connections') return []
        return undefined
      },
    }
  })

  await page.goto('/')
  await page.getByRole('button', { name: 'Configuración' }).click()
  const settings = page.locator('.settings-page')
  const themes = settings.getByRole('group', { name: 'Temas de color' })

  const light = themes.getByRole('button', { name: 'Claro' })
  await light.click()
  await expect(light).toHaveAttribute('aria-pressed', 'true')
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light')

  const custom = themes.getByRole('button', { name: 'Personalizado' })
  await custom.click()
  const background = settings.getByLabel('Fondo principal')
  await background.fill('#808080')
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'custom')
  await expect(page.locator('html')).toHaveCSS('--theme-background', '#808080')
  await expect(page.locator('html')).toHaveCSS('color-scheme', 'light')

  await themes.getByRole('button', { name: 'Oscuro DBSUAL' }).click()
  await custom.click()
  await expect(background).toHaveValue('#808080')
  await expect(page.locator('html')).toHaveCSS('--theme-background', '#808080')
  await expect.poll(async () => page.evaluate(() => (window as Window & {
    __appearanceCalls?: Array<{ command: string; args?: Record<string, unknown> }>
  }).__appearanceCalls?.some((call) => call.command === 'save_ui_preferences'
    && (call.args?.preferences as { theme?: string; customColors?: { background?: string } } | undefined)?.theme === 'custom'
    && (call.args?.preferences as { customColors?: { background?: string } } | undefined)?.customColors?.background === '#808080'))).toBe(true)
})
