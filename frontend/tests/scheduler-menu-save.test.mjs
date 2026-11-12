import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFileSync } from 'node:fs';
import { ref, computed, watch, nextTick } from 'vue';
import axios from 'axios';
import { invalidateConfigCache, useConfig } from '../src/composables/useConfig.js';

const source = readFileSync(new URL('../src/components/settings/SchedulerSettingsPage.vue', import.meta.url), 'utf8');
const script = source.match(/<script setup>([\s\S]*?)<\/script>/)[1].replace(/^import[^\n]+\n/gm, '');
const tasks = [{ key: 'backup-mediainfo', name: '备份媒体信息' }, { key: 'purge-ghost-actors', name: '清理演员' }];

async function fixture(options = {}) {
  invalidateConfigCache();
  const originalAdapter = axios.defaults.adapter, requests = [], messages = [], stops = [], unmount = [];
  axios.defaults.adapter = async config => {
    const payload = config.data ? JSON.parse(config.data) : null;
    requests.push({ method: config.method, url: config.url, payload });
    if ((config.method === 'post' && options.saveFails) || (config.url.endsWith('context=all') && options.catalogFails)) {
      throw new axios.AxiosError('fixture failure', 'ERR_BAD_RESPONSE', config, null, { status: 503, data: { error: '读取或保存失败' }, config });
    }
    const data = config.url === '/api/config'
      ? config.method === 'post' ? { saved_fields: payload } : { tg_menu_tasks: ['backup-mediainfo'], manual_task_order: [], other: 'keep' }
      : config.url.endsWith('context=manual') ? options.manualTasks || tasks : tasks;
    return { config, data, status: 200, statusText: 'OK', headers: {} };
  };
  const config = useConfig(); await config.fetchConfigData();
  const context = vm.createContext({ ref, computed, nextTick, axios, useConfig: () => config,
    watch: (...args) => { const stop = watch(...args); stops.push(stop); return stop; },
    onMounted: () => {}, onBeforeUnmount: fn => unmount.push(fn), useDisplay: () => ({ mdAndUp: ref(true) }), useContextSearch: () => {},
    useMessage: () => Object.fromEntries(['success', 'error', 'info', 'warning'].map(type => [type, message => messages.push({ type, message })])),
    useDialog: () => ({}), useTaskStatus: () => ({ isBackgroundTaskRunning: ref(false), backgroundTaskStatus: ref({}), refreshTaskStatus: async () => {} }),
    Sortable: { create: () => ({ destroy: () => {}, option: () => {} }) }, console,
  });
  vm.runInContext(script + `\nthis.api = { fetch: fetchAvailableTasks, save: saveTgMenuConfig, sequence: configuredTgMenuSequence, modal: showTgMenuConfigModal,
    current: enabledTgMenuTasks, order: manualTaskOrder, dirty: manualOrderDirty,
    open: typeof openTgMenuConfig === 'function' ? openTgMenuConfig : () => { showTgMenuConfigModal.value = true; } };`, context);
  return { api: context.api, config, options, requests, messages, unmount: () => unmount.forEach(fn => fn()), restore: () => { stops.forEach(stop => stop()); axios.defaults.adapter = originalAdapter; invalidateConfigCache(); } };
}

test('a rejected TG menu save preserves persisted settings and the open draft until retry succeeds', async () => {
  const f = await fixture({ saveFails: true });
  try {
    await f.api.fetch(); await nextTick(); f.api.open();
    f.api.sequence.value = tasks.map(task => ({ ...task, enabled: task.key === 'purge-ghost-actors' }));
    await f.api.save();
    assert.deepEqual(Array.from(f.config.configModel.value.tg_menu_tasks), ['backup-mediainfo']);
    assert.equal(f.api.modal.value, true);
    assert.deepEqual(Array.from(f.api.sequence.value.filter(task => task.enabled), task => task.key), ['purge-ghost-actors']);
    assert.equal(f.messages.some(message => message.type === 'success'), false);
    f.options.saveFails = false;
    await f.api.save();
    assert.deepEqual(Array.from(f.config.configModel.value.tg_menu_tasks), ['purge-ghost-actors']);
    assert.equal(f.api.modal.value, false);
    assert.equal(f.config.configModel.value.other, 'keep');
  } finally { f.restore(); }
});

test('a failed task catalog never saves an inferred empty TG menu', async () => {
  const f = await fixture({ catalogFails: true });
  try {
    await f.api.fetch(); await nextTick(); f.api.open();
    await f.api.save();
    assert.equal(f.requests.some(request => request.method === 'post'), false);
    assert.deepEqual(Array.from(f.config.configModel.value.tg_menu_tasks), ['backup-mediainfo']);
    f.options.catalogFails = false;
    await f.api.fetch(); await nextTick(); f.api.open();
    f.api.sequence.value = tasks.map(task => ({ ...task, enabled: task.key === 'purge-ghost-actors' }));
    await f.api.save();
    assert.equal(f.requests.filter(request => request.method === 'post').length, 1);
    assert.deepEqual(Array.from(f.config.configModel.value.tg_menu_tasks), ['purge-ghost-actors']);
  } finally { f.restore(); }
});

test('a successful catalog retry keeps dragged task order and appends legitimate new tasks', async () => {
  const f = await fixture();
  try {
    await f.api.fetch(); await nextTick();
    f.api.order.value = ['purge-ghost-actors', 'backup-mediainfo']; f.api.dirty.value = true;
    f.options.catalogFails = true;
    await f.api.fetch();
    f.options.catalogFails = false;
    f.options.manualTasks = [...tasks, { key: 'new-task', name: '新任务' }];
    await f.api.fetch(); await nextTick();
    assert.deepEqual(Array.from(f.api.order.value), ['purge-ghost-actors', 'backup-mediainfo', 'new-task']);
    assert.equal(f.api.dirty.value, true);
    assert.equal(f.requests.some(request => request.method === 'post'), false);
    assert.deepEqual(Array.from(f.config.configModel.value.manual_task_order), []);
  } finally { f.restore(); }
});
