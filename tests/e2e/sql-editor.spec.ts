import { expect, test } from '@playwright/test'

test('abre el editor SQL local sin conexión a un servidor', async ({ page }) => {
  await page.goto('/')
  await page.getByRole('button', { name: 'Nueva consulta SQL' }).click()

  const editor = page.getByRole('region', { name: 'Editor SQL' })
  await expect(editor).toBeVisible({ timeout: 15_000 })
  await expect(editor.getByText('Solo SELECT · lectura protegida')).toBeVisible()
  await expect(editor.locator('.monaco-editor')).toBeVisible({ timeout: 15_000 })
  await expect(editor.getByLabel('Conexión para la consulta')).toHaveValue('')
})

test('abre la cuadrícula de tabla y muestra el error del core cuando no hay IPC', async ({ page }) => {
  await page.goto('/')
  await page.evaluate(() => window.dispatchEvent(new CustomEvent('dbsual:open-table-data', {
    detail: { connectionId: 'connection-test', database: 'sales', table: 'orders' },
  })))

  const view = page.getByRole('region', { name: 'Datos de orders' })
  await expect(view).toBeVisible()
  await expect(view.getByText('sales · Solo lectura')).toBeVisible()
  await expect(view.getByRole('alert')).toContainText('solo está disponible en la aplicación de escritorio')
})

test('ordena y filtra filas de la cuadrícula mediante parámetros IPC', async ({ page }) => {
  await page.goto('/')
  await page.evaluate(() => {
    const w = window as Window & { isTauri?: boolean; __TAURI_INTERNALS__?: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> }; __tableReads?: Array<Record<string, unknown>> }
    w.__tableReads = []
    w.isTauri = true
    w.__TAURI_INTERNALS__ = {
      invoke: async (command, args = {}) => {
        if (command === 'read_table_page') {
          w.__tableReads?.push(args)
          return { columns: ['id', 'label'], rows: [['1', 'Alpha'], ['2', 'Beta']], returnedRows: 2, hasMore: true, nextOffset: 2, elapsedMs: 3 }
        }
        if (command === 'cancel_read_query') return undefined
        throw new Error(`Comando inesperado en esta prueba: ${command}`)
      },
    }
    window.dispatchEvent(new CustomEvent('dbsual:open-table-data', { detail: { connectionId: 'connection-test', database: 'sales', table: 'orders' } }))
  })

  const view = page.getByRole('region', { name: 'Datos de orders' })
  await expect(view.getByText('Alpha')).toBeVisible()
  await view.getByRole('button', { name: 'Ordenar por id' }).click()
  await expect.poll(() => page.evaluate(() => (window as Window & { __tableReads?: Array<{ options?: { sortColumn?: string; sortDirection?: string } }> }).__tableReads?.at(-1)?.options)).toMatchObject({ sortColumn: 'id', sortDirection: 'asc' })

  const maliciousLookingValue = "' OR 1=1 --"
  await view.getByLabel('Valor del filtro').fill(maliciousLookingValue)
  await view.getByRole('button', { name: 'Aplicar' }).click()
  await expect.poll(() => page.evaluate(() => (window as Window & { __tableReads?: Array<{ options?: { filterValue?: string; filterMode?: string } }> }).__tableReads?.at(-1)?.options)).toMatchObject({ filterValue: maliciousLookingValue, filterMode: 'contains' })
  await expect(view.getByText('filtro en id')).toBeVisible()

  await view.getByRole('button', { name: 'Cargar más filas' }).click()
  await expect.poll(() => page.evaluate(() => (window as Window & { __tableReads?: Array<{ offset?: number; options?: { filterValue?: string } }> }).__tableReads?.at(-1))).toMatchObject({ offset: 2, options: { filterValue: maliciousLookingValue } })
  await view.getByRole('button', { name: 'Limpiar filtro' }).click()
  await expect.poll(() => page.evaluate(() => (window as Window & { __tableReads?: Array<{ offset?: number; options?: { filterColumn?: string | null; filterValue?: string | null } }> }).__tableReads?.at(-1))).toMatchObject({ offset: 0, options: { filterColumn: null, filterValue: null } })
})
