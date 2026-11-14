import { ref, computed, watch, onUnmounted } from 'vue';
import axios from 'axios';
import { useAuthStore } from '../stores/auth.js';

const backgroundTaskStatus = ref({ is_running: false, current_action: '无', progress: 0, message: '等待任务', state: 'idle', history: [] });
const statusReadError = ref('');
const statusLastUpdatedAt = ref(null);
let timer;
let subscribers = 0;
let currentRequest = null;
let requestGeneration = 0;
let restarting = false;
let disconnected = false;
let subscribedAuth = null;
export function useTaskStatusReadState() {
  return { error: statusReadError, lastUpdatedAt: statusLastUpdatedAt };
}
export function refreshTaskStatus(afterControl = false) {
  if (afterControl && currentRequest) {
    requestGeneration++;
    currentRequest.controller.abort();
    currentRequest = null;
  }
  if (currentRequest) return currentRequest.promise;
  const request = { generation: requestGeneration, controller: new AbortController(), auth: subscribedAuth, user: subscribedAuth?.user };
  request.promise = axios.get('/api/status', { timeout: 10000, signal: request.controller.signal, headers: { 'Cache-Control': 'no-cache' } }).then(({ data }) => {
    if (request.generation !== requestGeneration) return;
    if (restarting && disconnected) { window.location.reload(); return; }
    backgroundTaskStatus.value = data;
    statusReadError.value = '';
    statusLastUpdatedAt.value = Date.now();
    restarting ||= String(data.message || '').includes('系统正在重启');
  }).catch(async error => {
    if (request.generation !== requestGeneration) return;
    const ownsRead = request.auth === subscribedAuth && request.user === subscribedAuth?.user;
    if (ownsRead && !request.controller.signal.aborted && ![401, 403].includes(error.response?.status)) {
      statusReadError.value = error.response?.data?.error || error.message || '任务状态读取失败，请重试';
    }
    if (restarting) disconnected = true;
    if (error.response?.status === 403 && subscribers > 0 && request.auth === subscribedAuth
      && request.auth?.isLoggedIn && request.auth.isAdmin && request.user === request.auth.user) {
      await request.auth.checkAuthStatus({ expectedUser: request.user });
    }
  }).finally(() => { if (currentRequest === request) currentRequest = null; });
  currentRequest = request;
  return request.promise;
}
function stop() { clearInterval(timer); timer = undefined; }
export function useTaskStatus() {
  subscribers++;
  const auth = useAuthStore();
  subscribedAuth = auth;
  watch(() => auth.isLoggedIn && auth.isAdmin, allowed => {
    if (allowed && !timer) { refreshTaskStatus(); timer = setInterval(refreshTaskStatus, 2000); }
    if (!allowed) {
      stop();
      requestGeneration++;
      currentRequest?.controller.abort();
      currentRequest = null;
      backgroundTaskStatus.value = { is_running: false, state: 'idle', history: [] };
      statusReadError.value = '';
      statusLastUpdatedAt.value = null;
      restarting = disconnected = false;
    }
  }, { immediate: true });
  onUnmounted(() => {
    subscribers--;
    if (!subscribers) {
      stop(); requestGeneration++; currentRequest?.controller.abort(); currentRequest = null;
      statusReadError.value = '';
      statusLastUpdatedAt.value = null;
      subscribedAuth = null;
    }
  });
  return { backgroundTaskStatus, isBackgroundTaskRunning: computed(() => backgroundTaskStatus.value.is_running), refreshTaskStatus };
}
