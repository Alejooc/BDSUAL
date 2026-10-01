import { expect, test } from '@playwright/test'

test('inserta una fila MySQL tras revisar y prepara su compensación como otra revisión', async ({ page }) => {
  await page.addInitScript(() => {
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = []
    let status = 'draft'
    let inserted = false
    let revisionId = 'insert-revision'
    const w = window as Window & { isTauri?: boolean; __TAURI_INTERNALS__?: { metadata: { currentWindow: { label: string } }; transformCallback: (callback: (...args: unknown[]) => unknown) => string; invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> }; __insertCalls?: typeof calls }
    w.isTauri = true
    w.__insertCalls = calls
    ;(window as Window & { __TAURI_EVENT_PLUGIN_INTERNALS__?: { unregisterListener: (event: string, id: number) => void } }).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined }
    w.__TAURI_INTERNALS__ = { metadata: { currentWindow: { label: 'main' } }, transformCallback: (callback) => { const key = `__tauri_callback_${Date.now()}_${Math.random()}`; (window as unknown as Record<string, unknown>)[key] = callback; return key }, invoke: async (command, args = {}) => {
      calls.push({ command, args })
      if (command === 'plugin:event|listen') return 1
      if (command === 'plugin:event|unlisten' || command === 'save_ui_preferences' || command === 'save_session' || command === 'confirm_history_revision') { if (command === 'confirm_history_revision') status = 'confirmed'; return undefined }
      if (command === 'bootstrap') return { contractVersion: 1, storageStatus: 'ready', preferences: { version: 1, section: 'explorer', sidebarWidth: 24, sidebarCollapsed: false, bottomHeight: 28, bottomCollapsed: false, theme: 'dark', fontScale: 'default', sqlFontSize: 14 }, session: { version: 1, tabs: [{ id: 'welcome', kind: 'welcome' }], activeTab: 'welcome' } }
      if (command === 'list_columns') return [
        { name: 'id', dataType: 'int', isNullable: false, isPrimaryKey: true, primaryKeyAvailable: true, defaultValue: null, defaultAvailable: true },
        { name: 'name', dataType: 'varchar', isNullable: false, isPrimaryKey: false, primaryKeyAvailable: true, defaultValue: null, defaultAvailable: true },
      ]
      if (command === 'read_table_page') return { columns: ['id', 'name'], rows: inserted ? [['11', 'Eva']] : [], returnedRows: inserted ? 1 : 0, hasMore: false, nextOffset: null, elapsedMs: 1 }
      if (command === 'prepare_mysql_row_insert') return { revision: { id: 'insert-revision', projectId: 'project', revisionNumber: 1, message: 'Insertar fila', status: 'draft', recoveryState: 'verified', planSha256: 'a'.repeat(64), artifactId: 'artifact', operationCount: 1, createdAtMs: 1, confirmedAtMs: null }, primaryKey: [{ column: 'id', value: '11' }], columnName: '', oldValue: null, newValue: null, operation: 'insert', values: args.values }
      if (command === 'prepare_mysql_row_revert') { revisionId = 'delete-revision'; status = 'draft'; return { revision: { id: revisionId, projectId: 'project', revisionNumber: 2, message: 'Revertir inserción de fila', status, recoveryState: 'verified', planSha256: 'b'.repeat(64), artifactId: 'artifact-2', operationCount: 1, createdAtMs: 2, confirmedAtMs: null }, primaryKey: [{ column: 'id', value: '11' }], columnName: '', oldValue: null, newValue: null, operation: 'delete', values: [{ column: 'id', value: '11' }, { column: 'name', value: 'Eva' }] } }
      if (command === 'apply_mysql_row_update') { if (status !== 'confirmed' || args.revisionId !== revisionId) throw new Error('Debe confirmarse una revisión antes de aplicar'); status = 'applied'; inserted = revisionId === 'insert-revision'; return undefined }
      throw new Error(`Comando inesperado: ${command}`)
    } }
  })
  await page.goto('/')
  await page.evaluate(() => window.dispatchEvent(new CustomEvent('dbsual:open-table-data', { detail: { engine: 'mysql', connectionId: 'conn', database: 'crm', table: 'clientes' } })))
  const grid = page.getByRole('region', { name: 'Datos de clientes' })
  await grid.getByRole('button', { name: 'Añadir fila' }).click()
  const draft = page.getByRole('dialog', { name: 'Preparar fila nueva' })
  await draft.getByRole('textbox', { name: 'Valor para id' }).fill('11')
  await draft.getByRole('textbox', { name: 'Valor para name' }).fill('Eva')
  await draft.getByRole('button', { name: 'Revisar fila' }).click()
  const review = page.getByRole('dialog', { name: 'Revisar fila nueva' })
  await expect(review.getByText('id = 11', { exact: true })).toBeVisible()
  expect(await page.evaluate(() => (window as Window & { __insertCalls?: Array<{ command: string }> }).__insertCalls?.some((call) => call.command === 'apply_mysql_row_update'))).toBe(false)
  await review.getByRole('button', { name: 'Confirmar revisión' }).click()
  expect(await page.evaluate(() => (window as Window & { __insertCalls?: Array<{ command: string }> }).__insertCalls?.some((call) => call.command === 'apply_mysql_row_update'))).toBe(false)
  await review.getByRole('button', { name: 'Aplicar fila' }).click()
  await expect(grid.getByText('Eva', { exact: true })).toBeVisible()
  await grid.getByRole('button', { name: 'Preparar reversión' }).click()
  const deletion = page.getByRole('dialog', { name: 'Revisar reversión de inserción' })
  await expect(deletion.getByText(/Eva/)).toBeVisible()
  await deletion.getByRole('button', { name: 'Confirmar revisión' }).click()
  await deletion.getByRole('button', { name: 'Aplicar reversión' }).click()
  await expect(grid.getByText('Eva', { exact: true })).toHaveCount(0)
  const commands = await page.evaluate(() => (window as Window & { __insertCalls?: Array<{ command: string; args?: Record<string, unknown> }> }).__insertCalls)
  expect(commands?.find((call) => call.command === 'prepare_mysql_row_insert')?.args?.values).toEqual([{ column: 'id', value: '11' }, { column: 'name', value: 'Eva' }])
  expect(commands?.filter((call) => call.command === 'apply_mysql_row_update').map((call) => call.args?.revisionId)).toEqual(['insert-revision', 'delete-revision'])
})
