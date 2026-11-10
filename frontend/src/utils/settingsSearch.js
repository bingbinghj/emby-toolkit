import { notificationKinds, scopeOptions } from './notificationOptions.js';
export const settingsCategoryNames = { system: '系统', directory: '存储 & 目录', rule: '规则', search: '数据源', notification: '通知' };

const notificationKeywords = { telegram_notify_types: notificationKinds.map(kind => kind.title).join(' '), notification_scope: scopeOptions.map(scope => scope.title).join(' ') };

export function searchSettings(catalog, query) {
  const words = String(query || '').trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return [];
  return catalog.filter(entry => {
    const text = [entry.label, entry.card, settingsCategoryNames[entry.category], entry.keywords?.join(' '), notificationKeywords[entry.field], entry.card === '通知渠道' ? 'Telegram' : ''].join(' ').toLocaleLowerCase();
    return words.every(word => text.includes(word));
  });
}

export function settingsResultRoute(entry) {
  return entry.route === 'notification' ? { name: 'settings-notifications' }
    : { name: 'settings-general', query: entry.category === 'system' ? {} : { tab: entry.category } };
}
