import { test, expect } from './fixtures.js';
import { measureReadingContrast } from './reading-contrast.js';

for (const theme of ['light','dark']) for (const width of [390,1440]) test(`MP 模板高亮和路径预览可读 ${theme} ${width}px`, async ({page,api},testInfo) => {
  test.setTimeout(60_000);
  await page.setViewportSize({width,height:1000});
  await page.addInitScript(theme=>localStorage.setItem('moviepilot-theme-customizer',JSON.stringify({theme,glassEnabled:false})),theme);
  await page.goto('/setting?tab=rule');
  const field=page.locator('[data-field="movie_rename_format"]');
  await field.scrollIntoViewIfNeeded();
  await expect(field.locator('.ace_editor')).toHaveClass(new RegExp(theme==='dark'?'ace-github-dark':'ace-github-light-default'));
  const input=page.getByRole('textbox',{name:'电影重命名格式',exact:true});
  const template="{{title}}{% if year %} ({{year}}){% endif %}/{{title}}{{fileExt}}";
  await input.press('ControlOrMeta+A');await input.pressSequentially(template);
  await expect.poll(()=>api.requests.filter(row=>row.path==='/api/config/rename_preview').at(-1)?.body.movie_format).toBe(template);
  const tokens=[field.locator('.ace_keyword').filter({hasText:'if'}).first(),field.locator('.ace_variable').filter({hasText:'title'}).first(),field.locator('.ace_constant').filter({hasText:'{{'}).first()];
  for (const token of tokens) {
    await expect(token).toBeVisible();
    for(const reading of await measureReadingContrast(page,[{role:'模板高亮',locator:token}]))expect(reading.contrast,JSON.stringify(reading)).toBeGreaterThanOrEqual(4.5);
  }
  expect(new Set(await Promise.all(tokens.map(token=>token.evaluate(node=>getComputedStyle(node).color)))).size).toBeGreaterThan(1);
  const dimensions=await field.evaluate(node=>({width:node.clientWidth,scrollWidth:node.scrollWidth}));
  expect(dimensions.scrollWidth).toBeLessThanOrEqual(dimensions.width+1);
  await expect(page.getByText(/电影示例|电视剧示例/)).toHaveCount(0);
  await page.screenshot({path:testInfo.outputPath('template-editor.png')});
  await page.getByRole('button',{name:'保存整理与刮削',exact:true}).click();
  await expect.poll(()=>api.requests.filter(row=>row.method==='POST' && row.path==='/api/config').at(-1)?.body.movie_rename_format).toBe(template);
  await page.reload();await field.scrollIntoViewIfNeeded();
  await expect(field.locator('.ace_content')).toContainText('{{title}}');
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1)).toBeTruthy();
});
