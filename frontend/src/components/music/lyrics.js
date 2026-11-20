// 整体偏移只改时间标签；保留歌词文本、多时间标签和逐字 LRC。
export function shiftLyrics(text, seconds) {
  if (!Number.isFinite(Number(seconds))) throw new Error('请输入有效偏移秒数');
  const offset = text.match(/\[offset:([+-]?\d+)\]/i);
  const delta = Math.round(Number(seconds)*1000) - Number(offset?.[1] || 0);
  let count = 0;
  const output = text.replace(/\[offset:[+-]?\d+\]\s*/gi, '').replace(/([\[<])(\d+):(\d{2})([.:](\d+))?([\]>])/g, (match, open, minutes, second, _fraction, fraction, close) => {
    if ((open === '[' && close !== ']') || (open === '<' && close !== '>')) return match;
    const time = Number(minutes)*60000 + Number(second)*1000 + Math.round(Number(`0.${fraction || 0}`)*1000) + delta;
    if (time < 0) throw new Error('偏移后存在负时间，请减小提前量');
    count++;
    return `${open}${String(Math.floor(time/60000)).padStart(2,'0')}:${String(Math.floor(time/1000)%60).padStart(2,'0')}.${String(time%1000).padStart(3,'0')}${close}`;
  });
  if (!count) throw new Error('当前歌词没有 LRC 时间标签');
  return output;
}

export function parseLyrics(text = '') {
  const offset = Number(text.match(/\[offset:([+-]?\d+)\]/i)?.[1] || 0) / 1000;
  const rows = [];
  for (const line of text.split(/\r?\n/)) {
    const stamps = [...line.matchAll(/\[(\d+):(\d{2})(?:[.:](\d+))?\]/g)];
    const content = line.replace(/\[\d+:\d{2}(?:[.:]\d+)?\]|<\d+:\d{2}(?:[.:]\d+)?>/g, '').trim();
    for (const stamp of stamps) rows.push({time: Math.max(0, Number(stamp[1])*60 + Number(stamp[2]) + Number(`0.${stamp[3] || 0}`) - offset), text:content});
  }
  return rows.sort((a,b)=>a.time-b.time);
}
