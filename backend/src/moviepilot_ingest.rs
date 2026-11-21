//! A notification identifies a scope, not a media revision. Resolve the current
//! entity before skipping work and recheck it before publishing new media data.
use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    media_edit::{details, list},
    media_paths,
    state::string,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::UNIX_EPOCH;

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Identity {
    pub id: String,
    pub path: String,
    pub source: Option<String>,
    pub entity: Value,
    pub cloud: Option<Value>,
    relative: Option<String>,
}
impl Identity {
    pub fn revision(&self) -> Value {
        json!({"id":self.id,"path":self.path,"source":self.source,"entity":self.entity})
    }
    pub fn same_file(&self, other: &Self) -> bool {
        self.id == other.id
            && self.path == other.path
            && self.source == other.source
            && (self.entity.is_null() || other.entity.is_null() || self.entity == other.entity)
    }
    fn content(&self) -> Value {
        let content = if let Some(cloud) = &self.cloud {
            json!({"sha1":string(cloud,"sha1").to_uppercase(),"size":cloud["size"]})
        } else {
            self.entity.clone()
        };
        json!({"id":self.id,"source":self.source,"content":content})
    }
}

pub(crate) async fn observe(
    state: &AppState,
    item: &Value,
    context: &JobContext,
) -> Result<Identity> {
    context.checkpoint().await?;
    let path = string(item, "Path");
    let source = media_paths::strm_target(&path).await;
    let actual = source.as_deref().unwrap_or(&path);
    let config = state.config.read().await.clone();
    let cloud = crate::fingerprints::pickcode(actual).is_some()
        || crate::media_save::protected(&config, actual);
    let (entity, cloud, relative) = if cloud {
        // Available file identity detects a replacement, but its lookup is not
        // an admission gate for the original native ingestion workflow.
        let found = crate::revalidation::resolve_ingest(state, actual, &path, context).await;
        context.checkpoint().await?;
        match found {
            Ok((cloud, relative))
                if string(&cloud, "sha1").len() == 40
                    && string(&cloud, "sha1")
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit())
                    && crate::identity::size(&cloud["size"])
                        .is_some_and(|size| size <= i64::MAX as u64) =>
            {
                (cloud.clone(), Some(cloud), relative)
            }
            _ => (Value::Null, None, None),
        }
    } else if actual.starts_with("http://") || actual.starts_with("https://") {
        (Value::Null, None, None)
    } else {
        let metadata = if media_paths::extension(&path) == ".strm" && source.is_none() {
            None
        } else {
            tokio::fs::metadata(actual)
                .await
                .ok()
                .filter(|data| data.is_file() && data.len() > 0)
        };
        let entity = metadata
            .and_then(|metadata| {
                let modified = metadata
                    .modified()
                    .ok()?
                    .duration_since(UNIX_EPOCH)
                    .ok()?
                    .as_nanos()
                    .to_string();
                let mut entity = json!({"size":metadata.len(),"modified":modified});
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    entity["device"] = json!(metadata.dev());
                    entity["inode"] = json!(metadata.ino());
                    entity["ctime"] = json!([metadata.ctime(), metadata.ctime_nsec()]);
                }
                Some(entity)
            })
            .unwrap_or(Value::Null);
        (entity, None, None)
    };
    Ok(Identity {
        id: string(item, "Id"),
        path,
        source,
        entity,
        cloud,
        relative,
    })
}

pub(crate) async fn unchanged(
    state: &AppState,
    identity: &Identity,
    context: &JobContext,
) -> Result<()> {
    let live = details(state, &identity.id, "Id,Path,MediaSources,MediaStreams").await?;
    if !identity.same_file(&observe(state, &live, context).await?) {
        return Err(ApiError::bad(
            "媒体文件在处理期间再次变化，已停止提交，请重试当前版本",
        ));
    }
    Ok(())
}

pub(crate) async fn unchanged_many(
    state: &AppState,
    identities: &[Identity],
    scope: Option<(&str, &[String])>,
    context: &JobContext,
) -> Result<()> {
    context.refresh_ingest_directories().await;
    for identity in identities {
        if scope.is_some_and(|(id, ids)| identity.id != id && !ids.contains(&identity.id)) {
            continue;
        }
        unchanged(state, identity, context).await?;
    }
    Ok(())
}

async fn slot(state: &AppState, id: &str) -> Result<Option<(Value, usize)>> {
    let row:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE item_type IN ('Movie','Episode') AND emby_item_ids_json @> $1::jsonb LIMIT 1")
        .bind(json!([id])).fetch_optional(&state.db).await?;
    Ok(row.and_then(|row| {
        list(&row["emby_item_ids_json"])
            .iter()
            .position(|value| value == id)
            .map(|index| (row, index))
    }))
}

pub(crate) async fn completed(state: &AppState, identity: &Identity) -> Result<bool> {
    let marked = slot(state, &identity.id)
        .await?
        .is_some_and(|(row, index)| {
            let asset = &row["asset_details_json"][index];
            row["in_library"] == true
                && (asset["moviepilot_ingest_identity"] == identity.revision()
                    || legacy_file_matches(&row, index, identity))
        });
    // This marks completion of the ingestion workflow, not a media-edit audit.
    Ok(marked)
}

fn legacy_file_matches(row: &Value, index: usize, identity: &Identity) -> bool {
    let asset = &row["asset_details_json"][index];
    let Some(cloud) = &identity.cloud else { return false; };
    // Older Python/sync admissions have no workflow receipt. Reuse their
    // verified file identity; a partially executed new workflow still retries.
    asset["moviepilot_ingest_identity"].is_null()
        && asset["moviepilot_media_identity"].is_null()
        && asset["moviepilot_media_pending"] != true
        && asset["fingerprint_pending"] != true
        && asset["path"] == identity.path
        && row["emby_item_ids_json"][index] == identity.id
        && string(cloud, "sha1").len() == 40
        && row["file_sha1_json"][index].as_str().is_some_and(|sha| {
            sha.eq_ignore_ascii_case(&string(cloud, "sha1"))
        })
        && row["file_pickcode_json"][index] == cloud["pick_code"]
        && crate::identity::size(&asset["size_bytes"]).is_some_and(|size| {
            Some(size) == crate::identity::size(&cloud["size"])
        })
}

async fn write_identity(
    state: &AppState,
    identity: &Identity,
    media: bool,
    complete: bool,
) -> Result<()> {
    let mut transaction = state.db.begin().await?;
    if let (Some(cloud), Some(relative)) = (&identity.cloud, &identity.relative) {
        sqlx::query(
            "DELETE FROM p115_filesystem_cache WHERE id=$1 AND (parent_id<>$2 OR name<>$3)",
        )
        .bind(string(cloud, "id"))
        .bind(string(cloud, "parent_id"))
        .bind(string(cloud, "name"))
        .execute(&mut *transaction)
        .await?;
        sqlx::query("INSERT INTO p115_filesystem_cache(id,parent_id,name,sha1,pick_code,local_path,size) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(parent_id,name) DO UPDATE SET id=EXCLUDED.id,sha1=EXCLUDED.sha1,pick_code=EXCLUDED.pick_code,local_path=EXCLUDED.local_path,size=EXCLUDED.size,updated_at=NOW()")
            .bind(string(cloud,"id")).bind(string(cloud,"parent_id")).bind(string(cloud,"name")).bind(string(cloud,"sha1").to_uppercase()).bind(string(cloud,"pick_code")).bind(relative).bind(crate::identity::size(&cloud["size"]).unwrap() as i64).execute(&mut *transaction).await?;
    }
    let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(m) FROM media_metadata m WHERE item_type IN ('Movie','Episode') AND emby_item_ids_json @> $1::jsonb FOR UPDATE")
        .bind(json!([identity.id])).fetch_all(&mut *transaction).await?;
    for row in rows {
        let Some(index) = list(&row["emby_item_ids_json"])
            .iter()
            .position(|id| id == &identity.id)
        else {
            continue;
        };
        let mut assets = list(&row["asset_details_json"]);
        let mut pcs = list(&row["file_pickcode_json"]);
        let mut shas = list(&row["file_sha1_json"]);
        assets.resize(list(&row["emby_item_ids_json"]).len(), json!({}));
        pcs.resize(assets.len(), Value::Null);
        shas.resize(assets.len(), Value::Null);
        if string(&assets[index], "path") != identity.path {
            if complete {
                return Err(ApiError::bad("当前媒体绑定路径已变化，停止记录入库完成"));
            }
            // The ordinary ingestion flow has not rebound an existing ID yet.
            continue;
        }
        if let Some(cloud) = &identity.cloud
            && (media || complete)
        {
            let (asset, pc, sha) = crate::identity::merge(
                &assets[index],
                &assets[index],
                &pcs[index],
                &shas[index],
                &cloud["pick_code"],
                &cloud["sha1"],
            );
            assets[index] = asset;
            pcs[index] = pc;
            shas[index] = sha;
            assets[index]["size_bytes"] = cloud["size"].clone();
            assets[index]["fingerprint_pending"] = json!(false);
        }
        if media {
            assets[index]["moviepilot_media_identity"] = identity.content();
            assets[index]["moviepilot_media_pending"] = json!(false);
        } else if !complete {
            assets[index]["moviepilot_media_pending"] = json!(true);
        }
        if complete {
            assets[index]["moviepilot_ingest_identity"] = identity.revision();
        }
        sqlx::query("UPDATE media_metadata SET asset_details_json=$1,file_pickcode_json=$2,file_sha1_json=$3,last_updated_at=NOW() WHERE tmdb_id=$4 AND item_type=$5")
            .bind(json!(assets)).bind(json!(pcs)).bind(json!(shas)).bind(string(&row,"tmdb_id")).bind(string(&row,"item_type")).execute(&mut *transaction).await?;
    }
    transaction.commit().await?;
    Ok(())
}

pub(crate) async fn finish(
    state: &AppState,
    identity: &Identity,
    context: &JobContext,
) -> Result<()> {
    unchanged(state, identity, context).await?;
    write_identity(state, identity, true, true).await
}

pub(crate) async fn remember_initial(
    state: &AppState,
    identity: &Identity,
    context: &JobContext,
) -> Result<()> {
    unchanged(state, identity, context).await?;
    write_identity(state, identity, true, false).await
}

/// Retire the obsolete media-field gate after the ordinary native preflight.
pub(crate) async fn clear_pending(state: &AppState, item: &Value) -> Result<()> {
    let Some((row, index)) = slot(state, &string(item, "Id")).await? else {
        return Ok(());
    };
    if row["asset_details_json"][index]["path"] != item["Path"]
        || row["asset_details_json"][index]["moviepilot_media_pending"] != true
    {
        return Ok(());
    }
    sqlx::query("UPDATE media_metadata SET asset_details_json=jsonb_set(asset_details_json,$1,'false'::jsonb),last_updated_at=NOW() WHERE tmdb_id=$2 AND item_type=$3 AND asset_details_json->$4->>'path'=$5 AND emby_item_ids_json->>$4=$6")
        .bind(vec![index.to_string(), "moviepilot_media_pending".into()])
        .bind(string(&row, "tmdb_id")).bind(string(&row, "item_type"))
        .bind(index as i32).bind(string(item, "Path")).bind(string(item, "Id")).execute(&state.db).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_admission_reuses_only_the_same_verified_cloud_file() {
        let cloud = json!({"sha1":"A".repeat(40),"pick_code":"pc","size":123});
        let identity = Identity { id:"old".into(), path:"/library/old.strm".into(), source:None,
            entity:cloud.clone(), cloud:Some(cloud), relative:None };
        let row = json!({"emby_item_ids_json":["old"],"file_sha1_json":["a".repeat(40)],
            "file_pickcode_json":["pc"],"asset_details_json":[{"path":identity.path,"size_bytes":123}]});
        assert!(legacy_file_matches(&row, 0, &identity));
        for (key, value) in [("moviepilot_media_pending", json!(true)), ("fingerprint_pending",json!(true)),
            ("moviepilot_media_identity",json!({})), ("moviepilot_ingest_identity",json!({})),
            ("size_bytes",json!(124)), ("path",json!("/other.strm"))] {
            let mut changed = row.clone(); changed["asset_details_json"][0][key] = value;
            assert!(!legacy_file_matches(&changed, 0, &identity), "{key}");
        }
        for key in ["file_sha1_json", "file_pickcode_json", "emby_item_ids_json"] {
            let mut changed=row.clone(); changed[key][0]=json!("replacement");
            assert!(!legacy_file_matches(&changed, 0, &identity), "{key}");
        }
        let mut unknown = identity; unknown.cloud=None;
        assert!(!legacy_file_matches(&row, 0, &unknown));
    }
}
