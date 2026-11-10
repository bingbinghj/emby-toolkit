import { expect } from './fixtures.js';
import { writeFileSync } from 'node:fs';

const luminance = colors => colors.reduce((sum, byte, index) => {
  const value = byte / 255;
  return sum + (value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4) * [.2126, .7152, .0722][index];
}, 0);

function readingGradient(state) {
  if (!state.backgroundClip.includes('text') || state.backgroundImage === 'none') return null;
  const fill = state.textFillColor.match(/[\d.]+/g)?.map(Number);
  if (!state.backgroundImage.startsWith('linear-gradient(') || state.fragmentCount !== 1 || fill?.[3] !== 0) {
    throw new Error(`Unsupported reading gradient: ${JSON.stringify(state)}`);
  }
  let depth = 0;
  for (const character of state.backgroundImage) {
    if (character === '(') depth += 1;
    else if (character === ')') depth -= 1;
    else if (character === ',' && depth === 0) throw new Error(`Multiple reading gradient layers are unsupported: ${state.backgroundImage}`);
  }
  if (depth !== 0) throw new Error(`Invalid reading gradient: ${state.backgroundImage}`);
  return { image: state.backgroundImage, backgroundClip: state.backgroundClip, textFillColor: state.textFillColor };
}

export async function measureReadingContrast(page, probes, { gradientArtifactPrefix } = {}) {
  const states = [];
  for (const [index, probe] of probes.entries()) {
    await expect(probe.locator, probe.role).toBeVisible();
    states.push(await probe.locator.evaluate((element, index) => {
      element.dataset.readingProbe = String(index);
      const box = element.getBoundingClientRect();
      const css = getComputedStyle(element);
      const backgroundClip = css.backgroundClip;
      if (backgroundClip.includes('text') && css.backgroundImage !== 'none') element.dataset.readingGradientProbe = String(index);
      let opacity = 1;
      for (let node = element; node; node = node.parentElement) opacity *= Number(getComputedStyle(node).opacity);
      return { color: css.color, opacity, backgroundClip, backgroundImage: css.backgroundImage,
        textFillColor: css.webkitTextFillColor, fragmentCount: element.getClientRects().length,
        box: { x: box.x, y: box.y, width: box.width, height: box.height },
        points: [.15, .5, .85].map(fraction => ({ x: box.x + box.width * fraction, y: box.y + box.height / 2 })) };
    }, index));
  }
  const gradients = states.map(readingGradient);
  const transparentText = '[data-reading-probe], [data-reading-probe] * { color: transparent !important; -webkit-text-fill-color: transparent !important; text-shadow: none !important; }';
  const nativeGradientScreenshot = gradients.some(Boolean) ? await page.screenshot({ animations: 'disabled',
    style: `${transparentText} [data-reading-gradient-probe] { background-clip: border-box !important; -webkit-background-clip: border-box !important; }` }) : null;
  const screenshot = await page.screenshot({ animations: 'disabled',
    style: `${transparentText} [data-reading-gradient-probe] { background-image: none !important; }` });
  if (nativeGradientScreenshot && gradientArtifactPrefix) {
    writeFileSync(`${gradientArtifactPrefix}-native-foreground.png`, nativeGradientScreenshot);
    writeFileSync(`${gradientArtifactPrefix}-background.png`, screenshot);
  }
  const { backgrounds, nativeForegrounds } = await page.evaluate(async ({ png, foregroundPng, states }) => {
    const image = new Image(); image.src = `data:image/png;base64,${png}`;
    await image.decode();
    const canvas = document.createElement('canvas'); canvas.width = image.width; canvas.height = image.height;
    const context = canvas.getContext('2d'); context.drawImage(image, 0, 0);
    const scaleX = image.width / innerWidth, scaleY = image.height / innerHeight;
    const samples = () => states.map(state => state.points.map(({ x, y }) => [...context.getImageData(Math.floor(x * scaleX), Math.floor(y * scaleY), 1, 1).data].slice(0, 3)));
    const backgrounds = samples();
    let nativeForegrounds = null;
    if (foregroundPng) {
      const foreground = new Image(); foreground.src = `data:image/png;base64,${foregroundPng}`;
      await foreground.decode();
      context.clearRect(0, 0, canvas.width, canvas.height); context.drawImage(foreground, 0, 0);
      nativeForegrounds = samples();
    }
    return { backgrounds, nativeForegrounds };
  }, { png: screenshot.toString('base64'), foregroundPng: nativeGradientScreenshot?.toString('base64'), states });
  for (const probe of probes) await probe.locator.evaluate(element => {
    delete element.dataset.readingProbe;
    delete element.dataset.readingGradientProbe;
  });
  return states.map((state, index) => {
    if (!/^rgba?\(/.test(state.color)) throw new Error(`Unexpected text color: ${state.color}`);
    const color = state.color.match(/[\d.]+/g).map(Number);
    const alpha = (color[3] ?? 1) * state.opacity;
    const gradient = gradients[index];
    const samples = backgrounds[index].map((background, sampleIndex) => {
      const foreground = gradient ? nativeForegrounds[index][sampleIndex]
        : color.slice(0, 3).map((value, channel) => value * alpha + background[channel] * (1 - alpha));
      const ink = luminance(foreground), fill = luminance(background);
      return { background, ...(gradient ? { foreground } : {}), contrast: (Math.max(ink, fill) + .05) / (Math.min(ink, fill) + .05) };
    });
    return { role: probes[index].role, color: state.color, opacity: state.opacity,
      ...(gradient ? { gradient: { ...gradient, box: state.box, nativeVerified: true,
        nativeForegroundPng: gradientArtifactPrefix ? `${gradientArtifactPrefix}-native-foreground.png` : undefined,
        backgroundPng: gradientArtifactPrefix ? `${gradientArtifactPrefix}-background.png` : undefined } } : {}), contrast: Math.min(...samples.map(sample => sample.contrast)), samples };
  });
}
