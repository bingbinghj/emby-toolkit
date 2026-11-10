import { expect } from './fixtures.js';

const luminance = colors => colors.reduce((sum, byte, index) => {
  const value = byte / 255;
  return sum + (value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4) * [.2126, .7152, .0722][index];
}, 0);

export async function measureReadingContrast(page, probes) {
  const states = [];
  for (const [index, probe] of probes.entries()) {
    await expect(probe.locator, probe.role).toBeVisible();
    states.push(await probe.locator.evaluate((element, index) => {
      element.dataset.readingProbe = String(index);
      const box = element.getBoundingClientRect();
      const css = getComputedStyle(element);
      let opacity = 1;
      for (let node = element; node; node = node.parentElement) opacity *= Number(getComputedStyle(node).opacity);
      return { color: css.color, opacity, points: [.15, .5, .85].map(fraction => ({ x: box.x + box.width * fraction, y: box.y + box.height / 2 })) };
    }, index));
  }
  const screenshot = await page.screenshot({ animations: 'disabled', style: '[data-reading-probe], [data-reading-probe] * { color: transparent !important; -webkit-text-fill-color: transparent !important; text-shadow: none !important; }' });
  const backgrounds = await page.evaluate(async ({ png, states }) => {
    const image = new Image(); image.src = `data:image/png;base64,${png}`;
    await image.decode();
    const canvas = document.createElement('canvas'); canvas.width = image.width; canvas.height = image.height;
    const context = canvas.getContext('2d'); context.drawImage(image, 0, 0);
    const scaleX = image.width / innerWidth, scaleY = image.height / innerHeight;
    return states.map(state => state.points.map(({ x, y }) => [...context.getImageData(Math.floor(x * scaleX), Math.floor(y * scaleY), 1, 1).data].slice(0, 3)));
  }, { png: screenshot.toString('base64'), states });
  for (const probe of probes) await probe.locator.evaluate(element => delete element.dataset.readingProbe);
  return states.map((state, index) => {
    if (!/^rgba?\(/.test(state.color)) throw new Error(`Unexpected text color: ${state.color}`);
    const color = state.color.match(/[\d.]+/g).map(Number);
    const alpha = (color[3] ?? 1) * state.opacity;
    const samples = backgrounds[index].map(background => {
      const foreground = color.slice(0, 3).map((value, channel) => value * alpha + background[channel] * (1 - alpha));
      const ink = luminance(foreground), fill = luminance(background);
      return { background, contrast: (Math.max(ink, fill) + .05) / (Math.min(ink, fill) + .05) };
    });
    return { role: probes[index].role, color: state.color, opacity: state.opacity, contrast: Math.min(...samples.map(sample => sample.contrast)), samples };
  });
}
