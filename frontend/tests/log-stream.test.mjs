import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { ref } from 'vue';
import { parseLogRecord } from '../src/utils/logRecords.js';

const source = readFileSync(new URL('../src/composables/useLogStream.js', import.meta.url), 'utf8')
  .replace(/^import .*;\n/gm, '').replaceAll('export function', 'function');
const entry = (index, level = 'INFO') => `${level}: 2026-11-13 13:19:14,476 sync.rs - 完整记录${index}`;
function fixture() {
  const streams = [], timers = new Map(); let unmount, sequence = 0;
  const context = vm.createContext({ ref, parseLogRecord,
    EventSource: class { constructor(url) { this.url = url; streams.push(this); } close() { this.closed = true; } },
    onMounted: callback => callback(), onUnmounted: callback => { unmount = callback; },
    setTimeout: (callback, delay) => { const id = ++sequence; timers.set(id, { callback, delay }); return id; },
    clearTimeout: id => timers.delete(id),
  });
  vm.runInContext(`${source}\nthis.flow = useLogStream();`, context);
  const flush = () => { const pending = [...timers.values()]; timers.clear(); pending.forEach(timer => timer.callback()); };
  return { flow: context.flow, streams, timers, flush, unmount: () => unmount() };
}

test('the independent stream batches raw DEBUG and multiline records without task status snapshots', () => {
  const f = fixture();
  assert.equal(f.streams[0].url, '/api/logs/stream');
  f.streams[0].onopen();
  f.streams[0].onmessage({ data: `${entry(1, 'DEBUG')}\n原始续行\n\n${entry(2)}` });
  assert.equal(f.flow.logs.value.length, 0);
  assert.equal([...f.timers.values()][0].delay, 80);
  f.flush();
  assert.deepEqual(Array.from(f.flow.logs.value), [`${entry(1, 'DEBUG')}\n原始续行\n`, entry(2)]);
  assert.equal(f.flow.receivedCount.value, 2);
  f.streams[0].onmessage({ data: `跨数据块续行\n${entry(3)}` }); f.flush();
  assert.deepEqual(Array.from(f.flow.logs.value), [`${entry(1, 'DEBUG')}\n原始续行\n`, `${entry(2)}\n跨数据块续行`, entry(3)]);
});

test('the stream retains the newest 600 complete records and counts actual new entries', () => {
  const f = fixture(); f.streams[0].onopen();
  f.streams[0].onmessage({ data: Array.from({ length: 605 }, (_, index) => entry(index)).join('\n') }); f.flush();
  assert.equal(f.flow.logs.value.length, 600);
  assert.equal(f.flow.logs.value[0], entry(5));
  assert.equal(f.flow.logs.value.at(-1), entry(604));
  assert.equal(f.flow.receivedCount.value, 605);
});

test('pause flushes received content, closes reception and resumes with an owned fresh stream', () => {
  const f = fixture(), old = f.streams[0]; old.onopen(); old.onmessage({ data: entry(1) });
  f.flow.pause();
  assert.equal(f.flow.paused.value, true);
  assert.equal(old.closed, true);
  assert.deepEqual(Array.from(f.flow.logs.value), [entry(1)]);
  old.onmessage({ data: entry(2) }); f.flush();
  assert.deepEqual(Array.from(f.flow.logs.value), [entry(1)]);
  f.flow.resume(); const current = f.streams[1]; current.onopen();
  old.onmessage({ data: entry(99) }); current.onmessage({ data: `${entry(1)}\n${entry(2)}` }); f.flush();
  assert.deepEqual(Array.from(f.flow.logs.value), [entry(1), entry(2)]);
  assert.equal(f.flow.paused.value, false);
});

test('connection failure keeps prior logs and reopening clears duplicate tail history', () => {
  const f = fixture(), stream = f.streams[0]; stream.onopen(); stream.onmessage({ data: entry(1) }); f.flush();
  stream.onerror();
  assert.equal(f.flow.error.value, '实时日志连接失败，正在重试…');
  assert.deepEqual(Array.from(f.flow.logs.value), [entry(1)]);
  stream.onopen(); stream.onmessage({ data: entry(1) }); f.flush();
  assert.equal(f.flow.error.value, '');
  assert.deepEqual(Array.from(f.flow.logs.value), [entry(1)]);
  assert.equal(f.flow.receivedCount.value, 1);
  assert.equal(f.flow.generation.value, 2);
});

test('the SSE payload preserves a real trailing blank continuation after its line terminator is removed by the server', () => {
  const f = fixture(), stream = f.streams[0]; stream.onopen();
  stream.onmessage({ data: `${entry(1)}\n` }); f.flush();
  assert.deepEqual(Array.from(f.flow.logs.value), [`${entry(1)}\n`]);
  assert.equal(f.flow.receivedCount.value, 1);
});

test('unmount closes the stream and prevents delayed data or buffers from changing logs', () => {
  const f = fixture(), stream = f.streams[0]; stream.onopen(); stream.onmessage({ data: entry(1) });
  f.unmount(); stream.onmessage({ data: entry(2) }); stream.onerror(); f.flush();
  assert.equal(stream.closed, true);
  assert.equal(f.timers.size, 0);
  assert.equal(f.flow.logs.value.length, 0);
  assert.equal(f.flow.error.value, '');
});
