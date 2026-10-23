<template>
  <div class="mp-app-shell" :class="{ 'mp-nav-collapsed': collapsed && !isMobile }">
    <aside v-if="!isMobile" class="mp-sidebar">
      <div class="mp-brand"><img src="./assets/logo.png" alt="Emby Toolkit" /><div v-if="!collapsed"><strong>Emby Toolkit</strong><span>媒体管理中心</span></div></div>
      <n-menu v-if="isMenuReady" class="mp-menu" :collapsed="collapsed" :collapsed-width="80" :collapsed-icon-size="22" :options="menuOptions" :value="activeMenuKey" :default-expanded-keys="defaultExpandedKeys" @update:value="handleMenuUpdate" />
    </aside>
    <div class="mp-main-column">
      <header class="mp-navbar">
        <n-button quaternary circle aria-label="切换导航" :aria-expanded="isMobile ? mobileMenuOpen : !collapsed" @click="isMobile ? mobileMenuOpen = !mobileMenuOpen : collapsed = !collapsed"><template #icon><n-icon :component="MenuOutline" /></template></n-button>
        <div class="mp-navbar-center">
          <button v-if="authStore.isAdmin && taskStatus?.is_running" class="mp-running-task" aria-label="查看运行任务日志" :title="taskStatus.message" @click="notificationOpen = true">
            <span class="mp-running-task-title"><strong>{{ taskStatus.current_action }}</strong><span>{{ taskStatus.progress_known ? `${taskStatus.progress}%` : (taskStatus.phase || '执行中') }}</span></span>
            <span class="mp-running-task-track" role="progressbar" :aria-label="taskStatus.current_action" aria-valuemin="0" aria-valuemax="100" :aria-valuenow="taskStatus.progress_known ? taskStatus.progress : undefined" :class="{ indeterminate: !taskStatus.progress_known }"><span :style="{ width: taskStatus.progress_known ? `${taskStatus.progress}%` : '35%' }" /></span>
          </button>
        </div>
        <div class="mp-navbar-tools">
          <button v-if="authStore.isAdmin" class="mp-task-trigger" :class="{ running: taskStatus?.is_running }" aria-label="通知中心" title="通知中心" @click="notificationOpen = true">
            <n-icon :component="NotificationsOutline" :size="19" /><span v-if="taskStatus?.is_running" class="mp-notification-dot" />
          </button>
          <n-dropdown trigger="click" :options="themeOptions" :value="themeMode" @select="emit('update:theme-mode', $event)"><n-button quaternary circle aria-label="选择主题" :title="`主题：${themeOptions.find(option => option.key === themeMode)?.label || '跟随系统'}`"><template #icon><n-icon :component="PaletteIcon" /></template></n-button></n-dropdown>
          <n-dropdown v-if="authStore.isLoggedIn" trigger="click" :options="userOptions" @select="handleUserSelect"><button class="mp-user-avatar" :aria-label="`${authStore.username} 的用户菜单`"><img v-if="authStore.avatarUrl && !avatarLoadFailed" :src="authStore.avatarUrl" alt="" @error="avatarLoadFailed = true" /><template v-else>{{ (authStore.username || 'U').slice(0, 1).toUpperCase() }}</template><span /></button></n-dropdown>
        </div>
      </header>
      <main class="mp-page-content" :class="{ 'mp-page-content-media': ['MediaCenter', 'MediaCenterLibrary'].includes(route.name) }"><router-view v-slot="slotProps"><component :is="slotProps.Component" :task-status="props.taskStatus" /></router-view></main>
    </div>
    <n-drawer v-model:show="mobileMenuOpen" placement="left" :width="280"><n-drawer-content :native-scrollbar="false"><div class="mp-brand"><img src="./assets/logo.png" alt="Emby Toolkit" /><strong>Emby Toolkit</strong></div><n-menu :options="menuOptions" :value="activeMenuKey" :default-expanded-keys="defaultExpandedKeys" @update:value="handleMenuUpdate" /></n-drawer-content></n-drawer>
    <n-drawer v-model:show="notificationOpen" placement="right" :width="isMobile || notificationExpanded ? '100vw' : 'min(1000px, 92vw)'">
      <n-drawer-content body-content-style="height: 100%; padding: 20px; box-sizing: border-box; overflow: hidden;">
        <NotificationCenter v-if="notificationOpen" :task="taskStatus" :expanded="notificationExpanded" :mobile="isMobile" @expand="notificationExpanded = !notificationExpanded" @close="notificationOpen = false" />
      </n-drawer-content>
    </n-drawer>

    <!-- ★★★ 自定义菜单编辑器模态框 ★★★ -->
    <n-modal v-model:show="isMenuEditorVisible" preset="card" class="menu-editor-modal" style="width: min(650px, calc(100vw - 32px));" title="自定义侧边栏菜单 (全局生效)">
      <div class="menu-editor-list">

        <div v-for="(group, gIndex) in menuConfigTree" :key="group.key" class="menu-editor-group">
          <div class="menu-editor-group-heading">
            <n-switch v-model:value="group.visible" size="small" :aria-label="`显示 ${getOriginalLabel(group.key)} 菜单组`" />
            <n-input v-model:value="group.label" :placeholder="getOriginalLabel(group.key)" :input-props="{ 'aria-label': `${getOriginalLabel(group.key)} 菜单组名称` }" size="small" class="menu-editor-group-name" />
            <span class="menu-editor-hint">一级菜单</span>

            <div class="menu-editor-expanded">
              <span>默认展开</span>
              <n-switch v-model:value="group.expanded" size="small" :aria-label="`默认展开 ${getOriginalLabel(group.key)}`" />
            </div>
          </div>

          <div
            class="menu-editor-children"
            @dragover.prevent="onDragOverEmpty($event, gIndex)"
            @drop="onDropEmpty($event, gIndex)"
          >
            <div
              v-for="(child, cIndex) in group.children"
              :key="child.key"
              class="draggable-item"
              :class="{
                'drag-over-top': dragTarget?.gIndex === gIndex && dragTarget?.cIndex === cIndex && dragPosition === 'top',
                'drag-over-bottom': dragTarget?.gIndex === gIndex && dragTarget?.cIndex === cIndex && dragPosition === 'bottom',
                'is-dragging': dragSource?.gIndex === gIndex && dragSource?.cIndex === cIndex
              }"
              draggable="true"
              @dragstart="onDragStart($event, gIndex, cIndex)"
              @dragover.prevent="onDragOver($event, gIndex, cIndex)"
              @drop="onDrop($event, gIndex, cIndex)"
              @dragend="onDragEnd"
            >
              <n-icon size="18" class="menu-editor-drag-handle"><MenuOutline /></n-icon>
              <n-switch v-model:value="child.visible" size="small" :aria-label="`显示 ${getOriginalLabel(child.key)}`" />
              <n-input v-model:value="child.label" :placeholder="getOriginalLabel(child.key)" :input-props="{ 'aria-label': `${getOriginalLabel(child.key)} 菜单名称` }" size="small" class="menu-editor-child-name" />
            </div>

            <div v-if="group.children.length === 0" class="empty-group-dropzone">
              拖拽菜单至此处
            </div>
          </div>
        </div>

      </div>
      <template #footer>
        <div class="menu-editor-footer">
          <span class="menu-editor-hint">
            提示：按住左侧图标可拖拽排序，支持跨组拖拽。<br/>
            <span>关闭一级菜单显示，可将其下开启的二级菜单自动提升为一级。</span>
          </span>
          <n-space>
            <n-popconfirm @positive-click="resetMenuConfig" negative-text="取消" positive-text="确定">
              <template #trigger>
                <n-button type="error" ghost :loading="isSavingMenu">恢复默认</n-button>
              </template>
              确定要恢复所有菜单的默认名称、顺序和显示状态吗？
            </n-popconfirm>
            <n-button type="primary" @click="saveMenuConfig" :loading="isSavingMenu">保存并应用</n-button>
          </n-space>
        </div>
      </template>
    </n-modal>

  </div>
</template>

<script setup>
import { ref, computed, h, watch, nextTick, onMounted, onUnmounted } from 'vue';
import { useRouter, useRoute } from 'vue-router';
import {
  NDrawer, NDrawerContent,
  NMenu, NSwitch, NIcon, NModal, NDropdown, NButton,
  useMessage, useDialog, NInput, NSpace, NPopconfirm
} from 'naive-ui';
import { useAuthStore } from './stores/auth';
import NotificationCenter from './components/NotificationCenter.vue';
import { themeOptions } from './theme.js';
import {
  ListOutline as ReviewListIcon,
  TimerOutline as SchedulerIcon,
  OptionsOutline as GeneralIcon,
  LogOutOutline as LogoutIcon,
  ColorPaletteOutline as PaletteIcon,
  NotificationsOutline,
  SyncOutline as RestartIcon,
  PersonCircleOutline as UserCenterIcon,
  FilmOutline as MediaCenterIcon,
  CompassOutline,
  LibraryOutline,
  SettingsOutline,
  MenuOutline,
  ListCircleOutline as MenuEditIcon,
} from '@vicons/ionicons5';
import axios from 'axios';

const message = useMessage();
const dialog = useDialog();

const isMobile = ref(window.innerWidth < 960);
const collapsed = ref(false);
const mobileMenuOpen = ref(false);
const notificationOpen = ref(false);
const notificationExpanded = ref(false);

const checkMobile = () => {
  isMobile.value = window.innerWidth < 960;
};

onMounted(() => {
  checkMobile();
  window.addEventListener('resize', checkMobile);
  initMenuConfig();
});

onUnmounted(() => window.removeEventListener('resize', checkMobile));

const props = defineProps({
  themeMode: String,
  taskStatus: Object
});
const emit = defineEmits(['update:theme-mode']);

const router = useRouter();
const route = useRoute();
const authStore = useAuthStore();
const avatarLoadFailed = ref(false);
watch(() => authStore.avatarUrl, () => { avatarLoadFailed.value = false; });
watch(() => authStore.isLoggedIn ? authStore.user?.id : null, async (id) => {
  if (id) {
    try { await authStore.fetchAccountInfo(); } catch { /* 保留默认头像，账户页提供错误提示。 */ }
  }
}, { immediate: true });
watch(() => route.fullPath, () => {
  mobileMenuOpen.value = false;
  nextTick(() => document.querySelector('.mp-page-content')?.scrollTo({ top: 0 }));
});

const activeMenuKey = computed(() => route.name === 'MediaCenterLibrary' ? 'MediaCenter' : route.name);
const renderIcon = (iconComponent) => () => h(NIcon, null, { default: () => h(iconComponent) });

const userOptions = computed(() => {
  const options = [];
  options.push({ label: '用户中心', key: 'user-center', icon: renderIcon(UserCenterIcon) });

  if (authStore.isAdmin) {
    options.push({
      label: '自定义菜单',
      key: 'edit-menu',
      icon: renderIcon(MenuEditIcon)
    });

    options.push({
      label: '重启容器',
      key: 'restart-container',
      icon: renderIcon(RestartIcon)
    });
  }

  if (options.length > 0) {
    options.push({ type: 'divider', key: 'd1' });
  }

  options.push({
    label: '退出登录',
    key: 'logout',
    icon: renderIcon(LogoutIcon)
  });

  return options;
});

const triggerRestart = async () => {
  message.info('正在发送重启指令...');
  try {
    await axios.post('/api/system/restart');
    message.success('重启指令已发送，应用正在后台重启。请稍后手动刷新页面。', { duration: 10000 });
  } catch (error) {
    if (error.response) {
      message.error(error.response.data.error || '发送重启请求失败，请查看日志。');
    } else {
      message.success('重启指令已发送，应用正在后台重启。请稍后手动刷新页面。', { duration: 10000 });
    }
  }
};

const handleUserSelect = async (key) => {
  if (key === 'user-center') {
    router.push({ name: 'UserCenter' });
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
const isMenuReady = ref(false); // 控制 n-menu 渲染时机
const menuConfigTree = ref([]); // 树形配置数据

// 1. 基础菜单（包含所有权限允许的菜单，带有 defaultLabel 用于重置）
const baseMenuOptions = computed(() => {
  const discoveryGroup = {
    label: '发现', defaultLabel: '发现', key: 'group-discovery', icon: renderIcon(CompassOutline), children: []
  };

  if (authStore.isLoggedIn) {
    discoveryGroup.children.push({ label: '用户中心', defaultLabel: '用户中心', key: 'UserCenter', icon: renderIcon(UserCenterIcon) });
    if (authStore.isAdmin) {
      discoveryGroup.children.push({ label: '媒体中心', defaultLabel: '媒体中心', key: 'MediaCenter', icon: renderIcon(MediaCenterIcon) });
    }
  }

  const finalMenu = [discoveryGroup];

  if (authStore.isAdmin) {
    finalMenu.push(
      {
        label: '整理', defaultLabel: '整理', key: 'group-management', icon: renderIcon(LibraryOutline),
        children: [
          { label: '手动处理', defaultLabel: '手动处理', key: 'ReviewList', icon: renderIcon(ReviewListIcon) },
        ]
      },
      {
        label: '系统', defaultLabel: '系统', key: 'group-system', icon: renderIcon(SettingsOutline),
        children: [
          { label: '通用设置', defaultLabel: '通用设置', key: 'settings-general', icon: renderIcon(GeneralIcon) },
          { label: '任务中心', defaultLabel: '任务中心', key: 'settings-scheduler', icon: renderIcon(SchedulerIcon) },
          { label: '封面生成', defaultLabel: '封面生成', key: 'CoverGeneratorConfig', icon: renderIcon(PaletteIcon) },
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
          const baseChild = baseMenuMap.value.get(savedChild.key);
          if (!baseChild || baseChild.isGroup) return; // 废弃的子项
          usedKeys.add(savedChild.key);
          newGroup.children.push({
            key: savedChild.key,
            label: savedChild.label || baseChild.defaultLabel,
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
        children: baseGroup.children ? baseGroup.children.map(c => ({
          key: c.key,
          label: c.defaultLabel,
          visible: true
        })) : []
      });
    } else {
      // 组存在，检查是否有新的子项
      const targetGroup = resultTree.find(g => g.key === baseGroup.key);
      if (baseGroup.children) {
        baseGroup.children.forEach(baseChild => {
          if (!usedKeys.has(baseChild.key)) {
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
  isMenuReady.value = false;
  try {
    const response = await axios.get('/api/system/menu_config');
    const saved = response.data || [];
    menuConfigTree.value = syncMenuConfig(saved);
  } catch (error) {
    console.error("获取菜单配置失败", error);
    menuConfigTree.value = syncMenuConfig([]); // 失败时回退到默认
  } finally {
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
  isSavingMenu.value = true;
  try {
    await axios.post('/api/system/menu_config/reset');
    menuConfigTree.value = syncMenuConfig([]); // 传入空数组生成默认树
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

    if (group.visible) {
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

// ==================================================

function handleMenuUpdate(key) {
  mobileMenuOpen.value = false;
  router.push({ name: key });
}

</script>

<style>
/* ... (保留你原有的 style 代码不变) ... */

/* ★★★ 拖拽相关样式 ★★★ */
.draggable-item {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 8px;
  padding: 8px;
  border-radius: 8px;
  transition: background-color 0.2s;
  border: 1px solid var(--ui-border);
  background:var(--ui-surface);
  min-width:0;
}

.draggable-item:hover {
  background-color:var(--ui-surface-muted);
}

/* 正在被拖拽的元素半透明 */
.draggable-item.is-dragging {
  opacity: 0.4;
}

/* 拖拽目标指示线 */
.drag-over-top {
  border-top: 2px solid var(--accent-color) !important;
}

.drag-over-bottom {
  border-bottom: 2px solid var(--accent-color) !important;
}

/* 空组的拖拽区域 */
.empty-group-dropzone {
  height: 32px;
  border: 1px dashed var(--ui-border-strong);
  border-radius: 4px;
  display: flex;
  align-items: center;
  justify-content: center;
  color:var(--ui-text-muted);
  font-size: 12px;
  margin-bottom: 8px;
}
.menu-editor-modal { container-type:inline-size; }
.menu-editor-list { max-height:60dvh; overflow-y:auto; padding-right:4px; }
.menu-editor-group { margin-bottom:20px; }
.menu-editor-group-heading { display:flex; align-items:center; flex-wrap:wrap; gap:10px; margin-bottom:8px; padding:12px; border:1px solid var(--ui-border); border-radius:8px; background:var(--ui-surface-muted); }
.menu-editor-group-name { flex:1 1 150px; min-width:0; }
.menu-editor-hint { color:var(--ui-text-muted); font-size:12px; line-height:1.6; }
.menu-editor-expanded { display:flex; align-items:center; gap:8px; color:var(--ui-text-muted); font-size:12px; }
.menu-editor-children { padding-left:20px; min-height:10px; }
.menu-editor-drag-handle { flex-shrink:0; cursor:grab; color:var(--ui-text-muted); }
.menu-editor-child-name { flex:1; min-width:0; }
.menu-editor-footer { display:flex; flex-wrap:wrap; justify-content:space-between; gap:16px; }
.menu-editor-footer > .menu-editor-hint { flex:1 1 300px; }
@container (max-width:460px) { .menu-editor-children { padding-left:8px; } .menu-editor-group-heading { padding:8px; } .menu-editor-group-heading > .menu-editor-hint { display:none; } .menu-editor-expanded { flex-basis:100%; justify-content:flex-end; } .menu-editor-footer > .n-space { margin-left:auto; } }
</style>
