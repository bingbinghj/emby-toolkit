import { ref, onMounted, onUnmounted } from 'vue';
import { parseLogRecord } from '@/utils/logRecords';

export function useLogStream() {
  const logs = ref([]), paused = ref(false), error = ref(''), receivedCount = ref(0), generation = ref(0);
  let stream, timer, buffer = [], active = true;
  const flush = () => {
    clearTimeout(timer); timer = undefined;
    if (buffer.length) { logs.value = [...logs.value, ...buffer].slice(-600); buffer = []; }
  };
  const close = () => { stream?.close(); stream = undefined; };
  const connect = () => {
    if (!active || paused.value) return;
    close();
    const current = new EventSource('/api/logs/stream');
    stream = current;
    current.onopen = () => {
      if (stream !== current || !active) return;
      clearTimeout(timer); timer = undefined; buffer = []; logs.value = []; error.value = ''; receivedCount.value = 0; generation.value++;
    };
    current.onmessage = event => {
      if (stream !== current || !active || paused.value || !event.data) return;
      error.value = '';
      const text = String(event.data).replace(/\r\n/g, '\n');
      for (const line of text.split('\n')) {
        if (parseLogRecord(line).level === 'PLAIN' && (buffer.length || logs.value.length)) {
          if (buffer.length) buffer[buffer.length - 1] += '\n' + line;
          else { const next = [...logs.value]; next[next.length - 1] += '\n' + line; logs.value = next; }
        } else { buffer.push(line); receivedCount.value++; }
      }
      timer ||= setTimeout(flush, 80);
    };
    current.onerror = () => {
      if (stream === current && active && !paused.value) error.value = '实时日志连接失败，正在重试…';
    };
  };
  const pause = () => { flush(); paused.value = true; close(); };
  const resume = () => { if (paused.value) { paused.value = false; connect(); } };
  onMounted(connect);
  onUnmounted(() => { active = false; close(); clearTimeout(timer); buffer = []; });
  return { logs, paused, error, receivedCount, generation, pause, resume };
}
