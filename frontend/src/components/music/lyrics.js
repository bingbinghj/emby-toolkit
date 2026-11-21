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

export function formatLyrics(text) {
  // 统一分秒位数与毫秒精度，保持正文、重复时间和逐字时间标签。
  const normalized = text.replace(/([\[<])(\d+):(\d{1,2})([.:]\d+)?([\]>])/g, (all, open, minutes, seconds, fraction, close) => {
    if (Number(seconds) >= 60) throw new Error('歌词时间的秒数必须小于 60');
    return `${open}${minutes.padStart(2,'0')}:${seconds.padStart(2,'0')}${fraction || '.000'}${close}`;
  });
  return shiftLyrics(normalized, 0);
}

export function formatLyricTime(seconds) {
  if (!Number.isFinite(seconds) || seconds < 0) throw new Error('请输入有效的歌词时间');
  const ms = Math.round(seconds * 1000);
  return `${String(Math.floor(ms/60000)).padStart(2,'0')}:${String(Math.floor(ms/1000)%60).padStart(2,'0')}.${String(ms%1000).padStart(3,'0')}`;
}

function timelineTime(value) {
  if (!/^\d+:([0-5]?\d)(?:\.\d{1,3})?$/.test(value)) throw new Error('时间请使用 分:秒.毫秒 格式，秒数小于 60');
  const [minutes,seconds] = value.split(':').map(Number);
  return minutes*60+seconds;
}

export function timelineRows(text) {
  const normalized = /\[\d+:\d/.test(text) ? formatLyrics(text) : text;
  return normalized.split(/\r?\n/).flatMap(line => {
    const prefix = line.match(/^(?:\[\d+:\d{2}\.\d{3}\])+/)?.[0] || '';
    if (!prefix) return [{time:'',originalTime:null,text:line}];
    const times = [...prefix.matchAll(/\[([^\]]+)\]/g)].map(match=>match[1]);
    const content = line.slice(prefix.length), first = timelineTime(times[0]);
    return times.map(time=>{
      const seconds=timelineTime(time);
      return {time,originalTime:seconds,text:/<\d+:\d/.test(content) ? shiftLyrics(content,seconds-first) : content};
    });
  });
}

export function serializeTimeline(rows) {
  return rows.map(row=>{
    if (!row.time.trim()) return row.text;
    const seconds=timelineTime(row.time.trim());
    const content=row.originalTime!==null && /<\d+:\d/.test(row.text) ? shiftLyrics(row.text,seconds-row.originalTime) : row.text;
    return `[${formatLyricTime(seconds)}]${content}`;
  }).join('\n');
}

// 用来源原文核对译文对应关系，沿用本地原文的时间标签；不按行号强行对齐不同版本。
export function mergeLyricsTranslation(original, sourceOriginal, translation) {
  const clean=text=>text.replace(/\[\d+:\d{2}(?:[.:]\d+)?\]|<\d+:\d{2}(?:[.:]\d+)?>/g,'').trim();
  const key=text=>clean(text).normalize('NFKC').toLocaleLowerCase().replace(/[\p{P}\p{Z}\s]/gu,'');
  const translated=parseLyrics(translation), source=parseLyrics(sourceOriginal), matches=new Map(), byTime=new Map();
  for(const row of translated){const stamp=Math.round(row.time*1000),lines=byTime.get(stamp) || [];if(row.text)lines.push(row.text);byTime.set(stamp,lines);}
  const add=(text,value)=>{if(!key(text) || !value || key(text)===key(value))return;const values=matches.get(key(text)) || new Set();values.add(value);matches.set(key(text),values);};
  for(const row of source){
    const lines=byTime.get(Math.round(row.time*1000)) || [];
    if(lines.length)add(row.text,[...new Set(lines)].join('\n'));
  }
  const plain=text=>text.split(/\r?\n/).filter(line=>line.trim() && !/^\[[a-z]+:/i.test(line));
  if(!source.length && !translated.length){const left=plain(sourceOriginal),right=plain(translation);if(left.length===right.length)left.forEach((text,index)=>add(text,right[index].trim()));}
  let matched=0,total=0;
  const lines=original.split(/\r?\n/), output=[];
  for(let index=0;index<lines.length;index++){
    const line=lines[index];output.push(line);
    if(!clean(line) || /^\[[a-z]+:/i.test(line))continue;
    total++;
    const values=matches.get(key(line));
    if(values?.size!==1)continue;
    const stamps=[...line.matchAll(/\[\d+:\d{2}(?:[.:]\d+)?\]/g)].map(match=>match[0]).join('');
    const additions=[...values][0].split('\n').map(text=>stamps+text);
    if(!additions.every((text,offset)=>lines[index+1+offset]===text))output.push(...additions);
    matched++;
  }
  return {text:output.join('\n'),matched,total};
}
