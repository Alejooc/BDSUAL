import { expect, test } from '@playwright/test'

test('previsualiza CSV MySQL y mantiene bloqueada toda mutación sin respaldo verificable', async ({ page }) => {
  await page.addInitScript(() => {
    const calls: string[] = []
    const w = window as Window & {
      isTauri?: boolean
      __TAURI_INTERNALS__?: { metadata: { currentWindow: { label: string } }; invoke: (command: string) => Promise<unknown> }
      __csvCalls?: string[]
    }
    w.isTauri = true
    w.__csvCalls = calls
    w.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' } },
      invoke: async (command) => {
        calls.push(command)
        if (command === 'plugin:event|listen') return 1
        if (command === 'plugin:event|unlisten') return undefined
        if (command === 'bootstrap') return { contractVersion: 1, storageStatus: 'ready', preferences: { version: 1, section: 'explorer', sidebarWidth: 24, sidebarCollapsed: false, bottomHeight: 28, bottomCollapsed: false, theme: 'dark', fontScale: 'default', sqlFontSize: 14 }, session: { version: 1, tabs: [{ id: 'welcome', kind: 'welcome' }], activeTab: 'welcome' } }
        if (command === 'save_ui_preferences' || command === 'save_session') return undefined
        if (command === 'read_table_page') return { columns: ['id', 'name', 'note'], rows: [['1', 'Ana', null]], returnedRows: 1, hasMore: false, nextOffset: null, elapsedMs: 1 }
        if (command === 'list_columns') return []
        throw new Error(`Comando inesperado: ${command}`)
      },
    }
  })

  await page.goto('/')
  await page.evaluate(() => window.dispatchEvent(new CustomEvent('dbsual:open-table-data', { detail: { engine: 'mysql', connectionId: 'csv-conn', database: 'crm', table: 'clientes' } })))
  const grid = page.getByRole('region', { name: 'Datos de clientes' })
  const open = grid.getByRole('button', { name: 'Vista previa CSV' })
  await expect(open).toBeEnabled()
  await open.click()
  const dialog = page.getByRole('dialog', { name: 'Vista previa de importación CSV' })
  await dialog.getByLabel('Separador CSV').selectOption(';')
  await dialog.getByLabel('Marcador NULL').fill('NULL')
  // The exporter prefixes a literal NULL marker with one backslash.
  await dialog.getByLabel('Archivo CSV').setInputFiles({ name: 'clientes.csv', mimeType: 'text/csv', buffer: Buffer.from('id;name;note\n2;Luis;NULL\n3;\"Ana;Maria\";\\NULL\n') })
  await expect(dialog.getByText('2 filas válidas · 3 columnas reconocidas. NULL se mostrará como tal.')).toBeVisible()
  const actualNullCell = dialog.locator('tbody tr').nth(0).getByRole('cell').nth(2)
  const literalNullCell = dialog.locator('tbody tr').nth(1).getByRole('cell').nth(2)
  await expect(actualNullCell.locator('em')).toHaveText('NULL')
  await expect(literalNullCell).toHaveText('NULL')
  await expect(literalNullCell.locator('em')).toHaveCount(0)
  await expect(dialog.getByText(/La aplicación permanece bloqueada/)).toBeVisible()
  await expect(dialog.getByRole('button', { name: 'Aplicar importación' })).toBeDisabled()

  await dialog.getByLabel('Archivo CSV').setInputFiles({ name: 'invalid.csv', mimeType: 'text/csv', buffer: Buffer.from('id;name\n4\n') })
  await expect(dialog.getByRole('alert').filter({ hasText: 'tiene 1 campos; se esperaban 2' })).toBeVisible()
  const mutationCalls = await page.evaluate(() => (window as Window & { __csvCalls?: string[] }).__csvCalls?.filter((command) => /apply|insert|import/i.test(command)))
  expect(mutationCalls).toEqual([])
})
