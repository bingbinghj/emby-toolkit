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

// 按正文顺序对齐字符，允许两个版本的分行位置不同；限制矩阵大小，避免长歌词阻塞页面。
function alignLyricText(source, local) {
  const width=local.length+1;
  if(!source || !local || (source.length+1)*width>8_000_000)return new Map();
  const lengths=new Uint16Array((source.length+1)*width);
  for(let i=source.length-1;i>=0;i--)for(let j=local.length-1;j>=0;j--){
    lengths[i*width+j]=source[i]===local[j] ? lengths[(i+1)*width+j+1]+1 : Math.max(lengths[(i+1)*width+j],lengths[i*width+j+1]);
  }
  const positions=new Map();
  let i=0,j=0;
  while(i<source.length && j<local.length){
    if(source[i]===local[j])positions.set(i++,j++);
    else if(lengths[(i+1)*width+j]>lengths[i*width+j+1])i++;
    else j++;
  }
  return positions;
}

// 只采用译文已有的分句；括号内空格、英文单词间空格不能作为中文分句。
function translationPhrases(text) {
  if(text.includes('\n'))return [text];
  const pairs={'（':'）','(':')','「':'」','『':'』','[':']'}, stack=[], parts=[];
  const speaker=text.match(/^[（(][^()（）\r\n]{1,30}[)）]\s*/)?.[0] || '';
  let part='';
  const flush=()=>{if(part.trim())parts.push(part.trim());part='';};
  for(const char of text.slice(speaker.length)){
    if(pairs[char])stack.push(pairs[char]);
    else if(stack.at(-1)===char)stack.pop();
    if(!stack.length && /\s/u.test(char))flush();
    else {part+=char;if(!stack.length && /[，。；！？,;!?]/u.test(char))flush();}
  }
  flush();
  if(parts.length>1 && parts.every(value=>/\p{Script=Han}/u.test(value))){parts[0]=speaker+parts[0];return parts;}
  return [text];
}

export function lyricTranslationRows(text='') {
  return text.split('\n').map((line,index)=>{
    const content=line.replace(/^(?:\[(?:\d+:\d{1,2}(?:[.:]\d+)?|[A-Za-z][\w-]*:[^\]\r\n]*)\])+/,'').replace(/<\d+:\d{1,2}(?:[.:]\d+)?>/g,'').trim();
    return {index,text:line,selectable:/\p{L}/u.test(content),foreign:/\p{L}/u.test(content.replace(/\p{Script=Han}/gu,''))};
  });
}

// 机器译文只提供分界证据，输出始终取自来源译文。两侧必须有连续文字锚点，不能靠长度猜。
function splitWithReference(text, references, key) {
  if(references.some(value=>!value) || /[\r\n]/.test(text))return null;
  const chars=[...text], normalized=[], offsets=[];
  chars.forEach((char,index)=>{for(const value of key(char)){normalized.push(value);offsets.push(index);}});
  const keys=references.map(key), target=normalized.join('');
  if(!keys.every(value=>value.length>=2) || !target)return null;
  const positions=alignLyricText(keys.join(''),target), groups=[];
  let start=0;
  for(const value of keys){
    const aligned=Array.from({length:value.length},(_,i)=>positions.get(start+i)).filter(i=>i!==undefined);
    start+=value.length;
    if(aligned.length/value.length<.65 || !aligned.some((v,i)=>i>0 && v===aligned[i-1]+1))return null;
    groups.push(aligned);
  }
  if(positions.size/target.length<.65)return null;
  const cuts=[0];
  for(let i=1;i<groups.length;i++){
    if(groups[i-1].at(-1)+1!==groups[i][0])return null;
    const cut=offsets[groups[i][0]];
    // 不在英文单词中间或括号内切开。
    if(/[A-Za-z]/.test(chars[cut-1]) && /[A-Za-z]/.test(chars[cut]))return null;
    const prefix=chars.slice(0,cut).join('');
    if((prefix.match(/[（(「『]/g)||[]).length!==(prefix.match(/[）)」』]/g)||[]).length)return null;
    cuts.push(cut);
  }
  cuts.push(chars.length);
  return cuts.slice(0,-1).map((cut,i)=>chars.slice(cut,cuts[i+1]).join('').trim());
}

// 用来源原文核对译文对应关系，沿用本地原文时间；有明确分句时逐行分配，否则保留整句。
export function mergeLyricsTranslation(original, sourceOriginal, translation, referenceLines=[]) {
  const clean=text=>text.replace(/\[\d+:\d{2}(?:[.:]\d+)?\]|<\d+:\d{2}(?:[.:]\d+)?>/g,'').trim();
  const key=text=>clean(text).normalize('NFKC').replace(/^\([^()\r\n]{1,30}\)\s*(?=\S)/,'').toLowerCase().replace(/[^\p{L}\p{M}\p{N}]/gu,'');
  const translated=parseLyrics(translation), source=parseLyrics(sourceOriginal), matches=new Map(), byTime=new Map();
  for(const row of translated){const stamp=Math.round(row.time*1000),lines=byTime.get(stamp) || [];if(row.text)lines.push(row.text);byTime.set(stamp,lines);}
  const add=(text,value)=>{if(!key(text) || !value || key(text)===key(value))return;const values=matches.get(key(text)) || new Set();values.add(value);matches.set(key(text),values);};
  for(const row of source){
    const lines=byTime.get(Math.round(row.time*1000)) || [];
    if(lines.length)add(row.text,[...new Set(lines)].join('\n'));
  }
  const plain=text=>text.split(/\r?\n/).filter(line=>line.trim() && !/^\[[a-z]+:/i.test(line));
  if(!source.length && !translated.length){const left=plain(sourceOriginal),right=plain(translation);if(left.length===right.length)left.forEach((text,index)=>add(text,right[index].trim()));}
  const lines=original.split(/\r?\n/), output=[];
  const sourceRows=source.filter(row=>key(row.text)).map(row=>({...row,key:key(row.text)}));
  const sourceText=sourceRows.length ? sourceRows.map(row=>row.key).join('') : plain(sourceOriginal).map(key).join('');
  const translationTexts=translated.length ? translated.map(row=>row.text) : plain(translation);
  const translationKeys=new Set(translationTexts.flatMap(text=>[text,...translationPhrases(text)]).map(key));
  const knownTranslation=value=>translationKeys.has(value) || /\p{Script=Han}/u.test(value) && value.length>=2 && [...translationKeys].some(text=>text.includes(value));
  const localRows=[];
  let offset=0;
  lines.forEach((line,index)=>{
    const value=key(line);
    if(!value || /^\[[a-z]+:/i.test(line) || knownTranslation(value) && !sourceText.includes(value))return;
    localRows.push({index,key:value,start:offset,end:offset+value.length});offset+=value.length;
  });
  const positions=alignLyricText(sourceText,localRows.map(row=>row.key).join(''));
  const additions=new Map(),covered=new Set(),replaced=new Set();
  const reference=new Map(referenceLines.map(row=>[row.index,row.text]));
  const pendingSegments=[], splits=[];
  const stamps=line=>[...line.matchAll(/\[\d+:\d{2}(?:[.:]\d+)?\]/g)].map(match=>match[0]).join('');
  const append=(index,text)=>{const values=additions.get(index) || new Set();text.split('\n').forEach(line=>values.add(line));additions.set(index,values);};
  offset=0;
  for(const row of sourceRows){
    let aligned=Array.from({length:row.key.length},(_,index)=>positions.get(offset+index)).filter(value=>value!==undefined);
    offset+=row.key.length;
    const value=[...new Set(byTime.get(Math.round(row.time*1000)) || [])].join('\n');
    // 重复副歌可能夹着和声；优先采用与当前对齐位置重合的完整原文行。
    const exact=localRows.find(local=>local.key===row.key && aligned.filter(position=>position>=local.start && position<local.end).length>=row.key.length*.6);
    if(exact)aligned=Array.from({length:row.key.length},(_,index)=>exact.start+index);
    // 原文与本地句段均需足够一致；少于六字只接受完整匹配，不能只靠时间或行号猜测。
    if(!value || key(value)===row.key || aligned.length/row.key.length<(row.key.length<6 ? 1 : .8) || aligned.length/(aligned.at(-1)-aligned[0]+1)<.8)continue;
    const first=localRows.find(local=>local.end>aligned[0]);
    if(!first)continue;
    const rows=localRows.filter(local=>local.end>aligned[0] && local.start<=aligned.at(-1));
    let phrases=translationPhrases(value), method='phrases';
    const completeRows=rows.length>1 && rows.every(local=>{
      const count=aligned.filter(position=>position>=local.start && position<local.end).length;
      return count/local.key.length>=(local.key.length<6 ? 1 : .8);
    });
    const comparableRows=rows.length>1 && rows.every(local=>aligned.filter(position=>position>=local.start && position<local.end).length/local.key.length>=.6);
    if(!(completeRows && phrases.length===rows.length) && comparableRows){
      const existing=rows.map(local=>{
        const next=localRows.find(item=>item.index>local.index)?.index ?? lines.length;
        const candidates=lines.slice(local.index+1,next).filter(line=>stamps(line)===stamps(lines[local.index]) && key(value).includes(key(line)) && key(line));
        return candidates.length===1 ? clean(candidates[0]) : '';
      });
      phrases=existing.every(Boolean) && existing.map(key).join('')===key(value) ? existing : splitWithReference(value,rows.map(local=>reference.get(local.index)),key) || [];
      method='reference';
    }
    if((completeRows || comparableRows && method==='reference') && phrases.length===rows.length){
      rows.forEach((local,index)=>append(local.index,phrases[index]));
      splits.push({indices:rows.map(local=>local.index),phrases,method});
      // 升级此前由同一来源合并的整句译文；用户自写的译文保留。
      for(let index=first.index+1;index<rows[1].index;index++){
        if(clean(lines[index])===value && stamps(lines[index])===stamps(lines[first.index]))replaced.add(index);
      }
    }else {
      append(first.index,value);
      if(rows.length>1)pendingSegments.push({indices:rows.map(local=>local.index),translation:value,canCompare:comparableRows});
    }
    aligned.forEach(position=>covered.add(position));
  }
  let matched=0;
  for(const row of localRows){
    const values=matches.get(row.key);
    let coveredCount=0;
    for(let i=row.start;i<row.end;i++)if(covered.has(i))coveredCount++;
    if(!additions.has(row.index) && coveredCount<row.key.length*.6 && values?.size===1){
      append(row.index,[...values][0]);coveredCount=row.key.length;
    }
    if(coveredCount>=row.key.length*.6)matched++;
  }
  for(let index=0;index<lines.length;index++){
    if(replaced.has(index))continue;
    const line=lines[index];output.push(line);
    const values=additions.get(index);
    if(!values)continue;
    const stamps=[...line.matchAll(/\[\d+:\d{2}(?:[.:]\d+)?\]/g)].map(match=>match[0]).join('');
    const next=localRows.find(row=>row.index>index)?.index ?? lines.length;
    for(const value of values){const addition=stamps+value;if(!lines.slice(index+1,next).includes(addition))output.push(addition);}
  }
  return {text:output.join('\n'),matched,total:localRows.length,splits,pendingSegments,
    pendingLineIndices:[...new Set(pendingSegments.filter(segment=>segment.canCompare).flatMap(segment=>segment.indices))]};
}
