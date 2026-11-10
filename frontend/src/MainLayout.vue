<script setup>
import { ref, computed, watch, nextTick, onMounted, onBeforeUnmount } from 'vue';
import { useRouter, useRoute } from 'vue-router';
import { useDisplay } from 'vuetify';
import axios from 'axios';
import { useAuthStore } from '@/stores/auth';
import { refreshTaskStatus } from '@/composables/useTaskStatus';
import { useMessage, useDialog } from '@/composables/useFeedback';
import { useThemeCustomizer, THEME_CUSTOMIZER_OPEN_EVENT } from '@/composables/useThemeCustomizer';
import DefaultLayout from '@/layouts/components/DefaultLayout.vue';
import NotificationCenter from '@/components/NotificationCenter.vue';
import ThemeCustomizer from '@/components/theme/ThemeCustomizer.vue';
import WallpaperSettings from '@/components/WallpaperSettings.vue';

const props = defineProps({ taskStatus: Object });
const router = useRouter(), route = useRoute(), authStore = useAuthStore();
const message = useMessage(), dialog = useDialog();
const appearance = useThemeCustomizer();
const { mdAndDown } = useDisplay();
const isMobile = computed(() => mdAndDown.value);
const collapsed = computed(() => appearance.settings.value.layout === 'collapsed' && !isMobile.value);
const notificationOpen = ref(false), themeCustomizerOpen = ref(false), avatarLoadFailed = ref(false);
const taskControlBusy = ref(false);
const settingsPage = computed(() => ['settings-general', 'settings-notifications'].includes(route.name));
const contentKey = computed(() => settingsPage.value ? route.name : route.path);
const activeMenuKey = computed(() => route.name === 'MediaCenterLibrary' ? 'Dashboard'
  : settingsPage.value ? 'settings-general' : route.name);
const settingsTabs = [
  { value: 'system', title: '系统', icon: 'mdi-server-network' },
  { value: 'directory', title: '存储 & 目录', icon: 'mdi-folder' },
  { value: 'rule', title: '规则', icon: 'mdi-filter' },
  { value: 'search', title: '搜索 & 下载', icon: 'mdi-magnify' },
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
onMounted(() => { initMenuConfig(); window.addEventListener(THEME_CUSTOMIZER_OPEN_EVENT, openThemeCustomizer); });
onBeforeUnmount(() => window.removeEventListener(THEME_CUSTOMIZER_OPEN_EVENT, openThemeCustomizer));

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
    await authStore.logout();
    router.push({ name: 'Login' });
  }
};

// ================= 自定义菜单逻辑 (树形结构 + 拖拽) =================
const isMenuEditorVisible = ref(false);
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
          { label: '用户管理', defaultLabel: '用户管理', key: 'UserManagement', icon: 'mdi-account-group-outline' },
          { label: '任务中心', defaultLabel: '任务中心', key: 'settings-scheduler', icon: 'mdi-timer-outline' },
          { label: '封面生成', defaultLabel: '封面生成', key: 'CoverGeneratorConfig', icon: 'mdi-palette-outline' },
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

// 核心合并算法：将后端保存的树与当前代码中的 baseMenuOptions 合并
const syncMenuConfig = (savedTree) => {
  const resultTree = [];
  const usedKeys = new Set();

  // 1. 遍历保存的树，保留存在的项
  if (Array.isArray(savedTree)) {
    savedTree.forEach(savedGroup => {
      const baseGroup = baseMenuMap.value.get(savedGroup.key);
      if (!baseGroup || !baseGroup.isGroup) return; // 废弃的组

      usedKeys.add(savedGroup.key);
      const newGroup = {
        key: savedGroup.key,
        label: savedGroup.label || baseGroup.defaultLabel,
        visible: savedGroup.visible !== false,
        expanded: savedGroup.expanded !== false, // 默认展开
        children: []
      };

      if (Array.isArray(savedGroup.children)) {
        savedGroup.children.forEach(savedChild => {
          const key = { MediaCenter: 'Dashboard', UserCenter: 'UserManagement', 'settings-notifications': 'settings-general' }[savedChild.key] || savedChild.key;
          const baseChild = baseMenuMap.value.get(key);
          if (!baseChild || baseChild.isGroup) return; // 废弃的子项
          if (usedKeys.has(key)) return;
          usedKeys.add(key);
          const oldDefault = { MediaCenter: '媒体中心', UserCenter: '用户中心', 'settings-notifications': '通知设置', 'settings-general': '通用设置' }[savedChild.key];
          newGroup.children.push({
            key,
            label: savedChild.label && savedChild.label !== oldDefault ? savedChild.label : baseChild.defaultLabel,
            visible: savedChild.visible !== false
          });
        });
      }
      resultTree.push(newGroup);
    });
  }

  // 2. 找出 baseOptions 中有，但 savedTree 中没有的新增项
  baseMenuOptions.value.forEach(baseGroup => {
    if (!usedKeys.has(baseGroup.key)) {
      // 整个组都是新的
      resultTree.push({
        key: baseGroup.key,
        label: baseGroup.defaultLabel,
        visible: true,
        expanded: true,
        children: baseGroup.children ? baseGroup.children.filter(c => !usedKeys.has(c.key)).map(c => {
          usedKeys.add(c.key);
          return { key: c.key, label: c.defaultLabel, visible: true };
        }) : []
      });
    } else {
      // 组存在，检查是否有新的子项
      const targetGroup = resultTree.find(g => g.key === baseGroup.key);
      if (baseGroup.children) {
        baseGroup.children.forEach(baseChild => {
          if (!usedKeys.has(baseChild.key)) {
            usedKeys.add(baseChild.key);
            targetGroup.children.push({
              key: baseChild.key,
              label: baseChild.defaultLabel,
              visible: true
            });
          }
        });
      }
    }
  });

  return resultTree;
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
  isSavingMenu.value = true;
  try {
    await axios.post('/api/system/menu_config', menuConfigTree.value);
    isMenuEditorVisible.value = false;
    message.success('菜单配置已保存，全局生效');

    // 强制重新渲染菜单以应用新的展开状态
    isMenuReady.value = false;
    nextTick(() => { isMenuReady.value = true; });
  } catch (error) {
    message.error(error.response?.data?.error || '保存菜单配置失败');
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

    isMenuReady.value = false;
    nextTick(() => { isMenuReady.value = true; });
  } catch (error) {
    message.error(error.response?.data?.error || '重置菜单配置失败');
  } finally {
    isSavingMenu.value = false;
  }
};

// 计算默认展开的 keys
const defaultExpandedKeys = computed(() => {
  return menuConfigTree.value.filter(g => g.expanded).map(g => g.key);
});

// 最终渲染的菜单
const menuOptions = computed(() => {
  if (menuConfigTree.value.length === 0) return [];

  const finalMenu = [];

  menuConfigTree.value.forEach(group => {
    const baseGroup = baseMenuMap.value.get(group.key);
    if (!baseGroup) return;

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

    if (group.visible && !(collapsed.value && !isMobile.value)) {
      // 2. 如果一级菜单可见，正常渲染为包含 children 的折叠组
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
const dragSource = ref(null); // { gIndex, cIndex }
const dragTarget = ref(null); // { gIndex, cIndex }
const dragPosition = ref(''); // 'top' 或 'bottom'

const onDragStart = (e, gIndex, cIndex) => {
  dragSource.value = { gIndex, cIndex };
  e.dataTransfer.effectAllowed = 'move';
  // 设置一个透明的拖拽图像，或者让浏览器默认处理
};

const onDragOver = (e, gIndex, cIndex) => {
  e.preventDefault();
  e.dataTransfer.dropEffect = 'move';

  // 计算鼠标在元素上半部还是下半部
  const rect = e.currentTarget.getBoundingClientRect();
  const midY = rect.top + rect.height / 2;
  dragPosition.value = e.clientY < midY ? 'top' : 'bottom';
  dragTarget.value = { gIndex, cIndex };
};

const onDragOverEmpty = (e, gIndex) => {
  e.preventDefault();
  e.dataTransfer.dropEffect = 'move';
  // 拖入空组
  if (menuConfigTree.value[gIndex].children.length === 0) {
    dragTarget.value = { gIndex, cIndex: -1 };
    dragPosition.value = 'inside';
  }
};

const onDrop = (e, targetGIndex, targetCIndex) => {
  e.preventDefault();
  if (!dragSource.value) return;

  const { gIndex: fromGIndex, cIndex: fromCIndex } = dragSource.value;

  // 如果是同一个元素，不操作
  if (fromGIndex === targetGIndex && fromCIndex === targetCIndex) {
    clearDragState();
    return;
  }

  // 1. 取出被拖拽的元素
  const item = menuConfigTree.value[fromGIndex].children.splice(fromCIndex, 1)[0];

  // 2. 计算插入位置
  let insertIndex = targetCIndex;

  // 如果在同一个组内往下拖，因为前面删除了一个元素，目标索引需要 -1
  if (fromGIndex === targetGIndex && fromCIndex < targetCIndex) {
    insertIndex -= 1;
  }

  // 如果放在下半部，插入到目标后面
  if (dragPosition.value === 'bottom') {
    insertIndex += 1;
  }

  // 3. 插入元素
  menuConfigTree.value[targetGIndex].children.splice(insertIndex, 0, item);

  clearDragState();
};

const onDropEmpty = (e, targetGIndex) => {
  e.preventDefault();
  if (!dragSource.value) return;

  // 只有当目标组真的为空时才处理
  if (menuConfigTree.value[targetGIndex].children.length === 0) {
    const { gIndex: fromGIndex, cIndex: fromCIndex } = dragSource.value;
    const item = menuConfigTree.value[fromGIndex].children.splice(fromCIndex, 1)[0];
    menuConfigTree.value[targetGIndex].children.push(item);
  }
  clearDragState();
};

const onDragEnd = () => {
  clearDragState();
};

const clearDragState = () => {
  dragSource.value = null;
  dragTarget.value = null;
  dragPosition.value = '';
};

</script>

<template>
  <DefaultLayout :menus="isMenuReady ? menuOptions : []" :active-key="String(activeMenuKey || '')"
    :has-header-tabs="settingsPage"
    :expanded-keys="defaultExpandedKeys" :layout="appearance.settings.value.layout" @navigate="handleMenuUpdate" @update:layout="setLayout">
    <template #brand="{ rail }">
      <RouterLink :to="{ name: 'Dashboard' }" class="app-brand"><img src="./assets/logo.png" alt="Emby Toolkit" />
        <span v-if="!rail" class="app-brand-name">Emby Toolkit</span>
      </RouterLink>
    </template>
    <template #navbar-leading>
      <VBtn v-if="authStore.isAdmin" variant="text" class="navbar-search" prepend-icon="mdi-magnify"
        :to="{ name: 'ReviewList' }" aria-label="搜索媒体"><span v-if="!isMobile">搜索媒体</span></VBtn>
      <VMenu v-if="authStore.isAdmin && taskStatus?.is_running" location="bottom" :close-on-content-click="false">
        <template #activator="{ props: taskMenuProps }">
          <VBtn v-bind="taskMenuProps" variant="text" class="running-task" aria-label="查看当前任务">
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
      <VBtn v-if="authStore.isAdmin" icon variant="text" title="通知中心" aria-label="通知中心" @click="notificationOpen = true">
        <VBadge :model-value="Boolean(taskStatus?.is_running)" color="primary" dot><VIcon icon="mdi-bell-outline" /></VBadge>
      </VBtn>
      <VBtn icon="mdi-palette-outline" variant="text" title="主题与壁纸" aria-label="主题与壁纸" @click="openThemeCustomizer" />
      <VMenu v-if="authStore.isLoggedIn" location="bottom end" offset="8">
        <template #activator="{ props: userMenuProps }">
          <VBtn v-bind="userMenuProps" icon variant="text" :aria-label="`${authStore.username} 的用户菜单`">
            <VAvatar size="38" color="primary"><VImg v-if="authStore.avatarUrl && !avatarLoadFailed" :src="authStore.avatarUrl" alt="" @error="avatarLoadFailed = true" />
              <span v-else>{{ (authStore.username || 'U').slice(0, 1).toUpperCase() }}</span>
            </VAvatar>
          </VBtn>
        </template>
        <VList min-width="216"><VListItem :title="authStore.username" :subtitle="authStore.isAdmin ? '管理员' : '用户'" /><VDivider />
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

  <VDialog v-model="notificationOpen" max-width="1100" scrollable>
    <VCard><NotificationCenter v-if="notificationOpen" :task="taskStatus" @close="notificationOpen = false" /></VCard>
  </VDialog>
  <ThemeCustomizer v-if="themeCustomizerOpen" @close="themeCustomizerOpen = false"><WallpaperSettings /></ThemeCustomizer>
  <VDialog v-model="isMenuEditorVisible" max-width="760" scrollable>
    <VCard>
      <VCardItem title="自定义导航" subtitle="修改菜单名称、显示状态和顺序，全局生效">
        <template #append><VBtn icon="mdi-close" variant="text" aria-label="关闭导航设置" @click="isMenuEditorVisible = false" /></template>
      </VCardItem>
      <VCardText class="menu-editor-content">
        <VAlert v-if="menuConfigError" type="warning" variant="tonal" title="导航设置尚未读取" class="mb-4">
          <div>{{ menuConfigError }}</div>
          <p class="mt-2">默认导航仍可使用，成功读取后才能保存更改。</p>
          <VBtn size="small" variant="tonal" class="mt-2" prepend-icon="mdi-refresh" :loading="isMenuLoading" @click="initMenuConfig">重试读取</VBtn>
        </VAlert>
        <VCard v-for="(group, groupIndex) in menuConfigTree" :key="group.key" variant="outlined" class="menu-editor-group">
          <VCardText>
            <div class="menu-editor-group-heading">
              <VSwitch v-model="group.visible" density="compact" hide-details :aria-label="`显示 ${getOriginalLabel(group.key)} 菜单组`" />
              <VTextField v-model="group.label" :placeholder="getOriginalLabel(group.key)" label="菜单组名称" density="compact" hide-details />
              <VSwitch v-model="group.expanded" label="默认展开" density="compact" hide-details />
            </div>
            <div class="menu-editor-children" @dragover.prevent="onDragOverEmpty($event, groupIndex)" @drop="onDropEmpty($event, groupIndex)">
              <div v-for="(child, childIndex) in group.children" :key="child.key" class="menu-editor-item" draggable="true"
                :class="{ 'drag-over-top': dragTarget?.gIndex === groupIndex && dragTarget?.cIndex === childIndex && dragPosition === 'top',
                  'drag-over-bottom': dragTarget?.gIndex === groupIndex && dragTarget?.cIndex === childIndex && dragPosition === 'bottom',
                  'is-dragging': dragSource?.gIndex === groupIndex && dragSource?.cIndex === childIndex }"
                @dragstart="onDragStart($event, groupIndex, childIndex)" @dragover.prevent="onDragOver($event, groupIndex, childIndex)"
                @drop="onDrop($event, groupIndex, childIndex)" @dragend="onDragEnd">
                <VIcon icon="mdi-drag" class="menu-drag-handle" />
                <VSwitch v-model="child.visible" density="compact" hide-details :aria-label="`显示 ${getOriginalLabel(child.key)}`" />
                <VTextField v-model="child.label" :placeholder="getOriginalLabel(child.key)" label="菜单名称" density="compact" hide-details />
                <VBtn icon="mdi-chevron-up" variant="text" size="small" :disabled="childIndex === 0" aria-label="上移菜单" @click="moveMenuItem(groupIndex, childIndex, -1)" />
                <VBtn icon="mdi-chevron-down" variant="text" size="small" :disabled="childIndex === group.children.length - 1" aria-label="下移菜单" @click="moveMenuItem(groupIndex, childIndex, 1)" />
              </div>
              <div v-if="!group.children.length" class="empty-group-dropzone">拖拽菜单至此处</div>
            </div>
          </VCardText>
        </VCard>
        <VAlert type="info" variant="tonal" density="compact">支持跨组拖拽。关闭菜单组显示时，其下已开启的菜单会提升为一级。</VAlert>
      </VCardText>
      <VCardActions><VBtn color="error" :loading="isSavingMenu" :disabled="isMenuLoading" prepend-icon="mdi-restore" @click="confirmResetMenu">恢复默认</VBtn>
        <VSpacer /><VBtn color="primary" variant="tonal" :loading="isSavingMenu" :disabled="!menuConfigLoaded" prepend-icon="mdi-content-save-outline" @click="saveMenuConfig">保存并应用</VBtn>
      </VCardActions>
    </VCard>
  </VDialog>
</template>

<style lang="scss" scoped>
.app-brand { display: flex; align-items: center; gap: 12px; color: rgb(var(--v-theme-on-surface)); text-decoration: none; min-inline-size: 0; }
.app-brand img { inline-size: 36px; block-size: 36px; object-fit: contain; }
.app-brand-name { font-size: 17px; font-weight: 700; white-space: nowrap; }
.navbar-search { color: rgba(var(--v-theme-on-surface), var(--v-high-emphasis-opacity)) !important; }
.running-task { max-inline-size: min(280px, 30vw); min-inline-size: 0; }
.running-task-content { display: grid; gap: 5px; min-inline-size: 0; inline-size: 100%; }
.running-task-content strong { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; font-weight: 500; }
.task-message { margin-block-start: 12px; overflow-wrap: anywhere; }
.menu-editor-content { max-block-size: 70dvh; }
.menu-editor-group { margin-block-end: 16px; }
.menu-editor-group-heading, .menu-editor-item { display: flex; align-items: center; gap: 8px; }
.menu-editor-group-heading { margin-block-end: 16px; flex-wrap: wrap; }
.menu-editor-group-heading :deep(.v-input) { flex: 0 0 auto; }
.menu-editor-group-heading :deep(.v-text-field) { flex: 1 1 180px; }
.menu-editor-item { padding-block: 10px; border-block: 2px solid transparent; }
.menu-editor-item :deep(.v-switch) { flex: 0 0 auto; }
.menu-editor-item :deep(.v-text-field) { min-inline-size: 80px; }
.menu-editor-children { min-block-size: 44px; }
.menu-drag-handle { cursor: grab; }
.drag-over-top { border-block-start-color: rgb(var(--v-theme-primary)); }
.drag-over-bottom { border-block-end-color: rgb(var(--v-theme-primary)); }
.is-dragging { opacity: .45; }
.empty-group-dropzone { text-align: center; color: rgba(var(--v-theme-on-surface), .55); padding: 16px; border: 1px dashed rgba(var(--v-theme-on-surface), .2); }
@media (width <= 600px) { .running-task { max-inline-size: 25vw; } .app-brand-name { font-size: 15px; } .menu-editor-item { gap: 3px; } }
</style>
