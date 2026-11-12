<template>
  <div class="mp-page user-management-page">
    <div v-if="authStore.isAdmin" class="user-toolbar d-flex flex-wrap align-center ga-3 mb-4">
      <VTextField v-model="userSearch" placeholder="搜索用户" aria-label="搜索用户" prepend-inner-icon="mdi-magnify" clearable hide-details variant="solo" flat rounded="pill" density="compact" />
      <VSelect v-model="statusFilter" :items="filterOptions" label="状态" hide-details density="compact" clearable />
    </div>
    <VAlert v-if="!authStore.isAdmin" type="info" variant="tonal">用户管理需要管理员权限。</VAlert>
    <VAlert v-if="usersError" type="error" variant="tonal" class="mb-4">
      {{ usersError }}
      <template #append><VBtn variant="text" :loading="loadingUsers" @click="loadUsers">重试</VBtn></template>
    </VAlert>
    <VProgressLinear v-if="loadingUsers" indeterminate color="primary" class="mb-4" aria-label="加载用户" />
    <div class="user-card-grid">
      <VCard v-for="item in filteredUsers" :key="item.id" class="user-card app-hover-lift-card" :class="{ 'user-card--disabled': item.is_disabled }">
        <VCardItem>
          <template #prepend>
            <div class="user-avatar-wrap mr-4">
              <VAvatar size="72" rounded="lg" color="primary" variant="tonal" :class="{ 'admin-avatar': item.is_administrator }">
                <VImg v-if="item.profile_image_tag" :src="userAvatar(item)" :alt="item.name" cover />
                <VIcon v-else icon="mdi-account" size="42" />
              </VAvatar>
              <VIcon v-if="item.is_administrator" icon="mdi-crown" color="warning" class="admin-crown" size="20" />
            </div>
          </template>
          <VCardTitle class="user-name text-h6 font-weight-bold">{{ item.name }}</VCardTitle>
          <div class="d-flex flex-wrap ga-1 mt-2">
            <VChip class="user-role-chip" size="x-small" :color="item.is_administrator ? 'error' : undefined" :variant="item.is_administrator ? 'outlined' : 'tonal'" label>{{ item.is_administrator ? '管理员' : '普通用户' }}</VChip>
            <VChip size="x-small" :color="item.is_disabled ? 'secondary' : 'success'" variant="tonal" label>{{ item.is_disabled ? '已停用' : '激活' }}</VChip>
            <VChip v-if="item.id === authStore.user?.id" size="x-small" variant="outlined" label>当前用户</VChip>
          </div>
          <template #append>
            <VBtn icon="mdi-pencil-outline" variant="text" size="small" :disabled="userBusy" :aria-label="`编辑 ${item.name} 的 Emby 资料`" @click="editUser(item)" />
          </template>
        </VCardItem>
        <VCardText class="user-activity">
          <div><span class="text-medium-emphasis">最近活动</span><span>{{ formatDateTime(item.last_activity_date) }}</span></div>
          <div><span class="text-medium-emphasis">最近登录</span><span>{{ formatDateTime(item.last_login_date) }}</span></div>
        </VCardText>
        <VDivider />
        <VCardActions class="px-4">
          <VBtn variant="text" prepend-icon="mdi-lock-reset" :disabled="userBusy" :aria-label="`修改 ${item.name} 的密码`" @click="editPassword(item)">修改密码</VBtn>
          <VSpacer />
          <VBtn v-if="item.id !== authStore.user?.id" icon="mdi-delete-outline" variant="text" color="error" :disabled="userBusy" :aria-label="`删除 ${item.name}`" @click="confirmDeleteUser(item)" />
        </VCardActions>
      </VCard>
    </div>
    <VCardText v-if="authStore.isAdmin && !loadingUsers && !usersError && !filteredUsers.length" class="text-center text-medium-emphasis py-12">{{ userSearch || statusFilter ? '没有匹配的用户' : '暂无 Emby 用户' }}</VCardText>
    <Teleport to="body">
      <div v-if="authStore.isAdmin" class="compact-fab-stack user-add-button"><VBtn icon="mdi-account-plus" color="primary" elevation="4" :disabled="userBusy" aria-label="添加用户" @click="addUser" /></div>
    </Teleport>

    <VDialog v-model="userDialogOpen" max-width="960" scrollable :fullscreen="mobile" :persistent="userBusy" @update:model-value="open => { if (!open) clearUserDraft(); }">
      <VCard>
        <VCardItem class="py-3">
          <template #prepend><VIcon icon="mdi-account" class="mr-2" /></template>
          <VCardTitle>{{ creatingUser ? '添加用户' : '编辑用户' }}</VCardTitle>
          <VCardSubtitle v-if="editingUser">{{ editingUser.name }}</VCardSubtitle>
          <template #append><VBtn icon="mdi-close" variant="text" color="default" :disabled="userBusy" aria-label="关闭编辑用户" @click="closeUserDialog" /></template>
        </VCardItem>
        <VDivider />
        <VCardText class="user-dialog-content">
          <VProgressLinear v-if="loadingUserDetails" indeterminate aria-label="读取用户资料" class="mb-4" />
          <VAlert v-if="userSaveError" type="error" variant="tonal" class="mb-4">{{ userSaveError }}</VAlert>
          <div v-if="editingUser" class="avatar-editor d-flex flex-wrap align-center ga-5 mb-5">
            <VAvatar size="100" rounded="lg" color="primary" variant="tonal">
              <VImg v-if="displayedAvatar" :src="displayedAvatar" :alt="editingUser.name" cover />
              <VIcon v-else icon="mdi-account" size="60" />
            </VAvatar>
            <div class="flex-grow-1">
              <div class="d-flex flex-wrap ga-2 mb-3">
                <VBtn color="primary" prepend-icon="mdi-cloud-upload-outline" :disabled="userBusy || loadingUserDetails" @click="fileInput?.click()">上传头像</VBtn>
                <VBtn v-if="!creatingUser" color="info" variant="tonal" prepend-icon="mdi-refresh" aria-label="取消头像修改" :disabled="userBusy || loadingUserDetails" @click="restoreCurrentAvatar">取消</VBtn>
                <VBtn color="error" variant="tonal" prepend-icon="mdi-image-sync-outline" :disabled="userBusy || loadingUserDetails" @click="resetDefaultAvatar">重置默认头像</VBtn>
              </div>
              <p class="text-body-2 mb-0">支持 JPG、PNG、GIF、WebP，最大 800 KB。</p>
              <input ref="fileInput" type="file" hidden accept="image/png,image/jpeg,image/gif,image/webp" @change="handleAvatarChange" />
            </div>
          </div>
          <VTabs v-if="editingUser && !creatingUser" v-model="userTab" class="mb-5" show-arrows>
            <VTab value="profile" prepend-icon="mdi-account-outline">基本信息</VTab>
            <VTab value="permissions" prepend-icon="mdi-account-key-outline" :disabled="loadingUserDetails">权限</VTab>
            <VTab value="playback" prepend-icon="mdi-play-circle-outline" :disabled="loadingUserDetails">播放设置</VTab>
          </VTabs>
          <div v-if="editingUser && userTab === 'profile'">
            <VRow>
              <VCol cols="12" sm="6"><VTextField v-model="editingUser.name" label="用户名" prepend-inner-icon="mdi-account-outline" :disabled="userBusy || loadingUserDetails" maxlength="128" /></VCol>
              <VCol cols="12" sm="6"><VSelect v-model="editingUser.is_administrator" label="角色" :items="roleOptions" :disabled="userBusy || loadingUserDetails || editingUser.id === authStore.user?.id" @update:model-value="value => { if (value) editingUser.is_disabled = false; }" /></VCol>
              <VCol cols="12" sm="6"><VSelect v-model="editingUser.is_disabled" label="状态" :items="statusOptions" :disabled="userBusy || loadingUserDetails || editingUser.id === authStore.user?.id || editingUser.original_administrator || editingUser.is_administrator" /></VCol>
              <VCol v-if="creatingUser" cols="12" sm="6"><VTextField v-model="createPassword" label="密码" :type="showCreatePassword ? 'text' : 'password'" :append-inner-icon="showCreatePassword ? 'mdi-eye-off-outline' : 'mdi-eye-outline'" :disabled="userBusy" autocomplete="new-password" maxlength="1024" @click:append-inner="showCreatePassword = !showCreatePassword" /></VCol>
              <VCol v-if="creatingUser" cols="12" sm="6"><VTextField v-model="createConfirmPassword" label="确认密码" type="password" :disabled="userBusy" autocomplete="new-password" maxlength="1024" /></VCol>
            </VRow>
            <p v-if="editingUser.id === authStore.user?.id" class="text-body-2 text-medium-emphasis mb-0">当前登录用户需保留管理员权限和启用状态。</p>
            <p v-else-if="editingUser.original_administrator" class="text-body-2 text-medium-emphasis mb-0">如需禁用管理员，请先保存普通用户角色，再修改状态。</p>
          </div>
          <div v-if="editingUser && userTab === 'permissions'" class="permission-sections">
            <div v-for="group in availablePermissionGroups" :key="group.title" class="permission-section">
              <div class="text-subtitle-1 font-weight-medium mb-2"><VIcon :icon="group.icon" class="mr-2" size="20" />{{ group.title }}</div>
              <VRow><VCol v-for="[key, label] in group.fields" :key="key" cols="12" sm="6" class="py-0"><VCheckbox v-model="editingUser.policy[key]" :label="label" :disabled="userBusy" hide-details density="compact" /></VCol></VRow>
            </div>
            <div v-if="typeof editingUser.policy?.EnableAllFolders === 'boolean'" class="permission-section">
              <div class="text-subtitle-1 font-weight-medium mb-2"><VIcon icon="mdi-folder-outline" class="mr-2" size="20" />媒体库访问</div>
              <VCheckbox v-model="editingUser.policy.EnableAllFolders" label="允许访问所有媒体库" :disabled="userBusy" hide-details />
              <VAlert v-if="librariesError" type="warning" variant="tonal" class="my-3">{{ librariesError }}</VAlert>
              <VSelect v-if="!editingUser.policy.EnableAllFolders" v-model="editingUser.policy.EnabledFolders" :items="libraries" item-title="name" item-value="id" label="允许访问的媒体库" multiple chips closable-chips :disabled="userBusy || !!librariesError" class="mt-3" />
            </div>
            <VAlert v-if="!availablePermissionGroups.length && typeof editingUser.policy?.EnableAllFolders !== 'boolean'" type="info" variant="tonal">Emby 未返回可编辑的权限设置。</VAlert>
          </div>
          <div v-if="editingUser && userTab === 'playback'">
            <VRow>
              <VCol v-if="typeof editingUser.configuration?.SubtitleMode === 'string'" cols="12" sm="6"><VSelect v-model="editingUser.configuration.SubtitleMode" label="字幕模式" :items="subtitleModes" :disabled="userBusy" /></VCol>
              <VCol v-for="[key, label] in availablePlaybackFields" :key="key" cols="12" sm="6" class="py-0"><VCheckbox v-model="editingUser.configuration[key]" :label="label" :disabled="userBusy" hide-details /></VCol>
            </VRow>
            <VAlert v-if="!availablePlaybackFields.length && typeof editingUser.configuration?.SubtitleMode !== 'string'" type="info" variant="tonal">Emby 未返回可编辑的播放设置。</VAlert>
          </div>
        </VCardText>
        <VDivider />
        <VCardActions class="px-6 py-4"><VSpacer /><VBtn color="default" variant="text" :disabled="userBusy" @click="closeUserDialog">取消</VBtn><VBtn color="primary" variant="flat" prepend-icon="mdi-content-save-outline" :loading="isSavingUser" :disabled="userBusy || loadingUserDetails || !editingUser" @click="saveUserDetails">{{ creatingUser ? '添加' : '保存' }}</VBtn></VCardActions>
      </VCard>
    </VDialog>

    <VDialog v-model="passwordDialogOpen" max-width="560" :persistent="isSavingPassword" @update:model-value="clearPasswordDraft">
      <VCard v-if="passwordUser">
        <VCardTitle class="password-title">修改 {{ passwordUser.name }} 的密码</VCardTitle>
        <VCardText>
          <VAlert v-if="passwordSaveError" type="error" variant="tonal" class="mb-4">{{ passwordSaveError }}</VAlert>
          <VTextField v-model="newPassword" label="新密码" :type="showPassword ? 'text' : 'password'" :append-inner-icon="showPassword ? 'mdi-eye-off-outline' : 'mdi-eye-outline'" :disabled="isSavingPassword" autocomplete="new-password" maxlength="1024" class="mb-4" @click:append-inner="showPassword = !showPassword" />
          <VTextField v-model="confirmPassword" label="确认新密码" type="password" :disabled="isSavingPassword" autocomplete="new-password" maxlength="1024" />
          <p class="text-body-2 text-medium-emphasis mb-0">保存后，该用户已登录的 Emby 客户端需要重新登录。</p>
        </VCardText>
        <VCardActions class="px-6 pb-4"><VSpacer /><VBtn color="default" variant="text" :disabled="isSavingPassword" @click="closePasswordDialog">取消</VBtn><VBtn color="primary" variant="flat" prepend-icon="mdi-content-save-outline" :loading="isSavingPassword" :disabled="isSavingPassword" @click="saveUserPassword">保存</VBtn></VCardActions>
      </VCard>
    </VDialog>
  </div>
</template>

<script setup>
import { ref, onMounted, onUnmounted, computed } from 'vue';
import { useDisplay } from 'vuetify';
import axios from 'axios';
import { useAuthStore } from '../stores/auth';
import { useContextSearch } from '@/composables/useContextSearch';
import { useMessage, useDialog } from '@/composables/useFeedback';
import { changedUserFields, userFieldsMatch, validAvatar, userPermissionGroups, userPlaybackFields, subtitleModes } from '@/utils/userManagement';

const authStore = useAuthStore();
const message = useMessage(), dialog = useDialog();
const { smAndDown: mobile } = useDisplay();
let accountPageActive = true;
const isCurrentAccount = user => accountPageActive && authStore.isLoggedIn && authStore.user === user;
const users = ref([]), userSearch = ref(''), statusFilter = ref(null);
useContextSearch({ label: '搜索用户', query: userSearch });
const loadingUsers = ref(false), usersError = ref('');
const userDialogOpen = ref(false), editingUser = ref(null), originalUser = ref(null), creatingUser = ref(false);
const loadingUserDetails = ref(false), isSavingUser = ref(false), userSaveError = ref(''), userTab = ref('profile');
const libraries = ref([]), librariesError = ref(''), isDeletingUser = ref(false);
const createPassword = ref(''), createConfirmPassword = ref(''), showCreatePassword = ref(false);
const fileInput = ref(null), avatarFile = ref(null), avatarPreview = ref(''), avatarAction = ref('keep');
const isUploadingAvatar = ref(false), isDeletingAvatar = ref(false);
const userBusy = computed(() => isSavingUser.value || isSavingPassword.value || isUploadingAvatar.value || isDeletingAvatar.value || isDeletingUser.value);
const availablePermissionGroups = computed(() => userPermissionGroups.map(group => ({ ...group, fields: group.fields.filter(([key]) => typeof editingUser.value?.policy?.[key] === 'boolean') })).filter(group => group.fields.length));
const availablePlaybackFields = computed(() => userPlaybackFields.filter(([key]) => typeof editingUser.value?.configuration?.[key] === 'boolean'));
const displayedAvatar = computed(() => avatarAction.value === 'upload' ? avatarPreview.value : avatarAction.value === 'reset' ? '' : editingUser.value?.profile_image_tag ? userAvatar(editingUser.value) : '');
let usersSequence = 0, detailSequence = 0;
const roleOptions = [{ title: '普通用户', value: false }, { title: '管理员', value: true }];
const statusOptions = [{ title: '激活', value: false }, { title: '已停用', value: true }];
const filterOptions = [{ title: '全部用户', value: null }, { title: '管理员', value: 'admin' }, { title: '普通用户', value: 'member' }, { title: '激活', value: 'active' }, { title: '已停用', value: 'disabled' }];
const filteredUsers = computed(() => {
  const search = String(userSearch.value || '').trim().toLocaleLowerCase();
  return users.value.filter(item => (!search || String(item.name).toLocaleLowerCase().includes(search))
    && (!statusFilter.value || (statusFilter.value === 'admin' ? item.is_administrator : statusFilter.value === 'member' ? !item.is_administrator : statusFilter.value === 'disabled' ? item.is_disabled : !item.is_disabled)));
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
const restoreCurrentAvatar = () => {
  if (avatarPreview.value) URL.revokeObjectURL(avatarPreview.value);
  avatarFile.value = null; avatarPreview.value = ''; avatarAction.value = 'keep';
  if (fileInput.value) fileInput.value.value = '';
};
const resetDefaultAvatar = () => { restoreCurrentAvatar(); avatarAction.value = 'reset'; };
const handleAvatarChange = event => {
  const file = event.target.files?.[0];
  if (!file || userBusy.value) return;
  if (!validAvatar(file)) { message.error('请选择不超过 800 KB 的 JPG、PNG、GIF 或 WebP 图片'); event.target.value = ''; return; }
  restoreCurrentAvatar(); avatarFile.value = file; avatarPreview.value = URL.createObjectURL(file); avatarAction.value = 'upload';
};
const clearUserDraft = () => {
  ++detailSequence; restoreCurrentAvatar(); editingUser.value = null; originalUser.value = null;
  createPassword.value = ''; createConfirmPassword.value = ''; showCreatePassword.value = false; loadingUserDetails.value = false;
};
const closeUserDialog = () => { if (userBusy.value) return; userDialogOpen.value = false; clearUserDraft(); };
const addUser = () => {
  if (userBusy.value) return;
  clearUserDraft(); creatingUser.value = true; userTab.value = 'profile'; userSaveError.value = '';
  editingUser.value = { id: '', name: '', is_administrator: false, is_disabled: false, policy: {}, configuration: {} };
  userDialogOpen.value = true;
};
const editUser = async item => {
  if (userBusy.value) return;
  clearUserDraft(); creatingUser.value = false; userTab.value = 'profile'; userSaveError.value = '';
  userDialogOpen.value = true; loadingUserDetails.value = true;
  const sequence = ++detailSequence, requestedUser = authStore.user;
  try {
    const { data } = await axios.get(`/api/portal/users/${encodeURIComponent(item.id)}`);
    if (!isCurrentAccount(requestedUser) || sequence !== detailSequence || !userDialogOpen.value) return;
    if (data.user?.id !== item.id || typeof data.user.is_administrator !== 'boolean' || typeof data.user.is_disabled !== 'boolean') throw new Error('Emby 用户资料尚未确认');
    originalUser.value = structuredClone(data.user);
    editingUser.value = { ...structuredClone(data.user), original_administrator: data.user.is_administrator };
    libraries.value = data.libraries?.items || []; librariesError.value = data.libraries?.error || '';
  } catch (error) {
    if (isCurrentAccount(requestedUser) && sequence === detailSequence) userSaveError.value = error.response?.data?.error || error.message || '用户资料读取失败';
  } finally { if (sequence === detailSequence) loadingUserDetails.value = false; }
};
const updateAvatarTag = (id, tag) => {
  const item = users.value.find(user => user.id === id);
  if (item) item.profile_image_tag = tag;
  if (editingUser.value?.id === id) editingUser.value.profile_image_tag = tag;
  if (id === authStore.user?.id && authStore.accountInfo) authStore.accountInfo.profile_image_tag = tag;
};
const saveAvatar = async (id, requestedUser) => {
  if (avatarAction.value === 'keep') return;
  let data;
  if (avatarAction.value === 'upload') {
    isUploadingAvatar.value = true;
    const body = new FormData(); body.append('avatar', avatarFile.value);
    ({ data } = await axios.post(`/api/portal/users/${encodeURIComponent(id)}/avatar`, body));
    if (data.id !== id || !data.new_tag) throw new Error('Emby 头像更新尚未确认');
  } else {
    isDeletingAvatar.value = true;
    ({ data } = await axios.delete(`/api/portal/users/${encodeURIComponent(id)}/avatar`));
    if (data.id !== id || data.new_tag !== null) throw new Error('Emby 默认头像尚未确认');
  }
  if (!isCurrentAccount(requestedUser)) return;
  updateAvatarTag(id, data.new_tag); restoreCurrentAvatar();
};
const saveUserDetails = async () => {
  if (userBusy.value || loadingUserDetails.value || !editingUser.value) return;
  const requestedUser = authStore.user;
  const submitted = JSON.parse(JSON.stringify(editingUser.value));
  const wasCreating = creatingUser.value;
  if (!String(submitted.name || '').trim()) { userSaveError.value = '请输入用户名'; return; }
  if (wasCreating && users.value.some(item => item.name.toLocaleLowerCase() === submitted.name.trim().toLocaleLowerCase())) { userSaveError.value = '用户名已存在'; return; }
  if (wasCreating && !createPassword.value) { userSaveError.value = '请输入密码'; return; }
  if (wasCreating && createPassword.value !== createConfirmPassword.value) { userSaveError.value = '两次输入的密码不一致'; return; }
  const body = { name: submitted.name.trim(), is_administrator: submitted.is_administrator, is_disabled: submitted.is_disabled };
  const policy = changedUserFields(submitted.policy, originalUser.value?.policy);
  const configuration = changedUserFields(submitted.configuration, originalUser.value?.configuration);
  if (Object.keys(policy).length) body.policy = policy;
  if (Object.keys(configuration).length) body.configuration = configuration;
  if (wasCreating) body.new_password = createPassword.value;
  isSavingUser.value = true; userSaveError.value = '';
  try {
    const { data } = await axios.post(wasCreating ? '/api/portal/users' : `/api/portal/users/${encodeURIComponent(submitted.id)}`, body);
    if (!isCurrentAccount(requestedUser) || editingUser.value?.id !== submitted.id) return;
    if (!data.id || (!wasCreating && data.id !== submitted.id) || data.user?.id !== data.id || data.user.name !== body.name
      || data.user.is_administrator !== submitted.is_administrator || data.user.is_disabled !== submitted.is_disabled
      || !userFieldsMatch(policy, data.user.policy) || !userFieldsMatch(configuration, data.user.configuration)) throw new Error('Emby 用户资料保存尚未确认');
    const item = users.value.find(user => user.id === data.id);
    if (item) Object.assign(item, data.user); else users.value.push(data.user);
    if (wasCreating) { editingUser.value.id = data.id; creatingUser.value = false; createPassword.value = ''; createConfirmPassword.value = ''; }
    originalUser.value = structuredClone(data.user);
    if (data.id === authStore.user?.id) { authStore.user.name = data.user.name; if (authStore.accountInfo) authStore.accountInfo.name = data.user.name; }
    await saveAvatar(data.id, requestedUser);
    if (!isCurrentAccount(requestedUser)) return;
    userDialogOpen.value = false; clearUserDraft(); message.success(data.message || 'Emby 用户资料已保存');
  } catch (error) {
    if (isCurrentAccount(requestedUser)) {
      userSaveError.value = error.response?.data?.error || error.message || '用户资料保存失败';
      // Reuse only the account ID explicitly confirmed by the backend creation receipt.
      if (wasCreating) {
        await loadUsers();
        const receipt = error.response?.data;
        if (receipt?.created === true && receipt.id && editingUser.value) {
          editingUser.value.id = receipt.id; creatingUser.value = false;
          createPassword.value = ''; createConfirmPassword.value = '';
          userSaveError.value += '；用户已创建，请核对资料后保存。';
          try {
            const { data } = await axios.get(`/api/portal/users/${encodeURIComponent(receipt.id)}`);
            if (isCurrentAccount(requestedUser) && editingUser.value?.id === receipt.id && data.user?.id === receipt.id) {
              originalUser.value = structuredClone(data.user);
              editingUser.value.policy = structuredClone(data.user.policy || {});
              editingUser.value.configuration = structuredClone(data.user.configuration || {});
              libraries.value = data.libraries?.items || []; librariesError.value = data.libraries?.error || '';
            }
          } catch { /* Keep the confirmed ID and error visible; never submit another creation. */ }
        }
      }
    }
  } finally { isSavingUser.value = false; isUploadingAvatar.value = false; isDeletingAvatar.value = false; }
};
const confirmDeleteUser = item => {
  if (item.id === authStore.user?.id || userBusy.value) return;
  dialog.warning({ title: '删除用户', content: `确定删除用户“${item.name}”吗？此操作会删除 Emby 用户账户。`, positiveText: '删除', negativeText: '取消', onPositiveClick: async () => {
    if (userBusy.value) return;
    const requestedUser = authStore.user; isDeletingUser.value = true;
    try {
      const { data } = await axios.delete(`/api/portal/users/${encodeURIComponent(item.id)}`);
      if (!isCurrentAccount(requestedUser)) return;
      if (data.id !== item.id) throw new Error('Emby 用户删除尚未确认');
      users.value = users.value.filter(user => user.id !== item.id); message.success(data.message || '用户已删除');
    } catch (error) { if (isCurrentAccount(requestedUser)) message.error(error.response?.data?.error || error.message || '用户删除失败'); }
    finally { isDeletingUser.value = false; }
  } });
};

const passwordDialogOpen = ref(false), passwordUser = ref(null), newPassword = ref(''), confirmPassword = ref(''), showPassword = ref(false), isSavingPassword = ref(false), passwordSaveError = ref('');
const clearPasswordDraft = open => {
  if (open) return;
  newPassword.value = ''; confirmPassword.value = ''; showPassword.value = false; passwordSaveError.value = '';
};
const closePasswordDialog = () => { passwordDialogOpen.value = false; clearPasswordDraft(false); };
const editPassword = item => {
  if (userBusy.value) return;
  clearPasswordDraft(false); passwordUser.value = { id: item.id, name: item.name }; passwordDialogOpen.value = true;
};
const saveUserPassword = async () => {
  if (isSavingPassword.value || !passwordUser.value) return;
  if (!newPassword.value) { passwordSaveError.value = '请输入新密码'; return; }
  if (newPassword.value !== confirmPassword.value) { passwordSaveError.value = '两次输入的密码不一致'; return; }
  const requestedUser = authStore.user, submittedId = passwordUser.value.id;
  isSavingPassword.value = true; passwordSaveError.value = '';
  try {
    const { data } = await axios.post(`/api/portal/users/${encodeURIComponent(submittedId)}/password`, { new_password: newPassword.value });
    if (!isCurrentAccount(requestedUser) || passwordUser.value?.id !== submittedId) return;
    if (data.id !== submittedId) throw new Error('Emby 用户密码更新尚未确认');
    closePasswordDialog(); message.success(data.message || 'Emby 用户密码已更新');
  } catch (error) { if (isCurrentAccount(requestedUser) && passwordUser.value?.id === submittedId) passwordSaveError.value = error.response?.data?.error || error.message || '密码更新失败'; }
  finally { isSavingPassword.value = false; }
};
onMounted(() => { if (authStore.isAdmin) loadUsers(); });
onUnmounted(() => { accountPageActive = false; clearUserDraft(); clearPasswordDraft(false); });
</script>

<style scoped>
.user-toolbar { max-width: 720px; }
.user-toolbar > :first-child { flex: 1 1 260px; }
.user-toolbar > :last-child { flex: 0 1 180px; }
.user-card-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 288px), 1fr)); gap: 16px; }
.user-card { display: flex; flex-direction: column; min-width: 0; }
.user-card--disabled { background: rgba(var(--v-theme-surface-variant), .35); }
.user-name, .password-title { white-space: normal; overflow-wrap: anywhere; line-height: 1.5; }
.user-name { padding: 0; }
.user-role-chip :deep(.v-chip__content) { color: rgba(var(--v-theme-on-surface), var(--v-high-emphasis-opacity)); }
.user-card :deep(.v-card-item__content) { min-width: 0; }
.user-dialog-content :deep(.v-selection-control .v-label) { white-space: normal; overflow-wrap: anywhere; line-height: 1.5; }
.user-avatar-wrap { position: relative; }
.admin-avatar { border: 4px solid rgba(var(--v-theme-warning), .3); }
.admin-crown { position: absolute; right: -7px; top: -7px; }
.user-activity { display: grid; gap: 8px; font-size: .875rem; flex: 1; }
.user-activity > div { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 8px; }
.user-dialog-content { padding-block: 24px !important; }
.permission-sections { display: grid; gap: 24px; }
.permission-section { padding: 16px; border: 1px solid rgba(var(--v-border-color), var(--v-border-opacity)); border-radius: 12px; }
.user-add-button { position: fixed; right: 24px; bottom: 24px; z-index: 8; }
@media (max-width: 540px) { .user-toolbar > :last-child { flex: 1 1 100%; } .avatar-editor > :last-child { flex-basis: 100%; } .user-add-button { right: 16px; bottom: 20px; } }
</style>
