import { reactive } from 'vue';
import axios from 'axios';

const queuePath = '/api/media_organize/queue';
const activeStates = ['queued', 'running'];
const states = [...activeStates, 'completed', 'failed', 'cancelled', 'interrupted'];

export function queueReceiptMatches(data, ids, mode) {
  return data?.mode === mode && Number.isInteger(data.queued_count) && data.queued_count >= 0
    && Number.isInteger(data.duplicate_count) && data.duplicate_count >= 0 && data.queued_count + data.duplicate_count === ids.length
    && Array.isArray(data.queue_ids) && data.queue_ids.length === ids.length && new Set(data.queue_ids).size === ids.length
    && Array.isArray(data.items) && data.items.length === ids.length && new Set(data.items.map(item => item?.item_id)).size === ids.length
    && data.items.every(item => item && typeof item.id === 'string' && item.id && ids.includes(item.item_id)
      && data.queue_ids.includes(item.id) && item.mode === mode && activeStates.includes(item.status));
}

export function useMediaOrganizeQueue({ client = axios, onSettled = () => {} } = {}) {
  const queue = reactive({ items: [], total: 0, page: 1, pageSize: 100, queuedCount: 0, runningCount: 0,
    loaded: false, loading: false, error: '', actionId: '', actionError: '', actionMessage: '' });
  let active = false, generation = 0, readSequence = 0, timer, request, requestQuery, controller;
  const queryKey = () => JSON.stringify([queue.page, queue.pageSize]);
  const invalidateRequest = () => {
    const staleController = controller;
    ++readSequence;
    controller = null;
    request = null;
    clearTimeout(timer);
    staleController?.abort();
  };
  const schedule = () => {
    clearTimeout(timer);
    if (active) timer = setTimeout(() => { void refresh(); }, 3000);
  };
  const refresh = () => {
    if (!active) return Promise.resolve();
    const targetQuery = queryKey();
    if (request && requestQuery === targetQuery) return request;
    if (request) invalidateRequest();
    const session = generation, sequence = ++readSequence, targetPage = queue.page, targetSize = queue.pageSize;
    const isCurrent = () => active && session === generation && sequence === readSequence && targetQuery === queryKey();
    const pendingController = new AbortController();
    controller = pendingController;
    clearTimeout(timer);
    queue.loading = true;
    const reading = (async () => {
      try {
        const { data } = await client.get(queuePath, { params: { page: targetPage, page_size: targetSize }, signal: pendingController.signal, timeout: 10000 });
        if (!isCurrent()) return;
        if (!Array.isArray(data?.items) || !Number.isInteger(data.total) || data.total < data.items.length
          || !Number.isInteger(data.queued_count) || data.queued_count < 0 || !Number.isInteger(data.running_count) || data.running_count < 0
          || data.items.some(item => typeof item.id !== 'string' || !item.id || typeof item.item_id !== 'string' || !item.item_id
            || !states.includes(item.status) || !['reprocess', 'rescrape'].includes(item.mode) || !item.item_json || typeof item.item_json !== 'object' || Array.isArray(item.item_json)
            || (item.status === 'running' && item.progress_known === true && (!Number.isFinite(item.progress) || item.progress < 0 || item.progress > 100)))
          || new Set(data.items.map(item => item.id)).size !== data.items.length) throw new Error('整理队列响应无效');
        if (targetPage > Math.max(1, Math.ceil(data.total / targetSize))) {
          queue.page = Math.max(1, Math.ceil(data.total / targetSize));
          return;
        }
        const previous = new Map(queue.items.map(item => [item.id, item.status]));
        const settled = queue.loaded && (data.queued_count + data.running_count < queue.queuedCount + queue.runningCount
          || data.items.some(item => activeStates.includes(previous.get(item.id)) && !activeStates.includes(item.status)));
        queue.items = data.items;
        queue.total = data.total;
        queue.queuedCount = data.queued_count;
        queue.runningCount = data.running_count;
        queue.loaded = true;
        queue.error = '';
        if (settled) await onSettled();
      } catch (error) {
        if (isCurrent() && !pendingController.signal.aborted) queue.error = error.response?.data?.error || error.message || '读取整理队列失败';
      } finally {
        if (isCurrent()) queue.loading = false;
      }
    })().finally(() => {
      if (request === reading) request = null;
      if (controller === pendingController) controller = null;
      if (active && session === generation && sequence === readSequence) {
        if (targetQuery !== queryKey()) void refresh();
        else schedule();
      }
    });
    request = reading;
    requestQuery = targetQuery;
    return reading;
  };
  const start = () => { if (!active) { active = true; ++generation; void refresh(); } };
  const stop = () => { active = false; ++generation; invalidateRequest(); queue.loading = false; queue.actionId = ''; queue.actionError = ''; queue.actionMessage = ''; };
  const mutate = async (item, operation) => {
    if (!active || queue.actionId || (operation === 'cancel' ? item.status !== 'queued' : !['failed', 'interrupted', 'cancelled'].includes(item.status))) return;
    const session = generation;
    queue.actionId = item.id;
    queue.actionError = '';
    queue.actionMessage = '';
    try {
      const url = queuePath + '/' + encodeURIComponent(item.id);
      const { data, status } = operation === 'cancel' ? await client.delete(url) : await client.post(url + '/retry');
      if (!active || session !== generation) return;
      const confirmed = operation === 'cancel'
        ? data?.item?.id === item.id && data.item.item_id === item.item_id && data.item.mode === item.mode && data.item.status === 'cancelled'
        : status === 202 && queueReceiptMatches(data, [item.item_id], item.mode);
      if (!confirmed) throw new Error('队列操作结果尚未确认，请重新检查');
      queue.actionMessage = data?.message || (operation === 'cancel' ? '等待任务已取消' : '已重新加入整理队列');
      invalidateRequest();
      if (!active || session !== generation) return;
      await refresh();
    } catch (error) {
      if (active && session === generation) queue.actionError = error.response?.data?.error || error.message || '队列操作结果尚未确认，请重新检查';
    } finally {
      if (active && session === generation) queue.actionId = '';
    }
  };
  return Object.assign(queue, { refresh, start, stop, cancel: item => mutate(item, 'cancel'), retry: item => mutate(item, 'retry') });
}
