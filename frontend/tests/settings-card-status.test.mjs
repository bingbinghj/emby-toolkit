import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';
import assert from 'node:assert/strict';

const source = readFileSync(new URL('../src/components/settings/GeneralSettingsPage.vue', import.meta.url), 'utf8');
const statuses = source.slice(source.indexOf('const p115ConnectionStatus ='), source.indexOf('// 辅助函数：获取规则摘要'));

function fixture(info, config = {}, loading = false, error = '') {
  const context = vm.createContext({
    computed: getter => ({ get value() { return getter(); } }),
    p115Info: { value: info }, p115StatusError: { value: error },
    loading115Info: { value: loading }, configModel: { value: config },
  });
  vm.runInContext(statuses + '\nthis.connection = p115ConnectionStatus; this.cards = settingCardStatus;', context);
  return context;
}

test('configured API keys do not claim verified connections', () => {
  const cards = fixture(null, { emby_server_url: 'http://emby.test', emby_api_key: 'test-key', tmdb_api_key: 'test-key', ai_api_key: 'test-key' }).cards.value;
  for (const key of ['emby', 'sources', 'ai']) {
    assert.match(cards[key][0].label, /已配置，未验证/);
    assert.doesNotMatch(cards[key][0].label, /已连接/);
  }
});

test('115 authorization presence requires actual validation before showing connected', () => {
  assert.equal(fixture({ has_token: true }).connection.value.label, '已授权，未验证');
  assert.equal(fixture({ has_cookie: true, valid: null }).connection.value.label, '已授权，未验证');
  assert.equal(fixture({ has_token: true, valid: true }).connection.value.label, '已连接');
  assert.match(fixture({ has_token: true, valid: false }).connection.value.label, /连接异常/);
});

test('status loading and read errors remain visible despite an earlier valid connection', () => {
  assert.equal(fixture({ valid: true }, {}, true).connection.value.label, '检测中');
  assert.equal(fixture({ valid: true }, {}, false, 'unavailable').connection.value.label, '状态读取失败');
});

test('disabled monitor and AI settings remain visibly disabled on collapsed cards', () => {
  const cards = fixture(null, { monitor_enabled: false, ai_api_key: 'test-key', ai_translate_actor_role: false }).cards.value;
  assert.equal(cards.monitor[0].label, '监控已停用');
  assert.equal(cards.ai[1].label, '翻译已停用');
});
