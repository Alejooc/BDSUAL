import { expect, test } from '@playwright/test'

test('agrupa objetos PostgreSQL por esquema y envía el esquema al cargar columnas', async ({ page }) => {
  await page.addInitScript(() => {
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = []
    const w = window as Window & {
      isTauri?: boolean
      __TAURI_INTERNALS__?: { metadata: { currentWindow: { label: string } }; invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> }
      __postgresCalls?: typeof calls
    }
    w.isTauri = true
    w.__postgresCalls = calls
    w.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' } },
      invoke: async (command, args = {}) => {
        calls.push({ command, args })
        if (command === 'plugin:event|listen') return 1
        if (command === 'bootstrap') return { contractVersion: 1, storageStatus: 'ready', preferences: { version: 1, section: 'explorer', sidebarWidth: 24, sidebarCollapsed: false, bottomHeight: 28, bottomCollapsed: false }, session: { version: 1, tabs: [{ id: 'welcome', kind: 'welcome' }], activeTab: 'welcome' } }
        if (command === 'list_connections') return [{ id: 'pg-test', name: 'PostgreSQL local', engine: 'postgresql', host: '127.0.0.1', port: 5432, user: 'postgres', tlsMode: 'disabled', sshEnabled: false }]
        if (command === 'open_connection') return { id: 'pg-test', state: 'connected', serverVersion: '16.4', tlsActive: false }
        if (command === 'list_databases') return ['app_db']
        if (command === 'list_database_objects') return [
          { schema: 'sales', name: 'duplicate_name', kind: 'table' },
          { schema: 'audit', name: 'duplicate_name', kind: 'table' },
          { schema: 'sales', name: 'recent_sales', kind: 'view' },
        ]
        if (command === 'list_columns') return args.objectSchema === 'sales'
          ? [{ name: 'sale_id', dataType: 'integer', isNullable: false, isPrimaryKey: true, primaryKeyAvailable: true, defaultValue: null, defaultAvailable: true }]
          : [{ name: 'event_id', dataType: 'uuid', isNullable: false, isPrimaryKey: false, primaryKeyAvailable: true, defaultValue: null, defaultAvailable: true }]
        if (command === 'read_postgres_table_page') return { columns: ['sale_id'], rows: [['42']], returnedRows: 1, hasMore: false, nextOffset: null, elapsedMs: 2 }
        if (command.startsWith('list_')) return []
        if (command.startsWith('save_') || command.startsWith('plugin:event|unlisten')) return undefined
        return []
      },
    }
  })

  await page.goto('/')
  const explorer = page.getByRole('button', { name: 'Explorador', exact: true })
  if (await explorer.count()) await explorer.click()
  const openConnection = page.getByRole('button', { name: 'Abrir conexión' })
  if (await openConnection.count()) await openConnection.click()
  const connectionDisclosure = page.getByRole('button', { name: /Expandir PostgreSQL local/ })
  if (await connectionDisclosure.count()) await connectionDisclosure.click()
  await expect(page.getByText('app_db', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Expandir base app_db' }).click()

  const sales = page.locator('.object-group').filter({ has: page.locator('.group-label').getByText('sales', { exact: true }) })
  const audit = page.locator('.object-group').filter({ has: page.locator('.group-label').getByText('audit', { exact: true }) })
  await expect(sales.getByText('duplicate_name', { exact: true })).toBeVisible()
  await expect(audit.getByText('duplicate_name', { exact: true })).toBeVisible()
  await sales.getByRole('button', { name: 'Expandir table duplicate_name' }).click()
  await expect(sales.getByText('sale_id', { exact: true })).toBeVisible()
  const calls = await page.evaluate(() => (window as Window & { __postgresCalls?: Array<{ command: string; args?: Record<string, unknown> }> }).__postgresCalls)
  expect(calls?.find((call) => call.command === 'list_columns')?.args).toMatchObject({ database: 'app_db', objectName: 'duplicate_name', objectType: 'table', objectSchema: 'sales' })
  await sales.getByRole('button', { name: 'duplicate_name', exact: true }).click()
  await expect(page.getByRole('region', { name: 'Datos de duplicate_name' })).toContainText('42')
  const pageCall = await page.evaluate(() => (window as Window & { __postgresCalls?: Array<{ command: string; args?: Record<string, unknown> }> }).__postgresCalls?.find((call) => call.command === 'read_postgres_table_page'))
  expect(pageCall?.args).toMatchObject({ connectionId: 'pg-test', database: 'app_db', schema: 'sales', table: 'duplicate_name', offset: 0 })
})
