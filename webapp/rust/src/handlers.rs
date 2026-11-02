use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use tokio::sync::Semaphore;

use crate::http::{query_str, HttpResp};
use crate::store::{
    now_unix, sha256_hex, shard_idx, LivecommentRec, LivestreamRec, NgWord, ReactionRec, ReportRec,
    ThinComment, ThinReaction, UserRec, ViewerRec, TERM_END_AT, TERM_START_AT, ZONE,
};
use crate::{
    create_session, etag_matches, json_bytes, parse_json, rebuild_dns_names, require_auth, AppState,
    Error, Inner, BCRYPT_COST, SESSION_COOKIE,
};
use bytes::Bytes;

use crate::store;

fn install_loaded(inner: &Inner, loaded: store::Store, shards: Vec<store::LsShard>) {
    inner.next_livecomment_id.store(loaded.next_livecomment_id, Ordering::Relaxed);
    inner.next_reaction_id.store(loaded.next_reaction_id, Ordering::Relaxed);
    inner.next_ng_word_id.store(loaded.next_ng_word_id, Ordering::Relaxed);
    inner.next_report_id.store(loaded.next_report_id, Ordering::Relaxed);
    let names = rebuild_dns_names(&loaded);
    let rank = store::RankState::from_store(&loaded);
    inner.total_tip.store(rank.total_tip, Ordering::Relaxed);
    *inner.store.write() = loaded;
    *inner.rank.lock() = rank;
    inner.install_shards(shards);
    *inner.dns_names.write() = names;
    inner.sessions.write().clear();
}

fn json_ok<T: serde::Serialize>(v: &T) -> Result<HttpResp, Error> {
    Ok(HttpResp::json(200, json_bytes(v)?))
}
fn json_created<T: serde::Serialize>(v: &T) -> Result<HttpResp, Error> {
    Ok(HttpResp::json(201, json_bytes(v)?))
}

pub async fn initialize(state: &AppState) -> Result<HttpResp, Error> {
    let _guard = state.inner.init_lock.lock().await;
    let started = Instant::now();
    let mut cmd = tokio::process::Command::new("../sql/init.sh");
    cmd.env("ISUCON13_SKIP_PDNS", "1");
    let output = cmd
        .output()
        .await
        .map_err(|e| Error::internal(format!("failed to spawn init.sh: {e}")))?;
    if !output.status.success() {
        return Err(Error::internal(format!(
            "failed to initialize: init.sh failed with stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        )));
    }
    let (loaded, shards) = store::load_store(&state.inner.pool, state.inner.fallback_hash.clone())
        .await
        .map_err(|e| Error::internal(format!("failed to load state: {e}")))?;
    install_loaded(&state.inner, loaded, shards);
    tracing::info!("initialize loaded in {:?}", started.elapsed());
    json_ok(&serde_json::json!({ "language": "rust" }))
}

pub fn get_tag(state: &AppState) -> Result<HttpResp, Error> {
    let store = state.inner.store.read();
    let tags: Vec<_> = store
        .tag_order
        .iter()
        .filter_map(|id| store.tags.get(id).cloned())
        .collect();
    json_ok(&serde_json::json!({ "tags": tags }))
}

pub fn get_theme(state: &AppState, cookie: Option<&str>, username: &str) -> Result<HttpResp, Error> {
    require_auth(state, cookie)?;
    let store = state.inner.store.read();
    let user_id = *store
        .users_by_name
        .get(username)
        .ok_or_else(|| Error::not_found("not found user that has the given username"))?;
    let theme = store
        .users
        .get(&user_id)
        .map(|u| u.theme.clone())
        .ok_or_else(|| Error::not_found("not found user that has the given username"))?;
    json_ok(&theme)
}

#[derive(serde::Deserialize)]
struct ReserveLivestreamRequest {
    tags: Vec<i64>,
    title: String,
    description: String,
    playlist_url: String,
    thumbnail_url: String,
    start_at: i64,
    end_at: i64,
}

pub fn reserve(state: &AppState, cookie: Option<&str>, body: &[u8]) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
    let req: ReserveLivestreamRequest = parse_json(body)?;
    if req.start_at >= TERM_END_AT || req.end_at <= TERM_START_AT {
        return Err(Error::bad("bad reservation time range"));
    }
    let mut store = state.inner.store.write();
    let start = req.start_at;
    let end = req.end_at;
    let lo = store
        .slots
        .partition_point(|s| s.start_at < start);
    let hi = lo + store.slots[lo..].partition_point(|s| s.end_at <= end);
    if store.slots[lo..hi].iter().any(|s| s.slot < 1) {
        return Err(Error::bad(format!(
            "予約期間 {TERM_START_AT} ~ {TERM_END_AT}に対して、予約区間 {} ~ {}が予約できません",
            start, end
        )));
    }
    for slot in &mut store.slots[lo..hi] {
        slot.slot -= 1;
    }
    let livestream_id = store.next_livestream_id;
    store.next_livestream_id += 1;
    let mut tag_ids = Vec::new();
    for tag_id in req.tags {
        tag_ids.push(tag_id);
        store
            .livestreams_by_tag
            .entry(tag_id)
            .or_default()
            .insert(0, livestream_id);
    }
    store.livestreams.insert(
        livestream_id,
        LivestreamRec {
            id: livestream_id,
            user_id: auth.user_id,
            title: req.title,
            description: req.description,
            playlist_url: req.playlist_url,
            thumbnail_url: req.thumbnail_url,
            start_at: req.start_at,
            end_at: req.end_at,
            tag_ids,
            reaction_count: AtomicI64::new(0),
            tip_sum: AtomicI64::new(0),
            max_tip: AtomicI64::new(0),
            viewers_count: AtomicI64::new(0),
            reports_count: AtomicI64::new(0),
        },
    );
    if let Some(user) = store.users.get_mut(&auth.user_id) {
        user.livestream_ids.push(livestream_id);
    }
    store.livestream_ids_desc.insert(0, livestream_id);
    store.cache_livestream(livestream_id);
    let body = store
        .ls_json(livestream_id)
        .ok_or_else(|| Error::internal("failed to fill livestream"))?;
    drop(store);
    state.inner.rank.lock().insert_ls(livestream_id);
    Ok(HttpResp::json(201, body.to_vec()))
}

pub fn search(state: &AppState, query: &[(String, String)]) -> Result<HttpResp, Error> {
    let tag = query_str(query, "tag");
    let limit = parse_limit(query)?;
    let store = state.inner.store.read();
    let ids: &[i64] = if tag.is_empty() {
        &store.livestream_ids_desc
    } else if let Some(tag_id) = store.tags_by_name.get(tag) {
        store
            .livestreams_by_tag
            .get(tag_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    } else {
        &[]
    };
    let ids = if tag.is_empty() {
        let n = limit.unwrap_or(ids.len()).min(ids.len());
        &ids[..n]
    } else {
        ids
    };
    Ok(HttpResp::json(200, livestream_array_json(&store, ids)))
}

fn livestream_array_json(store: &store::Store, ids: &[i64]) -> Vec<u8> {
    store::assemble_json_array(
        ids.iter()
            .filter_map(|id| store.cached_ls_json.get(id).map(|b| b.as_ref())),
    )
}

pub fn my_livestreams(state: &AppState, cookie: Option<&str>) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
    let store = state.inner.store.read();
    let ids = store
        .users
        .get(&auth.user_id)
        .map(|u| u.livestream_ids.clone())
        .unwrap_or_default();
    Ok(HttpResp::json(200, livestream_array_json(&store, &ids)))
}

pub fn user_livestreams(
    state: &AppState,
    cookie: Option<&str>,
    username: &str,
) -> Result<HttpResp, Error> {
    require_auth(state, cookie)?;
    let store = state.inner.store.read();
    let user_id = *store
        .users_by_name
        .get(username)
        .ok_or_else(|| Error::not_found("user not found"))?;
    let ids = store
        .users
        .get(&user_id)
        .map(|u| u.livestream_ids.clone())
        .unwrap_or_default();
    Ok(HttpResp::json(200, livestream_array_json(&store, &ids)))
}

pub fn enter(state: &AppState, cookie: Option<&str>, livestream_id: i64) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
    {
        let mut shard = state.inner.shards[shard_idx(livestream_id)].write();
        shard
            .viewers_by_ls
            .entry(livestream_id)
            .or_default()
            .push(ViewerRec {
                user_id: auth.user_id,
                created_at: now_unix(),
            });
    }
    state.inner.store.read().add_viewer_stats(livestream_id, 1);
    Ok(HttpResp::empty(200))
}

pub fn exit(state: &AppState, cookie: Option<&str>, livestream_id: i64) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
    let removed = {
        let mut shard = state.inner.shards[shard_idx(livestream_id)].write();
        if let Some(viewers) = shard.viewers_by_ls.get_mut(&livestream_id) {
            let before = viewers.len();
            viewers.retain(|v| v.user_id != auth.user_id);
            (before - viewers.len()) as i64
        } else {
            0
        }
    };
    if removed != 0 {
        state
            .inner
            .store
            .read()
            .add_viewer_stats(livestream_id, -removed);
    }
    Ok(HttpResp::empty(200))
}

pub fn get_livestream(
    state: &AppState,
    cookie: Option<&str>,
    livestream_id: i64,
) -> Result<HttpResp, Error> {
    require_auth(state, cookie)?;
    let store = state.inner.store.read();
    let body = store
        .ls_json(livestream_id)
        .ok_or_else(|| Error::not_found("not found livestream that has the given id"))?;
    Ok(HttpResp::json(200, body.to_vec()))
}

pub fn get_reports(
    state: &AppState,
    cookie: Option<&str>,
    livestream_id: i64,
) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
    {
        let store = state.inner.store.read();
        let ls = store
            .livestreams
            .get(&livestream_id)
            .ok_or_else(|| Error::internal("failed to get livestream"))?;
        if ls.user_id != auth.user_id {
            return Err(Error::forbidden(
                "can't get other streamer's livecomment reports",
            ));
        }
    }
    let shard = state.inner.shards[shard_idx(livestream_id)].read();
    let ids = shard
        .reports_by_ls
        .get(&livestream_id)
        .cloned()
        .unwrap_or_default();
    let store = state.inner.store.read();
    let livestream = store
        .fill_livestream(livestream_id)
        .ok_or_else(|| Error::internal("failed to fill livecomment report"))?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let r = shard
            .reports
            .get(&id)
            .ok_or_else(|| Error::internal("failed to fill livecomment report"))?;
        let reporter = store
            .users
            .get(&r.user_id)
            .ok_or_else(|| Error::internal("failed to fill livecomment report"))?
            .to_public();
        let c = shard
            .comments
            .get(&r.livecomment_id)
            .ok_or_else(|| Error::internal("failed to fill livecomment report"))?;
        let comment_user = store
            .users
            .get(&c.user_id)
            .ok_or_else(|| Error::internal("failed to fill livecomment report"))?
            .to_public();
        out.push(store::LivecommentReport {
            id: r.id,
            reporter,
            livecomment: store::Livecomment {
                id: c.id,
                user: comment_user,
                livestream: livestream.clone(),
                comment: c.comment.to_string(),
                tip: c.tip,
                created_at: c.created_at,
            },
            created_at: r.created_at,
        });
    }
    json_ok(&out)
}

fn parse_limit(query: &[(String, String)]) -> Result<Option<usize>, Error> {
    let limit = query_str(query, "limit");
    if limit.is_empty() {
        Ok(None)
    } else {
        Ok(Some(limit
            .parse::<usize>()
            .map_err(|_| Error::bad("limit query parameter must be integer"))?))
    }
}

pub fn get_livecomments(
    state: &AppState,
    cookie: Option<&str>,
    livestream_id: i64,
    query: &[(String, String)],
) -> Result<HttpResp, Error> {
    require_auth(state, cookie)?;
    let limit = parse_limit(query)?;
    let rows = {
        let shard = state.inner.shards[shard_idx(livestream_id)].read();
        let ids = shard
            .comments_by_ls
            .get(&livestream_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let take = limit.unwrap_or(ids.len()).min(ids.len());
        let slice = &ids[ids.len() - take..];
        let mut rows = Vec::with_capacity(slice.len());
        for id in slice.iter().rev() {
            let c = shard
                .comments
                .get(id)
                .ok_or_else(|| Error::internal("failed to fil livecomments"))?;
            rows.push(ThinComment {
                id: c.id,
                user_id: c.user_id,
                comment: Arc::clone(&c.comment),
                tip: c.tip,
                created_at: c.created_at,
            });
        }
        rows
    };
    let store = state.inner.store.read();
    let ls = store
        .ls_json(livestream_id)
        .ok_or_else(|| Error::internal("failed to fil livecomments"))?;
    let mut users = HashMap::new();
    for row in &rows {
        if let std::collections::hash_map::Entry::Vacant(e) = users.entry(row.user_id) {
            let json = store
                .user_json(row.user_id)
                .ok_or_else(|| Error::internal("failed to fil livecomments"))?;
            e.insert(json);
        }
    }
    drop(store);
    Ok(HttpResp::json(200, store::assemble_comments(&rows, &users, &ls)))
}

pub fn get_ngwords(
    state: &AppState,
    cookie: Option<&str>,
    livestream_id: i64,
) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
    let shard = state.inner.shards[shard_idx(livestream_id)].read();
    let mut words: Vec<NgWord> = shard
        .ng_by_ls
        .get(&livestream_id)
        .into_iter()
        .flatten()
        .filter_map(|id| shard.ng_words.get(id))
        .filter(|w| w.user_id == auth.user_id)
        .cloned()
        .collect();
    words.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.id.cmp(&a.id)));
    json_ok(&words)
}

#[derive(serde::Deserialize)]
struct PostLivecommentRequest {
    comment: String,
    tip: i64,
}

pub fn post_livecomment(
    state: &AppState,
    cookie: Option<&str>,
    livestream_id: i64,
    body: &[u8],
) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
    let req: PostLivecommentRequest = parse_json(body)?;
    let owner_id = {
        let store = state.inner.store.read();
        store
            .livestreams
            .get(&livestream_id)
            .map(|ls| ls.user_id)
            .ok_or_else(|| Error::not_found("livestream not found"))?
    };
    let id = state
        .inner
        .next_livecomment_id
        .fetch_add(1, Ordering::Relaxed);
    let created_at = now_unix();
    let comment: Arc<str> = Arc::from(req.comment);
    {
        let mut shard = state.inner.shards[shard_idx(livestream_id)].write();
        let hit = shard
            .ng_by_ls
            .get(&livestream_id)
            .into_iter()
            .flatten()
            .filter_map(|wid| shard.ng_words.get(wid))
            .filter(|w| w.user_id == owner_id)
            .any(|w| comment.contains(w.word.as_str()));
        if hit {
            return Err(Error::bad("このコメントがスパム判定されました"));
        }
        shard.comments.insert(
            id,
            LivecommentRec {
                id,
                user_id: auth.user_id,
                livestream_id,
                comment: Arc::clone(&comment),
                tip: req.tip,
                created_at,
            },
        );
        shard
            .comments_by_ls
            .entry(livestream_id)
            .or_default()
            .push(id);
    }
    state.inner.bump_comment(livestream_id, owner_id, req.tip);
    let store = state.inner.store.read();
    let user = store
        .user_json(auth.user_id)
        .ok_or_else(|| Error::internal("failed to fill livecomment"))?;
    let ls = store
        .ls_json(livestream_id)
        .ok_or_else(|| Error::internal("failed to fill livecomment"))?;
    drop(store);
    Ok(HttpResp::json(
        201,
        store::assemble_comment(id, &user, &ls, comment.as_ref(), req.tip, created_at),
    ))
}

pub fn post_report(
    state: &AppState,
    cookie: Option<&str>,
    livestream_id: i64,
    livecomment_id: i64,
) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
    if !state
        .inner
        .store
        .read()
        .livestreams
        .contains_key(&livestream_id)
    {
        return Err(Error::not_found("livestream not found"));
    }
    let id = state.inner.next_report_id.fetch_add(1, Ordering::Relaxed);
    let created_at = now_unix();
    {
        let mut shard = state.inner.shards[shard_idx(livestream_id)].write();
        if !shard.comments.contains_key(&livecomment_id) {
            return Err(Error::not_found("livecomment not found"));
        }
        shard.reports.insert(
            id,
            ReportRec {
                id,
                user_id: auth.user_id,
                livestream_id,
                livecomment_id,
                created_at,
            },
        );
        shard
            .reports_by_ls
            .entry(livestream_id)
            .or_default()
            .push(id);
    }
    if let Some(ls) = state.inner.store.read().livestreams.get(&livestream_id) {
        ls.reports_count.fetch_add(1, Ordering::Relaxed);
    }
    let shard = state.inner.shards[shard_idx(livestream_id)].read();
    let store = state.inner.store.read();
    let r = shard
        .reports
        .get(&id)
        .ok_or_else(|| Error::internal("failed to fill livecomment report"))?;
    let reporter = store
        .users
        .get(&r.user_id)
        .ok_or_else(|| Error::internal("failed to fill livecomment report"))?
        .to_public();
    let c = shard
        .comments
        .get(&livecomment_id)
        .ok_or_else(|| Error::internal("failed to fill livecomment report"))?;
    let comment_user = store
        .users
        .get(&c.user_id)
        .ok_or_else(|| Error::internal("failed to fill livecomment report"))?
        .to_public();
    let livestream = store
        .fill_livestream(livestream_id)
        .ok_or_else(|| Error::internal("failed to fill livecomment report"))?;
    json_created(&store::LivecommentReport {
        id,
        reporter,
        livecomment: store::Livecomment {
            id: c.id,
            user: comment_user,
            livestream,
            comment: c.comment.to_string(),
            tip: c.tip,
            created_at: c.created_at,
        },
        created_at,
    })
}

#[derive(serde::Deserialize)]
struct ModerateRequest {
    ng_word: String,
}

pub fn moderate(
    state: &AppState,
    cookie: Option<&str>,
    livestream_id: i64,
    body: &[u8],
) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
    let req: ModerateRequest = parse_json(body)?;
    let owned = state
        .inner
        .store
        .read()
        .livestreams
        .get(&livestream_id)
        .map(|ls| ls.user_id == auth.user_id)
        .unwrap_or(false);
    if !owned {
        return Err(Error::bad(
            "A streamer can't moderate livestreams that other streamers own",
        ));
    }
    let word_id = state.inner.next_ng_word_id.fetch_add(1, Ordering::Relaxed);
    let created_at = now_unix();
    let removed_tips = {
        let mut shard = state.inner.shards[shard_idx(livestream_id)].write();
        shard.ng_words.insert(
            word_id,
            NgWord {
                id: word_id,
                user_id: auth.user_id,
                livestream_id,
                word: req.ng_word,
                created_at,
            },
        );
        shard
            .ng_by_ls
            .entry(livestream_id)
            .or_default()
            .push(word_id);
        let tips = shard.moderate(livestream_id);
        let max_tip = shard.max_tip(livestream_id);
        (tips, max_tip)
    };
    state
        .inner
        .unbump_tips(livestream_id, auth.user_id, &removed_tips.0, removed_tips.1);
    json_created(&serde_json::json!({ "word_id": word_id }))
}

pub fn get_reactions(
    state: &AppState,
    cookie: Option<&str>,
    livestream_id: i64,
    query: &[(String, String)],
) -> Result<HttpResp, Error> {
    require_auth(state, cookie)?;
    let limit = parse_limit(query)?;
    let rows = {
        let shard = state.inner.shards[shard_idx(livestream_id)].read();
        let ids = shard
            .reactions_by_ls
            .get(&livestream_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let take = limit.unwrap_or(ids.len()).min(ids.len());
        let slice = &ids[ids.len() - take..];
        let mut rows = Vec::with_capacity(slice.len());
        for id in slice.iter().rev() {
            let r = shard
                .reactions
                .get(id)
                .ok_or_else(|| Error::internal("failed to fill reaction"))?;
            rows.push(ThinReaction {
                id: r.id,
                emoji_name: Arc::clone(&r.emoji_name),
                user_id: r.user_id,
                created_at: r.created_at,
            });
        }
        rows
    };
    let store = state.inner.store.read();
    let ls = store
        .ls_json(livestream_id)
        .ok_or_else(|| Error::internal("failed to fill reaction"))?;
    let mut users = HashMap::new();
    for row in &rows {
        if let std::collections::hash_map::Entry::Vacant(e) = users.entry(row.user_id) {
            let json = store
                .user_json(row.user_id)
                .ok_or_else(|| Error::internal("failed to fill reaction"))?;
            e.insert(json);
        }
    }
    drop(store);
    Ok(HttpResp::json(200, store::assemble_reactions(&rows, &users, &ls)))
}

#[derive(serde::Deserialize)]
struct PostReactionRequest {
    emoji_name: String,
}

pub fn post_reaction(
    state: &AppState,
    cookie: Option<&str>,
    livestream_id: i64,
    body: &[u8],
) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
    let req: PostReactionRequest = parse_json(body)?;
    let owner_id = {
        let store = state.inner.store.read();
        store
            .livestreams
            .get(&livestream_id)
            .map(|ls| ls.user_id)
            .ok_or_else(|| Error::not_found("livestream not found"))?
    };
    let id = state.inner.next_reaction_id.fetch_add(1, Ordering::Relaxed);
    let created_at = now_unix();
    let emoji: Arc<str> = Arc::from(req.emoji_name);
    {
        let mut shard = state.inner.shards[shard_idx(livestream_id)].write();
        shard.reactions.insert(
            id,
            ReactionRec {
                id,
                emoji_name: Arc::clone(&emoji),
                user_id: auth.user_id,
                livestream_id,
                created_at,
            },
        );
        shard
            .reactions_by_ls
            .entry(livestream_id)
            .or_default()
            .push(id);
    }
    state
        .inner
        .bump_reaction(livestream_id, owner_id, emoji.as_ref());
    let store = state.inner.store.read();
    let user = store
        .user_json(auth.user_id)
        .ok_or_else(|| Error::internal("failed to fill reaction"))?;
    let ls = store
        .ls_json(livestream_id)
        .ok_or_else(|| Error::internal("failed to fill reaction"))?;
    drop(store);
    Ok(HttpResp::json(
        201,
        store::assemble_reaction(id, emoji.as_ref(), &user, &ls, created_at),
    ))
}

fn from_base64<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use base64::Engine as _;
    use serde::de::{Deserialize as _, Error as _};
    let value = String::deserialize(deserializer)?;
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(D::Error::custom)
}

#[derive(serde::Deserialize)]
struct PostUserRequest {
    name: String,
    display_name: String,
    description: String,
    password: String,
    theme: PostUserRequestTheme,
}
#[derive(serde::Deserialize)]
struct PostUserRequestTheme {
    dark_mode: bool,
}
#[derive(serde::Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}
#[derive(serde::Deserialize)]
struct PostIconRequest {
    #[serde(deserialize_with = "from_base64")]
    image: Vec<u8>,
}

pub fn get_icon(
    state: &AppState,
    username: &str,
    if_none_match: Option<&str>,
) -> Result<HttpResp, Error> {
    let store = state.inner.store.read();
    let user_id = *store
        .users_by_name
        .get(username)
        .ok_or_else(|| Error::not_found("not found user that has the given username"))?;
    let user = store
        .users
        .get(&user_id)
        .ok_or_else(|| Error::not_found("not found user that has the given username"))?;
    let hash = user.icon_hash.clone();
    let body = user
        .icon
        .clone()
        .unwrap_or_else(|| state.inner.fallback_image.clone());
    drop(store);
    if let Some(inm) = if_none_match {
        if etag_matches(inm, &hash) {
            return Ok(HttpResp::empty(304).with_etag(hash));
        }
    }
    Ok(HttpResp::bytes(200, "image/jpeg", body.to_vec()).with_etag(hash))
}

pub async fn post_icon(state: &AppState, cookie: Option<&str>, body: &[u8]) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
    let req: PostIconRequest = parse_json(body)?;
    let (image, hash) = tokio::task::spawn_blocking(move || {
        let hash = sha256_hex(&req.image);
        (req.image, hash)
    })
    .await
    .map_err(|e| Error::internal(format!("sha256 join: {e}")))?;
    let req_image = image;
    let mut store = state.inner.store.write();
    let icon_id = store.next_icon_id;
    store.next_icon_id += 1;
    let user = store
        .users
        .get_mut(&auth.user_id)
        .ok_or_else(|| Error::not_found("not found user that has the userid in session"))?;
    user.icon = Some(Bytes::from(req_image));
    user.icon_hash = hash;
    user.icon_id = icon_id;
    store.invalidate_owner_cache(auth.user_id);
    json_created(&serde_json::json!({ "id": icon_id }))
}

pub fn me(state: &AppState, cookie: Option<&str>) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
    let store = state.inner.store.read();
    let body = store
        .user_json(auth.user_id)
        .ok_or_else(|| Error::not_found("not found user that has the userid in session"))?;
    Ok(HttpResp::json(200, body.to_vec()))
}

fn bcrypt_slots() -> &'static Semaphore {
    static SLOTS: OnceLock<Semaphore> = OnceLock::new();
    SLOTS.get_or_init(|| Semaphore::new(2))
}

async fn hash_password(password: String) -> Result<String, Error> {
    let _permit = bcrypt_slots()
        .acquire()
        .await
        .map_err(|_| Error::internal("bcrypt semaphore closed"))?;
    tokio::task::spawn_blocking(move || bcrypt::hash(password, BCRYPT_COST))
        .await
        .map_err(|e| Error::internal(format!("bcrypt join: {e}")))?
        .map_err(|e| Error::internal(format!("failed to generate hashed password: {e}")))
}

async fn verify_password(password: String, hashed: String) -> Result<bool, Error> {
    let _permit = bcrypt_slots()
        .acquire()
        .await
        .map_err(|_| Error::internal("bcrypt semaphore closed"))?;
    tokio::task::spawn_blocking(move || bcrypt::verify(password, &hashed))
        .await
        .map_err(|e| Error::internal(format!("bcrypt join: {e}")))?
        .map_err(|e| Error::internal(format!("failed to compare hash and password: {e}")))
}

pub async fn register(state: &AppState, body: &[u8]) -> Result<HttpResp, Error> {
    let req: PostUserRequest = parse_json(body)?;
    if req.name == "pipe" {
        return Err(Error::bad("the username 'pipe' is reserved"));
    }
    let hashed_password = hash_password(req.password).await?;
    let mut store = state.inner.store.write();
    if store.users_by_name.contains_key(&req.name) {
        return Err(Error::internal("failed to insert user: duplicate username"));
    }
    let user_id = store.next_user_id;
    store.next_user_id += 1;
    let theme_id = store.next_theme_id;
    store.next_theme_id += 1;
    let rec = UserRec {
        id: user_id,
        name: req.name.clone(),
        display_name: req.display_name,
        description: req.description,
        hashed_password,
        theme: store::Theme {
            id: theme_id,
            dark_mode: req.theme.dark_mode,
        },
        icon_hash: state.inner.fallback_hash.clone(),
        icon: None,
        icon_id: 0,
        livestream_ids: Vec::new(),
        reaction_count: AtomicI64::new(0),
        tip_sum: AtomicI64::new(0),
        livecomment_count: AtomicI64::new(0),
        viewers_count: AtomicI64::new(0),
        emoji_counts: Default::default(),
    };
    store.users_by_name.insert(req.name.clone(), user_id);
    store.users.insert(user_id, rec);
    store.cache_user_json(user_id);
    let body = store
        .user_json(user_id)
        .ok_or_else(|| Error::internal("failed to fill user"))?;
    drop(store);
    state.inner.rank.lock().insert_user(user_id, req.name.clone());
    state
        .inner
        .dns_names
        .write()
        .insert(format!("{}.{ZONE}", req.name.to_ascii_lowercase()));
    Ok(HttpResp::json(201, body.to_vec()))
}

pub async fn login(state: &AppState, body: &[u8]) -> Result<HttpResp, Error> {
    let req: LoginRequest = parse_json(body)?;
    let (user_id, hashed) = {
        let store = state.inner.store.read();
        let user_id = *store
            .users_by_name
            .get(&req.username)
            .ok_or_else(|| Error::unauthorized("invalid username or password"))?;
        let user = store
            .users
            .get(&user_id)
            .ok_or_else(|| Error::unauthorized("invalid username or password"))?;
        (user.id, user.hashed_password.clone())
    };
    if !verify_password(req.password, hashed).await? {
        return Err(Error::unauthorized("invalid username or password"));
    }
    let sid = create_session(state, user_id);
    Ok(HttpResp::empty(200).cookie(format!("{SESSION_COOKIE}={sid}")))
}

pub fn get_user(state: &AppState, cookie: Option<&str>, username: &str) -> Result<HttpResp, Error> {
    require_auth(state, cookie)?;
    let store = state.inner.store.read();
    let user_id = *store
        .users_by_name
        .get(username)
        .ok_or_else(|| Error::not_found("not found user that has the given username"))?;
    let body = store
        .user_json(user_id)
        .ok_or_else(|| Error::not_found("not found user that has the given username"))?;
    Ok(HttpResp::json(200, body.to_vec()))
}

pub fn user_stats(state: &AppState, cookie: Option<&str>, username: &str) -> Result<HttpResp, Error> {
    require_auth(state, cookie)?;
    let store = state.inner.store.read();
    let user_id = *store
        .users_by_name
        .get(username)
        .ok_or_else(|| Error::bad("not found user that has the given username"))?;
    let user = store
        .users
        .get(&user_id)
        .ok_or_else(|| Error::bad("not found user that has the given username"))?;
    let viewers_count = user.viewers_count.load(Ordering::Relaxed);
    let total_reactions = user.reaction_count.load(Ordering::Relaxed);
    let total_livecomments = user.livecomment_count.load(Ordering::Relaxed);
    let total_tip = user.tip_sum.load(Ordering::Relaxed);
    let favorite_emoji = user.favorite_emoji();
    drop(store);
    let rank = state
        .inner
        .rank
        .lock()
        .user_rank(user_id)
        .ok_or_else(|| Error::bad("not found user that has the given username"))?;
    json_ok(&serde_json::json!({
        "rank": rank,
        "viewers_count": viewers_count,
        "total_reactions": total_reactions,
        "total_livecomments": total_livecomments,
        "total_tip": total_tip,
        "favorite_emoji": favorite_emoji,
    }))
}

pub fn livestream_stats(
    state: &AppState,
    cookie: Option<&str>,
    livestream_id: i64,
) -> Result<HttpResp, Error> {
    require_auth(state, cookie)?;
    let store = state.inner.store.read();
    let ls = store
        .livestreams
        .get(&livestream_id)
        .ok_or_else(|| Error::bad("cannot get stats of not found livestream"))?;
    let viewers_count = ls.viewers_count.load(Ordering::Relaxed);
    let total_reactions = ls.reaction_count.load(Ordering::Relaxed);
    let total_reports = ls.reports_count.load(Ordering::Relaxed);
    let max_tip = ls.max_tip.load(Ordering::Relaxed);
    drop(store);
    let rank = state
        .inner
        .rank
        .lock()
        .livestream_rank(livestream_id)
        .ok_or_else(|| Error::bad("cannot get stats of not found livestream"))?;
    json_ok(&serde_json::json!({
        "rank": rank,
        "viewers_count": viewers_count,
        "total_reactions": total_reactions,
        "total_reports": total_reports,
        "max_tip": max_tip,
    }))
}

pub fn payment(state: &AppState) -> Result<HttpResp, Error> {
    json_ok(&serde_json::json!({
        "total_tip": state.inner.total_tip.load(Ordering::Relaxed)
    }))
}

pub async fn reload_on_boot(inner: &Inner) {
    match store::load_store(&inner.pool, inner.fallback_hash.clone()).await {
        Ok((loaded, shards)) => install_loaded(inner, loaded, shards),
        Err(err) => {
            tracing::warn!("startup load skipped: {err}");
            *inner.dns_names.write() = rebuild_dns_names(&inner.store.read());
        }
    }
}
