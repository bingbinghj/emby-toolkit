use crate::{
    AppState,
    error::{ApiError, Result},
    media_edit::list,
    state::string,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub struct Service {
    pub api_base: String,
}
impl Default for Service {
    fn default() -> Self {
        Self {
            api_base: "https://api.telegram.org".into(),
        }
    }
}

pub fn escape(text: &str) -> String {
    text.chars()
        .map(|c| {
            if "_*[]()~`>#+-=|{}.!".contains(c) {
                format!("\\{c}")
            } else {
                c.to_string()
            }
        })
        .collect()
}
fn code(text: &str) -> String {
    text.replace('\\', "\\\\").replace('`', "\\`")
}
pub fn ranges(episodes: &[(i64, i64)]) -> String {
    let mut seasons = BTreeMap::<i64, BTreeSet<i64>>::new();
    for (s, e) in episodes {
        seasons.entry(*s).or_default().insert(*e);
    }
    let mut parts = vec![];
    for (s, episodes) in seasons {
        let episodes = episodes.into_iter().collect::<Vec<_>>();
        let (mut start, mut previous) = (episodes[0], episodes[0]);
        let label = |start, previous| {
            if start == previous {
                format!("S{s:02}E{start:02}")
            } else {
                format!("S{s:02}E{start:02}-E{previous:02}")
            }
        };
        for e in episodes.into_iter().skip(1) {
            if e == previous + 1 {
                previous = e;
            } else {
                parts.push(label(start, previous));
                start = e;
                previous = e;
            }
        }
        parts.push(label(start, previous));
    }
    parts.join(", ")
}
fn overview_episodes(text: &str) -> Vec<(i64, i64)> {
    let pattern = regex::Regex::new(r"(?i)S(\d{1,3})\s*E(\d{1,4})(?:\s*-\s*E?(\d{1,4}))?").unwrap();
    let mut episodes = Vec::new();
    let mut seen = BTreeSet::new();
    for captures in pattern.captures_iter(text) {
        let (Ok(season), Ok(mut start)) = (captures[1].parse::<i64>(), captures[2].parse::<i64>())
        else {
            continue;
        };
        let Ok(mut end) = captures
            .get(3)
            .map_or(Ok(start), |value| value.as_str().parse())
        else {
            continue;
        };
        if season <= 0 || start <= 0 || end <= 0 {
            continue;
        }
        if end < start {
            std::mem::swap(&mut start, &mut end);
        }
        if end - start > 500 {
            end = start;
        }
        for episode in start..=end {
            if seen.insert((season, episode)) {
                episodes.push((season, episode));
            }
        }
    }
    episodes
}
async fn episode_numbers(state: &AppState, ids: &[String]) -> Vec<(i64, i64)> {
    let mut episodes = Vec::new();
    for id in ids.iter().map(|id| id.trim()).filter(|id| !id.is_empty()) {
        if let Ok(item) =
            crate::media_edit::details(state, id, "IndexNumber,ParentIndexNumber").await
            && let (Some(season), Some(episode)) = (
                item["ParentIndexNumber"].as_i64(),
                item["IndexNumber"].as_i64(),
            )
            && season != 0
            && episode != 0
        {
            episodes.push((season, episode));
            continue;
        }
        match sqlx::query_scalar::<_, Value>("SELECT jsonb_build_array(season_number,episode_number) FROM media_metadata WHERE item_type='Episode' AND emby_item_ids_json @> $1 LIMIT 1")
            .bind(json!([id])).fetch_optional(&state.db).await
        {
            Ok(Some(numbers)) => {
                if let (Some(season), Some(episode)) = (numbers[0].as_i64(), numbers[1].as_i64())
                    && season != 0 && episode != 0
                {
                    episodes.push((season, episode));
                }
            }
            Err(error) => tracing::warn!(item_id = id, "[通知] 从数据库读取分集号失败：{error}"),
            _ => (),
        }
    }
    episodes
}
pub fn title_pages(name: &str, report: &Value) -> Vec<String> {
    let heading = format!("⚠️ *标题待手动填充*：{}\n", escape(name));
    let mut page = heading.clone();
    let mut pages = vec![];
    for entry in list(&report["pending"]) {
        let label =
            if let (Some(s), Some(e)) = (entry["season"].as_i64(), entry["episode"].as_i64()) {
                format!("S{s:02}E{e:02}")
            } else {
                entry["name"]
                    .as_str()
                    .or_else(|| entry["item_id"].as_str())
                    .unwrap_or("")
                    .into()
            };
        let kind = match entry["type"].as_str() {
            Some("Audio") => "音轨",
            Some("Subtitle") => "字幕",
            _ => "媒体项",
        };
        let ordinal = entry["ordinal"]
            .as_u64()
            .map(|n| n.to_string())
            .unwrap_or_default();
        let language = entry["language"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("未标注")
            .chars()
            .take(80)
            .collect::<String>();
        let reason = entry["reason"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("需手动检查")
            .chars()
            .take(300)
            .collect::<String>();
        let line = format!(
            "{}\n",
            escape(&format!(
                "{} · {kind}{ordinal}（{language}）：{reason}",
                label.chars().take(180).collect::<String>()
            ))
        );
        if page.chars().count() + line.chars().count() > 3500 {
            pages.push(page.trim_end().into());
            page = heading.clone();
        }
        page.push_str(&line);
    }
    if page != heading {
        pages.push(page.trim_end().into());
    }
    pages
}
pub async fn media(
    state: &AppState,
    item: &Value,
    updated: bool,
    episode_ids: &[String],
    title_report: &Value,
) -> Result<()> {
    let config = state.config.read().await.clone();
    let token = string(&config, "telegram_bot_token");
    if token.is_empty() || !list(&config["telegram_notify_types"]).contains(&json!("library_new")) {
        return Ok(());
    }
    let fallback_name = format!("ID:{}", string(item, "Id"));
    let name = item["Name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .unwrap_or(&fallback_name);
    let title = if let Some(year) = item["ProductionYear"].as_i64().filter(|year| *year != 0) {
        format!("{name} ({year})")
    } else {
        name.into()
    };
    let mut episodes = if item["Type"] == "Series" {
        episode_numbers(state, episode_ids).await
    } else {
        vec![]
    };
    if item["Type"] == "Series" && episodes.is_empty() {
        episodes = overview_episodes(&string(item, "Overview"));
    }
    let mut episode_text = ranges(&episodes);
    if episode_text.chars().count() > 72 {
        episode_text = format!(
            "{} ...",
            episode_text
                .split(", ")
                .take(3)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let episode_text = if episode_text.is_empty() {
        String::new()
    } else {
        format!("🎞️ *集数*: `{}`\n", code(&episode_text))
    };
    let overview = item["Overview"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("暂无剧情简介。");
    let overview = if overview.chars().count() > 200 {
        format!("{}...", overview.chars().take(200).collect::<String>())
    } else {
        overview.into()
    };
    let mut caption = format!(
        "{} *{}* {}\n\n{episode_text}⏰ *时间*: `{}`\n📝 *剧情*: {}",
        if item["Type"] == "Movie" {
            "🎬"
        } else {
            "📺"
        },
        escape(&title),
        if updated {
            "🔄 已更新"
        } else {
            "✨ 入库成功"
        },
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        escape(&overview)
    );
    let mut pages = title_pages(name, title_report);
    if !pages.is_empty() {
        if pages.len() == 1 && caption.chars().count() + pages[0].chars().count() + 2 <= 1000 {
            caption.push_str("\n\n");
            caption.push_str(&pages.pop().unwrap());
        } else {
            caption.push_str("\n\n⚠️ *标题待手动填充*：清单见后续消息");
        }
    }
    let image: Option<String> = sqlx::query_scalar("SELECT COALESCE(NULLIF(m.backdrop_path,''),NULLIF(m.poster_path,''),CASE WHEN m.item_type='Episode' THEN COALESCE(NULLIF(p.backdrop_path,''),NULLIF(p.poster_path,'')) END) FROM media_metadata m LEFT JOIN media_metadata p ON m.parent_series_tmdb_id=p.tmdb_id AND p.item_type='Series' WHERE m.emby_item_ids_json @> $1 LIMIT 1")
        .bind(json!([string(item,"Id")])).fetch_optional(&state.db).await
        .unwrap_or_else(|error| { tracing::warn!("[通知] 读取通知图片失败：{error}"); None }).flatten();
    let mut targets = sqlx::query_scalar::<_,Option<String>>("SELECT e.telegram_chat_id FROM emby_users u JOIN emby_users_extended e ON u.id=e.emby_user_id WHERE u.is_administrator=true AND e.telegram_chat_id IS NOT NULL")
        .fetch_all(&state.db).await
        .unwrap_or_else(|error| { tracing::warn!("[通知] 读取管理员接收列表失败：{error}"); vec![] })
        .into_iter().flatten().collect::<BTreeSet<_>>();
    let channel = string(&config, "telegram_channel_id");
    if !channel.is_empty() {
        targets.insert(channel);
    }
    for target in targets {
        let target = target.trim();
        if target.is_empty() {
            continue;
        }
        tracing::info!(item_id = %string(item, "Id"), "[通知] 开始发送入库结果");
        let target = if target.starts_with("https://t.me/") {
            format!("@{}", target.rsplit('/').next().unwrap_or(""))
        } else {
            target.to_string()
        };
        let (method, payload) = if let Some(image) = &image {
            (
                "sendPhoto",
                json!({"chat_id":target,"photo":format!("https://image.tmdb.org/t/p/w780{image}"),"caption":caption,"parse_mode":"MarkdownV2","disable_notification":false}),
            )
        } else {
            (
                "sendMessage",
                json!({"chat_id":target,"text":caption,"parse_mode":"MarkdownV2","disable_web_page_preview":true,"disable_notification":false}),
            )
        };
        match send(state, &token, method, &payload).await {
            Ok(()) => tracing::info!(item_id = %string(item, "Id"), "[通知] 入库结果已发送"),
            Err(error) => {
                tracing::warn!(item_id = %string(item, "Id"), target, "[通知] 入库结果发送失败：{}", error.1)
            }
        }
        for page in &pages {
            if let Err(error) = send(state, &token, "sendMessage", &json!({"chat_id":target,"text":page,"parse_mode":"MarkdownV2","disable_web_page_preview":true,"disable_notification":false})).await {
                tracing::warn!(item_id = %string(item, "Id"), target, "[通知] 后续消息发送失败：{}", error.1);
            }
        }
    }
    Ok(())
}

async fn send(state: &AppState, token: &str, method: &str, payload: &Value) -> Result<()> {
    let response = state
        .http
        .post(format!(
            "{}/bot{token}/{method}",
            state.notifications.api_base.trim_end_matches('/')
        ))
        .timeout(std::time::Duration::from_secs(if method == "sendPhoto" {
            30
        } else {
            15
        }))
        .json(payload)
        .send()
        .await?;
    // The Python sender considers HTTP 200 successful without inspecting the JSON `ok` field.
    if response.status() != reqwest::StatusCode::OK {
        return Err(ApiError(
            reqwest::StatusCode::BAD_GATEWAY,
            format!("Telegram 返回 HTTP {}", response.status().as_u16()),
        ));
    }
    Ok(())
}
