import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { settingsCatalog, buildCatalog } from '../scripts/build-settings-search.mjs';

const catalog = JSON.parse(readFileSync(new URL('../src/assets/settings-search.json', import.meta.url), 'utf8'));

test('checked settings directory remains synchronized with actual field labels and hints', async () => {
  assert.deepEqual(catalog, await buildCatalog());
  const byField = Object.fromEntries(catalog.map(entry => [entry.field, entry]));
  assert.equal(byField.emby_api_key.label, 'API密钥');
  assert.equal(byField.emby_api_key.card, '媒体服务器');
  assert.equal(byField.emby_api_key.category, 'system');
  assert.equal(byField.movie_rename_format.label, '电影重命名格式');
  assert.equal(byField.movie_rename_format.category, 'rule');
  assert.equal(byField.telegram_bot_token.label, 'Bot Token');
  assert.equal(byField.telegram_bot_token.route, 'notification');
  assert.equal(byField.notification_scope.label, '通知发送范围');
  assert.ok(byField.emby_metadata_path.keywords.some(word => word.includes('metadata')));
  assert.ok(byField.ai_model_name.keywords.some(word => word.includes('指定使用的LLM模型')));
  assert.ok(!byField.min_score_for_review);
  assert.ok(!byField.emby_public_url);
  assert.equal(byField.monitor_sha1_pc_search.card, '目录');
  assert.equal(byField.tmdb_image_language_preference.card, '整理 & 刮削');
  assert.equal(byField.auto_extract_credits_on_ingest.label, '入库完成后提取片尾');
  for (const field of ['ai_translate_actor_role', 'ai_translate_on_sync', 'ai_translate_title', 'ai_translate_overview', 'ai_translate_episode_overview', 'ai_joke_fallback', 'libraries_to_ignore']) assert.ok(byField[field]);
});

test('catalog parses only displayed static names and hints without values or implementation expressions', () => {
  const source = `<template><VWindowItem value="system"><SettingCard title="媒体服务器" subtitle="连接配置"><div data-field="emby_api_key"><VTextField v-model="configModel.emby_api_key" label="API密钥" hint="请输入密钥" value="private-value-sentinel" :hint="privateHintExpression" /><div class="settings-hint">实际提示 {{ secretInterpolation }}</div></div></SettingCard></VWindowItem></template><script>const savedSecret = 'private-script-sentinel'</script>`;
  const [entry] = settingsCatalog(source, 'general');
  assert.deepEqual(entry.keywords, ['API密钥', '媒体服务器', '连接配置', '请输入密钥', '实际提示']);
  assert.doesNotMatch(JSON.stringify(entry), /private-|configModel|privateHintExpression|secretInterpolation|savedSecret/);
});

test('unlabeled fields and accidental duplicates fail instead of inventing search entries', () => {
  const wrap = body => `<template><VWindowItem value="rule"><SettingCard title="整理 &amp; 刮削">${body}</SettingCard></VWindowItem></template>`;
  assert.throws(() => settingsCatalog(wrap('<div data-field="unknown"><VTextField /></div>'), 'general'), /unknown/);
  assert.throws(() => settingsCatalog(wrap('<VTextField data-field="format" label="格式"/><VTextField data-field="format" label="格式"/>'), 'general'), /重复/);
});

test('notification scope groups combine repeated static hints under their actual title', () => {
  const source = '<template><SettingCard title="通知发送范围"><VRadioGroup data-field="notification_scope" data-settings-label="通知发送范围" hint="提醒操作用户"/><VRadioGroup data-field="notification_scope" data-settings-label="通知发送范围" hint="提醒管理员"/></SettingCard></template>';
  const entries = settingsCatalog(source, 'notification');
  assert.equal(entries.length, 1);
  assert.deepEqual(entries[0].keywords, ['通知发送范围', '提醒操作用户', '提醒管理员']);
});
