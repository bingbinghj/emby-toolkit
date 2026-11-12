这些 Playwright 用例验证 Vuetify 页面与 ETK 接口的实际交互。`fixtures.js` 提供明确的接口响应；未知 API 和未处理的页面异常都会使测试失败。配置及媒体数据均为测试数据，浏览器不会向真实 Emby 写入。

Node 和浏览器测试默认串行运行。浏览器临时文件默认保存在项目的 `test-results/tmp`，可通过 `TMPDIR` 指定其它磁盘目录。

导航默认等待 DOMContentLoaded，随后各用例等待真实控件、接口结果及图片解码。第三方字体的 `window.load` 不作为应用已就绪的条件；显式指定的导航等待条件仍会保留。

先启动前端服务，再从 `frontend` 目录运行：

```bash
npm run dev -- --host 127.0.0.1 --port 4173 --strictPort
```

```bash
ETK_UI_BASE_URL=http://127.0.0.1:4173 npx playwright test
```

可用 `ETK_UI_CHROMIUM_PATH` 指定现有 Chromium 可执行文件；不指定时使用 Playwright 安装的默认浏览器。用例覆盖路由和认证、10 张通用设置卡片、后台任务终态、媒体语言编辑和警告、演员草稿保护、导航菜单及主题和壁纸。外观用例包含页面刷新和图片解码，使用 60 秒总时限，行为断言仍使用 8 秒时限。
