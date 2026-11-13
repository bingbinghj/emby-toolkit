import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { ref, reactive, computed, watch, nextTick } from 'vue';
import { groupLogRecords, matchesLogRecord } from '../src/utils/logRecords.js';

const source = readFileSync(new URL('../src/components/NotificationCenter.vue', import.meta.url), 'utf8').match(/<script setup>([\s\S]*?)<\/script>/)[1].replace(/^import .*;\n/gm, '');
const plain = value => JSON.parse(JSON.stringify(value));
const line = index => `INFO: 2026-11-13 13:19:${String(index).padStart(2, '0')},476 sync.rs - 记录${index}`;
function fixture() {
  const props = reactive({ task: { logs: [line(1), line(2)] } });
  const stops = [];
  const stream = { logs: ref([...props.task.logs]), paused: ref(false), error: ref(''), receivedCount: ref(2), generation: ref(1) };
  const receive = logs => { if (!stream.paused.value) { stream.logs.value = [...logs]; stream.receivedCount.value++; } };
  stops.push(watch(() => props.task.logs, receive));
  stream.pause = () => { stream.paused.value = true; };
  stream.resume = () => { stream.paused.value = false; receive(props.task.logs); };
  const context = vm.createContext({
    computed, ref, nextTick, groupLogRecords, matchesLogRecord,
    watch: (...args) => { const stop = watch(...args); stops.push(stop); return stop; },
    defineProps: () => props, defineEmits() {},
    useDisplay: () => ({ mdAndUp: ref(true) }), useTheme: () => ({ global: { current: ref({ dark: false }) } }),
    useMessage: () => ({ error() {} }), useLogStream: () => stream,
    axios: {}, setTimeout, URL,
  });
  vm.runInContext(`${source}\nthis.logs = displayedLogs; this.follow = following;
    this.scroller = logRef; this.scroll = handleScroll;
    this.resume = typeof enableFollow === 'function' ? enableFollow : () => following.value = true;
    this.toggle = typeof toggleStream === 'function' ? toggleStream : () => following.value = !following.value;
    this.paused = typeof streamPaused === 'undefined' ? computed(() => !following.value) : streamPaused;
    this.pending = typeof pendingLogCount === 'undefined' ? ref(0) : pendingLogCount;`, context);
  return { context, props, stream, close: () => stops.forEach(stop => stop()) };
}

test('reconnecting while reading history clears old pending counts and does not count replayed tail lines as new', async t => {
  const f = fixture(); t.after(f.close); await nextTick();
  f.context.scroller.value = { scrollHeight: 1000, clientHeight: 100, scrollTop: 20 };
  f.context.scroll();
  f.props.task = { logs: [line(1), line(2), line(3)] }; await nextTick(); await nextTick();
  assert.equal(f.context.pending.value, 1);
  f.stream.generation.value++;
  f.stream.receivedCount.value = 50;
  f.stream.logs.value = Array.from({ length: 50 }, (_, index) => line(index));
  await nextTick(); await nextTick();
  assert.equal(f.context.pending.value, 0);
  f.stream.receivedCount.value++;
  await nextTick();
  assert.equal(f.context.pending.value, 1);
});

test('scrolling up keeps receiving logs and counts new entries without moving the viewport', async t => {
  const f = fixture(); t.after(f.close); await nextTick();
  const el = { scrollHeight: 1000, clientHeight: 100, scrollTop: 20 };
  f.context.scroller.value = el; f.context.scroll();
  assert.equal(f.context.follow.value, false);
  assert.equal(f.context.paused.value, false);
  f.props.task = { logs: [line(1), line(2), line(3)] }; await nextTick(); await nextTick();
  assert.deepEqual(plain(f.context.logs.value), [line(1), line(2), line(3)]);
  assert.equal(f.context.pending.value, 1);
  assert.equal(el.scrollTop, 20);
  f.props.task = { logs: [line(2), line(3), line(4)] }; await nextTick(); await nextTick();
  assert.equal(f.context.pending.value, 2, 'rolling snapshots count the new suffix');
  f.context.resume(); await nextTick(); await nextTick();
  assert.equal(f.context.follow.value, true);
  assert.equal(f.context.pending.value, 0);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(el.scrollTop, 1000);
});

test('explicit pause freezes reception until resumed and remains independent of scroll position', async t => {
  const f = fixture(); t.after(f.close); await nextTick();
  f.context.toggle();
  f.props.task = { logs: [line(1), line(2), line(3)] }; await nextTick();
  assert.deepEqual(plain(f.context.logs.value), [line(1), line(2)]);
  f.context.scroller.value = { scrollHeight: 1000, clientHeight: 100, scrollTop: 1000 };
  f.context.scroll();
  assert.equal(f.context.paused.value, true);
  f.context.toggle(); await nextTick(); await nextTick();
  assert.equal(f.context.paused.value, false);
  assert.deepEqual(plain(f.context.logs.value), [line(1), line(2), line(3)]);
});
