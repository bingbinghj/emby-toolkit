//! Reuse MP/Emby metadata without fetching or translating the same information again.
use crate::{AppState, error::Result, media_edit::list, state::string};
use serde_json::{Value, json};
use std::path::PathBuf;

pub(crate) async fn current(item: &Value) -> Value {
    let kind = string(item, "Type");
    let mut data = json!({});
    for (source, target) in [
        ("Name", "name"),
        ("OriginalTitle", "original_name"),
        ("Overview", "overview"),
        ("PremiereDate", "air_date"),
    ] {
        if let Some(value) = item.get(source) {
            data[target] = value.clone();
        }
    }
    if let Some(value) = list(&item["Taglines"]).first() {
        data["tagline"] = value.clone();
    }
    if let Ok(id) = string(&item["ProviderIds"], "Tmdb").parse::<u64>() {
        data["id"] = json!(id);
    }
    let cast = list(&item["People"]).iter().filter_map(|person| {
        if person["Type"].as_str().is_some_and(|kind|kind != "Actor") { return None; }
        let id = string(&person["ProviderIds"], "Tmdb").parse::<u64>().ok()?;
        Some(json!({"id":id,"name":person["Name"],"character":person["Role"],"order":person["SortOrder"]}))
    }).collect::<Vec<_>>();
    if !cast.is_empty() {
        data["credits"]["cast"] = json!(cast);
    }
    let path = PathBuf::from(string(item, "Path"));
    let path = match kind.as_str() {
        "Series" => path.join("tvshow.nfo"),
        "Season" => path.join("season.nfo"),
        _ => path.with_extension("nfo"),
    };
    if let Ok(xml) = tokio::fs::read_to_string(path).await
        && let Ok(nfo) = crate::emby::scrape_metadata_from_nfo(&xml, &kind)
        && (data["id"].is_null() || nfo["id"].is_null() || data["id"] == nfo["id"])
    {
        data.as_object_mut()
            .unwrap()
            .extend(nfo.as_object().unwrap().clone());
    }
    if kind == "Movie" {
        data["title"] = data["name"].clone();
        data["original_title"] = data["original_name"].clone();
        data["release_date"] = data["air_date"].clone();
    } else if kind == "Series" {
        data["first_air_date"] = data["air_date"].clone();
    }
    data
}

/// Only match people by their provider identity; never transfer a role by position.
pub(crate) fn preserve_translations(data: &mut Value, previous: &Value) {
    for field in ["title", "name", "overview", "tagline"] {
        if crate::translation::chinese(&string(previous, field)) {
            data[field] = previous[field].clone();
        }
    }
    for credit in ["credits", "aggregate_credits", "casts"] {
        for field in ["cast", "guest_stars", "crew"] {
            if let Some(people) = data[credit][field].as_array_mut() {
                let fields = if field == "crew" {
                    vec!["crew"]
                } else {
                    vec!["cast", "guest_stars"]
                };
                let old = ["credits", "aggregate_credits", "casts"]
                    .into_iter()
                    .flat_map(|key| {
                        fields
                            .iter()
                            .flat_map(move |field| list(&previous[key][*field]))
                    })
                    .collect::<Vec<_>>();
                for person in people {
                    if let Some(previous) = old
                        .iter()
                        .find(|old| !old["id"].is_null() && old["id"] == person["id"])
                    {
                        for key in ["name", "character"] {
                            if crate::translation::chinese(&string(previous, key)) {
                                person[key] = previous[key].clone();
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(creators) = data["created_by"].as_array_mut() {
        let previous = list(&previous["created_by"])
            .into_iter()
            .chain(list(&previous["credits"]["crew"]))
            .collect::<Vec<_>>();
        for creator in creators {
            if let Some(old) = previous
                .iter()
                .find(|old| !old["id"].is_null() && old["id"] == creator["id"])
                && crate::translation::chinese(&string(old, "name"))
            {
                creator["name"] = old["name"].clone();
            }
        }
    }
}

pub(crate) async fn preserve_tree(
    state: &AppState,
    item: &Value,
    children: &[Value],
    data: &mut Value,
) -> Result<()> {
    let kind = string(item, "Type");
    let cached = crate::metadata::reconstruct_full(
        state,
        &string(&item["ProviderIds"], "Tmdb"),
        &kind,
        true,
    )
    .await?;
    let (mut previous, cast) = cached.unwrap_or((json!({}), vec![]));
    previous["credits"]["cast"] = json!(cast);
    let top = if kind == "Series" && data.get("series_details").is_some() {
        &mut data["series_details"]
    } else {
        &mut *data
    };
    preserve_translations(top, &previous);
    preserve_translations(top, &current(item).await);
    for child in children {
        let number = child["IndexNumber"].as_i64();
        if child["Type"] == "Season" {
            if let Some(node) = data["seasons_details"].as_array_mut().and_then(|nodes| {
                nodes
                    .iter_mut()
                    .find(|node| node["season_number"].as_i64() == number)
            }) {
                if let Some(old) = list(&previous["seasons_details"])
                    .iter()
                    .find(|node| node["season_number"].as_i64() == number)
                {
                    preserve_translations(node, old);
                }
                preserve_translations(node, &current(child).await);
            }
        } else if child["Type"] == "Episode" {
            let key = format!("S{}E{}", child["ParentIndexNumber"], child["IndexNumber"]);
            if let Some(node) = data["episodes_details"].get_mut(&key) {
                preserve_translations(node, &previous["episodes_details"][&key]);
                preserve_translations(node, &current(child).await);
            }
        }
    }
    Ok(())
}

pub(crate) async fn prepare(
    state: &AppState,
    item: &Value,
    episode_ids: &[String],
    group: Option<&str>,
    context: &crate::jobs::JobContext,
) -> Result<Value> {
    let kind = string(item, "Type");
    let id = string(&item["ProviderIds"], "Tmdb");
    let native = current(item).await;
    let (mut payload, mut cast) = crate::metadata::reconstruct_full(state, &id, &kind, true)
        .await?
        .unwrap_or((native.clone(), vec![]));
    if payload["id"].is_null() {
        payload["id"] = json!(id.parse::<u64>().unwrap_or(0));
    }
    preserve_translations(&mut payload, &native);
    if cast.is_empty() {
        cast = list(&native["credits"]["cast"]);
    }
    if let Some(group) = group {
        payload["episode_group_id"] = json!(group);
    }
    let mut media_paths = vec![];
    let mut numbers = vec![];
    if kind == "Series" {
        let mut seasons = vec![];
        let mut episodes = json!({});
        let children = crate::emby::items(state, &string(item, "Id"), "Id,Type,Name,OriginalTitle,Overview,Taglines,Path,People,ProviderIds,ParentId,SeriesId,ParentIndexNumber,IndexNumber,PremiereDate", context).await?;
        for child in children.iter().filter(|child| {
            child["Type"] == "Episode" && episode_ids.contains(&string(child, "Id"))
        }) {
            media_paths.push(PathBuf::from(string(child, "Path")));
            if let (Some(season), Some(episode)) = (
                child["ParentIndexNumber"].as_i64(),
                child["IndexNumber"].as_i64(),
            ) {
                numbers.push((season, episode));
            }
            let key = format!("S{}E{}", child["ParentIndexNumber"], child["IndexNumber"]);
            let local = current(child).await;
            let mut node = payload["episodes_details"]
                .get(&key)
                .cloned()
                .unwrap_or(local.clone());
            preserve_translations(&mut node, &local);
            node["season_number"] = child["ParentIndexNumber"].clone();
            node["episode_number"] = child["IndexNumber"].clone();
            episodes[&key] = node;
        }
        for child in children.iter().filter(|child| child["Type"] == "Season") {
            if !episodes
                .as_object()
                .unwrap()
                .values()
                .any(|episode| episode["season_number"] == child["IndexNumber"])
            {
                continue;
            }
            let local = current(child).await;
            let mut node = list(&payload["seasons_details"])
                .into_iter()
                .find(|node| node["season_number"] == child["IndexNumber"])
                .unwrap_or(local.clone());
            preserve_translations(&mut node, &local);
            node["season_number"] = child["IndexNumber"].clone();
            if node["id"].is_null() {
                node["id"] = json!(format!("{id}-S{}", child["IndexNumber"]));
            }
            seasons.push(node);
        }
        payload["seasons_details"] = json!(seasons);
        payload["episodes_details"] = episodes;
    }
    let movie_paths = if kind == "Movie" {
        vec![PathBuf::from(string(item, "Path"))]
    } else {
        vec![]
    };
    let config = state.config.read().await.clone();
    let options = crate::nfo_files::Options {
        missing_only: true,
        config: &config,
        movie_paths: &movie_paths,
        media_paths: &media_paths,
        episodes: &numbers,
        ..Default::default()
    };
    context.checkpoint().await?;
    let written = crate::nfo_files::sync(&kind, item, &payload, &cast, &options).await?;
    for person in &cast {
        if let Some(id) = person["id"]
            .as_i64()
            .and_then(|id| i32::try_from(id).ok())
            .filter(|id| *id > 0)
        {
            sqlx::query("INSERT INTO person_metadata(tmdb_person_id,primary_name) VALUES($1,$2) ON CONFLICT(tmdb_person_id) DO NOTHING")
                .bind(id).bind(string(person,"name")).execute(&state.db).await?;
        }
    }
    context.checkpoint().await?;
    let mut records =
        crate::metadata_records::pending(&kind, &payload, &cast, &json!({"Id":"pending"}))?;
    // Existing rows already contain translations and manually edited fields.
    for record in &mut records {
        if let Some(old) = crate::metadata::row(
            state,
            &string(record, "tmdb_id"),
            &string(record, "item_type"),
        )
        .await?
        {
            record.as_object_mut().unwrap().retain(|key, value| {
                matches!(key.as_str(), "tmdb_id" | "item_type")
                    || matches!(key.as_str(), "title" | "overview" | "tagline")
                        && value.as_str().is_some_and(crate::translation::chinese)
                    || old.get(key).is_none_or(Value::is_null)
            });
            for key in [
                "in_library",
                "emby_item_ids_json",
                "asset_details_json",
                "file_pickcode_json",
                "file_sha1_json",
            ] {
                record[key] = old[key].clone();
            }
        }
    }
    crate::metadata::ingest(state, &mut records, false).await?;
    tracing::info!(
        "MoviePilot 元数据直接复用：《{}》，本批分集 {} 个，保留 NFO 与已有翻译，跳过重复刮削",
        string(item, "Name"),
        episode_ids.len()
    );
    Ok(
        json!({"reused_moviepilot_metadata":true,"nfo_written":written,"images_downloaded":0,"warnings":0,"warning_details":[]}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_nfo_translations_win_and_cdata_entities_survive() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("E201.strm");
        let xml = "<episodedetails><title><![CDATA[已翻译的标题 & 保留]]></title><plot>已翻译 &amp; 简介</plot><uniqueid type=\"tmdb\">201</uniqueid><actor><tmdbid>1</tmdbid><name>配音演员</name><role>韩立</role></actor></episodedetails>";
        tokio::fs::write(path.with_extension("nfo"), xml)
            .await
            .unwrap();
        let result = current(&json!({"Type":"Episode","Path":path,"Name":"Remote title","ProviderIds":{"Tmdb":"201"}})).await;
        assert_eq!(result["name"], "已翻译的标题 & 保留");
        assert_eq!(result["overview"], "已翻译 & 简介");
        assert_eq!(result["credits"]["cast"][0]["character"], "韩立");
        assert_eq!(
            tokio::fs::read_to_string(path.with_extension("nfo"))
                .await
                .unwrap(),
            xml
        );
        let unrelated = current(&json!({"Type":"Episode","Path":path,"Name":"Other episode","ProviderIds":{"Tmdb":"202"}})).await;
        assert_eq!(unrelated["name"], "Other episode");
    }

    #[test]
    fn fresh_metadata_keeps_translation_by_identity_and_updates_other_fields() {
        let mut fresh = json!({"name":"New source title","overview":"新官方简介","tagline":"New tagline","runtime":28,"poster_path":"/new.jpg", "credits":{"cast":[{"id":2,"name":"Second","character":"Role B"},{"id":1,"name":"First","character":"Role A"},{"id":3,"name":"New Actor","character":"New Role"}]}});
        let previous = json!({"name":"现有译名","overview":"已翻译的简介","tagline":"现有标语", "runtime":27,"credits":{"cast":[{"id":1,"name":"演员甲","character":"角色甲"},{"id":2,"name":"演员乙","character":"角色乙"}]}});
        preserve_translations(&mut fresh, &previous);
        assert_eq!(fresh["name"], "现有译名");
        assert_eq!(fresh["overview"], "已翻译的简介");
        assert_eq!(fresh["tagline"], "现有标语");
        assert_eq!(fresh["runtime"], 28);
        assert_eq!(fresh["poster_path"], "/new.jpg");
        assert_eq!(fresh["credits"]["cast"][0]["name"], "演员乙");
        assert_eq!(fresh["credits"]["cast"][1]["character"], "角色甲");
        assert_eq!(fresh["credits"]["cast"][2]["name"], "New Actor");
    }
}
