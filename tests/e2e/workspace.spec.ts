import { expect, test } from '@playwright/test'

test('muestra el espacio inicial y cambia entre secciones vacías', async ({ page }) => {
  await page.goto('/')
  await expect(page.getByRole('heading', { name: /Tu trabajo/ })).toBeVisible()
  await expect(page.getByText('Prepara tus respaldos protegidos')).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Configuración' })).toBeVisible()
  await expect(page.getByText('Aún no hay conexiones')).toBeVisible()
  await page.getByRole('button', { name: 'Cambios', exact: true }).click()
  await expect(page.getByText('Sin cambios preparados')).toBeVisible()
  await page.getByRole('button', { name: 'Historial', exact: true }).click()
  await expect(page.getByText('Sin historial todavía')).toBeVisible()
})

test('permite cerrar Bienvenida y regresar a Inicio desde el área vacía', async ({ page }) => {
  await page.goto('/')
  await page.getByRole('button', { name: 'Cerrar Bienvenida' }).click()
  await expect(page.getByRole('heading', { name: 'No hay pestañas abiertas' })).toBeVisible()
  await page.getByRole('button', { name: 'Abrir Inicio' }).click()
  await expect(page.getByRole('heading', { name: /Tu trabajo/ })).toBeVisible()
  await page.getByRole('button', { name: 'Cerrar Bienvenida' }).click()
  await page.locator('.empty-workspace').getByRole('button', { name: 'Nueva consulta SQL' }).click()
  await expect(page.getByRole('button', { name: 'Cerrar Consulta SQL' })).toBeVisible()
  await page.getByRole('button', { name: 'Cerrar Consulta SQL' }).click()
  await expect(page.getByRole('heading', { name: 'No hay pestañas abiertas' })).toBeVisible()
  await page.getByRole('button', { name: 'Abrir Inicio' }).click()
  await expect(page.getByRole('heading', { name: /Tu trabajo/ })).toBeVisible()
})

test('abre la configuración del depósito fuera de Inicio', async ({ page }) => {
  await page.goto('/')
  await page.getByRole('button', { name: 'Cerrar Bienvenida' }).click()
  await page.getByRole('button', { name: 'Configuración' }).click()
  const settings = page.locator('.settings-page')
  await expect(settings).toBeVisible()
  await settings.getByRole('button', { name: /Seguridad y recuperación/ }).click()
  await expect(settings.getByText('Prepara tus respaldos protegidos')).toBeVisible()
  await expect(settings.getByText('Disponible en la app de escritorio')).toBeVisible()
  await settings.getByRole('button', { name: 'Cerrar Configuración' }).click()
  await expect(settings).toBeHidden()
  await expect(page.getByRole('heading', { name: 'No hay pestañas abiertas' })).toBeVisible()
})

test('redimensiona el panel lateral y restablece la disposición', async ({ page }) => {
  await page.goto('/')
  const sidebar = page.locator('.side-panel')
  const handle = page.getByRole('separator', { name: 'Redimensionar panel lateral' })
  const initial = (await sidebar.boundingBox())?.width ?? 0
  const bounds = await handle.boundingBox()
  expect(bounds).not.toBeNull()
  await page.mouse.move(bounds!.x + bounds!.width / 2, bounds!.y + bounds!.height / 2)
  await page.mouse.down()
  await page.mouse.move(bounds!.x + 90, bounds!.y + bounds!.height / 2, { steps: 8 })
  await page.mouse.up()
  await expect.poll(async () => (await sidebar.boundingBox())?.width ?? 0).toBeGreaterThan(initial + 35)
  await page.getByRole('button', { name: 'Restablecer disposición' }).click()
  await expect.poll(async () => (await sidebar.boundingBox())?.width ?? 0).toBeLessThan(initial + 20)
})

test('permite navegar y ajustar paneles con teclado', async ({ page }) => {
  await page.goto('/')
  const changes = page.getByRole('button', { name: 'Cambios', exact: true })
  await changes.focus()
  await page.keyboard.press('Enter')
  await expect(page.getByText('Sin cambios preparados')).toBeVisible()
  const handle = page.getByRole('separator', { name: 'Redimensionar panel lateral' })
  const sidebar = page.locator('.side-panel')
  const before = (await sidebar.boundingBox())?.width ?? 0
  await handle.focus()
  await page.keyboard.press('ArrowRight')
  await expect.poll(async () => (await sidebar.boundingBox())?.width ?? 0).toBeGreaterThan(before)
})

test('mantiene cuadrículas independientes y activa la existente al volver a abrirla', async ({ page }) => {
  await page.goto('/')
  const openTable = (connectionId: string, database: string, table: string) => page.evaluate((target) => {
    window.dispatchEvent(new CustomEvent('dbsual:open-table-data', { detail: target }))
  }, { connectionId, database, table })

  await openTable('conn-1', 'ventas', 'clientes')
  await openTable('conn-2', 'analitica', 'eventos')
  await expect(page.getByRole('button', { name: 'Activar tabla clientes de ventas' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Activar tabla eventos de analitica' })).toBeVisible()
  await expect(page.getByRole('region', { name: 'Datos de eventos' })).toBeVisible()

  await page.getByRole('button', { name: 'Activar tabla clientes de ventas' }).click()
  await expect(page.getByRole('region', { name: 'Datos de clientes' })).toBeVisible()
  await expect(page.getByRole('region', { name: 'Datos de eventos' })).toBeHidden()

  await openTable('conn-1', 'ventas', 'clientes')
  await expect(page.getByRole('button', { name: 'Activar tabla clientes de ventas' })).toHaveCount(1)
  await expect(page.getByRole('region', { name: 'Datos de clientes' })).toBeVisible()

  await page.getByRole('button', { name: 'Cerrar tabla clientes' }).click()
  await expect(page.getByRole('region', { name: 'Datos de eventos' })).toBeVisible()
})

test('prepara, confirma y aplica la creación MySQL sin ejecutarla antes de aplicar', async ({ page }) => {
  await page.addInitScript(() => {
    const project = { id: 'project-test', connectionId: 'connection-test', databaseName: 'new_catalog', engine: 'mysql', serverVersion: '8.4.11', createdAtMs: 1 }
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = []
    let revision = { id: 'revision-test', projectId: project.id, revisionNumber: 1, message: 'Crear base de datos', status: 'draft', recoveryState: 'not_required', planSha256: 'a'.repeat(64), artifactId: 'artifact-test', operationCount: 1, createdAtMs: 2, confirmedAtMs: null as number | null }
    let databases = ['existing_catalog']
    const w = window as Window & { isTauri?: boolean; __TAURI_INTERNALS__?: { metadata: { currentWindow: { label: string } }; transformCallback: (callback: (...args: unknown[]) => unknown) => string; invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> }; __creationCalls?: typeof calls }
    w.isTauri = true
    w.__creationCalls = calls
    ;(window as Window & { __TAURI_EVENT_PLUGIN_INTERNALS__?: { unregisterListener: (event: string, id: number) => void } }).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined }
    w.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' } },
      transformCallback: (callback) => { const key = `__tauri_callback_${Date.now()}_${Math.random()}`; (window as unknown as Record<string, unknown>)[key] = callback; return key },
      invoke: async (command, args = {}) => {
        calls.push({ command, args })
        if (command === 'plugin:event|listen') return 1
        if (command === 'plugin:event|unlisten') return undefined
        switch (command) {
          case 'bootstrap': return { contractVersion: 1, storageStatus: 'ready', preferences: { version: 1, section: 'explorer', sidebarWidth: 24, sidebarCollapsed: false, bottomHeight: 28, bottomCollapsed: false }, session: { version: 1, tabs: [{ id: 'welcome', kind: 'welcome' }], activeTab: 'welcome' } }
          case 'save_ui_preferences': case 'save_session': return undefined
          case 'list_connections': return [{ id: 'connection-test', name: 'Local', engine: 'mysql', host: '127.0.0.1', port: 3306, user: 'root', tlsMode: 'disabled', sshEnabled: false }]
          case 'open_connection': return { id: 'connection-test', state: 'connected', serverVersion: '8.4.11', tlsActive: false }
          case 'list_databases': return databases
          case 'prepare_create_database':
            if (calls.some((call) => call.command === 'apply_create_database')) throw new Error('La aplicación no debe ejecutarse al preparar')
            return revision
          case 'list_history_projects': return calls.some((call) => call.command === 'prepare_create_database') ? [project] : []
          case 'read_history_plan': return '-- DBSUAL_CREATE_DATABASE_V1\nCREATE DATABASE `new_catalog`;'
          case 'confirm_history_revision': revision = { ...revision, status: 'confirmed', confirmedAtMs: 3 }; return undefined
          case 'list_history_revisions': return [revision]
          case 'apply_create_database':
            if (revision.status !== 'confirmed') throw new Error('Solo se aplica una revisión confirmada')
            revision = { ...revision, status: 'applied' }
            databases = [...databases, 'new_catalog']
            return undefined
          default: throw new Error(`Comando inesperado: ${command}`)
        }
      },
    }
  })
  await page.goto('/')
  await page.getByRole('button', { name: 'Abrir conexión' }).click()
  await expect(page.getByTitle('Desconectar')).toBeVisible()
  await page.getByRole('button', { name: 'Expandir Local' }).click()
  await expect(page.getByText('existing_catalog')).toBeVisible()
  await page.getByRole('button', { name: 'Preparar creación de base de datos' }).click()
  const createDialog = page.getByRole('dialog', { name: 'Preparar una base nueva' })
  await createDialog.getByLabel('Nombre').fill('new_catalog')
  await createDialog.getByRole('button', { name: 'Preparar revisión' }).click()

  const review = page.getByRole('dialog', { name: /Revisión 1/ })
  await expect(review.getByText('CREATE DATABASE `new_catalog`;')).toBeVisible()
  await expect(page.getByText('new_catalog', { exact: true })).toHaveCount(0)
  expect(await page.evaluate(() => (window as Window & { __creationCalls?: Array<{ command: string }> }).__creationCalls?.some((call) => call.command === 'apply_create_database'))).toBe(false)

  await review.getByRole('button', { name: 'Confirmar revisión' }).click()
  await expect(page.getByRole('dialog', { name: /Revisión 1/ })).toBeHidden()
  expect(await page.evaluate(() => (window as Window & { __creationCalls?: Array<{ command: string }> }).__creationCalls?.some((call) => call.command === 'apply_create_database'))).toBe(false)

  await page.getByRole('button', { name: 'Cambios', exact: true }).click()
  const revisionItem = page.getByRole('button', { name: /Rev\. 1 · new_catalog/ })
  await expect(revisionItem).toBeVisible()
  await revisionItem.click()
  const confirmedReview = page.getByRole('dialog', { name: /Revisión 1/ })
  await confirmedReview.getByRole('button', { name: 'Aplicar creación' }).click()
  await expect(page.getByRole('button', { name: /Rev\. 1 · new_catalog Aplicada/ })).toBeVisible()
  await page.getByRole('button', { name: 'Explorador', exact: true }).click()
  await expect(page.getByText('new_catalog', { exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Actualizar bases de datos' })).toBeVisible()
})

test('captura y ofrece restaurar un punto MariaDB desde el historial', async ({ page }) => {
  await page.addInitScript(() => {
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = []
    const project = { id: 'project-maria', connectionId: 'connection-maria', databaseName: 'catalogo', engine: 'mariadb', serverVersion: '10.11.19', createdAtMs: 1 }
    const point = { id: 'point-maria', projectId: project.id, engine: 'mariadb', serverVersion: '10.11.19', artifactId: 'artifact-maria', ciphertextSha256: 'a'.repeat(64), encryptedBytes: 4096, plaintextBytes: 2048, coverage: 'visible_tables_views_triggers', verificationState: 'captured', protectedRevisionId: null, createdAtMs: 2 }
    let revision: Record<string, unknown> | null = null
    const w = window as Window & { isTauri?: boolean; __TAURI_INTERNALS__?: { metadata: { currentWindow: { label: string } }; transformCallback: (callback: (...args: unknown[]) => unknown) => string; invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> }; __recoveryCalls?: typeof calls }
    w.isTauri = true
    w.__recoveryCalls = calls
    ;(window as Window & { __TAURI_EVENT_PLUGIN_INTERNALS__?: { unregisterListener: (event: string, id: number) => void } }).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined }
    w.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' } },
      transformCallback: (callback) => { const key = `__tauri_callback_${Date.now()}_${Math.random()}`; (window as unknown as Record<string, unknown>)[key] = callback; return key },
      invoke: async (command, args = {}) => {
        calls.push({ command, args })
        if (command === 'plugin:event|listen') return 1
        if (command === 'plugin:event|unlisten') return undefined
        switch (command) {
          case 'bootstrap': return { contractVersion: 1, storageStatus: 'ready', preferences: { version: 1, section: 'explorer', sidebarWidth: 24, sidebarCollapsed: false, bottomHeight: 28, bottomCollapsed: false }, session: { version: 1, tabs: [{ id: 'welcome', kind: 'welcome' }], activeTab: 'welcome' } }
          case 'save_ui_preferences': case 'save_session': return undefined
          case 'list_connections': return [{ id: project.connectionId, name: 'Maria local', engine: 'mariadb', host: '127.0.0.1', port: 3306, user: 'root', tlsMode: 'disabled', sshEnabled: false }]
          case 'open_connection': return { id: project.connectionId, state: 'connected', serverVersion: project.serverVersion, tlsActive: false }
          case 'list_history_projects': return [project]
          case 'list_history_revisions': return revision ? [revision] : []
          case 'list_history_recovery_points': return []
          case 'capture_recovery_point': return point
          case 'prepare_recovery_restore':
            revision = { id: 'restore-revision', projectId: project.id, revisionNumber: 1, message: 'Restaurar punto en catalogo_restore', status: 'draft', recoveryState: 'not_required', planSha256: 'b'.repeat(64), artifactId: 'restore-plan-artifact', operationCount: 1, createdAtMs: 3, confirmedAtMs: null }
            return revision
          case 'read_history_plan': return JSON.stringify({ version: 1, recoveryPointId: point.id, targetDatabase: 'catalogo_restore', artifactId: point.artifactId, ciphertextSha256: point.ciphertextSha256 })
          case 'confirm_history_revision':
            if (!revision) throw new Error('No hay revisión para confirmar')
            revision = { ...revision, status: 'confirmed', confirmedAtMs: 4 }
            return undefined
          case 'restore_recovery_point':
            if (!revision || revision.status !== 'confirmed') throw new Error('Solo se aplica una revisión confirmada')
            revision = { ...revision, status: 'applied' }
            return { databaseName: 'catalogo_restore', tablesRestored: 2, rowsRestored: 8 }
          default: throw new Error(`Comando inesperado: ${command}`)
        }
      },
    }
  })
  await page.goto('/')
  await page.getByRole('button', { name: 'Abrir conexión' }).click()
  await expect(page.getByTitle('Desconectar')).toBeVisible()
  await page.getByRole('button', { name: 'Historial', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Capturar punto cifrado' })).toBeVisible()
  await page.getByRole('button', { name: 'Capturar punto cifrado' }).click()
  await expect(page.getByText('Respaldo · captured')).toBeVisible()
  await page.getByRole('button', { name: 'Restaurar en base nueva' }).click()
  const dialog = page.getByRole('dialog', { name: 'Preparar restauración' })
  await expect(dialog.getByText('catalogo · MariaDB')).toBeVisible()
  await expect(dialog.getByLabel('Nombre para la base nueva')).toHaveValue('catalogo_restore')
  expect(await page.evaluate(() => (window as Window & { __recoveryCalls?: Array<{ command: string; args?: Record<string, unknown> }> }).__recoveryCalls?.find((call) => call.command === 'capture_recovery_point')?.args)).toEqual({ connectionId: 'connection-maria', databaseName: 'catalogo' })
  await dialog.getByRole('button', { name: 'Preparar revisión' }).click()
  const review = page.getByRole('dialog', { name: /Revisión 1/ })
  await expect(review.getByText('Base nueva: catalogo_restore')).toBeVisible()
  expect(await page.evaluate(() => (window as Window & { __recoveryCalls?: Array<{ command: string }> }).__recoveryCalls?.some((call) => call.command === 'restore_recovery_point'))).toBe(false)
  await review.getByRole('button', { name: 'Confirmar revisión' }).click()
  await page.getByRole('button', { name: 'Cambios', exact: true }).click()
  const revisionItem = page.getByRole('button', { name: /Rev\. 1/ })
  await expect(revisionItem).toBeVisible()
  await revisionItem.click()
  const confirmed = page.getByRole('dialog', { name: /Revisión 1/ })
  await expect(confirmed.getByRole('button', { name: 'Aplicar restauración' })).toBeVisible()
  expect(await page.evaluate(() => (window as Window & { __recoveryCalls?: Array<{ command: string }> }).__recoveryCalls?.some((call) => call.command === 'restore_recovery_point'))).toBe(false)
  await confirmed.getByRole('button', { name: 'Aplicar restauración' }).click()
  const applyCall = await page.evaluate(() => (window as Window & { __recoveryCalls?: Array<{ command: string; args?: Record<string, unknown> }> }).__recoveryCalls?.find((call) => call.command === 'restore_recovery_point'))
  expect(applyCall?.args).toEqual({ revisionId: 'restore-revision' })
})
