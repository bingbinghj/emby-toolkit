import { test, expect, mediaDetails, mediaInfo } from './fixtures.js';
import { measureReadingContrast } from './reading-contrast.js';

const tasks = [
  { key: 'extract-episode-credits', name: '提取电视剧片尾', category: 'media-maintenance', category_name: '媒体维护', help: '仅处理电视剧分集，检测片尾黑场' },
  { key: 'generate-thumbnails', name: '提取视频缩略图', category: 'media-maintenance', category_name: '媒体维护', help: '生成 BIF 缩略图' },
  { key: 'clear-mediainfo-cache', name: '清理媒体信息缓存', category: 'system-maintenance', category_name: '系统维护', help: '只清理缓存' },
];
const chapter = { MarkerType: 'CreditsStart', Name: '片尾', StartPositionTicks: 9000000000 };
const intro = { MarkerType: 'IntroStart', Name: '片头', StartPositionTicks: 100000000 };
const taskRequests = api => api.requests.filter(request => request.method === 'POST' && request.path === '/api/tasks/run');
const mediaSearches = api => api.requests.filter(request => request.path === '/api/search_emby_library');

async function taskSearch(page, mobile) {
  if (mobile) await page.getByRole('button', { name: '打开搜索任务', exact: true }).click();
  const input = page.getByRole('searchbox', { name: '搜索任务', exact: true });
  await expect(input).toBeVisible();
  return input;
}
function episode(api) {
  api.respond('GET', '/api/media_for_editing/101', { ...mediaDetails, item_type: 'Episode', item_name: '测试剧第九集' });
  api.respond('GET', '/api/media_info/edit/101', { ...structuredClone(mediaInfo), mediainfo: [{ ...structuredClone(mediaInfo.mediainfo[0]), Chapters: [intro] }] });
}

for (const mobile of [false, true]) {
  test(`${mobile ? 'mobile' : 'desktop'} task center searches its task names, descriptions, categories and keys`, async ({ page, api }) => {
    await page.setViewportSize({ width: mobile ? 390 : 1440, height: 900 });
    api.respond('GET', '/api/tasks/available', tasks);
    await page.goto('/settings/scheduler');
    const input = await taskSearch(page, mobile);
    await expect(page.locator('.task-record:visible')).toHaveCount(3);
    const reads = api.requests.filter(request => request.path === '/api/tasks/available').length;
    for (const [query, count] of [['片尾', 1], ['BIF', 1], ['系统维护', 1], ['EXTRACT-EPISODE-CREDITS', 1], ['媒体维护', 2]]) {
      await input.fill(query);
      await expect(page.locator('.task-record:visible')).toHaveCount(count);
    }
    await input.fill('不存在的任务');
    await expect(page.locator('.task-record:visible')).toHaveCount(0);
    await expect(page.getByText('未找到匹配任务', { exact: true }).filter({ visible: true })).toBeVisible();
    await page.getByRole('button', { name: '清空搜索', exact: true }).click();
    await expect(page.locator('.task-record:visible')).toHaveCount(3);
    expect(api.requests.filter(request => request.path === '/api/tasks/available')).toHaveLength(reads);
    expect(mediaSearches(api)).toEqual([]);
    await expect(page).toHaveURL(/\/settings\/scheduler$/);
  });
}

test('leaving the task center replaces its search scope and clears the task keyword', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  api.respond('GET', '/api/tasks/available', tasks);
  await page.goto('/settings/scheduler');
  await (await taskSearch(page, false)).fill('片尾');
  await page.locator('.layout-vertical-nav').getByText('用户管理', { exact: true }).click();
  const users = page.getByRole('searchbox', { name: '搜索用户', exact: true });
  await expect(users).toHaveValue('');
  await users.fill('Media Member');
  await expect(page.locator('.user-card')).toHaveCount(1);
  await page.locator('.layout-vertical-nav').getByText('任务中心', { exact: true }).click();
  await expect(await taskSearch(page, false)).toHaveValue('');
  await expect(page.locator('.task-record:visible')).toHaveCount(3);
  expect(mediaSearches(api)).toEqual([]);
});

test('filtered credits task runs the real task key and waits for its terminal warning', async ({ page, api }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  api.respond('GET', '/api/tasks/available', tasks);
  api.queueTask('POST', '/api/tasks/run', { message: '片尾任务部分失败', failed: 1, failures: [{ item_id: '102', reason: '视频片尾没有可确认黑场' }] });
  await page.goto('/settings/scheduler');
  await (await taskSearch(page, false)).fill('片尾');
  const run = page.locator('.task-record:visible').getByRole('button', { name: '执行', exact: true });
  await run.click();
  await expect(run).toBeDisabled();
  await expect(page.getByText('102：视频片尾没有可确认黑场', { exact: true }).first()).toBeVisible();
  await expect(run).toBeEnabled();
  expect(taskRequests(api).map(request => request.body)).toEqual([{ task_name: 'extract-episode-credits' }]);
  expect(mediaSearches(api)).toEqual([]);
});

test('movie editor hides credits extraction and renders save as an icon plus text', async ({ page }) => {
  await page.goto('/edit-media/101');
  await expect(page.getByTestId('episode-extract-credits')).toHaveCount(0);
  const actorSave = page.locator('.media-edit-page .editor-actions').getByRole('button', { name: '保存', exact: true });
  await expect(actorSave).toHaveClass(/v-btn--variant-flat/);
  await expect(actorSave.locator('.v-btn__prepend .v-icon')).toBeVisible();
  await page.getByTestId('media-info-open').click();
  await expect(page.getByTestId('media-info-extract-credits')).toHaveCount(0);
  const save = page.getByTestId('media-info-save');
  await expect(save).toHaveText('保存');
  await expect(save).toHaveClass(/v-btn--variant-flat/);
  await expect(save.locator('.v-btn__prepend .v-icon')).toBeVisible();
  await expect(save).toBeEnabled();
});

test('episode credits completion preserves audio and actor drafts and only adds CreditsStart', async ({ page, api }) => {
  episode(api);
  api.queueTask('POST', '/api/media/101/extract_credits', { message: '已验收当前集片尾', chapter, failed: 0 }, { status: 200, state: 'completed' });
  await page.goto('/edit-media/101');
  const actor = page.getByLabel('演员', { exact: true }).first();
  await actor.fill('演员新草稿');
  await page.getByTestId('media-info-open').click();
  const audio = page.getByRole('textbox', { name: /音轨.*1.*当前标题/ });
  await audio.fill('音轨新草稿');
  const extract = page.getByTestId('media-info-extract-credits');
  await extract.click();
  await expect(page.getByTestId('media-info-save')).toBeDisabled();
  await expect(page.getByTestId('media-info-credits-feedback')).toContainText('已验收当前集片尾');
  await expect(audio).toHaveValue('音轨新草稿');
  await expect(page.getByTestId('media-info-save')).toBeEnabled();
  await page.getByTestId('media-info-save').click();
  await expect(page.getByRole('button', { name: '关闭媒体信息编辑器', exact: true })).toBeVisible();
  await expect(audio).toHaveValue('音轨新草稿');
  const request = api.requests.find(request => request.method === 'POST' && request.path === '/api/media_info/edit/101');
  expect(request.body.mediainfo[0].Chapters).toEqual([intro, chapter]);
  expect(request.body.mediainfo[0].MediaSourceInfo.MediaStreams.find(stream => stream.Type === 'Audio').Title).toBe('音轨新草稿');
  await expect(actor).toHaveValue('演员新草稿');
});

test('episode credits failure keeps the editor and full upstream failure visible', async ({ page, api }) => {
  episode(api);
  api.queueTask('POST', '/api/media/101/extract_credits', { error: '115源提取失败', message: '片尾提取失败', failures: [{ item_id: '101', reason: '网盘直链不可用，请检查授权' }] }, { status: 500, state: 'failed' });
  await page.goto('/edit-media/101');
  await page.getByTestId('media-info-open').click();
  await page.getByTestId('media-info-extract-credits').click();
  await expect(page.getByTestId('media-info-credits-feedback')).toContainText('115源提取失败');
  await expect(page.getByTestId('media-info-credits-feedback')).toContainText('101：网盘直链不可用，请检查授权');
  await expect(page.getByTestId('media-info-save')).toBeEnabled();
  await expect(page.getByRole('textbox', { name: /音轨.*1.*当前标题/ })).toHaveValue('Original English');
});

test('pending recovery and running tasks disable episode credits extraction', async ({ page, api }) => {
  episode(api);
  api.respond('GET', '/api/media_info/recovery', { pending: true, records: [{ id: 'saved-test', directory: '/config/backups/test', can_recover: false, reason: 'BIF恢复未确认' }] });
  await page.goto('/edit-media/101');
  await expect(page.getByTestId('episode-extract-credits')).toBeDisabled();
  api.respond('GET', '/api/media_info/recovery', { pending: false, records: [] });
  api.setTask({ is_running: true, state: 'running', current_action: '提取缩略图', message: '处理中', progress: 20 });
  await page.reload();
  await expect(page.getByTestId('episode-extract-credits')).toBeDisabled();
  expect(api.requests.filter(request => request.path.endsWith('/extract_credits'))).toEqual([]);
});

test('series editor extracts only the selected episode rather than the series id', async ({ page, api }) => {
  api.respond('GET', '/api/media_for_editing/201', { ...mediaDetails, item_id: '201', item_type: 'Series', item_name: '测试电视剧' });
  api.respond('GET', '/api/media_info/series/201/episodes', [{ emby_id: '102', season_number: 1, episode_number: 9, title: '当前第九集' }]);
  api.respond('GET', '/api/media_info/edit/102', { ...structuredClone(mediaInfo), versions: [{ emby_id: '102', file_name: 'Episode.S01E09.mkv' }] });
  api.respond('POST', '/api/media/102/extract_credits', { message: '已提取第九集片尾', chapter });
  await page.goto('/edit-media/201');
  await expect(page.getByTestId('episode-extract-credits')).toHaveCount(0);
  await page.getByTestId('media-info-open').click();
  await page.locator('.episode-chip').filter({ hasText: '09' }).click();
  await page.getByTestId('media-info-extract-credits').click();
  await expect(page.getByTestId('media-info-credits-feedback')).toContainText('已提取第九集片尾');
  expect(api.requests.filter(request => request.method === 'POST' && request.path.endsWith('/extract_credits')).map(request => request.path)).toEqual(['/api/media/102/extract_credits']);
});

for (const [kind, id, label] of [['Series', '201', '整部电视剧'], ['Season', '202', '当前季']]) {
  test(`${kind} detail button extracts the full selected scope`, async ({ page, api }) => {
    api.respond('GET', `/api/media_for_editing/${id}`, { ...mediaDetails, item_id: id, item_type: kind, item_name: '测试电视剧' });
    api.queueTask('POST', `/api/media/${id}/extract_credits`, { message: `已验收${label}片尾`, failed: 0, updated: 2 });
    await page.goto(`/edit-media/${id}`);
    const extract = page.getByTestId('scope-extract-credits');
    await expect(extract).toHaveText(`提取${label}片尾`);
    await expect(extract).toBeEnabled();
    await extract.click();
    await expect(extract).toBeDisabled();
    await expect(page.getByTestId('credits-extraction-feedback')).toContainText(`已验收${label}片尾`);
    await expect(extract).toBeEnabled();
    expect(api.requests.filter(request => request.method === 'POST' && request.path.endsWith('/extract_credits')).map(request => request.path)).toEqual([`/api/media/${id}/extract_credits`]);
  });
}


test('episode detail button extracts that episode and reports verified completion', async ({ page, api }) => {
  episode(api);
  api.queueTask('POST', '/api/media/101/extract_credits', { message: '当前单集片尾已验收', chapter, failed: 0 }, { status: 200, state: 'completed' });
  await page.goto('/edit-media/101');
  const extract = page.getByTestId('episode-extract-credits');
  await expect(extract).toBeEnabled();
  await extract.click();
  await expect(page.getByTestId('credits-extraction-feedback')).toContainText('当前单集片尾已验收');
  await expect(extract).toBeEnabled();
  expect(api.requests.filter(request => request.method === 'POST' && request.path.endsWith('/extract_credits')).map(request => request.path)).toEqual(['/api/media/101/extract_credits']);
});

test('a credits request finishing after navigation cannot add feedback to the new page', async ({ page, api }) => {
  episode(api);
  const pending = api.defer('POST', '/api/media/101/extract_credits');
  await page.goto('/edit-media/101');
  await page.getByTestId('episode-extract-credits').click();
  await pending.started;
  await page.locator('.layout-vertical-nav').getByText('用户管理', { exact: true }).click();
  await expect(page).toHaveURL(/\/user-management$/);
  pending.release({ message: '旧媒体片尾提取结果', chapter });
  await expect(page.getByText('旧媒体片尾提取结果', { exact: true })).not.toBeVisible();
  await expect(page.getByRole('searchbox', { name: '搜索用户', exact: true })).toBeVisible();
});


for (const [theme, glassEnabled] of [['light', false], ['dark', false], ['light', true], ['dark', true]]) {
  test(`${theme} ${glassEnabled ? 'glass wallpaper' : 'plain'} credits and save controls keep readable text inside button bounds`, async ({ page, api }) => {
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.addInitScript(({ theme, glassEnabled }) => {
      const canvas = document.createElement('canvas'); canvas.width = canvas.height = 64;
      const context = canvas.getContext('2d');
      context.fillStyle = '#ffffff'; context.fillRect(0, 0, 64, 64);
      context.fillStyle = '#081422'; context.fillRect(0, 0, 32, 64);
      localStorage.setItem('etk-wallpaper-settings', JSON.stringify({ mode: 'custom', customImage: canvas.toDataURL(), rotationInterval: 0 }));
      localStorage.setItem('moviepilot-theme-customizer', JSON.stringify({ theme, glassEnabled, glassQuality: 'css', glassAppearance: 'clear', glassUIStyle: 'adaptive', glassDynamicsMode: 'off', layout: 'vertical' }));
    }, { theme, glassEnabled });
    episode(api);
    await page.goto('/edit-media/101');
    const actorSave = page.locator('.media-edit-page .editor-actions').getByRole('button', { name: '保存', exact: true });
    const extract = page.getByTestId('episode-extract-credits');
    const pageReadings = await measureReadingContrast(page, [
      { role: 'actor-save', locator: actorSave.locator('.v-btn__content') },
      { role: 'episode-credits', locator: extract.locator('.v-btn__content') },
    ]);
    for (const reading of pageReadings) expect(reading.contrast, reading.role).toBeGreaterThanOrEqual(4.5);
    await page.getByTestId('media-info-open').click();
    const save = page.getByTestId('media-info-save');
    const current = page.getByTestId('media-info-extract-credits');
    await expect.poll(() => save.evaluate(element => {
      for (let node = element; node; node = node.parentElement) {
        if (node.getAnimations().some(animation => !['finished', 'idle'].includes(animation.playState))) return false;
      }
      return true;
    })).toBe(true);
    const modalReadings = await measureReadingContrast(page, [
      { role: 'media-info-save', locator: save.locator('.v-btn__content') },
      { role: 'current-episode-credits', locator: current.locator('.v-btn__content') },
    ]);
    for (const reading of modalReadings) expect(reading.contrast, reading.role).toBeGreaterThanOrEqual(4.5);
    for (const button of [save, current]) {
      const geometry = await button.evaluate(element => {
        const content = element.querySelector('.v-btn__content'), button = element.getBoundingClientRect();
        const range = document.createRange(); range.selectNodeContents(content); const text = range.getBoundingClientRect();
        return { left: text.left - button.left, right: button.right - text.right, top: text.top - button.top, bottom: button.bottom - text.bottom };
      });
      expect(geometry.left).toBeGreaterThanOrEqual(4);
      expect(geometry.right).toBeGreaterThanOrEqual(4);
      expect(geometry.top).toBeGreaterThanOrEqual(2);
      expect(geometry.bottom).toBeGreaterThanOrEqual(2);
    }
  });
}
