// src/composables/useConfig.js (最终全自动修复版)

import { ref } from 'vue';
import axios from 'axios';

// 同一账号的设置页面共享自动加载结果，显式刷新仍重新读取。
let hasFetched = false;
let generation = 0;
let readVersion = 0;
let pendingFetch = null;

// 2. 将所有状态变量移到外部，创建真正的单例
const configModel = ref(null); // ★★★ 初始值设为 null
const loadingConfig = ref(false); // 初始加载状态为 false
const savingConfig = ref(false);
const configError = ref(null);

export function invalidateConfigCache() {
  generation++;
  readVersion++;
  hasFetched = false;
  pendingFetch = null;
  configModel.value = null;
  loadingConfig.value = false;
  savingConfig.value = false;
  configError.value = null;
}

export function useConfig() {
  const fetchConfigData = (options = {}) => {
    const requestGeneration = options.generation ?? generation;
    if (requestGeneration !== generation) return Promise.resolve(false);
    if (pendingFetch && !options.force) return pendingFetch;
    const version = ++readVersion;
    const isCurrent = () => requestGeneration === generation && version === readVersion;
    loadingConfig.value = true;
    configError.value = null;
    const fetching = (async () => {
      try {
        const response = await axios.get('/api/config');
        if (!isCurrent()) return false;
        if (!response.data || typeof response.data !== 'object' || Array.isArray(response.data)) {
          throw new Error('配置响应无效，请重试。');
        }
        const next = { ...response.data };
        const current = configModel.value;
        if (options.baseline && current) {
          for (const field of new Set([...Object.keys(options.baseline), ...Object.keys(current)])) {
            if (JSON.stringify(current[field]) !== JSON.stringify(options.baseline[field])
              || Object.hasOwn(current, field) !== Object.hasOwn(options.baseline, field)) {
              if (Object.hasOwn(current, field)) next[field] = current[field];
              else delete next[field];
            }
          }
        }
        configModel.value = next;
        hasFetched = true; // ★★★ 请求成功后，将标志设为 true
        return true;
      } catch (err) {
        if (!isCurrent()) return false;
        console.error('useConfig: 获取配置失败:', err);
        configError.value = err.response?.data?.error || '无法连接到后端或解析配置。';
        hasFetched = configModel.value !== null; // 保留已读草稿，未读成功时允许自动重试。
        return false;
      } finally {
        if (isCurrent()) {
          loadingConfig.value = false;
          pendingFetch = null;
        }
      }
    })();
    if (isCurrent() && loadingConfig.value) pendingFetch = fetching;
    return fetching;
  };

  const captureConfigRefresh = () => {
    const owner = generation;
    const baseline = configModel.value ? JSON.parse(JSON.stringify(configModel.value)) : null;
    return {
      isCurrent: () => owner === generation,
      refresh: () => fetchConfigData({ generation: owner, baseline, force: true }),
    };
  };

  const handleSaveConfig = async (configToSave) => {
    if (!configModel.value || savingConfig.value) return false;
    const requestGeneration = generation;
    // 检查是否有配置可以保存。优先使用传入的 configToSave，
    // 如果没有传入，则回退到使用全局的 configModel.value。
    const source = configToSave || configModel.value;
    const payload = source ? JSON.parse(JSON.stringify(source)) : null;
    if (!payload) {
      console.error('没有可保存的配置。');
      configError.value = '配置数据为空，无法保存。';
      return false;
    }
    const originalFields = JSON.parse(JSON.stringify(Object.fromEntries(
      Object.keys(payload).map(field => [field, configModel.value?.[field]])
    )));

    savingConfig.value = true;
    configError.value = null;
    try {
      // 使用最终确定的 payload 发送请求
      const response = await axios.post('/api/config', payload);
      if (requestGeneration !== generation || !configModel.value) return false;

      // 保存成功后，用我们刚刚成功保存的数据来更新全局的 configModel
      // 这样可以确保单例状态与后端保持同步
      const savedFields = response.data?.saved_fields || payload;
      for (const [field, savedValue] of Object.entries(savedFields)) {
        // 保存期间继续编辑的草稿不应被旧请求结果覆盖。
        if (Object.hasOwn(payload, field) && JSON.stringify(configModel.value[field]) === JSON.stringify(originalFields[field])) {
          configModel.value[field] = savedValue;
        }
      }

      return true;
    } catch (err) {
      if (requestGeneration !== generation) return false;
      console.error('useConfig: 保存配置失败:', err);
      configError.value = err.response?.data?.error || err.message;
      return false;
    } finally {
      if (requestGeneration === generation) savingConfig.value = false;
    }
  };

  // ★★★ 核心自动加载逻辑 ★★★
  // 如果从未获取过数据，并且当前没有在加载中，就立即触发获取
  if (!hasFetched && !loadingConfig.value) {
    fetchConfigData();
  }

  return {
    configModel,
    loadingConfig,
    savingConfig,
    configError,
    fetchConfigData,
    captureConfigRefresh,
    handleSaveConfig,
  };
}
