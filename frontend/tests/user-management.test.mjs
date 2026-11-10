import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
const source = readFileSync(new URL('../src/components/UserCenterPage.vue', import.meta.url), 'utf8');
const plain = value => JSON.parse(JSON.stringify(value));
function fixture({ get, post } = {}) {
  const requests = [], messages = [], user = { id: 'admin' };
  const context = vm.createContext({
    users: { value: [{ id: 'member', notes: 'old note' }] }, usersSequence: 0,
    loadingUsers: { value: false }, usersError: { value: '' },
    editingUser: { value: { id: 'member', name: 'Member', notes: ' new note ', telegram_chat_id: ' 123 ' } },
    isSavingUser: { value: false }, userSaveError: { value: '' }, userDialogOpen: { value: true },
    authStore: { user }, active: true,
    isCurrentAccount: requested => context.active && context.authStore.user === requested,
    message: { success: value => messages.push(value) },
    axios: { get: get || (async () => ({ data: { users: [] } })), post: async (path, body) => { requests.push({ path, body }); return post ? post(path,body) : { data: { id: 'member', message: 'saved' } }; } },
  });
  for (const name of ['loadUsers','saveUserDetails']) {
    const start = source.indexOf(`const ${name} = `);
    const end = source.indexOf('\n};', start) + 3;
    vm.runInContext(source.slice(start,end) + `\nthis.${name} = ${name};`, context);
  }
  return { context, requests, messages };
}

test('unreadable user list preserves existing rows and exposes the error', async () => {
  const f=fixture({ get: async () => { throw { response: { data: { error: 'users unavailable' } } }; } });
  await f.context.loadUsers();
  assert.equal(f.context.users.value[0].notes,'old note');
  assert.equal(f.context.usersError.value,'users unavailable');
});

test('user details submit only notes and notification contact and keep native fields', async () => {
  const f=fixture();f.context.users.value[0].is_disabled=true;
  await f.context.saveUserDetails();
  assert.deepEqual(plain(f.requests),[{ path:'/api/portal/users/member',body:{notes:' new note ',telegram_chat_id:' 123 '} }]);
  assert.equal(f.context.users.value[0].is_disabled,true);
  assert.equal(f.context.users.value[0].notes,'new note');
  assert.equal(f.context.userDialogOpen.value,false);
});

test('a response for another user does not claim save success or close the draft', async () => {
  const f=fixture({ post:async()=>({ data:{id:'other'} }) });
  await f.context.saveUserDetails();
  assert.equal(f.context.userDialogOpen.value,true);
  assert.equal(f.context.editingUser.value.notes,' new note ');
  assert.match(f.context.userSaveError.value,/未确认/);
  assert.deepEqual(f.messages,[]);
});

for (const replacement of ['unmount','account']) {
  test(`a late user save cannot affect an ${replacement} session`,async()=>{
    let release;const pending=new Promise(resolve=>{release=resolve;});
    const f=fixture({post:()=>pending});
    const save=f.context.saveUserDetails();
    if(replacement==='unmount')f.context.active=false;else f.context.authStore.user={id:'other'};
    release({data:{id:'member',message:'saved'}});await save;
    assert.equal(f.context.userDialogOpen.value,true);
    assert.equal(f.context.users.value[0].notes,'old note');assert.deepEqual(f.messages,[]);
  });
}

test('repeated user save clicks send one request while a save is pending',async()=>{
  let release;const pending=new Promise(resolve=>{release=resolve;});
  const f=fixture({post:()=>pending});const save=f.context.saveUserDetails();
  await f.context.saveUserDetails();assert.equal(f.requests.length,1);
  release({data:{id:'member'}});await save;
});
