import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { transform } from 'esbuild';

async function loadTypeScript(path) {
  const { code } = await transform(readFileSync(new URL(path, import.meta.url), 'utf8'), { loader: 'ts', format: 'esm' });
  return import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);
}

test('登录页沿用标准、均衡和高质量偏好，不升级图形开销', async () => {
  const { getLoginGlassOpticalSettings } = await loadTypeScript('../src/utils/loginPresentation.ts');
  for (const quality of ['css', 'balanced', 'high']) {
    const preference = { appearance: 'frosted', preset: 'balanced', quality, deformationStrength: .7, flowStrength: .8,
      reflectionStrength: .9, transmissionStrength: .6, translationStrength: .5, transparencyStrength: .4 };
    assert.deepEqual(getLoginGlassOpticalSettings(preference), preference);
  }
});

test('装饰 WebGL 拒绝低性能和丢失的上下文，保留静态降级', async () => {
  const { createDecorativeWebGLContext } = await loadTypeScript('../src/utils/webgl.ts');
  const original = globalThis.WebGL2RenderingContext;
  try {
    globalThis.WebGL2RenderingContext = class {};
    for (const lost of [false, true]) {
      const context = { isContextLost: () => lost };
      const canvas = { getContext(type, options) {
        assert.equal(type, 'webgl2');
        assert.equal(options.failIfMajorPerformanceCaveat, true);
        assert.equal(options.depth, false);
        return context;
      } };
      assert.equal(createDecorativeWebGLContext(canvas, { depth: false, failIfMajorPerformanceCaveat: false }), lost ? null : context);
    }
    assert.equal(createDecorativeWebGLContext({ getContext: () => null }, {}), null);
    delete globalThis.WebGL2RenderingContext;
    assert.equal(createDecorativeWebGLContext({ getContext: () => assert.fail('不支持 WebGL 时不得申请') }, {}), null);
  } finally {
    if (original === undefined) delete globalThis.WebGL2RenderingContext;
    else globalThis.WebGL2RenderingContext = original;
  }
});
