<template>
  <article class="log-record" :data-level="records[0].level">
    <time v-if="records[0].timestamp" class="log-record-time">{{ records[0].timestamp.replace(/[,\.]\d+$/, '').replaceAll('-', '/') }}</time>
    <span v-else aria-hidden="true" />
    <div class="log-record-panel" :class="`is-${stripe}`">
      <span class="log-record-accent" :class="{ 'is-burst': records.length > 1 }" aria-hidden="true" />
      <div class="log-record-lines">
        <div v-for="(record, recordIndex) in records" :key="recordIndex" class="log-record-line">
          <span v-if="record.level !== 'PLAIN'" class="log-record-level">{{ record.level }}:</span>
          <p class="log-record-body"><span v-if="record.timestamp" class="log-record-inline-time">{{ record.timestamp.split(' ').at(-1) }}</span><template v-if="record.timestamp">{{ ' ' }}</template><span v-if="record.module" class="log-record-module">{{ record.module }}</span><template v-if="record.module">{{ ' - ' }}</template><span v-if="file" class="log-record-file">[{{ file }}] </span><span class="log-record-message"><template v-for="(part, index) in record.parts" :key="index"><mark v-if="part.matched">{{ part.text }}</mark><template v-else>{{ part.text }}</template></template></span></p>
        </div>
      </div>
    </div>
  </article>
</template>

<script setup>
import { computed } from 'vue';
import { parseLogRecord, highlightLogText } from '@/utils/logRecords';
const props = defineProps({ line: { type: String, default: '' }, lines: { type: Array, default: () => [] }, file: { type: String, default: '' }, keyword: { type: String, default: '' }, stripe: { type: String, default: 'even' } });
const records = computed(() => (props.lines.length ? props.lines : [props.line]).map(line => {
  const record = parseLogRecord(line);
  return { ...record, parts: highlightLogText(record.message, props.keyword) };
}));
</script>

<style scoped>
.log-record { --log-accent: var(--ui-text-muted); display:grid; grid-template-columns:11rem minmax(0,1fr); align-items:start; flex-shrink:0; min-width:0; color:var(--ui-text); font:0.8125rem/1.6 'JetBrains Mono', 'Fira Code', Consolas, monospace; }
.log-record-line { display:flex; align-items:flex-start; gap:12px; min-width:0; color:var(--ui-text); }
.log-record-time { padding-block:10px; font-weight:600; line-height:1.5; color:color-mix(in srgb,rgb(var(--v-theme-on-surface)) 80%,var(--ui-primary-text)); overflow-wrap:anywhere; }
.log-record-level { flex:0 0 4rem; min-width:4rem; font-weight:700; color:color-mix(in srgb,rgb(var(--v-theme-on-surface)) 80%,var(--log-accent)); }
.log-record-file, .log-record-module { min-width:0; max-width:100%; overflow-wrap:anywhere; }
.log-record-inline-time { color:inherit; }
.log-record-panel { display:flex; align-items:stretch; gap:12px; padding:8px; min-width:0; border:1px solid rgba(var(--v-theme-on-surface),0.04); border-radius:var(--app-surface-radius, 12px); background:var(--logging-record-bg-even,var(--ui-surface-muted)); }
.is-dark-theme .log-record-panel { border-color:rgba(var(--v-theme-on-surface),0.08); }
.log-record-panel.is-odd { background:var(--logging-record-bg-odd,var(--ui-surface-muted)); }
.log-record-accent { flex:0 0 8px; width:8px; height:8px; align-self:flex-start; margin-top:0.45rem; border-radius:999px; background:var(--log-accent); }
.log-record-accent.is-burst { height:auto; align-self:stretch; margin-top:0; }
.log-record-lines { display:flex; flex:1 1 auto; flex-direction:column; gap:2px; min-width:0; }
.log-record-body { flex:1 1 auto; min-width:0; margin:0; color:var(--logging-text,var(--ui-text)); white-space:pre-wrap; overflow-wrap:anywhere; }
.log-record[data-level="WARNING"] { --log-accent:var(--ui-warning-text); }
.log-record[data-level="ERROR"], .log-record[data-level="CRITICAL"] { --log-accent:var(--ui-error-text); }
.log-record[data-level="INFO"] { --log-accent:var(--ui-success-text); }
.log-record[data-level="TRACE"], .log-record[data-level="DEBUG"] { --log-accent:rgb(var(--v-theme-secondary)); }
mark { background:var(--ui-primary-solid); color:var(--ui-on-primary); border-radius:2px; padding:0 1px; }
@media (max-width:960px) { .log-record { grid-template-columns:9.5rem minmax(0,1fr); gap:6px; } .log-record-level { flex-basis:4.75rem; min-width:4.75rem; } }
@media (max-width:640px) { .log-record { grid-template-columns:minmax(0,1fr); gap:4px; } .log-record-time { padding-block:0; } .log-record > span[aria-hidden] { display:none; } .log-record-panel { padding-inline:10px; } .log-record-line { gap:8px; } }
</style>
