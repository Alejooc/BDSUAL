import { describe, expect, it } from 'vitest'
import { isLightBackground } from './theme'

describe('clasificación de fondo para el tema del editor', () => {
  it('elige tema claro para fondos de luminancia alta', () => {
    expect(isLightBackground('#F5F6FB')).toBe(true)
    expect(isLightBackground('#FFFFFF')).toBe(true)
  })

  it('clasifica fondos saturados y grises por luminancia, no por promedio de canales', () => {
    expect(isLightBackground('#111316')).toBe(false)
    expect(isLightBackground('#FF0000')).toBe(true)
    expect(isLightBackground('#808080')).toBe(true)
    expect(isLightBackground('#00FF00')).toBe(true)
  })

  it('falla cerrado para colores malformados', () => {
    expect(isLightBackground('red')).toBe(false)
    expect(isLightBackground('#12345')).toBe(false)
  })
})

import { readFileSync } from 'node:fs'

const stylesheet = readFileSync(new URL('./styles.css', import.meta.url), 'utf8')
const themeRules = [...stylesheet.matchAll(/html\[data-theme="([^"]+)"\]\s*\{([^}]+)\}/g)]

function colorValue(value: string | undefined): string | undefined {
  if (!value || !/^#[\da-f]{3}(?:[\da-f]{3})?$/i.test(value)) return undefined
  if (value.length === 4) return `#${[...value.slice(1)].map((channel) => channel + channel).join('')}`
  return value
}

function relativeLuminance(hex: string): number {
  const channels = [1, 3, 5].map((index) => Number.parseInt(hex.slice(index, index + 2), 16) / 255)
  const [red, green, blue] = channels.map((channel) => channel <= 0.04045
    ? channel / 12.92
    : ((channel + 0.055) / 1.055) ** 2.4)
  return 0.2126 * red + 0.7152 * green + 0.0722 * blue
}

function contrastRatio(foreground: string, background: string): number {
  const first = relativeLuminance(foreground)
  const second = relativeLuminance(background)
  return (Math.max(first, second) + 0.05) / (Math.min(first, second) + 0.05)
}

describe('contraste de los tokens de paletas predeterminadas', () => {
  it('mantiene texto principal y secundario a 4.5:1 y énfasis a 3:1 en cada superficie', () => {
    const palettes = themeRules.flatMap(([, name, declarations]) => {
      const tokens = Object.fromEntries(
        [...declarations.matchAll(/--theme-([\w-]+)\s*:\s*(#[\da-f]{3}(?:[\da-f]{3})?)/gi)]
          .map(([, token, value]) => [token, colorValue(value)]),
      ) as Record<string, string | undefined>
      return tokens.background ? [{ name, tokens }] : []
    })

    expect(palettes.map(({ name }) => name)).toEqual([
      'dark', 'light', 'contrast', 'midnight', 'nord', 'forest',
    ])

    for (const { name, tokens } of palettes) {
      const surfaces = ['background', 'sidebar', 'surface', 'elevated']
      for (const surfaceName of surfaces) {
        const surface = tokens[surfaceName]
        expect(surface, `${name} must define ${surfaceName}`).toBeDefined()
        for (const textName of ['text', 'muted']) {
          const foreground = tokens[textName]
          expect(foreground, `${name} must define ${textName}`).toBeDefined()
          expect(
            contrastRatio(foreground!, surface!),
            `${name} ${textName} on ${surfaceName}`,
          ).toBeGreaterThanOrEqual(4.5)
        }
        expect(
          contrastRatio(tokens.accent!, surface!),
          `${name} accent on ${surfaceName}`,
        ).toBeGreaterThanOrEqual(3)
      }
    }
  })
})
