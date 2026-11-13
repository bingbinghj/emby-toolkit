import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const page = readFileSync(new URL('../src/components/MediaEditPage.vue', import.meta.url), 'utf8');
function definition(name) {
  const start = page.indexOf(`const ${name} = `);
  assert.ok(start >= 0);
  return page.slice(start, page.indexOf('\n};', start) + 4);
}

function fixture(operation) {
  let resolve, reject;
  const pending = new Promise((yes, no) => { resolve = yes; reject = no; });
  const requests = [], routes = [], notices = [], destroyed = [];
  const context = vm.createContext({
    itemId: { value: '101' }, currentEditMediaId: { value: '101' }, renameTargetId: { value: '101' },
    currentMediaInfoVersion: { value: { emby_id: '101' } }, mediaInfoVersionOptions: { value: [{}, {}] },
    showMediaInfoEditor: { value: true }, mediaOperationFeedback: { value: null },
    isReplacingEpisodeImages: { value: false }, isDeletingMediaVersion: { value: false },
    imageRefreshTokens: { value: { poster: 1 } },
    currentEditImageType: { value: 'poster' }, isUploadingImage: { value: false },
    imageUrlInput: { value: 'https://images.invalid/original.png' }, showUrlPrompt: { value: true },
    imageUrlPromptSession: 0, FormData, console: { error() {} },
    mediaDetailsLoadSequence: 1, mediaRecoverySession: 1, mediaInfoLoadSequence: 1,
    axios: { post: async path => { requests.push(path); return pending; } },
    router: { replace: async route => routes.push(route) }, fetchMediaDetails: async () => {},
    message: { loading: () => ({ destroy: () => destroyed.push(true) }),
      ...Object.fromEntries(['success', 'warning', 'error'].map(type => [type, text => notices.push({ type, text })])) },
  });
  const functions = [...new Set(['mediaOperationDetails', 'uploadImagePayload', 'openUrlPrompt', operation])];
  vm.runInContext(`${functions.map(definition).join('\n')}\nthis.run = ${operation}; this.openPrompt = openUrlPrompt;`, context);
  return { context, requests, routes, notices, destroyed, resolve, reject };
}

test('a completed image upload refreshes the submitted image type after the picker changes', async () => {
  const f = fixture('uploadImagePayload');
  f.context.imageRefreshTokens.value.fanart = 1;
  const uploading = f.context.run({ image_type: 'poster', image_url: 'https://images.invalid/original.png' });
  f.context.currentEditImageType.value = 'fanart';
  f.resolve({ status: 200, data: { message: '图片已保存' } });
  await uploading;
  assert.notEqual(f.context.imageRefreshTokens.value.poster, 1);
  assert.equal(f.context.imageRefreshTokens.value.fanart, 1);
});

for (const outcome of ['success', 'failure']) {
  test(`image upload ${outcome} after navigation leaves the new media unchanged`, async () => {
    const f = fixture('uploadImagePayload');
    const uploading = f.context.run({ image_type: 'poster', image_url: 'https://images.invalid/original.png' });
    f.context.itemId.value = '200';
    f.context.mediaDetailsLoadSequence++;
    f.context.mediaRecoverySession++;
    if (outcome === 'success') f.resolve({ status: 200, data: { message: '原图片已保存' } });
    else f.reject({ response: { status: 500, data: { error: '原图片上传失败' } } });
    await uploading;
    assert.equal(f.context.imageRefreshTokens.value.poster, 1);
    assert.deepEqual(f.notices, []);
    assert.equal(f.context.isUploadingImage.value, false);
    assert.equal(f.destroyed.length, 1);
  });
}

for (const type of ['poster', 'fanart']) {
  test(`an old URL upload cannot close a reopened ${type} prompt`, async () => {
    const f = fixture('submitUrlImage');
    const uploading = f.context.run();
    f.context.openPrompt(type);
    f.context.imageUrlInput.value = 'https://images.invalid/original.png';
    f.resolve({ status: 200, data: { message: '原图片已保存' } });
    await uploading;
    assert.equal(f.context.showUrlPrompt.value, true);
    assert.equal(f.context.currentEditImageType.value, type);
  });
}

test('a failed URL upload preserves the current input for retry', async () => {
  const f = fixture('submitUrlImage');
  const uploading = f.context.run();
  f.reject({ response: { status: 500, data: { error: '上传失败' } } });
  await uploading;
  assert.equal(f.context.showUrlPrompt.value, true);
  assert.equal(f.context.imageUrlInput.value, 'https://images.invalid/original.png');
  assert.equal(f.notices[0].type, 'error');
});

test('only a confirmed upload closes its unchanged URL prompt', async () => {
  const f = fixture('submitUrlImage');
  const uploading = f.context.run();
  f.resolve({ status: 200, data: { message: '图片已保存' } });
  await uploading;
  assert.equal(f.context.showUrlPrompt.value, false);
});

test('FormData uploads refresh their original image type', async () => {
  const f = fixture('uploadImagePayload');
  f.context.currentEditImageType.value = 'fanart';
  f.context.imageRefreshTokens.value.fanart = 1;
  const payload = new FormData();
  payload.set('image_type', 'poster');
  const uploading = f.context.run(payload);
  f.resolve({ status: 200, data: { message: '海报已保存' } });
  await uploading;
  assert.notEqual(f.context.imageRefreshTokens.value.poster, 1);
  assert.equal(f.context.imageRefreshTokens.value.fanart, 1);
});

test('a duplicate upload cannot submit another request or clear the first loading state', async () => {
  const f = fixture('uploadImagePayload');
  const payload = { image_type: 'poster', image_url: 'https://images.invalid/original.png' };
  const uploading = f.context.run(payload);
  assert.equal(await f.context.run(payload), false);
  assert.equal(f.requests.length, 1);
  assert.equal(f.context.isUploadingImage.value, true);
  f.resolve({ status: 200, data: { message: '图片已保存' } });
  assert.equal(await uploading, true);
  assert.equal(f.context.isUploadingImage.value, false);
  assert.equal(f.destroyed.length, 1);
});

test('navigating to another media closes the previous URL prompt', () => {
  const f = fixture('uploadImagePayload');
  for (const name of ['showEpisodeSelector', 'showRenameEditor', 'showImageEditor', 'showAddActorModal', 'showTmdbSelector']) f.context[name] = { value: false };
  for (const name of ['imagePreview', 'tmdbImagesCache', 'creditsExtractionFeedback']) f.context[name] = { value: null };
  f.context.closeMediaInfoEditor = () => {};
  f.context.debouncedSearchActors = { cancel() {} };
  f.context.loadMediaRecovery = () => {};
  f.context.mediaRecoveryError = { value: '旧媒体恢复错误' };
  f.context.route = { params: { itemId: '101' } };
  let change;
  f.context.watch = (getter, callback) => { change = callback; };
  const start = page.indexOf('watch(() => route.params.itemId,');
  vm.runInContext(page.slice(start, page.indexOf('\n});', start) + 5), f.context);
  change('200');
  assert.equal(f.context.showUrlPrompt.value, false);
  assert.equal(f.context.mediaRecoveryError.value, '');
});

const operations = [
  ['deleteCurrentMediaVersion', { deleted_item_id: '101', remaining_item_id: '102' }, 'isDeletingMediaVersion'],
  ['replaceEpisodeImages', { message: '原媒体任务完成', refreshed_count: 1 }, 'isReplacingEpisodeImages'],
];

for (const [operation, result, loading] of operations) {
  const invalidations = ['navigation', 'unmount', 'same-media-new-route'];
  if (operation === 'deleteCurrentMediaVersion') invalidations.push('reopened-editor');
  for (const invalidation of invalidations) {
    for (const outcome of ['success', 'failure']) {
      test(`${operation} ${outcome} after ${invalidation} leaves the new context unchanged`, async () => {
        const f = fixture(operation);
        const running = f.context.run();
        if (invalidation === 'navigation') {
          f.context.itemId.value = f.context.currentEditMediaId.value = '200';
          f.context.currentMediaInfoVersion.value = { emby_id: '200' };
          f.context.mediaDetailsLoadSequence++;
          f.context.mediaRecoverySession++;
        } else if (invalidation === 'unmount') {
          f.context.mediaDetailsLoadSequence++;
          f.context.mediaRecoverySession++;
          f.context.mediaInfoLoadSequence++;
        } else if (invalidation === 'reopened-editor') {
          f.context.mediaInfoLoadSequence++;
        } else {
          f.context.mediaRecoverySession++;
        }
        f.context.mediaOperationFeedback.value = { type: 'info', message: '新页面的反馈' };
        const target = f.context.itemId.value;
        if (outcome === 'success') f.resolve({ status: 200, data: result });
        else f.reject({ response: { status: 500, data: { error: '旧任务失败' } } });
        await running;
        assert.equal(f.context.itemId.value, target);
        assert.equal(f.context.showMediaInfoEditor.value, true);
        assert.equal(f.context.imageRefreshTokens.value.poster, 1);
        assert.equal(f.context.mediaOperationFeedback.value.message, '新页面的反馈');
        assert.deepEqual(f.routes, []);
        assert.deepEqual(f.notices, []);
        assert.equal(f.destroyed.length, 1);
        assert.equal(f.context[loading].value, false);
      });
    }
  }
  test(`${operation} ignores a duplicate submission while the first task is pending`, async () => {
    const f = fixture(operation);
    const running = f.context.run();
    const repeated = f.context.run();
    const submitted = f.requests.length;
    f.resolve({ status: 200, data: result });
    await Promise.all([running, repeated]);
    assert.equal(submitted, 1);
    assert.equal(f.destroyed.length, 1);
    assert.equal(f.context[loading].value, false);
  });
}
