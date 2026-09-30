import { expect, test } from '@playwright/test'

test('ejecuta la lectura PostgreSQL mediante su comando IPC y bloquea preparar cambios', async ({ page }) => {
  await page.addInitScript(() => {
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = []
    const w = window as Window & {
      isTauri?: boolean
      __TAURI_INTERNALS__?: { metadata: { currentWindow: { label: string } }; transformCallback: (callback: (...args: unknown[]) => unknown) => string; invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> }
      __postgresSqlCalls?: typeof calls
    }
    w.isTauri = true
    w.__postgresSqlCalls = calls
    w.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' } },
      transformCallback: () => '1',
      invoke: async (command, args = {}) => {
        calls.push({ command, args })
        if (command === 'plugin:event|listen') return 1
        if (command === 'bootstrap') return { contractVersion: 1, storageStatus: 'ready', preferences: { version: 1, section: 'explorer', sidebarWidth: 24, sidebarCollapsed: false, bottomHeight: 28, bottomCollapsed: false }, session: { version: 1, tabs: [{ id: 'welcome', kind: 'welcome' }], activeTab: 'welcome' } }
        if (command === 'list_connections') return [{ id: 'pg-test', name: 'PostgreSQL local', engine: 'postgresql', host: '127.0.0.1', port: 5432, user: 'postgres', tlsMode: 'disabled', sshEnabled: false }]
        if (command === 'open_connection') return { id: 'pg-test', state: 'connected', serverVersion: '16.4', tlsActive: false }
        if (command === 'list_databases') return ['postgres']
        if (command === 'execute_postgres_read_query') return { columns: ['value', 'empty'], rows: [['1', null]], returnedRows: 1, hasMore: false, nextOffset: null, elapsedMs: 4 }
        if (command.startsWith('list_')) return []
        if (command.startsWith('save_') || command.startsWith('plugin:event|unlisten')) return undefined
        return []
      },
    }
  })

  await page.goto('/')
  await page.getByRole('button', { name: 'Abrir editor SQL' }).click()
  const editor = page.getByRole('region', { name: 'Editor SQL' })
  await expect(editor).toBeVisible({ timeout: 20_000 })
  await editor.getByLabel('Conexión para la consulta').selectOption('pg-test')
  await editor.getByRole('button', { name: 'Conectar / actualizar' }).click()
  await expect(editor.getByLabel('Base de datos de destino')).toHaveValue('postgres')
  await expect(editor.getByRole('button', { name: 'Preparar cambio' })).toBeDisabled()
  await editor.getByRole('button', { name: 'Ejecutar lectura' }).click()
  await expect(editor.getByRole('table').getByText('1', { exact: true })).toBeVisible()
  await expect(editor.getByRole('table').getByText('NULL', { exact: true })).toBeVisible()
  const call = await page.evaluate(() => (window as Window & { __postgresSqlCalls?: Array<{ command: string; args?: Record<string, unknown> }> }).__postgresSqlCalls?.find((item) => item.command === 'execute_postgres_read_query'))
  expect(call?.args).toMatchObject({ connectionId: 'pg-test', database: 'postgres', sql: 'SELECT 1;', offset: 0 })
})
