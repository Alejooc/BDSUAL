import { expect, test } from '@playwright/test'

test('valida una conexión MySQL antes de probarla y no simula una conexión en el preview', async ({ page }) => {
  await page.goto('/')
  await page.getByRole('button', { name: 'Nueva conexión' }).first().click()
  const dialog = page.getByRole('dialog', { name: 'Nueva conexión' })
  await expect(dialog).toBeVisible()
  await expect(dialog.getByLabel('Usar TLS y verificar la identidad del servidor')).toBeChecked()
  await dialog.getByRole('button', { name: 'Probar conexión' }).click()
  await expect(dialog.getByText('Escribe un nombre para identificar la conexión.')).toBeVisible()

  await dialog.getByRole('textbox', { name: 'Nombre de conexión' }).fill('Local')
  await dialog.getByRole('textbox', { name: 'Servidor', exact: true }).fill('127.0.0.1')
  await dialog.getByRole('textbox', { name: 'Usuario' }).fill('root')
  await dialog.getByRole('button', { name: 'Probar conexión' }).click()
  await expect(dialog.getByText('El núcleo de DBSUAL solo está disponible en la aplicación de escritorio.')).toBeVisible()
  await expect(page.getByText('Aún no hay conexiones')).toBeVisible()
})

test('permite seleccionar MariaDB y muestra las versiones admitidas sin simular una prueba', async ({ page }) => {
  await page.goto('/')
  await page.getByRole('button', { name: 'Nueva conexión' }).first().click()
  const dialog = page.getByRole('dialog', { name: 'Nueva conexión' })
  await dialog.getByLabel('Motor de base de datos').selectOption('mariadb')
  await expect(dialog.getByText('MariaDB · la contraseña se guarda en el almacén seguro de Windows.')).toBeVisible()
  await expect(dialog.getByText('MARIADB', { exact: true })).toBeVisible()
  await expect(dialog.getByRole('option', { name: 'MariaDB · 10.6, 10.11 y 11.4' })).toBeAttached()
  await dialog.getByRole('textbox', { name: 'Nombre de conexión' }).fill('Local MariaDB')
  await dialog.getByRole('textbox', { name: 'Servidor', exact: true }).fill('127.0.0.1')
  await dialog.getByRole('textbox', { name: 'Usuario' }).fill('root')
  await dialog.getByRole('button', { name: 'Probar conexión' }).click()
  await expect(dialog.getByText('El núcleo de DBSUAL solo está disponible en la aplicación de escritorio.')).toBeVisible()
  await expect(page.getByText('Aún no hay conexiones')).toBeVisible()
})

test('muestra la conexión PostgreSQL directa con puerto por defecto y TLS', async ({ page }) => {
  await page.goto('/')
  await page.getByRole('button', { name: 'Nueva conexión' }).first().click()
  const dialog = page.getByRole('dialog', { name: 'Nueva conexión' })
  await dialog.getByLabel('Motor de base de datos').selectOption('postgresql')
  await expect(dialog.getByText('PostgreSQL · la contraseña se guarda en el almacén seguro de Windows.')).toBeVisible()
  await expect(dialog.getByText('POSTGRESQL', { exact: true })).toBeVisible()
  await expect(dialog.getByLabel('Puerto')).toHaveValue('5432')
  await expect(dialog.getByLabel('Usar TLS y verificar la identidad del servidor')).toBeChecked()
  await expect(dialog.getByText('Esta primera conexión PostgreSQL es directa. TLS con verificación de identidad está disponible; el túnel SSH se habilitará después de validar el adaptador.')).toBeVisible()
  await expect(dialog.getByText('Conectar mediante un túnel SSH')).toHaveCount(0)
})

test('presenta SQLite como archivo local de solo lectura', async ({ page }) => {
  await page.goto('/')
  await page.getByRole('button', { name: 'Nueva conexión' }).first().click()
  const dialog = page.getByRole('dialog', { name: 'Nueva conexión' })
  await dialog.getByLabel('Motor de base de datos').selectOption('sqlite')
  await expect(dialog.getByText('SQLite · archivo local, acceso de solo lectura.')).toBeVisible()
  await expect(dialog.getByLabel('Archivo de base de datos')).toBeVisible()
  await expect(dialog.getByRole('button', { name: 'Elegir archivo' })).toBeVisible()
  await expect(dialog.getByLabel('Puerto')).toHaveCount(0)
  await expect(dialog.getByLabel('Usar TLS y verificar la identidad del servidor')).toHaveCount(0)
})

test('envía una prueba PostgreSQL al comando de conexión con TLS verificado', async ({ page }) => {
  await page.addInitScript(() => {
    const calls: Array<{ command: string; args?: Record<string, unknown> }> = []
    const w = window as Window & { isTauri?: boolean; __TAURI_INTERNALS__?: { metadata: { currentWindow: { label: string } }; invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> }; __postgresCalls?: typeof calls }
    w.isTauri = true
    w.__postgresCalls = calls
    w.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' } },
      invoke: async (command, args = {}) => {
        calls.push({ command, args })
        if (command === 'plugin:event|listen') return 1
        if (command === 'plugin:event|unlisten') return undefined
        if (command === 'bootstrap') return { contractVersion: 1, storageStatus: 'ready', preferences: { version: 1, section: 'explorer', sidebarWidth: 24, sidebarCollapsed: false, bottomHeight: 28, bottomCollapsed: false }, session: { version: 1, tabs: [{ id: 'welcome', kind: 'welcome' }], activeTab: 'welcome' } }
        if (command === 'list_connections') return []
        if (command === 'test_connection') return { serverVersion: '16.4', tlsActive: true }
        throw new Error(`Comando inesperado: ${command}`)
      },
    }
  })
  await page.goto('/')
  await page.getByRole('button', { name: 'Nueva conexión' }).first().click()
  const dialog = page.getByRole('dialog', { name: 'Nueva conexión' })
  await dialog.getByLabel('Motor de base de datos').selectOption('postgresql')
  await dialog.getByRole('textbox', { name: 'Nombre de conexión' }).fill('PostgreSQL test')
  await dialog.getByRole('textbox', { name: 'Servidor', exact: true }).fill('localhost')
  await dialog.getByRole('textbox', { name: 'Usuario' }).fill('postgres')
  await dialog.getByRole('button', { name: 'Probar conexión' }).click()
  await expect(dialog.getByRole('status')).toContainText('Prueba correcta · PostgreSQL 16.4 · TLS activo')
  const testedInput = await page.evaluate(() => (window as Window & { __postgresCalls?: Array<{ command: string; args?: { input?: Record<string, unknown> } }> }).__postgresCalls?.find((call) => call.command === 'test_connection')?.args?.input)
  expect(testedInput).toMatchObject({ engine: 'postgresql', port: 5432, tlsMode: 'verifyIdentity', host: 'localhost', user: 'postgres' })
})
