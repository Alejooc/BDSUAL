import { expect, test } from '@playwright/test'

test('Historial prepara, confirma y aplica una reversión de edición solo por acción explícita', async ({ page }) => {
  await page.addInitScript(() => {
    const project = { id: 'project-history', connectionId: 'connection-history', databaseName: 'catalogo', engine: 'mysql', serverVersion: '8.4.11', createdAtMs: 1 }
    const originalEdit = { id: 'applied-edit', projectId: project.id, revisionNumber: 7, message: 'Actualizar fila', status: 'applied', recoveryState: 'verified', planSha256: 'a'.repeat(64), artifactId: 'edit-artifact', operationCount: 1, createdAtMs: 2, confirmedAtMs: 3 }
    const appliedInsert = { ...originalEdit, id: 'applied-insert', revisionNumber: 8, message: 'Insertar fila', planSha256: 'b'.repeat(64) }
    const revisions = [originalEdit, appliedInsert]
    const rowPlan = { version: 1, connectionId: project.connectionId, databaseName: project.databaseName, tableName: 'clientes', serverUuid: 'server-fixture', serverVersion: project.serverVersion, primaryKey: [{ column: 'id', value: '42' }], columnName: 'nombre', oldValue: 'Ana María', newValue: 'Ana', compensatesRevisionId: 'applied-edit', rowSha256: 'c'.repeat(64), columnNames: ['id', 'nombre'] }
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = []
    let mutationApplied = false
    let tableValue = 'Ana María'
    let tableReads = 0
    const w = window as Window & { isTauri?: boolean; __TAURI_INTERNALS__?: { metadata: { currentWindow: { label: string } }; transformCallback: (callback: (...args: unknown[]) => unknown) => string; invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> }; __historyCalls?: typeof calls; __mutationApplied?: boolean; __tableReads?: number }
    w.isTauri = true
    w.__historyCalls = calls
    w.__mutationApplied = mutationApplied
    w.__tableReads = tableReads
    ;(window as Window & { __TAURI_EVENT_PLUGIN_INTERNALS__?: { unregisterListener: (event: string, id: number) => void } }).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined }
    w.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' } },
      transformCallback: (callback) => { const key = `__tauri_callback_${Date.now()}_${Math.random()}`; (window as unknown as Record<string, unknown>)[key] = callback; return key },
      invoke: async (command, args = {}) => {
        calls.push({ command, args })
        if (command === 'plugin:event|listen') return 1
        if (command === 'plugin:event|unlisten') return undefined
        switch (command) {
          case 'bootstrap': return { contractVersion: 1, storageStatus: 'ready', preferences: { version: 1, section: 'explorer', sidebarWidth: 24, sidebarCollapsed: false, bottomHeight: 28, bottomCollapsed: false, theme: 'dark', customColors: { background: '#111316', sidebar: '#17191d', surface: '#15171b', elevated: '#1d2025', border: '#292c32', text: '#e4e6e9', muted: '#858992', accent: '#7778ec' }, fontScale: 'default', sqlFontSize: 14 }, session: { version: 1, tabs: [{ id: 'welcome', kind: 'welcome' }], activeTab: 'welcome' } }
          case 'save_ui_preferences': case 'save_session': return undefined
          case 'list_connections': return [{ id: project.connectionId, name: 'MySQL local', engine: 'mysql', host: '127.0.0.1', port: 3306, user: 'tester', tlsMode: 'disabled', sshEnabled: false }]
          case 'open_connection': return { id: project.connectionId, state: 'connected', serverVersion: project.serverVersion, tlsActive: false }
          case 'read_table_page':
            tableReads += 1
            w.__tableReads = tableReads
            return { columns: ['id', 'nombre'], rows: [['42', tableValue]], returnedRows: 1, hasMore: false, nextOffset: null, elapsedMs: 1 }
          case 'list_columns': return [{ name: 'id', dataType: 'int', isNullable: false, isPrimaryKey: true, primaryKeyAvailable: true, defaultValue: null, defaultAvailable: true }, { name: 'nombre', dataType: 'varchar', isNullable: true, isPrimaryKey: false, primaryKeyAvailable: true, defaultValue: null, defaultAvailable: true }]
          case 'list_history_projects': return [project]
          case 'list_history_revisions': return revisions
          case 'list_history_recovery_points': return []
          case 'prepare_mysql_row_revert': {
            const prepared = { id: 'compensation-draft', projectId: project.id, revisionNumber: 9, message: 'Revertir edición de fila', status: 'draft', recoveryState: 'verified', planSha256: 'd'.repeat(64), artifactId: 'compensation-artifact', operationCount: 1, createdAtMs: 5, confirmedAtMs: null }
            revisions.push(prepared)
            return { revision: prepared, primaryKey: rowPlan.primaryKey, columnName: rowPlan.columnName, oldValue: rowPlan.oldValue, newValue: rowPlan.newValue, operation: 'update' }
          }
          case 'read_history_plan': return JSON.stringify(rowPlan)
          case 'confirm_history_revision': {
            const revision = revisions.find((item) => item.id === args.revisionId)
            if (revision) { revision.status = 'confirmed'; revision.confirmedAtMs = 6 }
            return undefined
          }
          case 'apply_mysql_row_update': {
            const revision = revisions.find((item) => item.id === args.revisionId)
            if (revision) revision.status = 'applied'
            tableValue = 'Ana'
            mutationApplied = true
            w.__mutationApplied = mutationApplied
            return undefined
          }
          default: throw new Error(`Comando inesperado en esta prueba: ${command}`)
        }
      },
    }
  })

  await page.goto('/', { waitUntil: 'domcontentloaded' })
  await page.getByRole('button', { name: 'Historial', exact: true }).click()
  await page.getByRole('button', { name: /catalogo/ }).first().click()
  const rowRevision = page.getByRole('button', { name: /Aplicada.*Rev\. 7/ })
  await expect(rowRevision).toBeVisible()
  await expect(page.getByRole('button', { name: 'Preparar reversión' })).toBeDisabled()
  expect(await page.evaluate(() => (window as Window & { __historyCalls?: Array<{ command: string }> }).__historyCalls?.some((call) => call.command === 'open_connection'))).toBe(false)

  await page.getByRole('button', { name: 'Explorador', exact: true }).click()
  await page.getByTitle('Abrir conexión').click()
  await expect(page.getByTitle('Desconectar')).toBeVisible()
  await page.evaluate(() => window.dispatchEvent(new CustomEvent('dbsual:open-table-data', { detail: { engine: 'mysql', connectionId: 'connection-history', database: 'catalogo', table: 'clientes' } })))
  const grid = page.getByRole('region', { name: 'Datos de clientes' })
  await expect(grid.getByText('Ana María', { exact: true })).toBeVisible()
  const initialTableReads = await page.evaluate(() => (window as Window & { __tableReads?: number }).__tableReads ?? 0)
  await page.getByRole('button', { name: 'Historial', exact: true }).click()
  await page.getByRole('button', { name: /catalogo/ }).first().click()
  await page.getByRole('button', { name: 'Preparar reversión' }).click()

  const review = page.getByRole('dialog', { name: 'Revisión 9 · catalogo' })
  await expect(review.getByText('Valor actual')).toBeVisible()
  await expect(review.getByText('Ana María', { exact: true })).toBeVisible()
  await expect(review.getByText('Ana', { exact: true })).toBeVisible()
  await expect(review.getByRole('button', { name: 'Confirmar revisión' })).toBeVisible()
  expect(await page.evaluate(() => (window as Window & { __mutationApplied?: boolean }).__mutationApplied)).toBe(false)

  await review.getByRole('button', { name: 'Confirmar revisión' }).click()
  await expect(review.getByText('Estado: Confirmada sin aplicar')).toBeVisible()
  await expect(review.getByRole('button', { name: 'Aplicar reversión' })).toBeVisible()
  expect(await page.evaluate(() => (window as Window & { __mutationApplied?: boolean }).__mutationApplied)).toBe(false)

  await review.getByRole('button', { name: 'Aplicar reversión' }).click()
  await expect(review).toBeHidden()
  expect(await page.evaluate(() => (window as Window & { __mutationApplied?: boolean }).__mutationApplied)).toBe(true)
  await expect.poll(() => page.evaluate(() => (window as Window & { __tableReads?: number }).__tableReads ?? 0)).toBeGreaterThan(initialTableReads)
  await expect(grid.getByText('Ana', { exact: true })).toBeVisible()
  const calls = await page.evaluate(() => (window as Window & { __historyCalls?: Array<{ command: string; args?: Record<string, unknown> }> }).__historyCalls)
  expect(calls?.filter((call) => call.command === 'prepare_mysql_row_revert')).toHaveLength(1)
  expect(calls?.filter((call) => call.command === 'confirm_history_revision')).toHaveLength(1)
  expect(calls?.filter((call) => call.command === 'apply_mysql_row_update')).toEqual([{ command: 'apply_mysql_row_update', args: { revisionId: 'compensation-draft' } }])
})
