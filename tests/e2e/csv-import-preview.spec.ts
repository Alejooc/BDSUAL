import { expect, test } from '@playwright/test'

test('previsualiza CSV y mantiene bloqueada la mutación hasta validar el recorrido nativo', async ({ page }) => {
  await page.addInitScript(() => {
    const calls: string[] = []
    const w = window as Window & {
      isTauri?: boolean
      __TAURI_INTERNALS__?: { metadata: { currentWindow: { label: string } }; transformCallback: (callback: (...args: unknown[]) => unknown) => string; invoke: (command: string) => Promise<unknown> }
      __csvCalls?: string[]
    }
    w.isTauri = true
    w.__csvCalls = calls
    ;(window as Window & { __TAURI_EVENT_PLUGIN_INTERNALS__?: { unregisterListener: (event: string, id: number) => void } }).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined }
    w.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' } },
      transformCallback: (callback) => { const key = `__tauri_callback_${Date.now()}_${Math.random()}`; (window as unknown as Record<string, unknown>)[key] = callback; return key },
      invoke: async (command) => {
        calls.push(command)
        if (command === 'plugin:event|listen') return 1
        if (command === 'plugin:event|unlisten') return undefined
        if (command === 'bootstrap') return { contractVersion: 1, storageStatus: 'ready', preferences: { version: 1, section: 'explorer', sidebarWidth: 24, sidebarCollapsed: false, bottomHeight: 28, bottomCollapsed: false, theme: 'dark', fontScale: 'default', sqlFontSize: 14 }, session: { version: 1, tabs: [{ id: 'welcome', kind: 'welcome' }], activeTab: 'welcome' } }
        if (command === 'save_ui_preferences' || command === 'save_session') return undefined
        if (command === 'read_table_page') return { columns: ['id', 'name', 'note'], rows: [['1', 'Ana', null]], returnedRows: 1, hasMore: false, nextOffset: null, elapsedMs: 1 }
        if (command === 'list_columns') return []
        if (command === 'prepare_mysql_csv_import' || command === 'prepare_mysql_csv_import_revert') return { revision: { id: command === 'prepare_mysql_csv_import' ? 'csv-revision' : 'csv-revert-revision', projectId: 'project', revisionNumber: 4, message: command === 'prepare_mysql_csv_import' ? 'Importar CSV' : 'Revertir importación CSV', status: 'draft', recoveryState: 'verified', planSha256: 'hash', artifactId: 'artifact', operationCount: 2, createdAtMs: 1, confirmedAtMs: null }, headers: ['id', 'name', 'note'], rowCount: 2, sampleRows: [['2', 'Luis', null], ['3', 'Ana;Maria', 'NULL']] }
        if (command === 'confirm_history_revision' || command === 'apply_mysql_row_update') return undefined
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
  await dialog.getByLabel('Archivo CSV').setInputFiles({ name: 'invalid.csv', mimeType: 'text/csv', buffer: Buffer.from('id;name\n4\n') })
  await expect(dialog.getByRole('alert').filter({ hasText: 'tiene 1 campos; se esperaban 2' })).toBeVisible()
  // The exporter prefixes a literal NULL marker with one backslash.
  await dialog.getByLabel('Archivo CSV').setInputFiles({ name: 'clientes.csv', mimeType: 'text/csv', buffer: Buffer.from('id;name;note\n2;Luis;NULL\n3;\"Ana;Maria\";\\NULL\n') })
  await expect(dialog.getByText('2 filas válidas · 3 columnas reconocidas. NULL se mostrará como tal.')).toBeVisible()
  const actualNullCell = dialog.locator('tbody tr').nth(0).getByRole('cell').nth(2)
  const literalNullCell = dialog.locator('tbody tr').nth(1).getByRole('cell').nth(2)
  await expect(actualNullCell.locator('em')).toHaveText('NULL')
  await expect(literalNullCell).toHaveText('NULL')
  await expect(literalNullCell.locator('em')).toHaveCount(0)
  await expect(dialog.getByRole('button', { name: 'Preparar importación' })).toBeDisabled()
  await expect(dialog.getByText(/habilitará después de comprobar este recorrido desde la ventana Tauri nativa/)).toBeVisible()

  const mutationCalls = await page.evaluate(() => (window as Window & { __csvCalls?: string[] }).__csvCalls?.filter((command) => /apply|insert|import|confirm_history/i.test(command)))
  expect(mutationCalls).toEqual([])
})
