// MoviePilot v3 音乐控件的原始中文文案。
import messages from './messages.json';
const value = key => key.split('.').reduce((node, part) => node?.[part], messages);
export function useMusicI18n() {
  return { te: key => typeof value(key) === 'string', t: (key, values = {}) => String(value(key) ?? key).replace(/\{(\w+)\}/g, (_, name) => values[name] ?? name) };
}
