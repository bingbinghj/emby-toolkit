# Emby Toolkit

面向 Emby 媒体库的增强工具，后端使用 Rust，界面使用 Vue，数据保存于 PostgreSQL。支持 MoviePilot/STRM 入库增强、元数据与媒体信息编辑、AI 中文化、115 指纹、片头与电视剧片尾提取、封面、实时监控和 Telegram 通知。

## 部署

支持 `linux/amd64` 和 `linux/arm64`，访问端口为 `5257`。每次更新 `main` 后，GitHub Actions 自动构建并发布 `ghcr.io/bingbinghj/emby-toolkit:latest`。

下载本仓库的 `docker-compose.yml`，在其所在目录执行：

```sh
docker compose up -d
```

打开 `http://服务器IP:5257`，完成 Emby 连接设置，以 Emby 管理员账户登录。已有部署保留原数据库、`/config` 和媒体目录挂载，只需将应用镜像替换为 `ghcr.io/bingbinghj/emby-toolkit:latest`。

`/config` 保存本地配置、日志与备份。媒体相关操作需要将现有本地 STRM/媒体目录按原路径挂载到容器，并在设置中填写相同路径。需要读取 CD2 源文件时保留现有 CD2 挂载；媒体信息编辑仍使用本地媒体目录中的真实 JSON。

| 环境变量 | 用途 |
| --- | --- |
| `APP_DATA_DIR` | 配置目录，默认 `/config` |
| `DB_HOST` / `DB_PORT` | PostgreSQL 地址和端口 |
| `DB_USER` / `DB_PASSWORD` / `DB_NAME` | PostgreSQL 账户和数据库 |
| `PUID` / `PGID` | 容器内运行用户和组，默认 `0` |
| `UMASK` | 文件权限掩码，默认 `000` |
| `TZ` | 时区，默认 `Asia/Shanghai` |

原 `/config/config.ini` 数据库设置与数据库中的应用配置继续使用。Docker 管理类任务需要挂载 `/var/run/docker.sock`，重启目标容器名称按现有设置填写。

## 入库与使用

推荐使用 [ETK刮削完成通知插件](https://github.com/bingbinghj/MoviePilot-Plugins) 连接 MoviePilot，支持 MoviePilot v1、v2、v3。

```text
MoviePilot 整理和基础刮削完成 → ETK Webhook
→ 刷新所属 Emby 媒体库并等待建库 → 验收媒体信息
→ 元数据增强与 AI 中文化 → 115 PC/SHA1 补齐
→ Emby 最终刷新 → 媒体库封面 → 通知
```

在“设置 → 系统 → 媒体服务器”中点击 Emby 卡片配置共享密钥，插件通知地址为 `http://服务器IP:5257/webhook/moviepilot`，插件与 ETK 使用同一密钥，媒体路径须满足 ETK 的本地目录配置。

Emby 直接入库处理由“设置 → 系统 → 媒体服务器 → Webhook入库”开关控制，默认关闭。需要使用时开启该选项，并配置 `http://服务器IP:5257/webhook/emby?token=共享密钥`；MoviePilot 完成通知保持可用。

仪表盘展示 115 网盘空间、Emby 媒体统计、聚合后的最新入库和“我的媒体库”。右下角按钮可调整模块位置、尺寸和显隐，电脑、平板、手机分别保存布局，并按登录用户同步到其他浏览器。媒体库使用 Emby 原封面，每行最多七个，右上角显示真实媒体数量。

媒体整理展示 ETK 入库处理历史，可按标题和状态筛选并重新整理所选记录。电影包含关联版本，剧集只处理所选单集；重新整理保留原记录时间并更新实际处理结果。真实入库历史从更新后首次启用时开始记录，旧的作品处理缓存不会被当作逐集入库历史。用户管理可新增、编辑和删除真实 Emby 用户，管理资料、权限、播放配置、密码及头像；头像支持上传、取消和恢复默认。Telegram 入库通知使用 MP 的 Jinja2 模板，大小和格式取实际媒体源；渠道、接收账号和发送范围集中在“设置 → 通知”。手动处理支持电影、电视剧、季和分集；任务中心提供同步、中文化、媒体信息备份/恢复、指纹、音轨纠正、全库电视剧片尾提取及维护操作，支持暂停、恢复和停止。

入库自动处理按各自开关执行：标题填充验收通过后提取片头，再提取电视剧片尾，最后提取视频预览缩略图。媒体编辑页可单独提取当前分集片尾，电影不处理。图片语言偏好和 SHA1 检测不依赖文件系统监控开关；实际 Emby 和媒体提取仍遵循神医助手的线程、冷却配置。

顶栏搜索按所在页面检索当前媒体库、入库记录、用户、设置项或任务；仪表盘和媒体编辑页搜索全部媒体。

右上角“主题与壁纸”提供 MoviePilot v3 标准玻璃外观：浅色、深色、跟随系统，支持主色、圆角、清透/色调/磨砂材质及阅读风格。壁纸支持 TMDb、Bing、Emby 背景、自定义 API、图片地址和本地上传，提供 MP 轮换间隔；外观设置保存在当前浏览器，字体采用 MP 的系统字体配置。

## 本地构建

```sh
docker build -t emby-toolkit .
```

镜像包含 Rust 服务、Vue 静态页面及支持 chromaprint 声纹提取的 ffmpeg/ffprobe，启动日志为“应用程序启动”。

设置采用 MP 的顶栏分类和展开卡片，各卡片独立保存；媒体元数据目录支持浏览本地挂载目录。Emby 媒体库采用排除名单，默认不选即处理全部。我的媒体库详情提供右侧字母导航，顶栏“实时日志”窗口支持实时日志、分页历史日志、级别与关键词筛选和原文导出，日志按来源分组卡片完整换行。媒体信息编辑页仅显示当前媒体对应的待恢复记录，通过媒体与原配置验收后删除备份并允许保存；清理失败保留“仅清理备份”入口，可重试删除，无需再次导入媒体。

批量编辑音轨和字幕先核对原轨道，再尝试唯一的逻辑轨道匹配，兼容分集轨道重排和通用中文语言码差异。无法唯一匹配时保留目标原值并显示具体原因；未编辑的 `chi`／`zho` 原码保持不变，明确修改语言或简繁标题时才转换。顶栏任务卡按整个顶栏居中，媒体轨道技术标签保持单行，窄屏可横向滚动。

原媒体归档和短文件名恢复工具已移植到同一 Rust 可执行文件，无需 Python：

```sh
docker exec emby-toolkit /app/emby-toolkit media-archive pack /media/Movie /backup/movie.tar.gz
docker exec emby-toolkit /app/emby-toolkit media-archive unpack /backup/movie.tar.gz /media --keep-archive
docker exec emby-toolkit /app/emby-toolkit restore-media-short-names /media/Movie --dry-run
```

归档解包保留原验证顺序，成功后默认删除归档；短文件名恢复支持预览，配对不唯一或目标已存在时停止。
