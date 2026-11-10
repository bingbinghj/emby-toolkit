<template>
  <div class="mp-page user-management-page">
    <div class="d-flex flex-wrap align-center justify-space-between ga-3 mb-4">
      <MpPageTitle title="用户管理" />
      <div class="user-toolbar d-flex align-center ga-2">
        <VTextField v-if="authStore.isAdmin" v-model="userSearch" label="搜索用户" prepend-inner-icon="mdi-magnify" clearable hide-details variant="solo" flat rounded="pill" density="compact" />
        <VBtn v-if="authStore.isAdmin" icon="mdi-refresh" variant="text" :loading="loadingUsers" aria-label="刷新用户列表" @click="loadUsers" />
      </div>
    </div>
    <VAlert v-if="!authStore.isAdmin" type="info" variant="tonal">用户管理需要管理员权限。</VAlert>
    <VAlert v-if="usersError" type="error" variant="tonal" class="mb-4">
      {{ usersError }}
      <template #append><VBtn variant="text" :loading="loadingUsers" @click="loadUsers">重试</VBtn></template>
    </VAlert>
    <VProgressLinear v-if="loadingUsers" indeterminate color="primary" class="mb-4" aria-label="加载用户" />
    <div class="user-card-grid">
      <VCard v-for="item in filteredUsers" :key="item.id" class="user-card">
        <VCardItem>
          <template #prepend>
            <VAvatar size="72" rounded="lg" color="primary" variant="tonal" class="mr-3">
              <VImg v-if="item.profile_image_tag" :src="userAvatar(item)" :alt="item.name" cover />
              <span v-else class="text-h5">{{ String(item.name || '?').charAt(0).toUpperCase() }}</span>
            </VAvatar>
          </template>
          <VCardTitle class="user-name text-h6">{{ item.name }}</VCardTitle>
          <div class="d-flex flex-wrap ga-1 mt-2">
            <VChip size="x-small" :color="item.is_administrator ? 'warning' : 'secondary'" variant="tonal">{{ item.is_administrator ? '管理员' : '普通用户' }}</VChip>
            <VChip size="x-small" :color="item.is_disabled ? 'error' : 'success'" variant="tonal">{{ item.is_disabled ? '已禁用' : '正常' }}</VChip>
            <VChip v-if="item.id === authStore.user?.id" size="x-small" variant="outlined">当前用户</VChip>
          </div>
        </VCardItem>
        <VCardText class="user-activity">
          <div><span class="text-medium-emphasis">最近活动</span><span>{{ formatDateTime(item.last_activity_date) }}</span></div>
          <div><span class="text-medium-emphasis">最近登录</span><span>{{ formatDateTime(item.last_login_date) }}</span></div>
        </VCardText>
        <VDivider />
        <VCardActions class="px-4">
          <VBtn variant="text" prepend-icon="mdi-pencil-outline" :disabled="isSavingUser || isSavingPassword" :aria-label="`编辑 ${item.name} 的 Emby 资料`" @click="editUser(item)">编辑资料</VBtn>
          <VSpacer />
          <VBtn variant="text" prepend-icon="mdi-lock-reset" :disabled="isSavingUser || isSavingPassword" :aria-label="`修改 ${item.name} 的密码`" @click="editPassword(item)">修改密码</VBtn>
        </VCardActions>
      </VCard>
    </div>
    <VCardText v-if="authStore.isAdmin && !loadingUsers && !usersError && !filteredUsers.length" class="text-center text-medium-emphasis py-12">{{ userSearch ? '没有匹配的用户' : '暂无 Emby 用户' }}</VCardText>

    <VDialog v-model="userDialogOpen" max-width="640" :persistent="isSavingUser || isUploadingAvatar || isDeletingAvatar">
      <VCard v-if="editingUser">
        <VCardTitle class="d-flex align-center justify-space-between">
          <span>编辑用户</span>
          <VBtn icon="mdi-close" variant="text" :disabled="isSavingUser || isUploadingAvatar || isDeletingAvatar" aria-label="关闭编辑用户" @click="userDialogOpen = false" />
        </VCardTitle>
        <VCardText>
          <div v-if="editingUser.id === authStore.user?.id" class="d-flex align-center ga-3 mb-4">
            <VAvatar size="64" rounded="lg" color="primary" variant="tonal">
              <VImg v-if="editingUser.profile_image_tag" :src="userAvatar(editingUser)" :alt="editingUser.name" cover />
              <span v-else>{{ String(editingUser.name || '?').charAt(0).toUpperCase() }}</span>
            </VAvatar>
            <VBtn variant="tonal" prepend-icon="mdi-camera-outline" :loading="isUploadingAvatar" :disabled="isSavingUser || isDeletingAvatar" @click="fileInput?.click()">更换头像</VBtn>
            <VBtn v-if="editingUser.profile_image_tag" variant="text" color="error" :loading="isDeletingAvatar" :disabled="isSavingUser || isUploadingAvatar" @click="deleteAvatar">删除头像</VBtn>
            <input ref="fileInput" type="file" hidden accept="image/png,image/jpeg" @change="handleAvatarChange" />
          </div>
          <VAlert v-if="userSaveError" type="error" variant="tonal" class="mb-4">{{ userSaveError }}</VAlert>
          <VTextField v-model="editingUser.name" label="用户名" :disabled="isSavingUser" maxlength="128" class="mb-4" />
          <VRow>
            <VCol cols="12" sm="6">
              <VSelect v-model="editingUser.is_administrator" label="用户角色" :items="roleOptions" @update:model-value="value => { if (value) editingUser.is_disabled = false; }" :disabled="isSavingUser || editingUser.id === authStore.user?.id" />
            </VCol>
            <VCol cols="12" sm="6">
              <VSelect v-model="editingUser.is_disabled" label="用户状态" :items="statusOptions" :disabled="isSavingUser || editingUser.id === authStore.user?.id || editingUser.original_administrator || editingUser.is_administrator" />
            </VCol>
          </VRow>
          <p v-if="editingUser.id === authStore.user?.id" class="text-caption text-medium-emphasis mb-0">当前登录用户需保留管理员权限和启用状态。</p>
          <p v-else-if="editingUser.original_administrator" class="text-caption text-medium-emphasis mb-0">如需禁用管理员，请先保存普通用户角色，再修改状态。</p>
        </VCardText>
        <VCardActions class="px-6 pb-4"><VSpacer /><VBtn variant="text" :disabled="isSavingUser || isUploadingAvatar || isDeletingAvatar" @click="userDialogOpen = false">取消</VBtn><VBtn color="primary" variant="tonal" :loading="isSavingUser" :disabled="isSavingUser || isUploadingAvatar || isDeletingAvatar" @click="saveUserDetails">保存用户资料</VBtn></VCardActions>
      </VCard>
    </VDialog>

    <VDialog v-model="passwordDialogOpen" max-width="560" :persistent="isSavingPassword" @update:model-value="clearPasswordDraft">
      <VCard v-if="passwordUser">
        <VCardTitle>修改 {{ passwordUser.name }} 的密码</VCardTitle>
        <VCardText>
          <VAlert v-if="passwordSaveError" type="error" variant="tonal" class="mb-4">{{ passwordSaveError }}</VAlert>
          <VTextField v-model="newPassword" label="新密码" :type="showPassword ? 'text' : 'password'" :append-inner-icon="showPassword ? 'mdi-eye-off-outline' : 'mdi-eye-outline'" :disabled="isSavingPassword" autocomplete="new-password" maxlength="1024" class="mb-4" @click:append-inner="showPassword = !showPassword" />
          <VTextField v-model="confirmPassword" label="确认新密码" type="password" :disabled="isSavingPassword" autocomplete="new-password" maxlength="1024" />
          <p class="text-caption text-medium-emphasis mb-0">保存后，该用户已登录的 Emby 客户端需要重新登录。</p>
        </VCardText>
        <VCardActions class="px-6 pb-4"><VSpacer /><VBtn variant="text" :disabled="isSavingPassword" @click="closePasswordDialog">取消</VBtn><VBtn color="primary" variant="tonal" :loading="isSavingPassword" :disabled="isSavingPassword" @click="saveUserPassword">保存新密码</VBtn></VCardActions>
      </VCard>
    </VDialog>
  </div>
</template>

<script setup>
import { ref, onMounted, onUnmounted, computed } from 'vue';
import axios from 'axios';
import { useAuthStore } from '../stores/auth';
import MpPageTitle from './MpPageTitle.vue';
import { useMessage } from '@/composables/useFeedback';

const authStore = useAuthStore();
const message = useMessage();
let accountPageActive = true;
const isCurrentAccount = user => accountPageActive && authStore.isLoggedIn && authStore.user === user;
const users = ref([]);
const userSearch = ref('');
const loadingUsers = ref(false);
const usersError = ref('');
const userDialogOpen = ref(false);
const editingUser = ref(null);
const isSavingUser = ref(false);
const userSaveError = ref('');
let usersSequence = 0;
const roleOptions = [{ title: '普通用户', value: false }, { title: '管理员', value: true }];
const statusOptions = [{ title: '正常', value: false }, { title: '已禁用', value: true }];
const filteredUsers = computed(() => {
  const search = String(userSearch.value || '').trim().toLocaleLowerCase();
  return search ? users.value.filter(item => String(item.name).toLocaleLowerCase().includes(search)) : users.value;
});
const userAvatar = item => `/image_proxy/Users/${encodeURIComponent(item.id)}/Images/Primary?tag=${encodeURIComponent(item.profile_image_tag)}`;
const formatDateTime = value => {
  if (!value) return '—';
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? '—' : date.toLocaleString('zh-CN', { hour12: false });
};
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
const editUser = item => {
  if (isSavingUser.value || isSavingPassword.value) return;
  editingUser.value = { id: item.id, name: item.name, is_administrator: item.is_administrator, is_disabled: item.is_disabled, original_administrator: item.is_administrator, profile_image_tag: item.profile_image_tag };
  userSaveError.value = '';
  userDialogOpen.value = true;
};
const saveUserDetails = async () => {
  if (isSavingUser.value || isUploadingAvatar.value || isDeletingAvatar.value || !editingUser.value) return;
  const requestedUser = authStore.user;
  const submitted = { ...editingUser.value };
  if (!String(submitted.name || '').trim()) { userSaveError.value = '请输入用户名'; return; }
  isSavingUser.value = true;
  userSaveError.value = '';
  try {
    const { data } = await axios.post(`/api/portal/users/${encodeURIComponent(submitted.id)}`, {
      name: submitted.name.trim(), is_administrator: submitted.is_administrator, is_disabled: submitted.is_disabled,
    });
    if (!isCurrentAccount(requestedUser) || editingUser.value?.id !== submitted.id) return;
    if (data.id !== submitted.id || data.user?.id !== submitted.id || data.user.name !== submitted.name.trim()
      || data.user.is_administrator !== submitted.is_administrator || data.user.is_disabled !== submitted.is_disabled) throw new Error('Emby 用户资料保存尚未确认');
    const item = users.value.find(user => user.id === submitted.id);
    if (item) Object.assign(item, data.user);
    if (submitted.id === authStore.user?.id) {
      authStore.user.name = data.user.name;
      if (authStore.accountInfo) authStore.accountInfo.name = data.user.name;
    }
    userDialogOpen.value = false;
    message.success(data.message || 'Emby 用户资料已保存');
  } catch (error) {
    if (isCurrentAccount(requestedUser) && editingUser.value?.id === submitted.id) userSaveError.value = error.response?.data?.error || error.message || '用户资料保存失败';
  } finally {
    isSavingUser.value = false;
  }
};

const fileInput = ref(null);
const isUploadingAvatar = ref(false);
const isDeletingAvatar = ref(false);
const updateAvatarTag = tag => {
  const item = users.value.find(user => user.id === authStore.user?.id);
  if (item) item.profile_image_tag = tag;
  if (editingUser.value?.id === authStore.user?.id) editingUser.value.profile_image_tag = tag;
  if (authStore.accountInfo) authStore.accountInfo.profile_image_tag = tag;
};
const handleAvatarChange = async event => {
  const file = event.target.files?.[0];
  if (!file || isUploadingAvatar.value || isDeletingAvatar.value || isSavingUser.value) return;
  if (!['image/jpeg', 'image/png'].includes(file.type) || file.size > 5 * 1024 * 1024) {
    message.error('请选择不超过 5 MB 的 JPG 或 PNG 图片'); event.target.value = ''; return;
  }
  const requestedUser = authStore.user;
  isUploadingAvatar.value = true;
  const body = new FormData();
  body.append('avatar', file);
  try {
    const { data } = await axios.post('/api/portal/upload-avatar', body);
    if (!isCurrentAccount(requestedUser)) return;
    if (!data.new_tag) throw new Error('Emby 头像更新尚未确认');
    updateAvatarTag(data.new_tag);
    message.success('头像已更新');
  } catch (error) {
    if (isCurrentAccount(requestedUser)) message.error(error.response?.data?.error || error.message || '头像上传失败');
  } finally { isUploadingAvatar.value = false; event.target.value = ''; }
};
const deleteAvatar = async () => {
  if (isUploadingAvatar.value || isDeletingAvatar.value || isSavingUser.value) return;
  const requestedUser = authStore.user;
  isDeletingAvatar.value = true;
  try {
    await axios.delete('/api/portal/upload-avatar');
    if (!isCurrentAccount(requestedUser)) return;
    updateAvatarTag(null);
    message.success('头像已删除');
  } catch (error) {
    if (isCurrentAccount(requestedUser)) message.error(error.response?.data?.error || error.message || '头像删除失败');
  } finally { isDeletingAvatar.value = false; }
};

const passwordDialogOpen = ref(false);
const passwordUser = ref(null);
const newPassword = ref('');
const confirmPassword = ref('');
const showPassword = ref(false);
const isSavingPassword = ref(false);
const passwordSaveError = ref('');
const clearPasswordDraft = open => {
  if (open) return;
  newPassword.value = '';
  confirmPassword.value = '';
  showPassword.value = false;
  passwordSaveError.value = '';
};
const closePasswordDialog = () => {
  passwordDialogOpen.value = false;
  clearPasswordDraft(false);
};
const editPassword = item => {
  if (isSavingUser.value || isSavingPassword.value) return;
  clearPasswordDraft(false);
  passwordUser.value = { id: item.id, name: item.name };
  passwordDialogOpen.value = true;
};
const saveUserPassword = async () => {
  if (isSavingPassword.value || !passwordUser.value) return;
  if (!newPassword.value) { passwordSaveError.value = '请输入新密码'; return; }
  if (newPassword.value !== confirmPassword.value) { passwordSaveError.value = '两次输入的密码不一致'; return; }
  const requestedUser = authStore.user;
  const submittedId = passwordUser.value.id;
  isSavingPassword.value = true;
  passwordSaveError.value = '';
  try {
    const { data } = await axios.post(`/api/portal/users/${encodeURIComponent(submittedId)}/password`, { new_password: newPassword.value });
    if (!isCurrentAccount(requestedUser) || passwordUser.value?.id !== submittedId) return;
    if (data.id !== submittedId) throw new Error('Emby 用户密码更新尚未确认');
    closePasswordDialog();
    message.success(data.message || 'Emby 用户密码已更新');
  } catch (error) {
    if (isCurrentAccount(requestedUser) && passwordUser.value?.id === submittedId) passwordSaveError.value = error.response?.data?.error || error.message || '密码更新失败';
  } finally {
    isSavingPassword.value = false;
  }
};

onMounted(() => { if (authStore.isAdmin) loadUsers(); });
onUnmounted(() => { accountPageActive = false; clearPasswordDraft(false); });
</script>

<style scoped>
.user-toolbar { min-width: 260px; max-width: 400px; flex: 1; }
.user-card-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 288px), 1fr)); gap: 16px; }
.user-card { display: flex; flex-direction: column; min-width: 0; }
.user-name { white-space: normal; overflow-wrap: anywhere; line-height: 1.5; }
.user-activity { display: grid; gap: 8px; font-size: .875rem; flex: 1; }
.user-activity > div { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 8px; }
@media (max-width: 540px) { .user-toolbar { min-width: 100%; max-width: none; } }
</style>
