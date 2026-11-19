// Adapted from MoviePilot-Frontend v3 db17d9c (MIT).
export function previewSourceKey(item) {
  return JSON.stringify([item.source_item?.storage ?? item.source_storage, item.source_item?.path ?? item.source]);
}
export function groupMusicPreview(items) {
  const groups = /* @__PURE__ */ new Map();
  items.forEach((item, index) => {
    const id = item.music?.group_id ? JSON.stringify([item.source_storage ?? item.source_item?.storage, item.music.group_id]) : `file:${previewSourceKey(item)}:${index}`;
    const group = groups.get(id) ?? { id, items: [] };
    group.items.push(item);
    groups.set(id, group);
  });
  return [...groups.values()];
}
export function previewFileScope(items) {
  const files = [];
  const keys = /* @__PURE__ */ new Set();
  for (const item of items) {
    const source = item.source_item;
    const key = previewSourceKey(item);
    if (!source?.path || !source.storage || source.type !== "file" || keys.has(key) || item.source && source.path !== item.source || item.source_storage && source.storage !== item.source_storage)
      return;
    keys.add(key);
    files.push({ ...source });
  }
  return files.length ? files : void 0;
}
export function canCorrectMusicGroup(group) {
  return Boolean(
    group.items.length && group.items.every((item) => item.music?.group_id && item.source_item?.storage === "local") && group.items.some((item) => item.music?.file_role === "audio") && group.items.every(
      (item) => (item.music?.file_role === "companion" ? item.music.read_status === "companion" : ["tags", "stream_only"].includes(item.music?.read_status ?? "")) && item.music?.status !== "unsupported"
    ) && previewFileScope(group.items)
  );
}
