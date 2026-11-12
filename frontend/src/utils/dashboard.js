export const dashboardModules = [
  { id: 'storage', title: '存储空间', icon: 'mdi-harddisk', width: 'summary' },
  { id: 'statistics', title: '媒体统计', icon: 'mdi-chart-box-outline', width: 'statistics' },
  { id: 'recent', title: '最新入库 - Emby', icon: 'mdi-clock-outline', width: 'full' },
  { id: 'libraries', title: '我的媒体库', icon: 'mdi-folder-multiple-outline', width: 'full' },
];

export function normalizeDashboardLayout(value) {
  const ids = dashboardModules.map(item => item.id);
  const order = Array.isArray(value?.order) ? [...new Set(value.order.filter(id => ids.includes(id)))] : [];
  return {
    order: [...order, ...ids.filter(id => !order.includes(id))],
    hidden: Array.isArray(value?.hidden) ? [...new Set(value.hidden.filter(id => ids.includes(id)))] : [],
  };
}

export function dashboardLayoutProfile(width) {
  return width <= 640 ? 'mobile' : width <= 960 ? 'tablet' : 'desktop';
}

export function dashboardColumns(profile) {
  return profile === 'mobile' ? 1 : profile === 'tablet' ? 6 : 12;
}

export function normalizeDashboardProfile(value, profile = 'desktop') {
  const columns = dashboardColumns(profile);
  const items = {};
  for (const { id } of dashboardModules) {
    const item = value?.items?.[id];
    if (!item || !['x', 'y', 'w', 'h'].every(key => Number.isInteger(item[key]))) continue;
    if (item.x < 0 || item.y < 0 || item.y > 10000 || item.w < 1 || item.w > columns || item.x + item.w > columns || item.h < 1 || item.h > 1000) continue;
    items[id] = { x: item.x, y: item.y, w: item.w, h: item.h, manualHeight: item.manualHeight === true };
  }
  return { ...normalizeDashboardLayout(value), items, ...(Number.isSafeInteger(value?.updatedAt) && value.updatedAt >= 0 ? { updatedAt: value.updatedAt } : {}) };
}

const capacity = value => {
  const raw = value && typeof value === 'object' ? value.size : value;
  if (raw === null || raw === undefined || raw === '' || typeof raw === 'boolean') return null;
  const parsed = Number(raw);
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : null;
};

export function storageFrom115(response) {
  const data = response?.data;
  if (data?.valid !== true || !data.user_info) throw new Error(data?.msg || '尚未读取到有效的 115 空间信息');
  const user = data.user_info;
  const raw = user.rt_space_info || user.space_info || {};
  let total = capacity(raw.all_total ?? user.total_space ?? user.total_size);
  let used = capacity(raw.all_use ?? raw.all_used ?? user.used_space ?? user.used_size);
  let available = capacity(raw.all_remain ?? user.remaining_space ?? user.remaining_size);
  if (total === null && used !== null && available !== null) total = used + available;
  if (used === null && total !== null && available !== null && total >= available) used = total - available;
  if (available === null && total !== null && used !== null && total >= used) available = total - used;
  if (total !== null && ((used !== null && used > total) || (available !== null && available > total))) {
    throw new Error('115 空间数据不一致，请重新读取');
  }
  const text = (value, bytes) => bytes !== null ? formatDashboardBytes(bytes) : (value?.size_format || '未知');
  const result = {
    total, used, available,
    totalText: text(raw.all_total, total), usedText: text(raw.all_use ?? raw.all_used, used),
    availableText: text(raw.all_remain, available),
    percent: total > 0 && used !== null ? Math.round(used / total * 1000) / 10 : null,
  };
  if ([total, used, available].every(value => value === null)
      && [result.totalText, result.usedText, result.availableText].every(value => value === '未知')) {
    throw new Error('115 未返回可用空间信息，请重新读取');
  }
  return result;
}

export function formatDashboardBytes(bytes) {
  if (!Number.isFinite(bytes) || bytes < 0) return '未知';
  const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];
  const index = bytes > 0 ? Math.max(0, Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1)) : 0;
  return `${(bytes / 1024 ** index).toLocaleString('zh-CN', { maximumFractionDigits: 2 })} ${units[index]}`;
}

export function formatDashboardCount(value) {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0
    ? value.toLocaleString('zh-CN') : '—';
}

// MoviePilot useDashboardSnapshot: last successful result, per user, for 24 hours.
export function dashboardSnapshot(userId, key, storage, now = Date.now) {
  if (storage === undefined) { try { storage = globalThis.localStorage; } catch {} }
  const storageKey = userId ? `ETK_DASHBOARD_SNAPSHOT_V1:${userId}:${key}` : null;
  const clear = () => { try { if (storageKey) storage.removeItem(storageKey); } catch {} };
  return {
    clear,
    read() {
      if (!storageKey) return undefined;
      try {
        const raw = storage.getItem(storageKey);
        if (!raw) return undefined;
        const snapshot = JSON.parse(raw);
        const age = now() - Number(snapshot?.savedAt);
        if (!Number.isFinite(age) || age < 0 || age > 24 * 60 * 60 * 1000 || !Object.hasOwn(snapshot, 'value')) {
          clear(); return undefined;
        }
        return snapshot;
      } catch { clear(); return undefined; }
    },
    write(value) {
      const snapshot = { savedAt: now(), value };
      try { if (storageKey) storage.setItem(storageKey, JSON.stringify(snapshot)); } catch {}
      return snapshot;
    },
  };
}

export function projectDashboardItems(items, library = false) {
  if (!Array.isArray(items) || items.some(item => typeof item?.id !== 'string' || !/^[\w-]{1,128}$/.test(item.id) || typeof item.name !== 'string')) {
    throw new Error(library ? '媒体库响应未确认' : '最新入库响应未确认');
  }
  const fields = library ? ['id', 'name', 'collection_type', 'image_tag', 'item_count', 'count_error'] : [
    'id', 'name', 'item_type', 'production_year', 'image_tag', 'series_id', 'series_image_tag',
    'series_name', 'season_number', 'episode_number', 'date_created',
  ];
  return items.map(item => {
    const values = fields.filter(field => Object.hasOwn(item, field)).map(field => [field, item[field]]);
    if (values.some(([, value]) => value !== null && !['string', 'number'].includes(typeof value))) throw new Error('媒体卡片响应未确认');
    return Object.fromEntries(values);
  });
}

export function projectDashboardStatistics(value) {
  const result = Object.fromEntries(['movie_count', 'tv_count', 'episode_count', 'music_count'].map(key => {
    const item = value?.[key];
    if (typeof item?.count !== 'number' || !Number.isFinite(item.count) || item.count < 0 || item.error) {
      throw new Error(item?.error || '媒体统计响应未确认');
    }
    const projected = { count: item.count };
    if (Object.hasOwn(item, 'month_count')) {
      if (item.month_count !== null && (!Number.isSafeInteger(item.month_count) || item.month_count < 0)) throw new Error('本月新增统计响应未确认');
      projected.month_count = item.month_count;
    }
    return [key, projected];
  }));
  if (value.month !== undefined) {
    const month = value.month;
    if (!month || !/^\d{4}-(0[1-9]|1[0-2])$/.test(month.period) || !['recording', 'unavailable'].includes(month.state) || month.source !== 'etk-history') throw new Error('本月新增统计来源未确认');
    const fields = ['period', 'state', 'source', 'recording_since', 'partial', 'error'];
    if (fields.some(field => Object.hasOwn(month, field) && month[field] !== null && typeof month[field] !== (field === 'partial' ? 'boolean' : 'string'))) throw new Error('本月新增统计来源未确认');
    result.month = Object.fromEntries(fields.filter(field => Object.hasOwn(month, field)).map(field => [field, month[field]]));
  }
  return result;
}

// A legacy snapshot or a previous month's cached value must never appear as a
// confirmed zero (or be relabelled as this month's admissions).
export function dashboardMonthlyCount(statistics, key, now = new Date()) {
  const period = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}`;
  const count = statistics?.[key]?.month_count;
  return statistics?.month?.source === 'etk-history' && statistics.month.state === 'recording' && statistics.month.period === period && Number.isSafeInteger(count) && count >= 0 ? count : null;
}

export function embyDashboardImage(item, library = false) {
  const parent = !library && item.series_image_tag && item.series_id
    && (item.item_type === 'Episode' || !item.image_tag);
  const id = parent ? item.series_id : item.id;
  if (typeof id !== 'string' || !/^[\w-]{1,128}$/.test(id)) return '';
  const tag = parent ? item.series_image_tag : item.image_tag;
  return `/image_proxy/Items/${encodeURIComponent(id)}/Images/Primary?quality=90${tag ? `&tag=${encodeURIComponent(tag)}` : ''}`;
}
