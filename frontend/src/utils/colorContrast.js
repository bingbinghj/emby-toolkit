/** Choose the black or white text with the better WCAG contrast on a solid color. */
export function textColorForBackground(color) {
  if (typeof color !== 'string' || !/^#[\da-f]{6}$/i.test(color)) return '#FFFFFF'
  const channels = color.slice(1).match(/../g).map(value => parseInt(value, 16) / 255)
  const luminance = channels.reduce((sum, value, index) => sum
    + (value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4) * [0.2126, 0.7152, 0.0722][index], 0)
  const blackContrast = (luminance + 0.05) / 0.05
  const whiteContrast = 1.05 / (luminance + 0.05)
  return blackContrast >= whiteContrast ? '#000000' : '#FFFFFF'
}
