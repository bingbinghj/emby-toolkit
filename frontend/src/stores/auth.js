// src/stores/auth.js
import { defineStore } from 'pinia';
import { ref, computed } from 'vue';
import axios from 'axios';

export const useAuthStore = defineStore('auth', () => {
  const isLoggedIn = ref(false);
  const user = ref({});
  const accountInfo = ref(null);
  let accountRequest = null;
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
    if (accountRequest?.user === currentUser) return accountRequest.promise;
    const request = { user: currentUser };
    request.promise = axios.get('/api/portal/account-info').then(({ data }) => {
      if (isLoggedIn.value && user.value === currentUser) accountInfo.value = data;
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

  async function checkAuthStatus() {
    try {
      const response = await axios.get('/api/auth/check_status');
      const status = response.data.status;
      systemStatus.value = status;

      if (status === 'logged_in') {
        if (user.value?.id !== response.data.user?.id) accountInfo.value = null;
        isLoggedIn.value = true;
        user.value = response.data.user || {};
      } else {
        isLoggedIn.value = false;
        user.value = {};
        accountInfo.value = null;
        // 这里可以抛出特定错误供路由守卫捕获，或者直接返回状态
        if (status === 'setup_required') {
           throw new Error('SETUP_REQUIRED');
        }
      }
      return status;
    } catch (error) {
      isLoggedIn.value = false;
      accountInfo.value = null;
      throw error;
    }
  }

  async function login(credentials) {
    const response = await axios.post('/api/auth/login', credentials);
    accountInfo.value = null;
    isLoggedIn.value = true;
    // 确保把后端返回的 user 对象完整存进去
    user.value = response.data.user || {};
  }

  async function logout() {
    await axios.post('/api/auth/logout');
    isLoggedIn.value = false;
    user.value = {};
    accountInfo.value = null;
    systemStatus.value = 'login_required';
  }

  return {
    isLoggedIn,
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
