// src/stores/auth.js
import { defineStore } from 'pinia';
import { ref, computed } from 'vue';
import axios from 'axios';
import { invalidateConfigCache } from '../composables/useConfig.js';

export const useAuthStore = defineStore('auth', () => {
  const isLoggedIn = ref(false);
  const remember = ref(false);
  const user = ref({});
  const accountInfo = ref(null);
  let accountRequest = null;
  let authRevision = 0, statusRequest = 0;
  let authController = null;
  const systemStatus = ref('unknown');

  const avatarUrl = computed(() => {
    const id = user.value?.id;
    const tag = accountInfo.value?.profile_image_tag;
    return id && tag
      ? `/image_proxy/Users/${encodeURIComponent(id)}/Images/Primary?tag=${encodeURIComponent(tag)}`
      : null;
  });

  function fetchAccountInfo() {
    const currentUser = user.value;
    if (!isLoggedIn.value || !currentUser?.id) return Promise.resolve(null);
    if (accountRequest?.user === currentUser && accountRequest.revision === authRevision) return accountRequest.promise;
    const request = { user: currentUser, revision: authRevision };
    request.promise = axios.get('/api/portal/account-info').then(({ data }) => {
      if (!isLoggedIn.value || user.value !== currentUser || request.revision !== authRevision) return null;
      if (data?.id !== currentUser.id) throw new Error('当前用户资料尚未确认，请刷新后重试。');
      accountInfo.value = data;
      return data;
    }).finally(() => {
      if (accountRequest === request) accountRequest = null;
    });
    accountRequest = request;
    return request.promise;
  }

  const username = computed(() => {
    return user.value?.name || user.value?.username || '未登录';
  });

  const isAdmin = computed(() => user.value?.is_admin || false);

  // 为了兼容旧代码，加一个 userType
  const userType = computed(() => user.value?.user_type || 'emby_user');

  async function checkAuthStatus(options = {}) {
    const ownedProbe = Object.hasOwn(options, 'expectedUser');
    const expectedUser = options.expectedUser;
    if (ownedProbe && (authController || user.value !== expectedUser)) return systemStatus.value;
    const revision = authRevision, request = ++statusRequest;
    const current = () => revision === authRevision && request === statusRequest
      && (!ownedProbe || (!authController && user.value === expectedUser));
    try {
      const response = await axios.get('/api/auth/check_status', ownedProbe ? { timeout: 10000 } : undefined);
      if (!current()) return systemStatus.value;
      const status = response.data.status;
      if (ownedProbe && status !== 'login_required') return status;
      systemStatus.value = status;

      if (status === 'logged_in') {
        if (!isLoggedIn.value || user.value?.id !== response.data.user?.id) {
          accountInfo.value = null;
          invalidateConfigCache();
        }
        isLoggedIn.value = true;
        remember.value = response.data.remember ?? true;
        user.value = response.data.user || {};
      } else {
        isLoggedIn.value = false;
        remember.value = false;
        user.value = {};
        accountInfo.value = null;
        invalidateConfigCache();
        // 这里可以抛出特定错误供路由守卫捕获，或者直接返回状态
        if (status === 'setup_required') {
           throw new Error('SETUP_REQUIRED');
        }
      }
      return status;
    } catch (error) {
      if (!current()) return systemStatus.value;
      if (ownedProbe) return systemStatus.value;
      isLoggedIn.value = false;
      remember.value = false;
      user.value = {};
      accountInfo.value = null;
      invalidateConfigCache();
      throw error;
    }
  }

  async function login(credentials) {
    const revision = ++authRevision;
    authController?.abort();
    const controller = new AbortController();
    authController = controller;
    try {
      const response = await axios.post('/api/auth/login', credentials, { signal: controller.signal });
      if (revision !== authRevision) return false;
      invalidateConfigCache();
      accountInfo.value = null;
      isLoggedIn.value = true;
      remember.value = response.data.remember ?? credentials.remember ?? true;
      user.value = response.data.user || {};
      systemStatus.value = 'logged_in';
      return true;
    } catch (error) {
      if (revision !== authRevision) return false;
      if (error.response?.status === 403) {
        invalidateConfigCache();
        isLoggedIn.value = false;
        remember.value = false;
        user.value = {};
        accountInfo.value = null;
        systemStatus.value = 'login_required';
      }
      throw error;
    } finally {
      if (authController === controller) authController = null;
      // Also invalidate status reads that began while login was in flight.
      if (revision === authRevision) ++authRevision;
    }
  }

  async function logout() {
    const revision = ++authRevision;
    authController?.abort();
    const controller = new AbortController();
    authController = controller;
    try {
      await axios.post('/api/auth/logout', undefined, { signal: controller.signal });
      if (revision !== authRevision) return false;
      invalidateConfigCache();
      isLoggedIn.value = false;
      remember.value = false;
      user.value = {};
      accountInfo.value = null;
      systemStatus.value = 'login_required';
      return true;
    } catch (error) {
      if (revision !== authRevision) return false;
      throw error;
    } finally {
      if (authController === controller) authController = null;
      if (revision === authRevision) ++authRevision;
    }
  }

  return {
    isLoggedIn,
    remember,
    user,
    accountInfo,
    avatarUrl,
    fetchAccountInfo,
    username, // 导出修复后的计算属性
    isAdmin,
    userType,
    systemStatus,
    checkAuthStatus,
    login,
    logout
  };
});
