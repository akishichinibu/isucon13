use std::time::Instant;

use crate::http::{query_str, HttpResp};
use crate::store::{
    now_unix, sha256_hex, LivecommentRec, LivestreamRec, NgWord, ReactionRec, ReportRec, UserRec,
    ViewerRec, TERM_END_AT, TERM_START_AT, ZONE,
};
use crate::{
    create_session, etag_matches, json_bytes, parse_json, rebuild_dns_names, require_auth, AppState,
    Error, Inner, BCRYPT_COST, SESSION_COOKIE,
};
use bytes::Bytes;

use crate::store;

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
    let loaded = store::load_store(&state.inner.pool, state.inner.fallback_hash.clone())
        .await
        .map_err(|e| Error::internal(format!("failed to load state: {e}")))?;
    let names = rebuild_dns_names(&loaded);
    *state.inner.store.write() = loaded;
    *state.inner.dns_names.write() = names;
    state.inner.sessions.write().clear();
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
    for slot in store.slots.iter() {
        if slot.start_at >= req.start_at && slot.end_at <= req.end_at && slot.slot < 1 {
            return Err(Error::bad(format!(
                "予約期間 {TERM_START_AT} ~ {TERM_END_AT}に対して、予約区間 {} ~ {}が予約できません",
                req.start_at, req.end_at
            )));
        }
    }
    for slot in store.slots.iter_mut() {
        if slot.start_at >= req.start_at && slot.end_at <= req.end_at {
            slot.slot -= 1;
        }
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
            reaction_count: 0,
            tip_sum: 0,
            max_tip: 0,
            viewers_count: 0,
            reports_count: 0,
        },
    );
    if let Some(user) = store.users.get_mut(&auth.user_id) {
        user.livestream_ids.push(livestream_id);
    }
    let livestream = store
        .fill_livestream(livestream_id)
        .ok_or_else(|| Error::internal("failed to fill livestream"))?;
    json_created(&livestream)
}

pub fn search(state: &AppState, query: &[(String, String)]) -> Result<HttpResp, Error> {
    let tag = query_str(query, "tag");
    let limit = query_str(query, "limit");
    let store = state.inner.store.read();
    let mut ids: Vec<i64> = if tag.is_empty() {
        let mut ids: Vec<i64> = store.livestreams.keys().copied().collect();
        ids.sort_unstable_by(|a, b| b.cmp(a));
        if !limit.is_empty() {
            let n: usize = limit
                .parse()
                .map_err(|_| Error::bad("limit query parameter must be integer"))?;
            ids.truncate(n);
        }
        ids
    } else if let Some(tag_id) = store.tags_by_name.get(tag) {
        store
            .livestreams_by_tag
            .get(tag_id)
            .cloned()
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    if !tag.is_empty() {
        ids.sort_unstable_by(|a, b| b.cmp(a));
        ids.dedup();
    }
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(ls) = store.fill_livestream(id) {
            out.push(ls);
        }
    }
    json_ok(&out)
}

pub fn my_livestreams(state: &AppState, cookie: Option<&str>) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
    let store = state.inner.store.read();
    let ids = store
        .users
        .get(&auth.user_id)
        .map(|u| u.livestream_ids.clone())
        .unwrap_or_default();
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(ls) = store.fill_livestream(id) {
            out.push(ls);
        }
    }
    json_ok(&out)
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
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(ls) = store.fill_livestream(id) {
            out.push(ls);
        }
    }
    json_ok(&out)
}

pub fn enter(state: &AppState, cookie: Option<&str>, livestream_id: i64) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
    let mut store = state.inner.store.write();
    store
        .viewers_by_ls
        .entry(livestream_id)
        .or_default()
        .push(ViewerRec {
            user_id: auth.user_id,
            created_at: now_unix(),
        });
    store.add_viewer_stats(livestream_id, 1);
    Ok(HttpResp::empty(200))
}

pub fn exit(state: &AppState, cookie: Option<&str>, livestream_id: i64) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
    let mut store = state.inner.store.write();
    if let Some(viewers) = store.viewers_by_ls.get_mut(&livestream_id) {
        let before = viewers.len();
        viewers.retain(|v| v.user_id != auth.user_id);
        let removed = (before - viewers.len()) as i64;
        if removed != 0 {
            store.add_viewer_stats(livestream_id, -removed);
        }
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
    let livestream = store
        .fill_livestream(livestream_id)
        .ok_or_else(|| Error::not_found("not found livestream that has the given id"))?;
    json_ok(&livestream)
}

pub fn get_reports(
    state: &AppState,
    cookie: Option<&str>,
    livestream_id: i64,
) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
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
    let ids = store
        .reports_by_ls
        .get(&livestream_id)
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let report = store
            .fill_report(id)
            .ok_or_else(|| Error::internal("failed to fill livecomment report"))?;
        out.push(report);
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
    let store = state.inner.store.read();
    let ids = store
        .comments_by_ls
        .get(&livestream_id)
        .cloned()
        .unwrap_or_default();
    let iter = ids.into_iter().rev();
    let chosen: Vec<i64> = match limit {
        Some(n) => iter.take(n).collect(),
        None => iter.collect(),
    };
    let mut out = Vec::with_capacity(chosen.len());
    for id in chosen {
        let c = store
            .fill_livecomment(id)
            .ok_or_else(|| Error::internal("failed to fil livecomments"))?;
        out.push(c);
    }
    json_ok(&out)
}

pub fn get_ngwords(
    state: &AppState,
    cookie: Option<&str>,
    livestream_id: i64,
) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
    let store = state.inner.store.read();
    let mut words: Vec<NgWord> = store
        .ng_by_ls
        .get(&livestream_id)
        .into_iter()
        .flatten()
        .filter_map(|id| store.ng_words.get(id))
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
    let mut store = state.inner.store.write();
    let ls = store
        .livestreams
        .get(&livestream_id)
        .ok_or_else(|| Error::not_found("livestream not found"))?;
    let owner_id = ls.user_id;
    let hit = store
        .ng_by_ls
        .get(&livestream_id)
        .into_iter()
        .flatten()
        .filter_map(|id| store.ng_words.get(id))
        .filter(|w| w.user_id == owner_id)
        .any(|w| req.comment.contains(&w.word));
    if hit {
        return Err(Error::bad("このコメントがスパム判定されました"));
    }
    let id = store.next_livecomment_id;
    store.next_livecomment_id += 1;
    let created_at = now_unix();
    store.comments.insert(
        id,
        LivecommentRec {
            id,
            user_id: auth.user_id,
            livestream_id,
            comment: req.comment,
            tip: req.tip,
            created_at,
        },
    );
    store
        .comments_by_ls
        .entry(livestream_id)
        .or_default()
        .push(id);
    store.add_comment_stats(livestream_id, req.tip);
    let livecomment = store
        .fill_livecomment(id)
        .ok_or_else(|| Error::internal("failed to fill livecomment"))?;
    json_created(&livecomment)
}

pub fn post_report(
    state: &AppState,
    cookie: Option<&str>,
    livestream_id: i64,
    livecomment_id: i64,
) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
    let mut store = state.inner.store.write();
    if !store.livestreams.contains_key(&livestream_id) {
        return Err(Error::not_found("livestream not found"));
    }
    if !store.comments.contains_key(&livecomment_id) {
        return Err(Error::not_found("livecomment not found"));
    }
    let id = store.next_report_id;
    store.next_report_id += 1;
    let created_at = now_unix();
    store.reports.insert(
        id,
        ReportRec {
            id,
            user_id: auth.user_id,
            livestream_id,
            livecomment_id,
            created_at,
        },
    );
    store
        .reports_by_ls
        .entry(livestream_id)
        .or_default()
        .push(id);
    if let Some(ls) = store.livestreams.get_mut(&livestream_id) {
        ls.reports_count += 1;
    }
    let report = store
        .fill_report(id)
        .ok_or_else(|| Error::internal("failed to fill livecomment report"))?;
    json_created(&report)
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
    let mut store = state.inner.store.write();
    let owned = store
        .livestreams
        .get(&livestream_id)
        .map(|ls| ls.user_id == auth.user_id)
        .unwrap_or(false);
    if !owned {
        return Err(Error::bad(
            "A streamer can't moderate livestreams that other streamers own",
        ));
    }
    let word_id = store.next_ng_word_id;
    store.next_ng_word_id += 1;
    let created_at = now_unix();
    store.ng_words.insert(
        word_id,
        NgWord {
            id: word_id,
            user_id: auth.user_id,
            livestream_id,
            word: req.ng_word,
            created_at,
        },
    );
    store
        .ng_by_ls
        .entry(livestream_id)
        .or_default()
        .push(word_id);
    store.moderate_livestream(livestream_id);
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
    let store = state.inner.store.read();
    let ids = store
        .reactions_by_ls
        .get(&livestream_id)
        .cloned()
        .unwrap_or_default();
    let iter = ids.into_iter().rev();
    let chosen: Vec<i64> = match limit {
        Some(n) => iter.take(n).collect(),
        None => iter.collect(),
    };
    let mut out = Vec::with_capacity(chosen.len());
    for id in chosen {
        let r = store
            .fill_reaction(id)
            .ok_or_else(|| Error::internal("failed to fill reaction"))?;
        out.push(r);
    }
    json_ok(&out)
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
    let mut store = state.inner.store.write();
    let id = store.next_reaction_id;
    store.next_reaction_id += 1;
    let created_at = now_unix();
    store.reactions.insert(
        id,
        ReactionRec {
            id,
            emoji_name: req.emoji_name.clone(),
            user_id: auth.user_id,
            livestream_id,
            created_at,
        },
    );
    store
        .reactions_by_ls
        .entry(livestream_id)
        .or_default()
        .push(id);
    store.add_reaction_stats(livestream_id, &req.emoji_name);
    let reaction = store
        .fill_reaction(id)
        .ok_or_else(|| Error::internal("failed to fill reaction"))?;
    json_created(&reaction)
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

pub fn post_icon(state: &AppState, cookie: Option<&str>, body: &[u8]) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
    let req: PostIconRequest = parse_json(body)?;
    let hash = sha256_hex(&req.image);
    let mut store = state.inner.store.write();
    let icon_id = store.next_icon_id;
    store.next_icon_id += 1;
    let user = store
        .users
        .get_mut(&auth.user_id)
        .ok_or_else(|| Error::not_found("not found user that has the userid in session"))?;
    user.icon = Some(Bytes::from(req.image));
    user.icon_hash = hash;
    user.icon_id = icon_id;
    json_created(&serde_json::json!({ "id": icon_id }))
}

pub fn me(state: &AppState, cookie: Option<&str>) -> Result<HttpResp, Error> {
    let auth = require_auth(state, cookie)?;
    let store = state.inner.store.read();
    let user = store
        .users
        .get(&auth.user_id)
        .ok_or_else(|| Error::not_found("not found user that has the userid in session"))?;
    json_ok(&user.to_public())
}

pub fn register(state: &AppState, body: &[u8]) -> Result<HttpResp, Error> {
    let req: PostUserRequest = parse_json(body)?;
    if req.name == "pipe" {
        return Err(Error::bad("the username 'pipe' is reserved"));
    }
    let hashed_password = bcrypt::hash(&req.password, BCRYPT_COST)
        .map_err(|e| Error::internal(format!("failed to generate hashed password: {e}")))?;
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
        reaction_count: 0,
        tip_sum: 0,
        livecomment_count: 0,
        viewers_count: 0,
        emoji_counts: Default::default(),
    };
    let public = rec.to_public();
    store.users_by_name.insert(req.name.clone(), user_id);
    store.users.insert(user_id, rec);
    drop(store);
    state
        .inner
        .dns_names
        .write()
        .insert(format!("{}.{ZONE}", req.name.to_ascii_lowercase()));
    json_created(&public)
}

pub fn login(state: &AppState, body: &[u8]) -> Result<HttpResp, Error> {
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
    match bcrypt::verify(&req.password, &hashed) {
        Ok(true) => {}
        Ok(false) => return Err(Error::unauthorized("invalid username or password")),
        Err(e) => {
            return Err(Error::internal(format!(
                "failed to compare hash and password: {e}"
            )))
        }
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
    let user = store
        .users
        .get(&user_id)
        .ok_or_else(|| Error::not_found("not found user that has the given username"))?;
    json_ok(&user.to_public())
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
    let rank = store
        .user_rank(username)
        .ok_or_else(|| Error::bad("not found user that has the given username"))?;
    json_ok(&serde_json::json!({
        "rank": rank,
        "viewers_count": user.viewers_count,
        "total_reactions": user.reaction_count,
        "total_livecomments": user.livecomment_count,
        "total_tip": user.tip_sum,
        "favorite_emoji": user.favorite_emoji(),
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
    let rank = store
        .livestream_rank(livestream_id)
        .ok_or_else(|| Error::bad("cannot get stats of not found livestream"))?;
    json_ok(&serde_json::json!({
        "rank": rank,
        "viewers_count": ls.viewers_count,
        "total_reactions": ls.reaction_count,
        "total_reports": ls.reports_count,
        "max_tip": ls.max_tip,
    }))
}

pub fn payment(state: &AppState) -> Result<HttpResp, Error> {
    let store = state.inner.store.read();
    json_ok(&serde_json::json!({ "total_tip": store.total_tip }))
}

pub async fn reload_on_boot(inner: &Inner) {
    match store::load_store(&inner.pool, inner.fallback_hash.clone()).await {
        Ok(loaded) => {
            let names = rebuild_dns_names(&loaded);
            *inner.store.write() = loaded;
            *inner.dns_names.write() = names;
        }
        Err(err) => {
            tracing::warn!("startup load skipped: {err}");
            *inner.dns_names.write() = rebuild_dns_names(&inner.store.read());
        }
    }
}
