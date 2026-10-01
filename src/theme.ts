/** Decide si un color de fondo necesita controles y texto de tema claro. */
export function isLightBackground(hex: string): boolean {
  if (!/^#[0-9a-f]{6}$/i.test(hex)) return false

  const channels = [1, 3, 5].map((index) => Number.parseInt(hex.slice(index, index + 2), 16) / 255)
  const linear = channels.map((channel) => channel <= 0.04045
    ? channel / 12.92
    : ((channel + 0.055) / 1.055) ** 2.4)
  const luminance = 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]
  return luminance > 0.179
}
