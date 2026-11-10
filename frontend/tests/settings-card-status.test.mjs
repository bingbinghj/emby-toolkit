import { readFileSync } from 'node:fs';
import test from 'node:test';
import assert from 'node:assert/strict';

const source = readFileSync(new URL('../src/components/settings/GeneralSettingsPage.vue', import.meta.url), 'utf8');
const card = readFileSync(new URL('../src/components/settings/general/SettingCard.vue', import.meta.url), 'utf8');

test('expanded setting cards expose their settings without synthetic status badges', () => {
  assert.doesNotMatch(source, /template #status|settingCardStatus|已配置，未验证/);
  assert.doesNotMatch(card, /slots\.status|setting-card__status/);
  assert.match(card, /setting-card__content/);
});

test('numeric settings use MoviePilot native number fields with numeric Vue models', () => {
  const fields = [...source.matchAll(/<VTextField type="number"([\s\S]*?)>/g)];
  assert.ok(fields.length >= 10);
  for (const [, field] of fields) assert.match(field, /v-model\.number=/);
  assert.doesNotMatch(source, /VNumberInput|data-field="min_score_for_review"/);
});
