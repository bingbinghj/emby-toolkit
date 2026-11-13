export function parseLogRecord(value) {
  const raw = String(value ?? '');
  const mp = raw.match(/^(TRACE|DEBUG|INFO|WARN(?:ING)?|ERROR|CRITICAL):\s*((?:\d{4}-\d{2}-\d{2}\s)?\d{2}:\d{2}:\d{2}(?:[,\.]\d+)?)\s+([^\s]+)\s+-\s+([\s\S]*)$/);
  const full = raw.match(/^(\d{4}-\d{2}-\d{2}\s\d{2}:\d{2}:\d{2}(?:[,\.]\d+)?)\s-\s(.+?)\s-\s(TRACE|DEBUG|INFO|WARN(?:ING)?|ERROR|CRITICAL)\s-\s([\s\S]*)$/);
  const live = raw.match(/^\[(\d{2}:\d{2}:\d{2})\]\s\[(TRACE|DEBUG|INFO|WARN(?:ING)?|ERROR|CRITICAL)\]\s([\s\S]*)$/);
  const timestamp = mp?.[2] || full?.[1] || live?.[1] || '';
  const level = (mp?.[1] || full?.[3] || live?.[2] || 'PLAIN').replace(/^WARN$/, 'WARNING');
  const message = mp?.[4] ?? full?.[4] ?? live?.[3] ?? raw;
  return { raw, timestamp, level, module: mp?.[3] || full?.[2] || '', message };
}

export function matchesLogRecord(value, keyword = '', level = 'all') {
  const record = parseLogRecord(value);
  const query = String(keyword || '').trim().toLocaleLowerCase();
  if (query && !record.raw.toLocaleLowerCase().includes(query)) return false;
  if (level === 'all') return true;
  if (level === 'progress') return /\[任务(?:进度|开始|结束|暂停|继续|恢复|控制)\]/.test(record.raw);
  if (level === 'error') return ['ERROR', 'CRITICAL'].includes(record.level);
  return record.level === String(level).toUpperCase();
}

export function highlightLogText(value, keyword = '') {
  const text = String(value ?? '');
  const query = String(keyword || '').trim();
  if (!query) return [{ text, matched: false }];
  const pattern = new RegExp(query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'gi');
  const parts = [];
  let cursor = 0;
  for (const match of text.matchAll(pattern)) {
    if (match.index > cursor) parts.push({ text: text.slice(cursor, match.index), matched: false });
    parts.push({ text: match[0], matched: true });
    cursor = match.index + match[0].length;
  }
  if (cursor < text.length) parts.push({ text: text.slice(cursor), matched: false });
  return parts;
}

// MP groups adjacent entries with the same second and level, separated by at most 100 ms.
export function groupLogRecords(lines) {
  const groups = [];
  for (const [index, line] of lines.entries()) {
    const record = parseLogRecord(line);
    const second = record.timestamp.replace(/[,\.]\d+$/, '');
    const timestamp = Date.parse(record.timestamp.replace(' ', 'T').replace(',', '.'));
    const previous = groups.at(-1);
    if (second && previous?.second === second && previous.level === record.level
      && (!Number.isFinite(timestamp) || !Number.isFinite(previous.timestamp) || Math.abs(timestamp - previous.timestamp) <= 100)) {
      previous.lines.push(line);
      previous.timestamp = timestamp;
    } else {
      groups.push({ key: `${second || 'log'}-${record.level}-${index}`, second, timestamp, level: record.level, lines: [line] });
    }
  }
  return groups;
}
