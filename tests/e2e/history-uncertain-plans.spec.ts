import { expect, test } from '@playwright/test'

test('explica estados inciertos y presenta ediciones de fila como diferencias sin reejecutarlas', async ({ page }) => {
  await page.addInitScript(() => {
    const project = { id: 'project-history', connectionId: 'connection-history', databaseName: 'catalogo', engine: 'mysql', serverVersion: '8.4.11', createdAtMs: 1 }
    const revisions = [
      { id: 'uncertain-row', projectId: project.id, revisionNumber: 7, message: 'Actualizar fila', status: 'uncertain', recoveryState: 'verified', planSha256: 'a'.repeat(64), artifactId: 'uncertain-artifact', operationCount: 1, createdAtMs: 2, confirmedAtMs: 3 },
      { id: 'revert-row', projectId: project.id, revisionNumber: 8, message: 'Revertir edición de fila', status: 'draft', recoveryState: 'verified', planSha256: 'b'.repeat(64), artifactId: 'revert-artifact', operationCount: 1, createdAtMs: 4, confirmedAtMs: null },
    ]
    const updatePlan = { version: 1, connectionId: project.connectionId, databaseName: project.databaseName, tableName: 'clientes', serverUuid: 'server-fixture', serverVersion: project.serverVersion, primaryKey: [{ column: 'id', value: '42' }], columnName: 'nombre', oldValue: 'Ana', newValue: 'Ana María', compensatesRevisionId: null, rowSha256: 'c'.repeat(64), columnNames: ['id', 'nombre'] }
    const revertPlan = { ...updatePlan, oldValue: 'Ana María', newValue: 'Ana', compensatesRevisionId: 'uncertain-row' }
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = []
    const w = window as Window & { isTauri?: boolean; __TAURI_INTERNALS__?: { metadata: { currentWindow: { label: string } }; invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> }; __historyCalls?: typeof calls }
    w.isTauri = true
    w.__historyCalls = calls
    w.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' } },
      invoke: async (command, args = {}) => {
        calls.push({ command, args })
        if (command === 'plugin:event|listen') return 1
        if (command === 'plugin:event|unlisten') return undefined
        switch (command) {
          case 'bootstrap': return { contractVersion: 1, storageStatus: 'ready', preferences: { version: 1, section: 'explorer', sidebarWidth: 24, sidebarCollapsed: false, bottomHeight: 28, bottomCollapsed: false }, session: { version: 1, tabs: [{ id: 'welcome', kind: 'welcome' }], activeTab: 'welcome' } }
          case 'save_ui_preferences': case 'save_session': return undefined
          case 'list_connections': return []
          case 'list_history_projects': return [project]
          case 'list_history_revisions': return revisions
          case 'list_history_recovery_points': return []
          case 'read_history_plan':
            if (args.revisionId === 'uncertain-row') return JSON.stringify(updatePlan)
            if (args.revisionId === 'revert-row') return JSON.stringify(revertPlan)
            throw new Error('Revisión simulada desconocida')
          default: throw new Error(`Comando inesperado en esta prueba: ${command}`)
        }
      },
    }
  })

  await page.goto('/')
  await page.getByRole('button', { name: 'Historial', exact: true }).click()
  await page.getByRole('button', { name: /catalogo/ }).first().click()

  const uncertainItem = page.getByRole('button', { name: /Resultado incierto/ })
  await expect(uncertainItem).toBeVisible()
  await expect(page.getByRole('alert').filter({ hasText: 'DBSUAL no repetirá esta revisión automáticamente' })).toBeVisible()
  await uncertainItem.click()

  const uncertainDialog = page.getByRole('dialog', { name: 'Revisión 7 · catalogo' })
  await expect(uncertainDialog.getByText('Estado: Resultado incierto')).toBeVisible()
  await expect(uncertainDialog.getByRole('alert')).toContainText('Comprueba la base de datos antes de continuar.')
  await expect(uncertainDialog.getByText('clientes', { exact: true })).toBeVisible()
  await expect(uncertainDialog.getByText('Ana María', { exact: true })).toBeVisible()
  await expect(uncertainDialog.getByText('"tableName"')).toHaveCount(0)
  await expect(uncertainDialog.getByRole('button', { name: /Aplicar/ })).toHaveCount(0)
  await expect(uncertainDialog.getByRole('button', { name: 'Confirmar revisión' })).toHaveCount(0)
  await uncertainDialog.getByRole('button', { name: 'Cerrar revisión' }).click()

  await page.getByRole('button', { name: /Borrador.*Rev\. 8/ }).click()
  const revertDialog = page.getByRole('dialog', { name: 'Revisión 8 · catalogo' })
  await expect(revertDialog.getByText('Estado: Borrador')).toBeVisible()
  await expect(revertDialog.getByText('Valor actual')).toBeVisible()
  await expect(revertDialog.getByText('Ana María', { exact: true })).toBeVisible()
  await expect(revertDialog.getByText('Valor propuesto')).toBeVisible()
  await expect(revertDialog.getByText('Ana', { exact: true })).toBeVisible()
  await expect(revertDialog.getByText('"compensatesRevisionId"')).toHaveCount(0)
  await expect(revertDialog.getByRole('button', { name: 'Confirmar revisión' })).toBeVisible()

  const calls = await page.evaluate(() => (window as Window & { __historyCalls?: Array<{ command: string }> }).__historyCalls)
  expect(calls?.some((call) => call.command === 'apply_mysql_row_update' || call.command === 'restore_recovery_point' || call.command === 'apply_create_database')).toBe(false)
})
