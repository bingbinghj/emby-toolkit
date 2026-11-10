<template>
  <div class="mp-page user-center-page">
    <div class="d-flex flex-wrap align-center justify-space-between ga-3 mb-6">
      <div>
        <h1 class="text-h4 mb-1">用户管理</h1>
        <p class="text-body-2 text-medium-emphasis mb-0">查看 Emby 用户，管理 ETK 备注和通知资料</p>
      </div>
      <VChip :color="authStore.isAdmin ? 'primary' : 'secondary'" variant="tonal" prepend-icon="mdi-account-circle-outline">
        {{ authStore.isAdmin ? '管理员' : '普通用户' }}
      </VChip>
    </div>

    <VRow>
      <VCol cols="12" lg="5">
        <VCard class="account-card h-100">
          <VCardItem title="账户资料" prepend-icon="mdi-account-outline">
            <template #append><VProgressCircular v-if="loading" indeterminate size="20" width="2" /></template>
          </VCardItem>
          <VCardText v-if="accountInfo">
            <div class="profile-avatar-section mb-6 pb-6">
              <VTooltip text="点击更换头像" location="top">
                <template #activator="{ props }">
                  <button
                    v-bind="props"
                    type="button"
                    class="avatar-wrapper"
                    aria-label="更换头像"
                    :disabled="isUploadingAvatar || isDeletingAvatar"
                    @click="triggerFileUpload"
                  >
                    <VAvatar :size="isMobile ? 72 : 96" color="primary" variant="tonal">
                      <VImg v-if="avatarUrl" :src="avatarUrl" :alt="`${authStore.username} 的头像`" cover />
                      <span v-else class="text-h4">{{ authStore.username ? authStore.username.charAt(0).toUpperCase() : 'U' }}</span>
                    </VAvatar>
                    <span class="avatar-overlay"><VIcon icon="mdi-camera-outline" size="24" /></span>
                  </button>
                </template>
              </VTooltip>
              <input ref="fileInput" type="file" hidden accept="image/png, image/jpeg, image/jpg" @change="handleAvatarChange" />
              <div class="profile-copy">
                <h2 class="text-h6 mb-1">{{ accountInfo.name || authStore.username }}</h2>
                <p class="text-caption text-medium-emphasis mb-3">{{ isUploadingAvatar ? '正在上传头像…' : '点击头像更换 · JPG / PNG，最大 5 MB' }}</p>
                <VBtn
                  v-if="avatarUrl"
                  size="small"
                  color="error"
                  variant="tonal"
                  prepend-icon="mdi-delete-outline"
                  :loading="isDeletingAvatar"
                  :disabled="isUploadingAvatar"
                  @click="deleteAvatar"
                >删除头像</VBtn>
              </div>
            </div>
            <VList class="account-details" bg-color="transparent" density="compact">
              <VListItem title="账户状态" prepend-icon="mdi-shield-check-outline">
                <template #append><VChip :color="statusType" size="small" variant="tonal">{{ statusText }}</VChip></template>
              </VListItem>
              <VListItem title="注册时间" prepend-icon="mdi-calendar-outline">
                <template #append><span class="text-body-2">{{ formatDateTime(accountInfo.registration_date) }}</span></template>
              </VListItem>
              <VListItem title="到期时间" prepend-icon="mdi-calendar-check-outline">
                <template #append><span class="text-body-2">{{ accountInfo.expiration_date ? formatDateTime(accountInfo.expiration_date) : '永久有效' }}</span></template>
              </VListItem>
              <VListItem title="账户等级" prepend-icon="mdi-account-key-outline">
                <template #append><span class="text-body-2 font-weight-medium">{{ authStore.isAdmin ? '管理员' : '普通用户' }}</span></template>
              </VListItem>
              <VListItem title="等级说明" :subtitle="authStore.isAdmin ? '拥有系统所有管理权限' : '可查看账户详情并接收通知'" prepend-icon="mdi-information-outline" />
            </VList>
          </VCardText>
          <VCardText v-else-if="loading"><VSkeletonLoader type="avatar, list-item-two-line, list-item-two-line" /></VCardText>
          <VCardText v-else><VAlert type="warning" variant="tonal" text="未获取到账户信息" /></VCardText>
        </VCard>
      </VCol>

      <VCol v-if="authStore.isAdmin" cols="12" lg="7">
        <VCard class="h-100">
          <VCardItem title="Emby 用户" subtitle="用户权限与启用状态来自 Emby。" prepend-icon="mdi-account-group-outline">
            <template #append><VBtn icon="mdi-refresh" variant="text" :loading="loadingUsers" aria-label="刷新用户列表" @click="loadUsers" /></template>
          </VCardItem>
          <VCardText>
            <VAlert v-if="usersError" type="error" variant="tonal" class="mb-4">{{ usersError }}</VAlert>
            <VTextField v-model="userSearch" label="搜索用户" prepend-inner-icon="mdi-magnify" clearable hide-details class="mb-4" />
            <VDataTable :headers="userHeaders" :items="users" :search="userSearch || ''" :loading="loadingUsers" item-value="id" no-data-text="暂无 Emby 用户" items-per-page-text="每页用户数">
              <template #item.name="{ item }">
                <div class="d-flex align-center ga-3 py-2">
                  <VAvatar size="36" color="primary" variant="tonal"><VImg v-if="item.profile_image_tag" :src="userAvatar(item)" :alt="item.name" /><span v-else>{{ String(item.name || '?').charAt(0) }}</span></VAvatar>
                  <div><div class="text-body-2 font-weight-medium">{{ item.name }}</div><div v-if="item.notes" class="text-caption text-medium-emphasis">{{ item.notes }}</div></div>
                </div>
              </template>
              <template #item.is_administrator="{ item }"><VChip size="small" :color="item.is_administrator ? 'primary' : 'secondary'">{{ item.is_administrator ? '管理员' : '普通用户' }}</VChip></template>
              <template #item.is_disabled="{ item }"><VChip size="small" :color="item.is_disabled ? 'error' : 'success'">{{ item.is_disabled ? '已禁用' : '正常' }}</VChip></template>
              <template #item.last_activity_date="{ item }">{{ formatDateTime(item.last_activity_date) }}</template>
              <template #item.actions="{ item }"><VBtn size="small" variant="text" prepend-icon="mdi-pencil-outline" :aria-label="`编辑 ${item.name} 的ETK资料`" @click="editUser(item)">编辑资料</VBtn></template>
            </VDataTable>
          </VCardText>
        </VCard>
      </VCol>
    </VRow>
    <VDialog v-model="userDialogOpen" max-width="560" :persistent="isSavingUser">
      <VCard v-if="editingUser">
        <VCardTitle>编辑 {{ editingUser.name }} 的 ETK 资料</VCardTitle>
        <VCardText>
          <VAlert type="info" variant="tonal" class="mb-4">Emby 用户权限和启用状态请在 Emby 中管理。</VAlert>
          <VAlert v-if="userSaveError" type="error" variant="tonal" class="mb-4">{{ userSaveError }}</VAlert>
          <VTextField v-model="editingUser.telegram_chat_id" label="Telegram 用户ID（私聊ID）" :disabled="isSavingUser" maxlength="128" class="mb-4" />
          <VTextarea v-model="editingUser.notes" label="备注" :disabled="isSavingUser" maxlength="2000" rows="3" />
        </VCardText>
        <VCardActions><VSpacer /><VBtn variant="text" :disabled="isSavingUser" @click="userDialogOpen = false">取消</VBtn><VBtn prepend-icon="mdi-content-save-outline" :loading="isSavingUser" @click="saveUserDetails">保存用户资料</VBtn></VCardActions>
      </VCard>
    </VDialog>
  </div>
</template>

<script setup>
import { ref, onMounted, onUnmounted, computed } from 'vue';
import axios from 'axios';
import { useAuthStore } from '../stores/auth';
import { useMessage } from '@/composables/useFeedback';

const authStore = useAuthStore();
const message = useMessage();

const loading = ref(true);
const accountInfo = computed(() => authStore.accountInfo);
const isUploadingAvatar = ref(false);
const isDeletingAvatar = ref(false);
const fileInput = ref(null);
const isMobile = ref(false);
let accountPageActive = true;
const isCurrentAccount = user => accountPageActive && authStore.isLoggedIn && authStore.user === user;

const checkMobile = () => {
  isMobile.value = window.innerWidth < 768;
};

const avatarUrl = computed(() => authStore.avatarUrl);

const statusMap = {
  active: { text: '正常', type: 'success' },
  pending: { text: '待审批', type: 'warning' },
  expired: { text: '已过期', type: 'error' },
  disabled: { text: '已禁用', type: 'error' },
};

const statusText = computed(() => statusMap[accountInfo.value?.status]?.text || '未知');
const statusType = computed(() => statusMap[accountInfo.value?.status]?.type || 'secondary');

const formatDateTime = (value) => {
  return value ? new Date(value).toLocaleDateString('zh-CN') : '—';
};

const triggerFileUpload = () => {
  fileInput.value?.click();
};

const handleAvatarChange = async (event) => {
  const file = event.target.files[0];
  if (!file || isUploadingAvatar.value || isDeletingAvatar.value) return;
  if (!['image/jpeg', 'image/png', 'image/jpg'].includes(file.type)) {
    message.error('只支持 JPG/PNG 格式的图片');
    event.target.value = '';
    return;
  }
  if (file.size > 5 * 1024 * 1024) {
    message.error('图片大小不能超过 5 MB');
    event.target.value = '';
    return;
  }

  const formData = new FormData();
  formData.append('avatar', file);
  const loadingMsg = message.loading('正在上传头像...', { duration: 0 });
  isUploadingAvatar.value = true;
  const requestedUser = authStore.user;

  try {
    const res = await axios.post('/api/portal/upload-avatar', formData, {
      headers: { 'Content-Type': 'multipart/form-data' }
    });
    if (!isCurrentAccount(requestedUser)) return;
    message.success('头像更新成功');
    if (accountInfo.value && res.data.new_tag) {
      accountInfo.value.profile_image_tag = res.data.new_tag;
    }
  } catch (error) {
    if (isCurrentAccount(requestedUser)) message.error(error.response?.data?.message || '上传失败');
  } finally {
    loadingMsg.destroy();
    isUploadingAvatar.value = false;
    event.target.value = '';
  }
};

const deleteAvatar = async () => {
  if (isUploadingAvatar.value || isDeletingAvatar.value) return;
  isDeletingAvatar.value = true;
  const requestedUser = authStore.user;
  try {
    await axios.delete('/api/portal/upload-avatar');
    if (!isCurrentAccount(requestedUser)) return;
    if (accountInfo.value) accountInfo.value.profile_image_tag = null;
    message.success('头像已删除');
  } catch (error) {
    if (isCurrentAccount(requestedUser)) message.error(error.response?.data?.message || error.response?.data?.error || '删除头像失败');
  } finally {
    isDeletingAvatar.value = false;
  }
};

const users = ref([]);
const userSearch = ref('');
const loadingUsers = ref(false);
const usersError = ref('');
const userDialogOpen = ref(false);
const editingUser = ref(null);
const isSavingUser = ref(false);
const userSaveError = ref('');
let usersSequence = 0;
const userHeaders = [
  { title: '用户', key: 'name' }, { title: '角色', key: 'is_administrator' },
  { title: '状态', key: 'is_disabled' }, { title: '最近活动', key: 'last_activity_date' },
  { title: '操作', key: 'actions', sortable: false },
];
const userAvatar = (item) => `/image_proxy/Users/${encodeURIComponent(item.id)}/Images/Primary?tag=${encodeURIComponent(item.profile_image_tag)}`;
const loadUsers = async () => {
  const sequence = ++usersSequence;
  const requestedUser = authStore.user;
  loadingUsers.value = true;
  usersError.value = '';
  try {
    const { data } = await axios.get('/api/portal/users');
    if (!isCurrentAccount(requestedUser) || sequence !== usersSequence) return;
    if (!Array.isArray(data.users)) throw new Error('用户列表格式无效');
    users.value = data.users;
  } catch (error) {
    if (isCurrentAccount(requestedUser) && sequence === usersSequence) usersError.value = error.response?.data?.error || error.message || '用户列表读取失败';
  } finally {
    if (sequence === usersSequence) loadingUsers.value = false;
  }
};
const editUser = (item) => {
  if (isSavingUser.value) return;
  editingUser.value = { id: item.id, name: item.name, notes: item.notes || '', telegram_chat_id: item.telegram_chat_id || '' };
  userSaveError.value = '';
  userDialogOpen.value = true;
};
const saveUserDetails = async () => {
  if (isSavingUser.value || !editingUser.value) return;
  const requestedUser = authStore.user;
  const submitted = { ...editingUser.value };
  isSavingUser.value = true;
  userSaveError.value = '';
  try {
    const { data } = await axios.post(`/api/portal/users/${encodeURIComponent(submitted.id)}`, { notes: submitted.notes, telegram_chat_id: submitted.telegram_chat_id });
    if (!isCurrentAccount(requestedUser) || editingUser.value?.id !== submitted.id) return;
    if (data.id !== submitted.id) throw new Error('用户资料保存尚未确认');
    const item = users.value.find(user => user.id === submitted.id);
    if (item) Object.assign(item, { notes: submitted.notes.trim(), telegram_chat_id: submitted.telegram_chat_id.trim() });
    userDialogOpen.value = false;
    message.success(data.message || '用户资料已保存');
  } catch (error) {
    if (isCurrentAccount(requestedUser) && editingUser.value?.id === submitted.id) userSaveError.value = error.response?.data?.error || error.message || '用户资料保存失败';
  } finally {
    isSavingUser.value = false;
  }
};

onMounted(async () => {
  checkMobile();
  window.addEventListener('resize', checkMobile);
  const requestedUser = authStore.user;
  if (authStore.isAdmin) loadUsers();
  try {
    await authStore.fetchAccountInfo();
    if (!isCurrentAccount(requestedUser)) return;
  } catch (error) {
    if (isCurrentAccount(requestedUser)) message.error('加载账户信息失败');
  } finally {
    loading.value = false;
  }
});
onUnmounted(() => {
  accountPageActive = false;
  window.removeEventListener('resize', checkMobile);
});
</script>

<style scoped>
.profile-avatar-section { display:flex; align-items:center; gap:20px; border-bottom:1px solid rgba(var(--v-border-color),var(--v-border-opacity)); }
.profile-copy { min-width:0; overflow-wrap:anywhere; }
.avatar-wrapper { position:relative; display:flex; flex-shrink:0; padding:0; border:0; border-radius:50%; background:transparent; cursor:pointer; overflow:hidden; }
.avatar-wrapper:disabled { cursor:wait; }
.avatar-wrapper:focus-visible { outline:2px solid rgb(var(--v-theme-primary)); outline-offset:4px; }
.avatar-overlay { position:absolute; inset:0; display:flex; align-items:center; justify-content:center; color:white; background:rgba(0,0,0,.4); opacity:0; transition:opacity .2s ease; }
.avatar-wrapper:hover .avatar-overlay, .avatar-wrapper:focus-visible .avatar-overlay { opacity:1; }
.account-details :deep(.v-list-item) { padding-inline:0; }
.account-details :deep(.v-list-item__prepend) { margin-inline-end:8px; }
@media (max-width:540px) {
  .profile-avatar-section { gap:16px; }
  .account-details :deep(.v-list-item__append) { max-width:55%; text-align:end; }
}
</style>
