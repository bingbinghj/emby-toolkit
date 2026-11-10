# ETK 前端

界面基于 [MoviePilot-Frontend v3](https://github.com/jxxghp/MoviePilot-Frontend/tree/v3) 的原生 Vue/Vuetify 组件、布局样式、主题定制器和玻璃运行时，保留 ETK 名称、Logo 和业务功能。

上游版本：**3.2.6**，提交 `609ea05eac7efc7c2cb31edcf5364d26c9c592f1`。Vuetify 固定为上游使用的 **3.7.3**。复制的代码保留 MIT 来源标记，许可证位于 [third-party/MoviePilot-LICENSE](third-party/MoviePilot-LICENSE)。

## 开发和验证

需要 Node.js 20.19 或更新版本。

```sh
npm ci
npm run dev
npm test
npm run build
```

开发代理连接 `http://localhost:5257` 的 ETK Rust 接口。生产包由 ETK 在同一来源提供。

浏览器测试使用严格的本地接口模拟，覆盖认证、业务表单、后台任务终态、导航、主题和壁纸。启动生产预览后执行：

```sh
npx playwright install chromium
npm run preview -- --host 127.0.0.1 --port 4173
# 在另一终端执行
npm run test:ui -- --workers=1
```

可通过 `ETK_UI_BASE_URL` 指定测试地址，通过 `ETK_UI_CHROMIUM_PATH` 使用已有 Chromium 可执行文件。

## 与 Rust 接口衔接

- 认证继续使用同源 `etk_session` Cookie，保留初始化、登录和退出流程。
- 全局 Axios 拦截器等待 `202 + task_id` 的实际终态，保留失败、取消及警告详情。
- 媒体信息修改继续提交目标 ID 和轨道补丁，保留字幕、关联版本及草稿保护。
- 配置保存保持字段子集或已保存基线；读取失败会阻止空配置覆盖保存。
- `/api` 和 `/image_proxy` 继续由 ETK 提供；管理 API 的 PWA 策略为 `NetworkOnly + no-store`。

## 外观

主题支持明亮、深色和系统偏好，包含主色、圆角、纵向/折叠/水平导航及完整玻璃设置。玻璃设置提供材质、质量、动态模式、光学参数、预览、取消和保存。浏览器不支持的渲染能力沿用上游 CSS 回退。

壁纸支持本地预设、TMDb、Bing、Emby 背景、自定义 API、固定 URL 和图片上传，提供 MP 的轮换间隔与淡化效果。主题及壁纸在浏览器中持久化，迁移已有 ETK 外观设置；字体采用 Inter 和 Noto Sans SC。图标由构建脚本提取并打包到本地资源。

## 仪表盘刷新

统计、最新入库和媒体库沿用 MoviePilot v3 的成功快照逻辑：按组件创建时的登录用户隔离，缓存有效期 24 小时，仅完整成功结果（包括空列表）更新快照。刷新失败保留成功数据并显示时间和重试入口；首次失败显示未读取状态。115 容量只保留当前页面内的成功数据，不写入快照。

参考上游 `609ea05eac7efc7c2cb31edcf5364d26c9c592f1` 的 `useDashboardSnapshot.ts`、`AnalyticsStorage.vue`、`AnalyticsMediaStatistic.vue`、`MediaServerLatest.vue` 和 `MediaServerLibrary.vue`。Rust `/api/dashboard/media` 的分区错误映射为对应卡片未成功，快照仅含计数及安全的海报/标题字段。
