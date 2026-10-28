# Emby Toolkit

面向 Emby 媒体库的增强工具，后端使用 Rust，界面使用 Vue，数据保存于 PostgreSQL。支持 MoviePilot/STRM 入库增强、元数据与媒体信息编辑、AI 中文化、115 指纹、封面、实时监控和 Telegram 通知。

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

在通用设置中配置共享密钥，插件通知地址为 `http://服务器IP:5257/webhook/moviepilot`，插件与 ETK 使用同一密钥，媒体路径须满足 ETK 的本地目录配置。

Emby 直接入库处理由“通用设置 → Emby → Webhook入库”开关控制，默认关闭。需要使用时开启该选项，并配置 `http://服务器IP:5257/webhook/emby?token=共享密钥`；MoviePilot 完成通知保持可用。

用户中心可更换或删除头像，并统一配置 Telegram 机器人、通知类型和 Telegram 用户ID（私聊ID）；手动处理编辑电影、电视剧、季和分集；任务中心提供同步、演员与角色中文化、媒体信息备份/恢复、指纹、音轨纠正及维护操作。实时监控按现有媒体目录与开关运行，任务支持暂停、恢复和停止。

右上角“主题与壁纸”提供 MoviePilot v3 标准玻璃外观：浅色、深色、跟随系统，支持主色、圆角、清透/色调/磨砂材质及阅读风格。壁纸可使用图片地址或本地上传，外观设置即时生效并保存在当前浏览器。

## 本地构建

```sh
docker build -t emby-toolkit .
```

镜像包含 Rust 服务、Vue 静态页面和 ffmpeg/ffprobe，启动日志为“应用程序启动”。

设置按卡片独立保存；Emby 媒体库采用忽略名单，默认不选即处理全部。媒体中心使用右侧字母导航，通知中心以居中窗口浏览实时与分页历史日志。

原媒体归档和短文件名恢复工具已移植到同一 Rust 可执行文件，无需 Python：

```sh
docker exec emby-toolkit /app/emby-toolkit media-archive pack /media/Movie /backup/movie.tar.gz
docker exec emby-toolkit /app/emby-toolkit media-archive unpack /backup/movie.tar.gz /media --keep-archive
docker exec emby-toolkit /app/emby-toolkit restore-media-short-names /media/Movie --dry-run
```

归档解包保留原验证顺序，成功后默认删除归档；短文件名恢复支持预览，配对不唯一或目标已存在时停止。
