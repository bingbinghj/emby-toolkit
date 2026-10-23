use crate::{AppState, error::Result, media_edit::list, state::string};
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
    let name = item["Name"].as_str().unwrap_or("未知项目");
    let title = if let Some(year) = item["ProductionYear"].as_i64() {
        format!("{name} ({year})")
    } else {
        name.into()
    };
    let episodes: Vec<Value> = if episode_ids.is_empty() {
        vec![]
    } else {
        sqlx::query_scalar("SELECT jsonb_build_array(season_number,episode_number) FROM media_metadata WHERE item_type='Episode' AND emby_item_ids_json ?| $1").bind(episode_ids).fetch_all(&state.db).await?
    };
    let mut episode_text = ranges(
        &episodes
            .iter()
            .filter_map(|e| Some((e[0].as_i64()?, e[1].as_i64()?)))
            .collect::<Vec<_>>(),
    );
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
    let image:Option<String>=sqlx::query_scalar("SELECT COALESCE(backdrop_path,poster_path) FROM media_metadata WHERE emby_item_ids_json @> $1 LIMIT 1").bind(json!([string(item,"Id")])).fetch_optional(&state.db).await?.flatten();
    let mut targets=sqlx::query_scalar::<_,Option<String>>("SELECT e.telegram_chat_id FROM emby_users u JOIN emby_users_extended e ON u.id=e.emby_user_id WHERE u.is_administrator=true AND e.telegram_chat_id IS NOT NULL").fetch_all(&state.db).await?.into_iter().flatten().collect::<BTreeSet<_>>();
    let channel = string(&config, "telegram_channel_id");
    if !channel.is_empty() {
        targets.insert(channel);
    }
    for target in targets {
        let target = if target.starts_with("https://t.me/") {
            format!("@{}", target.rsplit('/').next().unwrap_or(""))
        } else {
            target
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
        state
            .http
            .post(format!(
                "{}/bot{token}/{method}",
                state.notifications.api_base
            ))
            .json(&payload)
            .send()
            .await?
            .error_for_status()?;
        for page in &pages {
            state.http.post(format!("{}/bot{token}/sendMessage",state.notifications.api_base)).json(&json!({"chat_id":target,"text":page,"parse_mode":"MarkdownV2","disable_web_page_preview":true,"disable_notification":false})).send().await?.error_for_status()?;
        }
    }
    Ok(())
}
