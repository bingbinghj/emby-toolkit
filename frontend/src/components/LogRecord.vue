<template>
  <article class="log-record" :data-level="record.level">
    <div class="log-record-meta">
      <time v-if="record.timestamp" class="log-record-time">{{ record.timestamp }}</time>
      <span v-if="record.level !== 'PLAIN'" class="log-record-level">{{ record.level }}</span>
      <span v-if="record.module" class="log-record-module">{{ record.module }}</span>
      <span v-if="file" class="log-record-file">{{ file }}</span>
    </div>
    <div class="log-record-panel">
      <span class="log-record-accent" aria-hidden="true" />
      <p class="log-record-body"><template v-for="(part, index) in parts" :key="index"><mark v-if="part.matched">{{ part.text }}</mark><template v-else>{{ part.text }}</template></template></p>
    </div>
  </article>
</template>

<script setup>
import { computed } from 'vue';
import { parseLogRecord, highlightLogText } from '@/utils/logRecords';
const props = defineProps({ line: { type: String, default: '' }, file: { type: String, default: '' }, keyword: { type: String, default: '' } });
const record = computed(() => parseLogRecord(props.line));
const parts = computed(() => highlightLogText(record.value.message, props.keyword));
</script>

<style scoped>
.log-record { --log-accent: var(--ui-text-muted); flex-shrink:0; min-width:0; color:var(--ui-text); font:13px/1.65 ui-monospace, SFMono-Regular, Consolas, monospace; }
.log-record-meta { display:flex; align-items:baseline; flex-wrap:wrap; gap:8px; margin-bottom:4px; min-width:0; color:var(--ui-text); font-size:12px; }
.log-record-time { color:inherit; }
.log-record-level { font-weight:700; color:inherit; }
.log-record-file, .log-record-module { min-width:0; max-width:100%; overflow-wrap:anywhere; }
.log-record-panel { display:flex; align-items:flex-start; gap:10px; padding:12px; min-width:0; border:1px solid var(--ui-border); border-radius:var(--app-surface-radius, 12px); background:var(--ui-surface-muted); }
.log-record-accent { flex:0 0 6px; width:6px; height:6px; margin-top:7px; border-radius:50%; background:var(--log-accent); }
.log-record-body { flex:1; min-width:0; margin:0; color:var(--ui-text); white-space:pre-wrap; overflow-wrap:anywhere; }
.log-record[data-level="WARNING"] { --log-accent:var(--ui-warning-text); }
.log-record[data-level="ERROR"], .log-record[data-level="CRITICAL"] { --log-accent:var(--ui-error-text); }
.log-record[data-level="INFO"] { --log-accent:var(--ui-primary-text); }
mark { background:var(--ui-primary-solid); color:var(--ui-on-primary); border-radius:2px; padding:0 1px; }
</style>
