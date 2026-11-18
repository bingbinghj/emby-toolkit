export function primaryImageUrl(id, tag) {
  if (typeof id !== 'string' || !/^[\w-]{1,128}$/.test(id)) return '';
  const query = new URLSearchParams({ maxWidth: '240', quality: '90' });
  if (tag) query.set('tag', tag);
  return `/image_proxy/Items/${encodeURIComponent(id)}/Images/Primary?${query}`;
}
