const compareText = (a, b) => String(a ?? '').localeCompare(String(b ?? ''), 'zh-CN', { numeric: true });

function compareNumber(a, b, direction) {
  const valid = value => value !== null && value !== undefined && value !== '' && Number.isFinite(Number(value));
  if (valid(a) !== valid(b)) return valid(a) ? -1 : 1;
  return valid(a) ? (Number(a) - Number(b)) * direction : 0;
}

export function sortReviewSearchResults(items, order = 'episode_asc') {
  const direction = order.endsWith('_desc') ? -1 : 1;
  return [...items].sort((a, b) => {
    if (order.startsWith('id_')) return compareText(a.item_id, b.item_id) * direction;
    const group = compareText(a.library_name, b.library_name)
      || compareText(a.series_name || a.item_name, b.series_name || b.item_name)
      || compareText(a.series_id, b.series_id);
    if (group) return group;
    const parent = item => ['Movie', 'Series'].includes(item.item_type) ? 0 : 1;
    return parent(a) - parent(b)
      || compareNumber(a.season_number, b.season_number, direction)
      || Number(a.item_type === 'Episode') - Number(b.item_type === 'Episode')
      || compareNumber(a.episode_number, b.episode_number, direction)
      || compareText(a.item_id, b.item_id);
  });
}

export function reviewEpisodeLabel(item) {
  const season = item.season_number;
  if (season === null || season === undefined || !['Season', 'Episode'].includes(item.item_type)) return '';
  const label = `S${String(season).padStart(2, '0')}`;
  return item.item_type === 'Episode' && item.episode_number !== null && item.episode_number !== undefined
    ? `${label}E${String(item.episode_number).padStart(2, '0')}` : label;
}
