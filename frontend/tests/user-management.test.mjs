import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
const source = readFileSync(new URL('../src/components/UserCenterPage.vue', import.meta.url), 'utf8');
const plain = value => JSON.parse(JSON.stringify(value));
function fixture({ get, post } = {}) {
  const requests = [], messages = [], user = { id: 'admin', name: 'Administrator' };
  const savedUser = { id: 'member', name: 'New Member', is_administrator: false, is_disabled: false };
  const context = vm.createContext({
    users: { value: [{ id: 'member', name: 'Member', is_administrator: false, is_disabled: true }] }, usersSequence: 0,
    loadingUsers: { value: false }, usersError: { value: '' },
    editingUser: { value: { id: 'member', name: ' New Member ', is_administrator: false, is_disabled: false } },
    isSavingUser: { value: false }, userSaveError: { value: '' }, userDialogOpen: { value: true },
    isUploadingAvatar: { value: false }, isDeletingAvatar: { value: false },
    passwordUser: { value: { id: 'member', name: 'Member' } }, passwordDialogOpen: { value: true },
    newPassword: { value: 'fixture-password' }, confirmPassword: { value: 'fixture-password' },
    isSavingPassword: { value: false }, passwordSaveError: { value: '' }, showPassword: { value: false },
    authStore: { user }, active: true,
    isCurrentAccount: requested => context.active && context.authStore.user === requested,
    message: { success: value => messages.push(value) },
    axios: { get: get || (async () => ({ data: { users: [] } })), post: async (path, body) => {
      requests.push({ path, body }); return post ? post(path, body) : { data: { id: 'member', user: savedUser, message: 'saved' } };
    } },
  });
  for (const name of ['loadUsers', 'saveUserDetails', 'clearPasswordDraft', 'closePasswordDialog', 'saveUserPassword']) {
    const start = source.indexOf(`const ${name} = `);
    const end = source.indexOf('\n};', start) + 3;
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
