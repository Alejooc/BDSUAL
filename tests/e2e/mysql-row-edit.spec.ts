import { expect, test } from '@playwright/test'

test('edita una celda MySQL mediante revisión sin aplicar al preparar o confirmar', async ({ page }) => {
  await page.addInitScript(() => {
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = []
    let revision = { id: 'row-revision', projectId: 'project-row', revisionNumber: 4, message: 'Actualizar fila', status: 'draft', recoveryState: 'verified', planSha256: 'a'.repeat(64), artifactId: 'row-artifact', operationCount: 1, createdAtMs: 2, confirmedAtMs: null as number | null }
    let storedValue: string | null = 'Ana'
    const w = window as Window & {
      isTauri?: boolean
      __TAURI_INTERNALS__?: { metadata: { currentWindow: { label: string } }; transformCallback: (callback: (...args: unknown[]) => unknown) => string; invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> }
      __rowEditCalls?: typeof calls
    }
    w.isTauri = true
    w.__rowEditCalls = calls
    ;(window as Window & { __TAURI_EVENT_PLUGIN_INTERNALS__?: { unregisterListener: (event: string, id: number) => void } }).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined }
    w.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' } },
      transformCallback: (callback) => { const key = `__tauri_callback_${Date.now()}_${Math.random()}`; (window as unknown as Record<string, unknown>)[key] = callback; return key },
      invoke: async (command, args = {}) => {
        calls.push({ command, args })
        if (command === 'plugin:event|listen') return 1
        if (command === 'plugin:event|unlisten') return undefined
        switch (command) {
          case 'bootstrap': return { contractVersion: 1, storageStatus: 'ready', preferences: { version: 1, section: 'explorer', sidebarWidth: 24, sidebarCollapsed: false, bottomHeight: 28, bottomCollapsed: false, theme: 'dark', fontScale: 'default', sqlFontSize: 14 }, session: { version: 1, tabs: [{ id: 'welcome', kind: 'welcome' }], activeTab: 'welcome' } }
          case 'save_ui_preferences': case 'save_session': return undefined
          case 'list_columns': return [
            { name: 'id', dataType: 'int', isNullable: false, isPrimaryKey: true, primaryKeyAvailable: true, defaultValue: null, defaultAvailable: true },
            { name: 'name', dataType: 'varchar', isNullable: true, isPrimaryKey: false, primaryKeyAvailable: true, defaultValue: null, defaultAvailable: true },
          ]
          case 'read_table_page': return { columns: ['id', 'name'], rows: [['7', storedValue]], returnedRows: 1, hasMore: false, nextOffset: null, elapsedMs: 1 }
          case 'prepare_mysql_row_update':
            if (calls.some((call) => call.command === 'apply_mysql_row_update')) throw new Error('Preparar nunca debe aplicar')
            return { revision, primaryKey: args.primaryKey, columnName: args.columnName, oldValue: storedValue, newValue: args.newValue }
          case 'prepare_mysql_row_revert':
            if (args.revisionId !== 'row-revision' || revision.status !== 'applied') throw new Error('Solo se revierte una edición aplicada')
            revision = { ...revision, id: 'row-revert-revision', revisionNumber: 5, message: 'Revertir edición de fila', status: 'draft', confirmedAtMs: null }
            return { revision, primaryKey: [{ column: 'id', value: '7' }], columnName: 'name', oldValue: storedValue, newValue: 'Ana' }
          case 'confirm_history_revision': revision.status = 'confirmed'; revision.confirmedAtMs = 3; return undefined
          case 'apply_mysql_row_update':
            if (revision.status !== 'confirmed' || args.revisionId !== revision.id) throw new Error('Se requiere una revisión confirmada')
            storedValue = 'Ana María'
            if (revision.id === 'row-revert-revision') storedValue = 'Ana'
            revision.status = 'applied'
            return undefined
          default: throw new Error(`Comando inesperado: ${command}`)
        }
      },
    }
  })

  await page.goto('/')
  await page.evaluate(() => window.dispatchEvent(new CustomEvent('dbsual:open-table-data', { detail: { engine: 'mysql', connectionId: 'conn-row', database: 'crm', table: 'clientes' } })))
  const grid = page.getByRole('region', { name: 'Datos de clientes' })
  await expect(grid.getByText('Ana', { exact: true })).toBeVisible()

  await grid.getByRole('button', { name: 'Editar name, fila 1' }).click()
  await grid.getByRole('textbox', { name: 'Nuevo valor para name' }).fill('Ana María')
  await grid.getByRole('button', { name: 'Revisar cambio de name' }).click()
  const review = page.getByRole('dialog', { name: 'Revisar cambio de fila' })
  await expect(review.getByText('Valor actual')).toBeVisible()
  await expect(review.getByText('Valor propuesto')).toBeVisible()
  const prepared = await page.evaluate(() => (window as Window & { __rowEditCalls?: Array<{ command: string; args?: Record<string, unknown> }> }).__rowEditCalls?.find((call) => call.command === 'prepare_mysql_row_update'))
  expect(prepared?.args).toMatchObject({ connectionId: 'conn-row', databaseName: 'crm', tableName: 'clientes', primaryKey: [{ column: 'id', value: '7' }], columnName: 'name', newValue: 'Ana María' })
  expect(await page.evaluate(() => (window as Window & { __rowEditCalls?: Array<{ command: string }> }).__rowEditCalls?.some((call) => call.command === 'apply_mysql_row_update'))).toBe(false)

  await review.getByRole('button', { name: 'Cerrar revisión' }).click()
  await expect(grid.getByText('El borrador sigue guardado en el historial; no se descartó.')).toBeVisible()
  expect(await page.evaluate(() => (window as Window & { __rowEditCalls?: Array<{ command: string }> }).__rowEditCalls?.some((call) => call.command.includes('discard')))).toBe(false)
  expect(await page.evaluate(() => (window as Window & { __rowEditCalls?: Array<{ command: string }> }).__rowEditCalls?.some((call) => call.command === 'apply_mysql_row_update'))).toBe(false)

  await grid.getByRole('button', { name: 'Reabrir revisión' }).click()
  const secondReview = page.getByRole('dialog', { name: 'Revisar cambio de fila' })
  await secondReview.getByRole('button', { name: 'Confirmar revisión' }).click()
  await expect(secondReview.getByText('Revisión confirmada y guardada en el historial. Aún no se ha aplicado.')).toBeVisible()
  expect(await page.evaluate(() => (window as Window & { __rowEditCalls?: Array<{ command: string }> }).__rowEditCalls?.some((call) => call.command === 'apply_mysql_row_update'))).toBe(false)
  await secondReview.getByRole('button', { name: 'Aplicar cambio' }).click()
  await expect(page.getByRole('dialog', { name: 'Revisar cambio de fila' })).toBeHidden()
  await expect(grid.getByText('Ana María', { exact: true })).toBeVisible()
  const calls = await page.evaluate(() => (window as Window & { __rowEditCalls?: Array<{ command: string; args?: Record<string, unknown> }> }).__rowEditCalls)
  expect(calls?.find((call) => call.command === 'confirm_history_revision')?.args).toEqual({ revisionId: 'row-revision' })
  expect(calls?.find((call) => call.command === 'apply_mysql_row_update')?.args).toEqual({ revisionId: 'row-revision' })

  await grid.getByRole('button', { name: 'Preparar reversión' }).click()
  const revertReview = page.getByRole('dialog', { name: 'Revisar reversión compensatoria' })
  await expect(revertReview.getByText('Esto prepara una nueva revisión con el valor anterior. No cambia la base ni elimina la revisión original. Confirma y aplica esta revisión por separado.')).toBeVisible()
  expect(await page.evaluate(() => (window as Window & { __rowEditCalls?: Array<{ command: string }> }).__rowEditCalls?.filter((call) => call.command === 'apply_mysql_row_update').length)).toBe(1)
  const preparedRevert = await page.evaluate(() => (window as Window & { __rowEditCalls?: Array<{ command: string; args?: Record<string, unknown> }> }).__rowEditCalls?.find((call) => call.command === 'prepare_mysql_row_revert'))
  expect(preparedRevert?.args).toEqual({ revisionId: 'row-revision' })
  await revertReview.getByRole('button', { name: 'Confirmar revisión' }).click()
  expect(await page.evaluate(() => (window as Window & { __rowEditCalls?: Array<{ command: string }> }).__rowEditCalls?.filter((call) => call.command === 'apply_mysql_row_update').length)).toBe(1)
  await revertReview.getByRole('button', { name: 'Aplicar reversión' }).click()
  await expect(page.getByRole('dialog', { name: 'Revisar reversión compensatoria' })).toBeHidden()
  await expect(grid.getByText('Ana', { exact: true })).toBeVisible()
  const revertApply = await page.evaluate(() => (window as Window & { __rowEditCalls?: Array<{ command: string; args?: Record<string, unknown> }> }).__rowEditCalls?.filter((call) => call.command === 'apply_mysql_row_update').at(-1))
  expect(revertApply?.args).toEqual({ revisionId: 'row-revert-revision' })
})

test('la vista previa web informa que la edición requiere el núcleo de escritorio', async ({ page }) => {
  await page.goto('/')
  await page.evaluate(() => window.dispatchEvent(new CustomEvent('dbsual:open-table-data', { detail: { engine: 'mysql', connectionId: 'conn-preview', database: 'crm', table: 'clientes' } })))
  await expect(page.getByRole('region', { name: 'Datos de clientes' }).getByText('La edición protegida y el historial requieren el núcleo de DBSUAL; en la vista previa solo puedes consultar.')).toBeVisible()
})
