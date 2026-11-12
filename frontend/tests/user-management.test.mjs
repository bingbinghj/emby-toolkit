import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { changedUserFields, userFieldsMatch, validAvatar } from '../src/utils/userManagement.js';
const source = readFileSync(new URL('../src/components/UserCenterPage.vue', import.meta.url), 'utf8');
const plain = value => JSON.parse(JSON.stringify(value));
function fixture({ get, post, remove } = {}) {
  const requests = [], messages = [], user = { id: 'admin', name: 'Administrator' };
  const savedUser = { id: 'member', name: 'New Member', is_administrator: false, is_disabled: false };
  const context = vm.createContext({
    users: { value: [{ id: 'member', name: 'Member', is_administrator: false, is_disabled: true }] }, usersSequence: 0,
    loadingUsers: { value: false }, usersError: { value: '' },
    editingUser: { value: { id: 'member', name: ' New Member ', is_administrator: false, is_disabled: false } },
    isSavingUser: { value: false }, userSaveError: { value: '' }, userDialogOpen: { value: true },
    isUploadingAvatar: { value: false }, isDeletingAvatar: { value: false },
    creatingUser: { value: false }, loadingUserDetails: { value: false }, originalUser: { value: { policy: {}, configuration: {} } },
    createPassword: { value: '' }, createConfirmPassword: { value: '' }, passwordSetupPending: { value: false }, libraries: { value: [] }, librariesError: { value: '' },
    avatarAction: { value: 'keep' }, avatarPreview: { value: '' }, avatarFile: { value: null }, fileInput: { value: null },
    userBusy: { get value() { return context.isSavingUser.value || context.isUploadingAvatar.value || context.isDeletingAvatar.value; } },
    changedUserFields, userFieldsMatch, validAvatar, structuredClone, FormData,
    URL: { createObjectURL: () => 'blob:avatar-preview', revokeObjectURL: value => requests.push({ revoke: value }) },
    saveAvatar: async () => {}, clearUserDraft: () => {},
    passwordUser: { value: { id: 'member', name: 'Member' } }, passwordDialogOpen: { value: true },
    newPassword: { value: 'fixture-password' }, confirmPassword: { value: 'fixture-password' },
    isSavingPassword: { value: false }, passwordSaveError: { value: '' }, showPassword: { value: false },
    authStore: { user }, active: true,
    isCurrentAccount: requested => context.active && context.authStore.user === requested,
    message: { success: value => messages.push(value), error: value => messages.push(value) },
    axios: { get: get || (async () => ({ data: { users: [] } })), post: async (path, body) => {
      requests.push({ path, body }); return post ? post(path, body) : { data: { id: 'member', user: savedUser, message: 'saved' } };
    }, delete: async (path) => { requests.push({ path, method: 'DELETE' }); return remove ? remove(path) : { data: { id: 'member', new_tag: null } }; } },
  });
  for (const name of ['loadUsers', 'saveUserDetails', 'clearPasswordDraft', 'closePasswordDialog', 'saveUserPassword']) {
    const start = source.indexOf(`const ${name} = `);
    const nextDeclaration = source.indexOf('\nconst ', start);
    const nextMounted = source.indexOf('\nonMounted', start);
    const end = Math.min(...[nextDeclaration, nextMounted].filter(value => value > start));
    vm.runInContext(source.slice(start, end) + `\nthis.${name} = ${name};`, context);
  }
  return { context, requests, messages };
}

test('unreadable user list retains existing cards and exposes retryable error', async () => {
  const f = fixture({ get: async () => { throw { response: { data: { error: 'users unavailable' } } }; } });
  await f.context.loadUsers();
  assert.equal(f.context.users.value[0].name, 'Member');
  assert.equal(f.context.usersError.value, 'users unavailable');
});

test('user profile saves real Emby name role and status and accepts only native verified fields', async () => {
  const f = fixture();
  await f.context.saveUserDetails();
  assert.deepEqual(plain(f.requests), [{ path: '/api/portal/users/member', body: { name: 'New Member', is_administrator: false, is_disabled: false } }]);
  assert.equal(f.context.users.value[0].name, 'New Member');
  assert.equal(f.context.users.value[0].is_disabled, false);
  assert.equal(f.context.userDialogOpen.value, false);
});

for (const response of [{ id: 'other' }, { id: 'member' }, { id: 'member', user: { id: 'member', name: 'New Member', is_administrator: true, is_disabled: false } }]) {
  test(`unconfirmed Emby profile response ${JSON.stringify(response)} preserves the draft`, async () => {
    const f = fixture({ post: async () => ({ data: response }) });
    await f.context.saveUserDetails();
    assert.equal(f.context.userDialogOpen.value, true);
    assert.equal(f.context.editingUser.value.name, ' New Member ');
    assert.match(f.context.userSaveError.value, /未确认/);
    assert.deepEqual(f.messages, []);
  });
}

for (const operation of ['saveUserDetails', 'saveUserPassword']) {
  for (const replacement of ['unmount', 'account']) {
    test(`late ${operation} response cannot affect an ${replacement} session`, async () => {
      let release; const pending = new Promise(resolve => { release = resolve; });
      const f = fixture({ post: () => pending });
      const save = f.context[operation]();
      if (replacement === 'unmount') f.context.active = false; else f.context.authStore.user = { id: 'other' };
      release({ data: { id: 'member', user: { id: 'member', name: 'New Member', is_administrator: false, is_disabled: false } } }); await save;
      assert.equal(f.context.userDialogOpen.value, true); assert.equal(f.context.passwordDialogOpen.value, true);
      assert.equal(f.context.users.value[0].name, 'Member'); assert.deepEqual(f.messages, []);
    });
  }
  test(`repeated ${operation} clicks send one request while pending`, async () => {
    let release; const pending = new Promise(resolve => { release = resolve; });
    const f = fixture({ post: () => pending }); const save = f.context[operation]();
    await f.context[operation](); assert.equal(f.requests.length, 1);
    release({ data: { id: 'member', user: { id: 'member', name: 'New Member', is_administrator: false, is_disabled: false } } }); await save;
  });
}

test('password validation rejects empty or mismatched values without submitting', async () => {
  const f = fixture(); f.context.newPassword.value = '';
  await f.context.saveUserPassword(); assert.match(f.context.passwordSaveError.value, /请输入/);
  f.context.newPassword.value = 'different';
  await f.context.saveUserPassword(); assert.match(f.context.passwordSaveError.value, /不一致/); assert.deepEqual(f.requests, []);
});

test('password operation is separate from profile and clears secrets on confirmed success', async () => {
  const f = fixture(); await f.context.saveUserPassword();
  assert.deepEqual(plain(f.requests), [{ path: '/api/portal/users/member/password', body: { new_password: 'fixture-password' } }]);
  assert.equal(f.context.newPassword.value, ''); assert.equal(f.context.confirmPassword.value, ''); assert.equal(f.context.passwordDialogOpen.value, false);
  assert.equal(f.context.users.value[0].name, 'Member');
});

test('failed password update keeps draft and never closes or claims success', async () => {
  const f = fixture({ post: async () => { throw { response: { data: { error: 'Emby unavailable' } } }; } });
  await f.context.saveUserPassword();
  assert.equal(f.context.passwordDialogOpen.value, true); assert.equal(f.context.passwordSaveError.value, 'Emby unavailable');
  assert.equal(f.context.newPassword.value, 'fixture-password'); assert.deepEqual(f.messages, []);
});

test('blank username is rejected before request', async () => {
  const f = fixture(); f.context.editingUser.value.name = '  ';
  await f.context.saveUserDetails(); assert.deepEqual(f.requests, []); assert.match(f.context.userSaveError.value, /用户名/);
});

test('editing permissions sends only changed Emby fields and checks their returned values', async () => {
  const f = fixture({ post: async () => ({ data: { id: 'member', user: { id: 'member', name: 'New Member', is_administrator: false, is_disabled: false,
    policy: { EnableRemoteAccess: false, EnableContentDownloading: true, EnabledFolders: ['movies'] }, configuration: { SubtitleMode: 'OnlyForced', PlayDefaultAudioTrack: true } } } }) });
  f.context.originalUser.value = { policy: { EnableRemoteAccess: true, EnableContentDownloading: true, EnabledFolders: [] }, configuration: { SubtitleMode: 'Smart', PlayDefaultAudioTrack: true } };
  f.context.editingUser.value.policy = { EnableRemoteAccess: false, EnableContentDownloading: true, EnabledFolders: ['movies'], UnknownNativeField: false };
  f.context.editingUser.value.configuration = { SubtitleMode: 'OnlyForced', PlayDefaultAudioTrack: true };
  await f.context.saveUserDetails();
  assert.deepEqual(plain(f.requests[0].body), { name: 'New Member', is_administrator: false, is_disabled: false, policy: { EnableRemoteAccess: false, EnabledFolders: ['movies'] }, configuration: { SubtitleMode: 'OnlyForced' } });
  assert.equal(f.context.userDialogOpen.value, false);
});

test('unconfirmed permission changes retain the dialog and never claim success', async () => {
  const f = fixture();
  f.context.originalUser.value.policy = { EnableRemoteAccess: true };
  f.context.editingUser.value.policy = { EnableRemoteAccess: false };
  await f.context.saveUserDetails();
  assert.match(f.context.userSaveError.value, /未确认/);
  assert.equal(f.context.userDialogOpen.value, true);
  assert.equal(f.context.editingUser.value.policy.EnableRemoteAccess, false);
  assert.deepEqual(f.messages, []);
});

function newUser(f) {
  f.context.creatingUser.value = true;
  f.context.editingUser.value = { id: '', name: 'New Account', is_administrator: false, is_disabled: false, policy: {}, configuration: {} };
  f.context.createPassword.value = 'creation-secret';
  f.context.createConfirmPassword.value = 'creation-secret';
}

test('creating a user targets the native creation endpoint and clears password only after confirmation', async () => {
  const f = fixture({ post: async () => ({ data: { id: 'new-id', user: { id: 'new-id', name: 'New Account', is_administrator: false, is_disabled: false } } }) });
  newUser(f); await f.context.saveUserDetails();
  assert.deepEqual(plain(f.requests), [{ path: '/api/portal/users', body: { name: 'New Account', is_administrator: false, is_disabled: false, new_password: 'creation-secret' } }]);
  assert.equal(f.context.createPassword.value, '');
  assert.equal(f.context.users.value.at(-1).id, 'new-id');
});

test('a partial creation receipt retries the confirmed user instead of creating a duplicate', async () => {
  let attempts = 0;
  const created = { id: 'new-id', name: 'New Account', is_administrator: false, is_disabled: false, has_password: true, policy: {}, configuration: {} };
  const f = fixture({
    get: async path => ({ data: path === '/api/portal/users' ? { users: [created] } : { user: created, libraries: { items: [] } } }),
    post: async () => {
      if (++attempts === 1) throw { response: { data: { created: true, id: 'new-id', error: '权限保存尚未确认' } } };
      return { data: { id: 'new-id', user: created } };
    },
  });
  newUser(f); await f.context.saveUserDetails();
  assert.equal(f.context.creatingUser.value, false);
  assert.equal(f.context.editingUser.value.id, 'new-id');
  assert.equal(f.context.userDialogOpen.value, true);
  assert.match(f.context.userSaveError.value, /用户已创建/);
  await f.context.saveUserDetails();
  assert.deepEqual(f.requests.map(request => request.path), ['/api/portal/users', '/api/portal/users/new-id']);
});

function failedPasswordCreation(options = {}) {
  const native = { id: 'new-id', name: 'New Account', is_administrator: false, is_disabled: true, has_password: false, policy: {}, configuration: {} };
  const events = [];
  let passwordPosted = false;
  const f = fixture({
    get: async path => {
      events.push(path);
      if (path === '/api/portal/users') return { data: { users: [structuredClone(native)] } };
      if (passwordPosted && options.confirmation === 'read failed') throw new Error('密码回读不可用');
      const user = structuredClone(native);
      if (passwordPosted && options.confirmation === 'missing password') user.has_password = false;
      if (passwordPosted && options.confirmation === 'wrong user') user.id = 'another-user';
      return { data: { user, libraries: { items: [] } } };
    },
    post: async (path, body) => {
      events.push(path);
      if (path === '/api/portal/users') throw { response: { data: { created: true, id: 'new-id', user: structuredClone(native), error: '密码设置失败' } } };
      if (path.endsWith('/password')) {
        passwordPosted = true;
        if (options.passwordError) throw new Error('密码设置仍不可用');
        native.has_password = true;
        return { data: { id: 'new-id' } };
      }
      Object.assign(native, body);
      return { data: { id: 'new-id', user: structuredClone(native) } };
    },
  });
  newUser(f);
  return { ...f, events, native, options };
}

test('failed creation repairs and reads back its password before enabling the confirmed account', async () => {
  const f = failedPasswordCreation();
  await f.context.saveUserDetails();
  assert.equal(f.context.passwordSetupPending.value, true);
  assert.equal(f.context.createPassword.value, 'creation-secret');
  await f.context.saveUserDetails();
  assert.deepEqual(f.requests.map(request => request.path), ['/api/portal/users', '/api/portal/users/new-id/password', '/api/portal/users/new-id']);
  const password = f.events.indexOf('/api/portal/users/new-id/password');
  assert.ok(f.events.indexOf('/api/portal/users/new-id', password) < f.events.lastIndexOf('/api/portal/users/new-id'));
  assert.equal(f.native.has_password, true);
  assert.equal(f.native.is_disabled, false);
  assert.equal(f.context.passwordSetupPending.value, false);
  assert.equal(f.context.createPassword.value, '');
  assert.equal(f.context.userDialogOpen.value, false);
});

test('a failed password retry keeps its draft and cannot enable the partial account', async () => {
  const f = failedPasswordCreation({ passwordError: true });
  await f.context.saveUserDetails();
  await f.context.saveUserDetails();
  assert.deepEqual(f.requests.map(request => request.path), ['/api/portal/users', '/api/portal/users/new-id/password']);
  assert.equal(f.native.has_password, false);
  assert.equal(f.native.is_disabled, true);
  assert.equal(f.context.createPassword.value, 'creation-secret');
  assert.equal(f.context.createConfirmPassword.value, 'creation-secret');
  assert.equal(f.context.userDialogOpen.value, true);
  assert.match(f.context.userSaveError.value, /密码/);
  f.options.passwordError = false;
  await f.context.saveUserDetails();
  assert.equal(f.requests.filter(request => request.path === '/api/portal/users').length, 1);
  assert.equal(f.native.has_password, true);
  assert.equal(f.native.is_disabled, false);
});

for (const confirmation of ['read failed', 'missing password', 'wrong user']) {
  test(`password ${confirmation} never enables the partial account`, async () => {
    const f = failedPasswordCreation({ confirmation });
    await f.context.saveUserDetails();
    await f.context.saveUserDetails();
    assert.deepEqual(f.requests.map(request => request.path), ['/api/portal/users', '/api/portal/users/new-id/password']);
    assert.equal(f.native.is_disabled, true);
    assert.equal(f.context.passwordSetupPending.value, true);
    assert.equal(f.context.createPassword.value, 'creation-secret');
    assert.equal(f.context.userDialogOpen.value, true);
    assert.ok(f.context.userSaveError.value);
  });
}

test('a matching username without an explicit creation receipt never becomes an editable account', async () => {
  const existing = { id: 'other-account', name: 'New Account', is_administrator: false, is_disabled: false };
  const f = fixture({ get: async () => ({ data: { users: [existing] } }), post: async () => { throw { response: { data: { error: '连接中断，请核对' } } }; } });
  newUser(f); await f.context.saveUserDetails();
  assert.equal(f.context.creatingUser.value, true);
  assert.equal(f.context.editingUser.value.id, '');
  await f.context.saveUserDetails();
  assert.equal(f.requests.length, 1);
  assert.match(f.context.userSaveError.value, /已存在/);
});

function loadFunction(context, name) {
  const start = source.indexOf(`const ${name} = `);
  const next = source.indexOf('\nconst ', start);
  vm.runInContext(source.slice(start, next) + `\nthis.${name} = ${name};`, context);
}

test('avatar upload, cancel and default reset remain drafts until the profile is saved', () => {
  const f = fixture();
  for (const name of ['restoreCurrentAvatar', 'resetDefaultAvatar', 'handleAvatarChange']) loadFunction(f.context, name);
  const input = { files: [{ type: 'image/png', size: 123 }], value: 'selected.png' };
  f.context.handleAvatarChange({ target: input });
  assert.equal(f.context.avatarAction.value, 'upload');
  assert.equal(f.context.avatarPreview.value, 'blob:avatar-preview');
  assert.equal(f.requests.filter(request => request.path).length, 0);
  f.context.restoreCurrentAvatar();
  assert.equal(f.context.avatarAction.value, 'keep');
  assert.equal(f.context.avatarFile.value, null);
  f.context.resetDefaultAvatar();
  assert.equal(f.context.avatarAction.value, 'reset');
  assert.equal(f.requests.filter(request => request.path).length, 0);
});

test('invalid and oversized avatar files do not replace the selected avatar draft', () => {
  const f = fixture();
  for (const name of ['restoreCurrentAvatar', 'handleAvatarChange']) loadFunction(f.context, name);
  f.context.avatarAction.value = 'keep';
  for (const file of [{ type: 'text/plain', size: 8 }, { type: 'image/png', size: 800 * 1024 + 1 }, { type: 'image/png', size: 0 }]) f.context.handleAvatarChange({ target: { files: [file], value: 'file' } });
  assert.equal(f.context.avatarAction.value, 'keep'); assert.equal(f.context.avatarPreview.value, '');
  assert.equal(f.messages.length, 3); assert.deepEqual(f.requests, []);
});

test('saving an avatar targets the edited user and updates only their verified tag', async () => {
  const f = fixture({ post: async () => ({ data: { id: 'member', new_tag: 'new-image-tag' } }) });
  for (const name of ['restoreCurrentAvatar', 'updateAvatarTag', 'saveAvatar']) loadFunction(f.context, name);
  f.context.avatarFile.value = new Blob(['image-bytes'], { type: 'image/png' });
  f.context.avatarAction.value = 'upload';
  await f.context.saveAvatar('member', f.context.authStore.user);
  assert.equal(f.requests[0].path, '/api/portal/users/member/avatar');
  assert.equal(f.context.users.value[0].profile_image_tag, 'new-image-tag');
  assert.equal(f.context.editingUser.value.profile_image_tag, 'new-image-tag');
  assert.equal(f.context.avatarAction.value, 'keep');
});

test('an unverified avatar response leaves the avatar draft available for retry', async () => {
  const f = fixture({ post: async () => ({ data: { id: 'member', new_tag: null } }) });
  for (const name of ['restoreCurrentAvatar', 'updateAvatarTag', 'saveAvatar']) loadFunction(f.context, name);
  f.context.avatarFile.value = new Blob(['image-bytes'], { type: 'image/png' });
  f.context.avatarAction.value = 'upload';
  await assert.rejects(f.context.saveAvatar('member', f.context.authStore.user), /未确认/);
  assert.equal(f.context.avatarAction.value, 'upload');
  assert.equal(f.context.users.value[0].profile_image_tag, undefined);
});

test('a late avatar response cannot update another login session', async () => {
  let release;
  const pending = new Promise(resolve => { release = resolve; });
  const f = fixture({ post: () => pending });
  for (const name of ['restoreCurrentAvatar', 'updateAvatarTag', 'saveAvatar']) loadFunction(f.context, name);
  f.context.avatarFile.value = new Blob(['image-bytes'], { type: 'image/png' });
  f.context.avatarAction.value = 'upload';
  const save = f.context.saveAvatar('member', f.context.authStore.user);
  f.context.authStore.user = { id: 'replacement' };
  release({ data: { id: 'member', new_tag: 'late-image-tag' } });
  await save;
  assert.equal(f.context.users.value[0].profile_image_tag, undefined);
  assert.equal(f.context.avatarAction.value, 'upload');
});
