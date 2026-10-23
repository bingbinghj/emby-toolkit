use crate::media_edit::list;
use serde_json::{Value, json};

fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
fn element(output: &mut String, tag: &str, value: &Value) {
    let value = text(value);
    if value.trim().is_empty() {
        return;
    }
    output.push_str(&format!(
        "<{tag}>{}</{tag}>",
        quick_xml::escape::partial_escape(&value)
    ));
}
pub fn initials(title: &str) -> String {
    crate::pinyin::initials(title)
}
fn id(output: &mut String, kind: &str, value: &Value, default: bool) {
    if text(value).trim().is_empty() || *value == 0 {
        return;
    }
    output.push_str(&format!(
        "<uniqueid type=\"{kind}\"{}>{}</uniqueid>",
        if default { " default=\"true\"" } else { "" },
        quick_xml::escape::escape(text(value))
    ));
}
fn actor(output: &mut String, actor: &Value) {
    output.push_str("<actor>");
    for (field, tag) in [
        ("name", "name"),
        ("character", "role"),
        ("type", "type"),
        ("order", "order"),
    ] {
        let value = if field == "type" && actor.get(field).is_none() {
            json!("Actor")
        } else {
            actor[field].clone()
        };
        element(output, tag, &value);
    }
    let profile = text(&actor["profile_path"]);
    if !profile.is_empty() {
        element(
            output,
            "thumb",
            &json!(if profile.starts_with("http") {
                profile
            } else {
                format!("https://image.tmdb.org/t/p/w500{profile}")
            }),
        );
    }
    element(
        output,
        "tmdbid",
        actor
            .get("id")
            .filter(|v| !v.is_null())
            .unwrap_or(&actor["tmdb_id"]),
    );
    element(output, "imdbid", &actor["imdb_id"]);
    output.push_str("</actor>");
}
pub fn directors(data: &Value) -> Vec<Value> {
    let mut found: Vec<(Value, bool, i64)> = list(&data["created_by"])
        .into_iter()
        .filter(|d| !d["id"].is_null() && d["id"] != 0)
        .map(|d| (d, true, 9999))
        .collect();
    let credits = ["aggregate_credits", "credits", "casts"]
        .iter()
        .find_map(|key| {
            data.get(*key)
                .filter(|v| v.as_object().is_some_and(|o| !o.is_empty()))
        })
        .unwrap_or(&Value::Null);
    for person in list(&credits["crew"]) {
        if person["id"].is_null() || person["id"] == 0 {
            continue;
        }
        let is_director =
            |v: &Value| matches!(v["job"].as_str(), Some("Director" | "Series Director"));
        let jobs = list(&person["jobs"]);
        if !is_director(&person) && !jobs.iter().any(is_director) {
            continue;
        }
        let count = i64::from(is_director(&person))
            + jobs
                .iter()
                .filter(|j| is_director(j))
                .map(|j| j["episode_count"].as_i64().unwrap_or(1))
                .sum::<i64>();
        if let Some((old, _, total)) = found.iter_mut().find(|(d, _, _)| d["id"] == person["id"]) {
            *total += count;
            if text(&old["profile_path"]).is_empty() {
                old["profile_path"] = person["profile_path"].clone();
            }
        } else {
            found.push((person, false, count));
        }
    }
    found.sort_by_key(|(d, creator, count)| {
        std::cmp::Reverse((*creator, *count, !text(&d["profile_path"]).is_empty()))
    });
    found
        .into_iter()
        .take(3)
        .map(|(d, _, _)| {
            json!({"id":d["id"],"name":d["name"],
        "original_name":d["original_name"],"job":"Director","profile_path":d["profile_path"]})
        })
        .collect()
}
pub fn build(kind: &str, data: &Value, cast: &[Value]) -> String {
    let tag = match kind {
        "Movie" => "movie",
        "Series" => "tvshow",
        "Season" => "season",
        _ => "episodedetails",
    };
    let mut output = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?><{tag}>");
    element(&mut output, "plot", &data["overview"]);
    element(&mut output, "lockdata", &json!(true));
    let date = text(&data["date_added"])
        .replace('T', " ")
        .chars()
        .take(19)
        .collect::<String>();
    element(&mut output, "dateadded", &json!(date));
    let title = if kind == "Movie" {
        &data["title"]
    } else if kind == "Season" {
        &data["name"]
    } else {
        data.get("name")
            .filter(|v| !text(v).is_empty())
            .unwrap_or(&data["title"])
    };
    element(&mut output, "title", title);
    if matches!(kind, "Movie" | "Series") {
        element(
            &mut output,
            "originaltitle",
            data.get(if kind == "Movie" {
                "original_title"
            } else {
                "original_name"
            })
            .filter(|v| !text(v).is_empty())
            .unwrap_or(&data["original_title"]),
        );
    }
    let sorttitle = if kind == "Series" && !text(&data["sorttitle"]).is_empty() {
        text(&data["sorttitle"])
    } else {
        initials(&text(title))
    };
    element(&mut output, "sorttitle", &json!(sorttitle));
    if matches!(kind, "Movie" | "Series") {
        element(&mut output, "tagline", &data["tagline"]);
        let date = &data[if kind == "Movie" {
            "release_date"
        } else {
            "first_air_date"
        }];
        let year = if kind == "Movie"
            && !data["release_year"].is_null()
            && data["release_year"] != 0
            && data["release_year"] != ""
        {
            data["release_year"].clone()
        } else {
            json!(text(date).chars().take(4).collect::<String>())
        };
        element(&mut output, "year", &year);
        element(&mut output, "premiered", date);
        element(&mut output, "rating", &data["vote_average"]);
        if kind == "Series" {
            element(&mut output, "status", &data["status"]);
        }
        element(
            &mut output,
            "mpaa",
            data.get("mpaa")
                .filter(|v| !text(v).is_empty())
                .unwrap_or(&data["certification"]),
        );
    } else {
        if kind == "Season" {
            element(&mut output, "seasonnumber", &data["season_number"]);
        } else {
            element(&mut output, "season", &data["season_number"]);
            element(&mut output, "episode", &data["episode_number"]);
        }
        element(&mut output, "premiered", &data["air_date"]);
    }
    if kind == "Episode" {
        element(&mut output, "rating", &data["vote_average"]);
    }
    id(&mut output, "tmdb", &data["id"], true);
    if data["id"] != 0 {
        element(&mut output, "tmdbid", &data["id"]);
    }
    if matches!(kind, "Movie" | "Series") {
        id(&mut output, "imdb", &data["imdb_id"], false);
        element(
            &mut output,
            if kind == "Movie" { "imdbid" } else { "imdb_id" },
            &data["imdb_id"],
        );
        if kind == "Series" {
            let mut guide = json!({});
            if !text(&data["id"]).is_empty() && data["id"] != 0 {
                guide["tmdb"] = json!(text(&data["id"]));
            }
            if !text(&data["imdb_id"]).is_empty() {
                guide["imdb"] = data["imdb_id"].clone();
            }
            if !guide.as_object().unwrap().is_empty() {
                let guide = ["tmdb", "imdb"]
                    .iter()
                    .filter_map(|key| guide.get(*key).map(|v| format!("{}: {}", json!(key), v)))
                    .collect::<Vec<_>>()
                    .join(", ");
                element(&mut output, "episodeguide", &json!(format!("{{{guide}}}")));
            }
        }
        let provider = data
            .get("imdb_id")
            .filter(|v| !text(v).is_empty())
            .unwrap_or(&data["id"]);
        if *provider != 0 {
            element(&mut output, "id", provider);
        }
        if kind == "Series" {
            element(&mut output, "season", &json!(-1));
            element(&mut output, "episode", &json!(-1));
            element(&mut output, "displayorder", &json!("aired"));
        }
        for genre in list(&data["genres"]) {
            let name = text(genre.get("name").unwrap_or(&genre));
            let name = match name.as_str() {
                "Sci-Fi & Fantasy" => "科幻奇幻",
                "War & Politics" => "战争政治",
                other => other,
            };
            element(&mut output, "genre", &json!(name));
        }
        let tags = if !data["_mapped_chinese_tags"].is_null() {
            list(&data["_mapped_chinese_tags"])
        } else if data["keywords"].is_object() {
            let mut tags = list(&data["keywords"]["keywords"]);
            if tags.is_empty() {
                tags = list(&data["keywords"]["results"]);
            }
            tags
        } else {
            list(&data["keywords"])
        };
        for tag in tags {
            element(&mut output, "tag", tag.get("name").unwrap_or(&tag));
        }
        let mut studios = if kind == "Series" {
            list(&data["networks"])
        } else {
            vec![]
        };
        studios.extend(list(&data["production_companies"]));
        for studio in studios {
            element(&mut output, "studio", studio.get("name").unwrap_or(&studio));
        }
    }
    for member in cast {
        actor(&mut output, member);
    }
    if kind == "Episode" {
        id(&mut output, "tmdb", &data["id"], true);
        if data["id"] != 0 {
            element(&mut output, "tmdbid", &data["id"]);
        }
    }
    if kind != "Season" {
        for director in directors(data) {
            output.push_str("<director");
            if !text(&director["id"]).is_empty() {
                output.push_str(&format!(
                    " tmdbid=\"{}\"",
                    quick_xml::escape::escape(text(&director["id"]))
                ));
            }
            output.push('>');
            output.push_str(&quick_xml::escape::partial_escape(text(&director["name"])));
            output.push_str("</director>");
            output.push_str("<actor>");
            for (tag, value) in [
                ("name", director["name"].clone()),
                ("type", json!("Director")),
                ("role", json!("Director")),
                ("order", json!(1000)),
                ("tmdbid", director["id"].clone()),
            ] {
                element(&mut output, tag, &value);
            }
            let profile = text(&director["profile_path"]);
            if !profile.is_empty() {
                element(
                    &mut output,
                    "thumb",
                    &json!(if profile.starts_with("http") {
                        profile
                    } else {
                        format!("https://image.tmdb.org/t/p/w500{profile}")
                    }),
                );
            }
            output.push_str("</actor>");
        }
    }
    output.push_str(&format!("</{tag}>"));
    output
}
