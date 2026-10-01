import { expect, test } from '@playwright/test'

test('abre estructura como pestaña amplia y conserva otras pestañas de trabajo', async ({ page }) => {
  await page.addInitScript(() => {
    const w = window as Window & {
      isTauri?: boolean
      __TAURI_INTERNALS__?: { metadata: { currentWindow: { label: string } }; transformCallback: (callback: (...args: unknown[]) => unknown) => string; invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> }
    }
    w.isTauri = true
    ;(window as Window & { __TAURI_EVENT_PLUGIN_INTERNALS__?: { unregisterListener: (event: string, id: number) => void } }).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined }
    w.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' } },
      transformCallback: (callback) => { const key = `__tauri_callback_${Date.now()}_${Math.random()}`; (window as unknown as Record<string, unknown>)[key] = callback; return key },
      invoke: async (command) => {
        if (command === 'plugin:event|listen') return 1
        if (command === 'bootstrap') return { contractVersion: 1, storageStatus: 'ready', preferences: { version: 1, section: 'explorer', sidebarWidth: 24, sidebarCollapsed: false, bottomHeight: 28, bottomCollapsed: false }, session: { version: 1, tabs: [{ id: 'welcome', kind: 'welcome' }], activeTab: 'welcome' } }
        if (command === 'list_connections') return [{ id: 'mysql-local', name: 'MySQL local', engine: 'mysql', host: '127.0.0.1', port: 3306, user: 'dev', tlsMode: 'disabled', sshEnabled: false }]
        if (command === 'open_connection') return { id: 'mysql-local', state: 'connected', serverVersion: '8.4.11', tlsActive: false }
        if (command === 'list_databases') return ['app_db']
        if (command === 'list_database_objects') return [{ name: 'orders', kind: 'table' }]
        if (command === 'list_columns') return [{ name: 'order_id', dataType: 'bigint', isNullable: false, isPrimaryKey: true, primaryKeyAvailable: true, defaultValue: null, defaultAvailable: true }]
        if (command === 'get_table_structure') return {
          indexes: [{ name: 'PRIMARY', unique: true, indexType: 'BTREE', columns: ['order_id'] }],
          constraints: [{ name: 'orders_customer_fk', kind: 'FOREIGN KEY', columns: ['customer_id'], referencedDatabase: 'app_db', referencedTable: 'customers', referencedColumns: ['id'] }],
        }
        if (command.startsWith('save_') || command === 'plugin:event|unlisten') return undefined
        return []
      },
    }
  })

  await page.goto('/')
  await page.getByRole('button', { name: 'Abrir conexión' }).click()
  await page.getByRole('button', { name: 'Expandir MySQL local' }).click()
  await page.getByRole('button', { name: 'Expandir base app_db' }).click()
  await page.getByRole('button', { name: 'Más opciones para orders' }).click()
  await page.getByRole('menuitem', { name: 'Ver estructura' }).click()

  await expect(page.getByRole('button', { name: 'Activar estructura orders' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Cerrar estructura orders' })).toBeVisible()
  await expect(page.getByRole('heading', { name: 'Estructura · orders' })).toBeVisible()
  await expect(page.locator('.table-structure-workspace')).toContainText('MySQL local · app_db · MySQL')
  await expect(page.locator('.structure-table')).toContainText('order_id')
  await expect(page.locator('.structure-card').filter({ hasText: 'PRIMARY' })).toBeVisible()
  await expect(page.locator('.structure-card').filter({ hasText: 'orders_customer_fk' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Bienvenida', exact: true })).toBeVisible()

  await page.getByRole('button', { name: 'Bienvenida', exact: true }).click()
  await expect(page.locator('.welcome')).toBeVisible()
  await page.getByRole('button', { name: 'Activar estructura orders' }).click()
  await expect(page.getByRole('heading', { name: 'Estructura · orders' })).toBeVisible()
  await page.getByRole('button', { name: 'Cerrar estructura orders' }).click()
  await expect(page.locator('.welcome')).toBeVisible()
})
