<script setup>
import { ref, computed, watch, nextTick, onMounted, onBeforeUnmount, unref } from 'vue';
import { useRouter, useRoute } from 'vue-router';
import { useDisplay } from 'vuetify';
import axios from 'axios';
import { useAuthStore } from '@/stores/auth';
import { refreshTaskStatus } from '@/composables/useTaskStatus';
import { useMessage, useDialog } from '@/composables/useFeedback';
import { useThemeCustomizer, THEME_CUSTOMIZER_OPEN_EVENT } from '@/composables/useThemeCustomizer';
import { activeContextSearch } from '@/composables/useContextSearch';
import { mergeNavigationConfig, moveNavigationItem } from '@/utils/navigationConfig';
import DefaultLayout from '@/layouts/components/DefaultLayout.vue';
import NotificationCenter from '@/components/NotificationCenter.vue';
import ThemeCustomizer from '@/components/theme/ThemeCustomizer.vue';
import WallpaperSettings from '@/components/WallpaperSettings.vue';

const props = defineProps({ taskStatus: Object });
const router = useRouter(), route = useRoute(), authStore = useAuthStore();
const message = useMessage(), dialog = useDialog();
const appearance = useThemeCustomizer();
const { mdAndDown, mdAndUp } = useDisplay();
const isMobile = computed(() => mdAndDown.value);
const notificationOpen = ref(false), themeCustomizerOpen = ref(false), avatarLoadFailed = ref(false);
const searchOpen = ref(false), desktopSearchInput = ref(null), mobileSearchInput = ref(null);
const fallbackSearch = ref(''), searchComposing = ref(false);
let searchRequestSequence = 0, searchWindowSession = 0;
watch(searchOpen, () => { ++searchWindowSession; }, { flush: 'sync' });
const searchAvailable = computed(() => authStore.isLoggedIn && (Boolean(activeContextSearch.value) || authStore.isAdmin));
const searchShortcutLabel = /Mac|iPhone|iPad/.test(navigator.platform) ? '⌘ K' : 'Ctrl K';
const searchLabel = computed(() => unref(activeContextSearch.value?.label) || '搜索全部媒体');
const searchDisabled = computed(() => Boolean(unref(activeContextSearch.value?.disabled)));
const searchLoading = computed(() => Boolean(unref(activeContextSearch.value?.loading)));
const searchWord = computed({
  get: () => String(activeContextSearch.value?.query.value ?? fallbackSearch.value),
  set: updateSearch,
});
function updateSearch(value) {
  if (searchDisabled.value) return;
  const text = String(value ?? '');
  const context = activeContextSearch.value;
  if (context) {
    context.query.value = text;
    if (!searchComposing.value) context.onInput?.(text);
  } else fallbackSearch.value = text;
}
async function submitSearch() {
  if (searchDisabled.value || searchComposing.value) return;
  const context = activeContextSearch.value;
  const sequence = ++searchRequestSequence, session = searchWindowSession, requestedUser = authStore.user;
  const query = searchWord.value.trim();
  const isCurrent = () => sequence === searchRequestSequence && session === searchWindowSession
    && context === activeContextSearch.value && query === searchWord.value.trim()
    && authStore.isLoggedIn && authStore.user === requestedUser;
  try {
    if (context) await context.submit?.();
    else await router.push({ name: 'ReviewList', query: searchWord.value.trim() ? { q: searchWord.value.trim() } : {} });
    if (isCurrent()) searchOpen.value = false;
  } catch (error) { if (isCurrent()) message.error(error.response?.data?.error || error.message || '搜索失败'); }
}
async function clearSearch() {
  if (searchDisabled.value) return;
  const sequence = ++searchRequestSequence, session = searchWindowSession, requestedUser = authStore.user;
  updateSearch('');
  const context = activeContextSearch.value;
  const query = searchWord.value.trim();
  const isCurrent = () => sequence === searchRequestSequence && session === searchWindowSession
    && context === activeContextSearch.value && query === searchWord.value.trim()
    && authStore.isLoggedIn && authStore.user === requestedUser;
  try { if (context) await (context.clear || context.submit)?.(); }
  catch (error) { if (isCurrent()) message.error(error.response?.data?.error || error.message || '清空搜索失败'); }
}
function startSearchComposition() {
  searchComposing.value = true;
  if (activeContextSearch.value?.composing) activeContextSearch.value.composing.value = true;
}
function finishSearchComposition() {
  searchComposing.value = false;
  const context = activeContextSearch.value;
  if (context?.composing) context.composing.value = false;
  if (context?.compositionEnd) context.compositionEnd();
  else context?.onInput?.(searchWord.value);
}
async function openSearch() {
  if (!searchAvailable.value) return;
  searchOpen.value = isMobile.value || !activeContextSearch.value;
  await nextTick();
  (isMobile.value ? mobileSearchInput.value : desktopSearchInput.value)?.focus();
}
function searchShortcut(event) {
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k' && searchAvailable.value) {
    event.preventDefault();
    openSearch();
  }
}
watch([() => route.path, activeContextSearch, isMobile], () => {
  searchOpen.value = false;
  searchComposing.value = false;
  fallbackSearch.value = '';
});
const taskControlBusy = ref(false);
const settingsPage = computed(() => ['settings-general', 'settings-notifications'].includes(route.name));
const contentKey = computed(() => settingsPage.value ? route.name : route.path);
const activeMenuKey = computed(() => route.name === 'MediaCenterLibrary' ? 'Dashboard'
  : settingsPage.value ? 'settings-general' : route.name);
const settingsTabs = [
  { value: 'system', title: '系统', icon: 'mdi-server-network' },
  { value: 'directory', title: '存储 & 目录', icon: 'mdi-folder' },
  { value: 'rule', title: '规则', icon: 'mdi-filter' },
  { value: 'search', title: '数据源', icon: 'mdi-database-search-outline' },
  { value: 'notification', title: '通知', icon: 'mdi-bell' },
];
const activeSettingsTab = computed(() => route.name === 'settings-notifications' ? 'notification'
  : settingsTabs.some(tab => tab.value === route.query.tab) ? route.query.tab : 'system');
function navigateSettings(value) {
  router.push(value === 'notification' ? { name: 'settings-notifications' }
    : { name: 'settings-general', query: value === 'system' ? {} : { tab: value } });
}
const taskPaused = computed(() => props.taskStatus?.state === 'paused');
const taskStopping = computed(() => props.taskStatus?.stop_requested || props.taskStatus?.state === 'stopping');
const userOptions = computed(() => [
  { key: 'user-management', label: '用户管理', icon: 'mdi-account-group-outline' },
  ...(authStore.isAdmin ? [
    { key: 'edit-menu', label: '自定义导航', icon: 'mdi-menu-open' },
    { key: 'restart-container', label: '重启容器', icon: 'mdi-restart' },
  ] : []),
  { key: 'logout', label: '退出登录', icon: 'mdi-logout' },
]);
watch(() => authStore.avatarUrl, () => { avatarLoadFailed.value = false; });
watch(() => authStore.isLoggedIn ? authStore.user?.id : null, async id => {
  if (id) {
    try { await authStore.fetchAccountInfo(); } catch { /* Account page exposes account failures. */ }
  }
}, { immediate: true });
function openThemeCustomizer() { themeCustomizerOpen.value = true; }
function handleMenuUpdate(key) { router.push({ name: key }); }
async function setLayout(layout) {
  try { await appearance.setLayout(layout); }
  catch (error) { message.error(error.message || '保存导航布局失败'); }
}
async function controlTask(action) {
  const taskId = props.taskStatus?.task_id;
  if (!taskId || taskControlBusy.value || taskStopping.value) return;
  taskControlBusy.value = true;
  try {
    const endpoint = action === 'stop' ? '/api/trigger_stop_task' : `/api/tasks/${action}`;
    const { data } = await axios.post(endpoint, { task_id: taskId });
    message.info(data.message || '任务控制请求已发送');
    await refreshTaskStatus(true);
  } catch (error) { message.error(error.response?.data?.error || error.message || '任务控制失败'); }
  finally { taskControlBusy.value = false; }
}
function confirmStopTask() {
  const taskId = props.taskStatus?.task_id;
  const taskName = props.taskStatus?.current_action || '当前任务';
  if (!taskId || !props.taskStatus?.is_running) return;
  dialog.warning({ title: '停止当前任务', content: `确定停止“${taskName}”吗？任务将在安全停止点结束，并保留恢复和清理结果。`,
    positiveText: '停止任务', negativeText: '取消', onPositiveClick: () => {
      if (props.taskStatus?.task_id !== taskId || !props.taskStatus?.is_running) {
        message.warning('当前任务已变化，未执行停止操作。');
        return;
      }
      return controlTask('stop');
    } });
}
function confirmResetMenu() {
  dialog.warning({ title: '恢复默认导航', content: '确定恢复菜单名称、顺序及显示状态吗？',
    positiveText: '恢复默认', negativeText: '取消', onPositiveClick: resetMenuConfig });
}
function moveMenuItem(groupIndex, childIndex, delta) {
  const children = menuConfigTree.value[groupIndex].children;
  const target = childIndex + delta;
  if (target < 0 || target >= children.length) return;
  children.splice(target, 0, children.splice(childIndex, 1)[0]);
}
function moveRootMenu(index, delta) {
  const target = index + delta;
  if (target < 0 || target >= menuConfigTree.value.length) return;
  menuConfigTree.value.splice(target, 0, menuConfigTree.value.splice(index, 1)[0]);
}
function promoteMenuItem(groupKey, key) {
  moveNavigationItem(menuConfigTree.value, key, null, groupKey);
}
function moveMenuToGroup(key, groupKey) {
  moveNavigationItem(menuConfigTree.value, key, groupKey);
}
onMounted(() => { initMenuConfig(); window.addEventListener(THEME_CUSTOMIZER_OPEN_EVENT, openThemeCustomizer); window.addEventListener('keydown', searchShortcut); });
onBeforeUnmount(() => { ++searchRequestSequence; window.removeEventListener(THEME_CUSTOMIZER_OPEN_EVENT, openThemeCustomizer); window.removeEventListener('keydown', searchShortcut); });

const triggerRestart = async () => {
  message.info('正在发送重启指令...');
  try {
    await axios.post('/api/system/restart');
    message.success('重启指令已发送，应用正在后台重启。请稍后手动刷新页面。', { duration: 10000 });
  } catch (error) {
    if (error.response) {
      message.error(error.response.data.error || '发送重启请求失败，请查看日志。');
    } else {
      message.warning('连接已中断，重启结果尚未确认。请稍后刷新页面检查。', { duration: 10000 });
    }
  }
};

const handleUserSelect = async (key) => {
  if (key === 'user-management') {
    router.push({ name: 'UserManagement' });
  } else if (key === 'edit-menu') {
    isMenuEditorVisible.value = true;
  } else if (key === 'restart-container') {
    dialog.warning({
      title: '确认重启容器',
      content: '确定要重启容器吗？应用将在短时间内无法访问，重启后需要手动刷新页面。',
      positiveText: '确定重启',
      negativeText: '取消',
      onPositiveClick: triggerRestart,
    });
  } else if (key === 'logout') {
    if (await authStore.logout()) router.push({ name: 'Login' });
  }
};

// ================= 自定义菜单逻辑 (树形结构 + 拖拽) =================
const isMenuEditorVisible = ref(false);
let menuEditorSession = 0;
const isSavingMenu = ref(false);
const isMenuReady = ref(false);
const isMenuLoading = ref(false);
const menuConfigLoaded = ref(false);
const menuConfigError = ref('');
const menuConfigTree = ref([]); // 树形配置数据

// 1. 基础菜单（包含所有权限允许的菜单，带有 defaultLabel 用于重置）
const baseMenuOptions = computed(() => {
  const discoveryGroup = {
    label: '发现', defaultLabel: '发现', key: 'group-discovery', icon: 'mdi-compass-outline', children: []
  };

  if (authStore.isLoggedIn) {
    discoveryGroup.children.push({ label: '仪表盘', defaultLabel: '仪表盘', key: 'Dashboard', icon: 'mdi-view-dashboard-outline' });
  }

  const finalMenu = [discoveryGroup];

  if (authStore.isAdmin) {
    finalMenu.push(
      {
        label: '整理', defaultLabel: '整理', key: 'group-management', icon: 'mdi-folder-play-outline',
        children: [
          { label: '媒体整理', defaultLabel: '媒体整理', key: 'MediaOrganize', icon: 'mdi-folder-move-outline' },
          { label: '手动处理', defaultLabel: '手动处理', key: 'ReviewList', icon: 'mdi-playlist-edit' },
        ]
      },
      {
        label: '系统', defaultLabel: '系统', key: 'group-system', icon: 'mdi-cog-outline',
        children: [
          { label: '设置', defaultLabel: '设置', key: 'settings-general', icon: 'mdi-cog-outline' },
          { label: '任务中心', defaultLabel: '任务中心', key: 'settings-scheduler', icon: 'mdi-timer-outline' },
          { label: '封面生成', defaultLabel: '封面生成', key: 'CoverGeneratorConfig', icon: 'mdi-palette-outline' },
          { label: '用户管理', defaultLabel: '用户管理', key: 'UserManagement', icon: 'mdi-account-group-outline' },
        ]
      }
    );
  }
  return finalMenu;
});

// 辅助函数：获取原始名称和图标
const baseMenuMap = computed(() => {
  const map = new Map();
  baseMenuOptions.value.forEach(group => {
    map.set(group.key, { ...group, isGroup: true });
    if (group.children) {
      group.children.forEach(child => {
        map.set(child.key, { ...child, isGroup: false });
      });
    }
  });
  return map;
});

const getOriginalLabel = (key) => {
  return baseMenuMap.value.get(key)?.defaultLabel || '';
};
const menuGroups = computed(() => menuConfigTree.value.filter(item => item.children));

// 核心合并算法：将后端保存的树与当前代码中的 baseMenuOptions 合并
const syncMenuConfig = (savedTree) => {
  return mergeNavigationConfig(savedTree, baseMenuOptions.value);
};

// 初始化配置
const initMenuConfig = async () => {
  if (isMenuLoading.value) return;
  isMenuLoading.value = true;
  menuConfigLoaded.value = false;
  menuConfigError.value = '';
  if (!menuConfigTree.value.length) isMenuReady.value = false;
  try {
    const response = await axios.get('/api/system/menu_config');
    if (response.data != null && !Array.isArray(response.data)) throw new Error('导航配置格式无效，请重试读取。');
    const saved = response.data || [];
    menuConfigTree.value = syncMenuConfig(saved);
    menuConfigLoaded.value = true;
  } catch (error) {
    console.error("获取菜单配置失败", error);
    menuConfigError.value = error.response?.data?.error || error.message || '读取导航配置失败。';
    menuConfigTree.value = syncMenuConfig([]); // 失败时回退到默认
  } finally {
    isMenuLoading.value = false;
    // 确保数据加载完再渲染菜单
    nextTick(() => {
      isMenuReady.value = true;
    });
  }
};

// 监听基础菜单变化（权限变化）
watch(baseMenuOptions, () => {
  // 重新合并，保留用户已有的排序和名称，加入新权限的菜单
  menuConfigTree.value = syncMenuConfig(menuConfigTree.value);
}, { deep: true });

// 保存配置到后端
const saveMenuConfig = async () => {
  if (!menuConfigLoaded.value || isSavingMenu.value) return;
  const session = menuEditorSession, requestedUser = authStore.user;
  const payload = JSON.parse(JSON.stringify(menuConfigTree.value));
  const isCurrent = () => session === menuEditorSession && authStore.isLoggedIn && authStore.user === requestedUser;
  isSavingMenu.value = true;
  try {
    await axios.post('/api/system/menu_config', payload);
    if (!isCurrent()) return;
    if (JSON.stringify(menuConfigTree.value) !== JSON.stringify(payload)) {
      message.info('提交时的导航配置已保存，当前修改尚未保存。');
      return;
    }
    isMenuEditorVisible.value = false;
    message.success('菜单配置已保存，全局生效');

  } catch (error) {
    if (isCurrent()) {
      const detail = error.response?.data?.error || error.message || '保存菜单配置失败';
      message.error(JSON.stringify(menuConfigTree.value) === JSON.stringify(payload) ? detail
        : `提交时的导航配置保存失败，当前修改已保留：${detail}`);
    }
  } finally {
    isSavingMenu.value = false;
  }
};

// 恢复默认
const resetMenuConfig = async () => {
  if (isSavingMenu.value || isMenuLoading.value) return;
  isSavingMenu.value = true;
  try {
    await axios.post('/api/system/menu_config/reset');
    menuConfigTree.value = syncMenuConfig([]); // 传入空数组生成默认树
    menuConfigLoaded.value = true;
    menuConfigError.value = '';
    message.success('已恢复默认菜单');

  } catch (error) {
    message.error(error.response?.data?.error || '重置菜单配置失败');
  } finally {
    isSavingMenu.value = false;
  }
};

// 最终渲染的菜单
const menuOptions = computed(() => {
  if (menuConfigTree.value.length === 0) return [];

  const finalMenu = [];

  menuConfigTree.value.forEach(group => {
    const baseGroup = baseMenuMap.value.get(group.key);
    if (!baseGroup) return;
    if (!baseGroup.isGroup) {
      if (group.visible) finalMenu.push({ key: group.key, label: group.label || baseGroup.defaultLabel, icon: baseGroup.icon });
      return;
    }

    // 1. 先筛选出该组下所有可见的二级菜单
    const visibleChildren = [];
    if (group.children && group.children.length > 0) {
      group.children.forEach(child => {
        if (child.visible) {
          const baseChild = baseMenuMap.value.get(child.key);
          if (baseChild) {
            visibleChildren.push({
              key: child.key,
              label: child.label || baseChild.defaultLabel,
              icon: baseChild.icon
            });
          }
        }
      });
    }

    // 如果该组下没有任何可见的子菜单，则直接跳过（不渲染组，也不渲染子项）
    if (visibleChildren.length === 0) return;

    if (group.visible) {
      // 显示分区标题及其中的菜单项。
      finalMenu.push({
        key: group.key,
        label: group.label || baseGroup.defaultLabel,
        icon: baseGroup.icon,
        children: visibleChildren
      });
    } else {
      // 3. 【核心逻辑】如果一级菜单不可见，将可见的二级菜单直接“扁平化”提升为一级菜单
      finalMenu.push(...visibleChildren);
    }
  });

  return finalMenu;
});

// ================= 原生拖拽逻辑 =================
const dragSource = ref(null); // { key, groupKey }; null group means a top-level item.
const dragTarget = ref(null);
const dragPosition = ref('');
const menuEditorContent = ref(null);
let menuDragFrame = 0, menuDragVelocity = 0, menuDragTime = 0;

const scrollMenuDrag = time => {
  const content = menuEditorContent.value?.$el;
  menuDragFrame = 0;
  if (!content || !dragSource.value || !menuDragVelocity) return;
  const before = content.scrollTop;
  content.scrollTop += menuDragVelocity * Math.min(menuDragTime ? time - menuDragTime : 16, 32);
  menuDragTime = time;
  if (content.scrollTop !== before) menuDragFrame = requestAnimationFrame(scrollMenuDrag);
};
const updateMenuDragScroll = event => {
  const content = menuEditorContent.value?.$el;
  if (!content || !dragSource.value) return;
  const rect = content.getBoundingClientRect();
  const edge = Math.min(48, rect.height / 4);
  menuDragVelocity = event.clientX < rect.left || event.clientX > rect.right
    || event.clientY < rect.top || event.clientY > rect.bottom ? 0
    : event.clientY < rect.top + edge ? -(1 - (event.clientY - rect.top) / edge) * .6
    : event.clientY > rect.bottom - edge ? (1 - (rect.bottom - event.clientY) / edge) * .6 : 0;
  if (menuDragVelocity && !menuDragFrame) {
    menuDragTime = 0;
    menuDragFrame = requestAnimationFrame(scrollMenuDrag);
  } else if (!menuDragVelocity && menuDragFrame) {
    cancelAnimationFrame(menuDragFrame);
    menuDragFrame = 0;
  }
};
const setMenuDragTarget = (key, groupKey, position) => {
  if (dragTarget.value?.key !== key || dragTarget.value?.groupKey !== groupKey) dragTarget.value = { key, groupKey };
  if (dragPosition.value !== position) dragPosition.value = position;
};

const onDragStart = (e, key, groupKey = null) => {
  dragSource.value = { key, groupKey };
  document.addEventListener('dragover', updateMenuDragScroll, true);
  e.dataTransfer.effectAllowed = 'move';
  e.dataTransfer.setData('text/plain', key);
};

const onDragOver = (e, key, groupKey = null) => {
  e.preventDefault();
  e.dataTransfer.dropEffect = 'move';
  const rect = e.currentTarget.getBoundingClientRect();
  const midY = rect.top + rect.height / 2;
  setMenuDragTarget(key, groupKey, e.clientY < midY ? 'top' : 'bottom');
};

const onDragOverEmpty = (e, groupKey = null) => {
  e.preventDefault();
  e.dataTransfer.dropEffect = 'move';
  setMenuDragTarget(null, groupKey, 'inside');
};
const onDragOverRootGap = (e, key) => {
  e.preventDefault();
  e.dataTransfer.dropEffect = 'move';
  setMenuDragTarget(key, null, 'top');
};

const onDrop = (e, targetKey, groupKey = null) => {
  e.preventDefault();
  if (!dragSource.value) return;
  moveNavigationItem(menuConfigTree.value, dragSource.value.key, groupKey, targetKey, dragPosition.value);
  clearDragState();
};

const onDropEmpty = (e, groupKey = null) => {
  e.preventDefault();
  if (!dragSource.value) return;
  moveNavigationItem(menuConfigTree.value, dragSource.value.key, groupKey);
  clearDragState();
};

const onDragEnd = () => {
  clearDragState();
};

const clearDragState = () => {
  document.removeEventListener('dragover', updateMenuDragScroll, true);
  cancelAnimationFrame(menuDragFrame);
  menuDragFrame = menuDragVelocity = menuDragTime = 0;
  dragSource.value = null;
  dragTarget.value = null;
  dragPosition.value = '';
};
watch(isMenuEditorVisible, visible => { ++menuEditorSession; if (!visible) clearDragState(); }, { flush: 'sync' });
onBeforeUnmount(() => { ++menuEditorSession; clearDragState(); });

</script>

<template>
  <DefaultLayout :menus="isMenuReady ? menuOptions : []" :active-key="String(activeMenuKey || '')"
    :has-header-tabs="settingsPage"
    :layout="appearance.settings.value.layout" @navigate="handleMenuUpdate" @update:layout="setLayout">
    <template #brand="{ rail }">
      <RouterLink :to="{ name: 'Dashboard' }" class="app-brand"><img src="./assets/logo.png" alt="Emby Toolkit" />
        <span v-if="!rail" class="app-brand-name">Emby Toolkit</span>
      </RouterLink>
    </template>
    <template #navbar-leading>
      <template v-if="searchAvailable">
        <VBtn v-if="isMobile" icon="mdi-magnify" variant="text" :aria-label="`打开${searchLabel}`" @click="openSearch" />
        <VMenu v-else v-model="searchOpen" location="bottom start" offset="8" :close-on-content-click="false" :open-on-click="false">
          <template #activator="{ props: searchProps }">
            <div v-bind="searchProps" class="navbar-search navbar-search-desktop">
              <VIcon icon="mdi-magnify" size="22" />
              <input id="global-media-search" ref="desktopSearchInput" v-model="searchWord" class="navbar-context-search" type="search" autocomplete="off"
                :aria-label="searchLabel" :placeholder="searchLabel" :disabled="searchDisabled" @focus="searchOpen = !activeContextSearch"
                @keydown.enter.stop.prevent="submitSearch" @keydown.escape.stop="searchOpen = false"
                @compositionstart="startSearchComposition" @compositionend="finishSearchComposition" />
              <VBtn v-if="searchWord" icon="mdi-close" color="default" variant="text" size="x-small" aria-label="清空搜索" :disabled="searchDisabled" @click.stop="clearSearch" />
              <kbd class="search-shortcut-badge">{{ searchShortcutLabel }}</kbd>
            </div>
          </template>
          <VCard class="navbar-search-popover" min-width="280" max-width="calc(100vw - 24px)">
            <VList class="pa-2">
              <VListItem :title="searchLabel" :subtitle="searchWord || undefined" prepend-icon="mdi-magnify" :disabled="searchDisabled" @click="submitSearch" />
            </VList>
          </VCard>
        </VMenu>
      </template>
    </template>
    <template #navbar-center>
      <VMenu v-if="authStore.isAdmin && taskStatus?.is_running" location="bottom" :close-on-content-click="false">
        <template #activator="{ props: taskMenuProps }">
          <VBtn v-bind="taskMenuProps" variant="tonal" color="default" class="running-task" aria-label="查看当前任务">
            <span class="running-task-content"><strong>{{ taskStatus.current_action }}</strong>
              <VProgressLinear :model-value="taskStatus.progress" :indeterminate="!taskStatus.progress_known" color="primary" rounded height="3" />
            </span>
          </VBtn>
        </template>
        <VCard width="360" max-width="calc(100vw - 24px)">
          <VCardItem :title="taskStatus.current_action" :subtitle="taskStatus.phase || taskStatus.state" />
          <VCardText><VProgressLinear :model-value="taskStatus.progress" :indeterminate="!taskStatus.progress_known" color="primary" rounded height="6" />
            <div class="task-message">{{ taskStatus.message }}</div>
            <div v-if="taskStatus.progress_known" class="text-caption">{{ taskStatus.progress }}%</div>
          </VCardText>
          <VCardActions class="flex-wrap">
            <VBtn :prepend-icon="taskPaused ? 'mdi-play' : 'mdi-pause'" :loading="taskControlBusy" :disabled="taskStopping || taskStatus.state === 'pausing'"
              @click="controlTask(taskPaused ? 'resume' : 'pause')">{{ taskPaused ? '继续' : taskStatus.state === 'pausing' ? '暂停中' : '暂停' }}</VBtn>
            <VBtn color="error" prepend-icon="mdi-stop" :disabled="taskStopping || taskControlBusy" @click="confirmStopTask">{{ taskStopping ? '停止中' : '停止' }}</VBtn>
            <VSpacer /><VBtn icon="mdi-text-box-search-outline" aria-label="查看任务日志" @click="notificationOpen = true" />
          </VCardActions>
        </VCard>
      </VMenu>
    </template>
    <template #navbar-actions>
      <VBtn v-if="authStore.isAdmin" icon variant="text" color="default" title="实时日志" aria-label="实时日志" @click="notificationOpen = true">
        <VBadge :model-value="Boolean(taskStatus?.is_running)" color="primary" dot><VIcon icon="mdi-bell-outline" /></VBadge>
      </VBtn>
      <VBtn icon="mdi-palette-outline" color="default" variant="text" title="主题与壁纸" aria-label="主题与壁纸" @click="openThemeCustomizer" />
      <VMenu v-if="authStore.isLoggedIn" location="bottom end" offset="8">
        <template #activator="{ props: userMenuProps }">
          <VBtn v-bind="userMenuProps" icon variant="text" :aria-label="`${authStore.username} 的用户菜单`">
            <VAvatar size="38" color="primary"><VImg v-if="authStore.avatarUrl && !avatarLoadFailed" :src="authStore.avatarUrl" alt="" @error="avatarLoadFailed = true" />
              <span v-else>{{ (authStore.username || 'U').slice(0, 1).toUpperCase() }}</span>
            </VAvatar>
          </VBtn>
        </template>
        <VList min-width="240" class="user-profile-menu pt-0">
          <VListItem class="user-profile-summary py-4">
            <template #prepend>
              <VAvatar size="60" rounded="sm" color="primary" variant="tonal" class="mr-3">
                <VImg v-if="authStore.avatarUrl && !avatarLoadFailed" :src="authStore.avatarUrl" alt="" @error="avatarLoadFailed = true" />
                <span v-else>{{ (authStore.username || 'U').slice(0, 1).toUpperCase() }}</span>
              </VAvatar>
            </template>
            <span class="user-profile-role">{{ authStore.isAdmin ? '管理员' : '普通用户' }}</span>
            <span class="user-profile-name">{{ authStore.username }}</span>
          </VListItem>
          <VDivider class="mb-2" />
          <VListItem v-for="item in userOptions" :key="item.key" :title="item.label" :prepend-icon="item.icon" @click="handleUserSelect(item.key)" />
        </VList>
      </VMenu>
    </template>
    <template v-if="settingsPage" #header-tabs>
      <VTabs :model-value="activeSettingsTab" color="primary" height="44" class="settings-header-tabs" aria-label="设置分类" @update:model-value="navigateSettings">
        <VTab v-for="tab in settingsTabs" :key="tab.value" :value="tab.value" :prepend-icon="tab.icon">{{ tab.title }}</VTab>
      </VTabs>
    </template>
    <RouterView v-slot="{ Component }"><component :is="Component" :key="contentKey" :task-status="props.taskStatus" /></RouterView>
  </DefaultLayout>

  <VDialog v-if="isMobile && searchAvailable" v-model="searchOpen" fullscreen max-width="640" scrollable>
    <VCard>
      <VCardTitle class="d-flex align-center justify-space-between"><span>{{ searchLabel }}</span><VBtn icon="mdi-close" variant="text" color="default" aria-label="关闭搜索" @click="searchOpen = false" /></VCardTitle>
      <VCardText>
        <div class="navbar-search navbar-search-mobile">
          <VIcon icon="mdi-magnify" size="22" />
          <input id="global-media-search" ref="mobileSearchInput" v-model="searchWord" class="navbar-context-search" type="search" autocomplete="off"
            :aria-label="searchLabel" :placeholder="searchLabel" :disabled="searchDisabled" @keydown.enter.stop.prevent="submitSearch"
            @compositionstart="startSearchComposition" @compositionend="finishSearchComposition" />
          <VBtn v-if="searchWord" icon="mdi-close" color="default" variant="text" size="small" aria-label="清空搜索" :disabled="searchDisabled" @click="clearSearch" />
          <VBtn icon="mdi-magnify" variant="text" aria-label="提交搜索" :loading="searchLoading" :disabled="searchDisabled" @click="submitSearch" />
        </div>
      </VCardText>
    </VCard>
  </VDialog>
  <VDialog v-model="notificationOpen" :scrollable="mdAndUp" max-width="80rem" :fullscreen="!mdAndUp">
    <VCard class="notification-dialog-card"><NotificationCenter v-if="notificationOpen" :task="taskStatus" @close="notificationOpen = false" /></VCard>
  </VDialog>
  <ThemeCustomizer v-if="themeCustomizerOpen" @close="themeCustomizerOpen = false"><WallpaperSettings /></ThemeCustomizer>
  <VDialog v-model="isMenuEditorVisible" max-width="760" scrollable>
    <VCard>
      <VCardItem title="自定义导航" subtitle="修改菜单名称、显示状态和顺序，全局生效">
        <template #append><VBtn icon="mdi-close" variant="text" color="default" aria-label="关闭导航设置" @click="isMenuEditorVisible = false" /></template>
      </VCardItem>
      <VCardText ref="menuEditorContent" class="menu-editor-content">
        <VAlert v-if="menuConfigError" type="warning" variant="tonal" title="导航设置尚未读取" class="mb-4">
          <div>{{ menuConfigError }}</div>
          <p class="mt-2">默认导航仍可使用，成功读取后才能保存更改。</p>
          <VBtn size="small" variant="tonal" class="mt-2" prepend-icon="mdi-refresh" :loading="isMenuLoading" @click="initMenuConfig">重试读取</VBtn>
        </VAlert>
        <div class="menu-editor-roots" :class="{ 'menu-editor-roots--dragging': dragSource }">
          <template v-for="(group, groupIndex) in menuConfigTree" :key="group.key">
            <div class="menu-editor-root-dropzone" :data-before-key="group.key"
              :class="{ 'drag-over-top': dragTarget?.key === group.key && dragTarget?.groupKey === null && dragPosition === 'top' }"
              @dragover.prevent.stop="onDragOverRootGap($event, group.key)" @drop.stop="onDrop($event, group.key)">拖至此处设为独立导航</div>
            <VCard variant="outlined" class="menu-editor-node" :class="{ 'menu-editor-group': group.children, 'menu-editor-standalone': !group.children }" :data-menu-key="group.key">
              <VCardText>
                <div class="menu-editor-group-heading" draggable="true" :class="{ 'is-dragging': dragSource?.key === group.key }"
                  @dragstart.stop="onDragStart($event, group.key)" @dragover.prevent.stop="onDragOver($event, group.key)"
                  @drop.stop="onDrop($event, group.key)" @dragend="onDragEnd">
                  <VIcon icon="mdi-drag" class="menu-drag-handle" />
                  <VSwitch v-model="group.visible" density="compact" hide-details :aria-label="group.children ? `显示 ${getOriginalLabel(group.key)} 分区` : `显示 ${getOriginalLabel(group.key)}`" />
                  <VTextField v-model="group.label" :placeholder="getOriginalLabel(group.key)" :label="group.children ? '分区名称' : '菜单名称'" density="compact" hide-details />
                  <VMenu v-if="!group.children">
                    <template #activator="{ props: moveProps }"><VBtn v-bind="moveProps" icon="mdi-folder-plus-outline" color="default" variant="text" size="small" :aria-label="`将 ${group.label} 移入分区`" /></template>
                    <VList><VListItem v-for="target in menuGroups" :key="target.key" :title="target.label" @click="moveMenuToGroup(group.key, target.key)" /></VList>
                  </VMenu>
                  <VBtn icon="mdi-chevron-up" color="default" variant="text" size="small" :disabled="groupIndex === 0" aria-label="上移导航" @click="moveRootMenu(groupIndex, -1)" />
                  <VBtn icon="mdi-chevron-down" color="default" variant="text" size="small" :disabled="groupIndex === menuConfigTree.length - 1" aria-label="下移导航" @click="moveRootMenu(groupIndex, 1)" />
                </div>
                <div v-if="group.children" class="menu-editor-children" @dragover.prevent.stop="onDragOverEmpty($event, group.key)" @drop.stop="onDropEmpty($event, group.key)">
                  <div v-for="(child, childIndex) in group.children" :key="child.key" class="menu-editor-item" :data-menu-key="child.key" draggable="true"
                    :class="{ 'drag-over-top': dragTarget?.groupKey === group.key && dragTarget?.key === child.key && dragPosition === 'top',
                      'drag-over-bottom': dragTarget?.groupKey === group.key && dragTarget?.key === child.key && dragPosition === 'bottom', 'is-dragging': dragSource?.key === child.key }"
                    @dragstart.stop="onDragStart($event, child.key, group.key)" @dragover.prevent.stop="onDragOver($event, child.key, group.key)"
                    @drop.stop="onDrop($event, child.key, group.key)" @dragend="onDragEnd">
                    <VIcon icon="mdi-drag" class="menu-drag-handle" />
                    <VSwitch v-model="child.visible" density="compact" hide-details :aria-label="`显示 ${getOriginalLabel(child.key)}`" />
                    <VTextField v-model="child.label" :placeholder="getOriginalLabel(child.key)" label="菜单名称" density="compact" hide-details />
                    <div class="menu-editor-item-actions">
                      <VBtn icon="mdi-arrow-top-right" color="default" variant="text" size="small" :aria-label="`将 ${child.label} 移出分区`" @click="promoteMenuItem(group.key, child.key)" />
                      <VBtn icon="mdi-chevron-up" color="default" variant="text" size="small" :disabled="childIndex === 0" aria-label="上移菜单" @click="moveMenuItem(groupIndex, childIndex, -1)" />
                      <VBtn icon="mdi-chevron-down" color="default" variant="text" size="small" :disabled="childIndex === group.children.length - 1" aria-label="下移菜单" @click="moveMenuItem(groupIndex, childIndex, 1)" />
                    </div>
                  </div>
                  <div v-if="!group.children.length" class="empty-group-dropzone">拖拽菜单至此处</div>
                </div>
              </VCardText>
            </VCard>
          </template>
          <div class="menu-editor-root-dropzone menu-editor-root-dropzone--last" @dragover.prevent.stop="onDragOverEmpty($event)" @drop.stop="onDropEmpty($event)">拖至此处设为独立导航</div>
        </div>
        <VAlert type="info" variant="tonal" density="compact">拖到分区之间可设为独立导航，拖入分区可调整所属分区。菜单项始终直接显示；关闭分区显示时，其中开启的菜单仍会显示。</VAlert>
      </VCardText>
      <VCardActions><VBtn color="error" :loading="isSavingMenu" :disabled="isMenuLoading" prepend-icon="mdi-restore" @click="confirmResetMenu">恢复默认</VBtn>
          <VSpacer /><VBtn class="menu-apply-button" color="primary" variant="flat" :loading="isSavingMenu" :disabled="!menuConfigLoaded" prepend-icon="mdi-content-save-outline" @click="saveMenuConfig">保存</VBtn>
      </VCardActions>
    </VCard>
  </VDialog>
</template>

<style lang="scss" scoped>
.app-brand { display: flex; align-items: center; gap: 12px; color: rgb(var(--v-theme-on-surface)); text-decoration: none; min-inline-size: 0; }
.app-brand img { inline-size: 36px; block-size: 36px; object-fit: contain; }
.app-brand-name { font-size: 17px; font-weight: 700; white-space: nowrap; }
.navbar-search { color: rgba(var(--v-theme-on-surface), var(--v-high-emphasis-opacity)) !important; }
.navbar-search-desktop, .navbar-search-mobile { display: flex; align-items: center; gap: 10px; min-inline-size: 0; padding: 8px 12px; border-radius: var(--app-field-radius); }
.navbar-search-desktop { inline-size: clamp(240px, 30vw, 440px); max-inline-size: 100%; min-inline-size: 0; }
.navbar-context-search { flex: 1; min-inline-size: 0; inline-size: 100%; color: inherit; font: inherit; font-size: 15px; line-height: 1.5; background: transparent; border: 0; outline: none; }
.navbar-context-search::placeholder { color: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity)); opacity: 1; }
.navbar-context-search::-webkit-search-cancel-button { display: none; }
.navbar-search:focus-within { outline: 2px solid rgba(var(--v-theme-primary), .5); outline-offset: -2px; }
.search-shortcut-badge { flex: 0 0 auto; white-space: nowrap; font-size: 11px; line-height: 1.5; padding: 2px 5px; border: 1px solid rgba(var(--v-theme-on-surface), .2); border-radius: 4px; }
.navbar-search-popover :deep(.v-list-item-title), .navbar-search-popover :deep(.v-list-item-subtitle) { display: block; white-space: normal; overflow: visible; overflow-wrap: anywhere; -webkit-line-clamp: unset; }
.user-profile-summary { background: rgba(var(--v-theme-primary), .05); }
.user-profile-role { display: block; font-size: 14px; font-weight: 500; color: rgba(var(--v-theme-on-surface), var(--v-high-emphasis-opacity)); }
.user-profile-name { display: block; font-size: 18px; font-weight: 700; color: rgba(var(--v-theme-on-surface), var(--v-high-emphasis-opacity)); overflow-wrap: anywhere; }
.running-task { inline-size: min(240px, 100%); min-inline-size: 0; block-size: 46px; padding: 8px 12px; border: 1px solid rgba(var(--v-theme-on-surface), .16); border-radius: 12px; background-color: rgb(var(--v-theme-surface)); }
.running-task-content { display: grid; gap: 5px; min-inline-size: 0; inline-size: 100%; text-align: center; }
.running-task-content strong { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; font-weight: 500; }
.task-message { margin-block-start: 12px; overflow-wrap: anywhere; }
.menu-editor-content { max-block-size: 70dvh; }
.menu-editor-node { min-inline-size: 0; }
.menu-editor-root-dropzone { block-size: 32px; padding: 4px; border-block: 2px solid transparent; font-size: 12px; line-height: 20px; text-align: center; color: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity)); }
.menu-editor-roots:not(.menu-editor-roots--dragging) .menu-editor-root-dropzone { color: transparent; }
.menu-editor-roots--dragging .menu-editor-root-dropzone { border-style: dashed; border-color: rgba(var(--v-theme-primary), .3); }
.menu-editor-root-dropzone--last { margin-block-end: 16px; }
.menu-editor-group-heading, .menu-editor-item { display: flex; align-items: center; gap: 8px; }
.menu-editor-group-heading { margin-block-end: 16px; flex-wrap: wrap; }
.menu-editor-group-heading :deep(.v-input) { flex: 0 0 auto; }
.menu-editor-group-heading :deep(.v-text-field) { flex: 1 1 180px; }
.menu-editor-item { padding-block: 10px; border-block: 2px solid transparent; }
.menu-editor-item :deep(.v-switch) { flex: 0 0 auto; }
.menu-editor-item :deep(.v-text-field) { min-inline-size: 80px; flex: 1 1 120px; }
.menu-editor-item-actions { display: flex; flex: 0 0 auto; gap: 3px; }
.menu-editor-children { min-block-size: 44px; }
.menu-drag-handle { cursor: grab; }
.drag-over-top { border-block-start-color: rgb(var(--v-theme-primary)); }
.drag-over-bottom { border-block-end-color: rgb(var(--v-theme-primary)); }
.is-dragging { opacity: .45; }
.empty-group-dropzone { text-align: center; color: rgba(var(--v-theme-on-surface), .55); padding: 16px; border: 1px dashed rgba(var(--v-theme-on-surface), .2); }
@media (width <= 600px) {
  .running-task { padding-inline: 6px; }
  .app-brand-name { font-size: 15px; }
  .menu-editor-item { display: grid; grid-template-columns: 24px 40px minmax(0, 1fr); gap: 8px; }
  .menu-editor-item :deep(.v-text-field) { min-inline-size: 0; }
  .menu-editor-item-actions { grid-column: 3; }
}
</style>
