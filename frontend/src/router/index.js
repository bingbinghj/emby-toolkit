// src/router/index.js

import { createRouter, createWebHistory } from 'vue-router';
import { useAuthStore } from '../stores/auth';

// --- 1. 导入所有页面组件 ---
import ReviewList from '../components/ReviewList.vue';
import SchedulerSettingsPage from '../components/settings/SchedulerSettingsPage.vue';
import GeneralSettingsPage from '../components/settings/GeneralSettingsPage.vue';
import Login from '../components/Login.vue';
import CoverGeneratorConfig from '../components/CoverGeneratorConfig.vue';
import UserCenterPage from '../components/UserCenterPage.vue'
import SetupPage from '../components/Setup.vue';
import MediaCenterPage from '../components/MediaCenterPage.vue';
import DashboardPage from '../components/DashboardPage.vue';
import MediaOrganizePage from '../components/MediaOrganizePage.vue';
import NotificationSettingsPage from '../components/settings/NotificationSettingsPage.vue';

// --- 2. 定义路由规则 (带 meta.public 标签) ---
const routes = [
  {
    path: '/login',
    name: 'Login',
    component: Login,
    meta: {
      requiresAuth: false,
      public: true // <-- ★★★ 在这里打上“公共页面”标签 ★★★
    },
  },
  {
    path: '/setup',
    name: 'Setup',
    component: SetupPage,
    meta: {
      requiresAuth: false,
      public: true // 必须是公开的，否则死循环
    },
  },
  {
    path: '/',
    redirect: '/dashboard'
  },
  {
    path: '/review',
    name: 'ReviewList',
    component: ReviewList,
    meta: { requiresAuth: true },
  },
  {
    path: '/dashboard',
    name: 'Dashboard',
    component: DashboardPage,
    meta: { requiresAuth: true },
  },
  {
    path: '/logs',
    name: 'Logs',
    component: () => import('../components/LogPage.vue'),
    meta: { requiresAuth: true, standalone: true },
  },
  {
    path: '/media-organize',
    name: 'MediaOrganize',
    component: MediaOrganizePage,
    meta: { requiresAuth: true },
  },
  {
    path: '/media-center',
    name: 'MediaCenter',
    redirect: '/dashboard',
  },
  {
    path: '/library/:libraryId',
    name: 'MediaCenterLibrary',
    component: MediaCenterPage,
    props: true,
    meta: { requiresAuth: true },
  },
  {
    path: '/media-center/library/:libraryId',
    redirect: to => ({ name: 'MediaCenterLibrary', params: to.params, query: to.query }),
  },
  {
    path: '/settings/notifications',
    name: 'settings-notifications',
    component: NotificationSettingsPage,
    meta: { requiresAuth: true },
  },
  {
    path: '/settings/scheduler',
    name: 'settings-scheduler',
    component: SchedulerSettingsPage,
    meta: { requiresAuth: true },
  },
  {
    path: '/settings/general',
    alias: '/setting',
    name: 'settings-general',
    component: GeneralSettingsPage,
    meta: { requiresAuth: true },
  },
  {
    path: '/edit-media/:itemId',
    name: 'MediaEditPage',
    component: () => import('../components/MediaEditPage.vue'),
    props: true,
    meta: { requiresAuth: true },
  },
  {
    path: '/settings/cover-generator',
    name: 'CoverGeneratorConfig',
    component: CoverGeneratorConfig,
    meta: { requiresAuth: true },
  },
  {
    path: '/user-management',
    name: 'UserManagement',
    component: UserCenterPage,
    meta: {
      requiresAuth: true // 这个页面必须登录才能访问
    },
  },
  {
    path: '/user-center',
    name: 'UserCenter',
    redirect: '/user-management',
  },
];

function createAppHistory() {
  const originalAddEventListener = document.addEventListener;
  document.addEventListener = function (type, listener, options) {
    if (type === 'visibilitychange') return;
    return originalAddEventListener.call(this, type, listener, options);
  };

  try {
    return createWebHistory(import.meta.env.BASE_URL);
  } finally {
    document.addEventListener = originalAddEventListener;
  }
}

const router = createRouter({
  history: createAppHistory(),
  routes,
  scrollBehavior: () => ({ top: 0 })
});

// --- 3. 路由守卫 (核心修改) ---
router.beforeEach(async (to, from, next) => {
  // MoviePilot setting links carry the category in query.tab.
  if (to.name === 'settings-general' && to.query.tab === 'notification') {
    next({ name: 'settings-notifications', query: { ...to.query, tab: undefined }, replace: true });
    return;
  }
  const authStore = useAuthStore();

  // 1. 如果是公共页面 (Login, Setup)，直接放行
  if (to.meta.public) {
    next();
    return;
  }

  // 2. 如果前端状态已知已登录，直接放行
  if (authStore.isLoggedIn) {
    next();
    return;
  }

  // 3. 前端状态未登录，向后端检查状态
  try {
    await authStore.checkAuthStatus();

    if (authStore.isLoggedIn) {
      next();
    } else {
      // 没登录，去登录页
      next({ name: 'Login' });
    }
  } catch (error) {
    // ★★★ 核心逻辑：捕获 SETUP_REQUIRED 错误 ★★★
    // 这个错误是由 auth.js 中的 checkAuthStatus 抛出的
    if (error.message === 'SETUP_REQUIRED') {
      // 如果目标已经是 Setup 页面，就不要再跳了，防止死循环（虽然上面 meta.public 已经挡了一层，但双重保险）
      if (to.name === 'Setup') {
        next();
      } else {
        next({ name: 'Setup' });
      }
    } else {
      // 其他错误（如网络错误、未登录），去登录页
      next({ name: 'Login' });
    }
  }
});

export default router;
