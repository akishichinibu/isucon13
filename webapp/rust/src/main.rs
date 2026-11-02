mod dns;
mod store;

use std::borrow::Cow;
use std::collections::HashSet;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use async_session::{CookieStore, SessionStore};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, FromRef, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum_extra::extract::cookie::{Key, SignedCookieJar};
use bytes::Bytes as SharedBytes;
use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use sqlx::mysql::MySqlPoolOptions;
use sqlx::MySqlPool;
use store::{
    now_unix, sha256_hex, LivecommentRec, LivestreamRec, NgWord, ReactionRec, ReportRec, Store,
    UserRec, ViewerRec, FALLBACK_IMAGE_PATH, TERM_END_AT, TERM_START_AT, ZONE, ZONE_PATH,
};
use uuid::Uuid;

const DEFAULT_SESSION_ID_KEY: &str = "SESSIONID";
const DEFAULT_SESSION_EXPIRES_KEY: &str = "EXPIRES";
const DEFAULT_USER_ID_KEY: &str = "USERID";
const DEFAULT_USERNAME_KEY: &str = "USERNAME";
const BCRYPT_COST: u32 = 4;
const LISTEN_PORT: u16 = 8080;

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("{0}")]
    BadRequest(Cow<'static, str>),
    #[error("{0}")]
    Unauthorized(Cow<'static, str>),
    #[error("{0}")]
    Forbidden(Cow<'static, str>),
    #[error("{0}")]
    NotFound(Cow<'static, str>),
    #[error("{0}")]
    Internal(Cow<'static, str>),
}

impl Error {
    fn bad(msg: impl Into<Cow<'static, str>>) -> Self {
        Self::BadRequest(msg.into())
    }
    fn unauthorized(msg: impl Into<Cow<'static, str>>) -> Self {
        Self::Unauthorized(msg.into())
    }
    fn forbidden(msg: impl Into<Cow<'static, str>>) -> Self {
        Self::Forbidden(msg.into())
    }
    fn not_found(msg: impl Into<Cow<'static, str>>) -> Self {
        Self::NotFound(msg.into())
    }
    fn internal(msg: impl Into<Cow<'static, str>>) -> Self {
        Self::Internal(msg.into())
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        #[derive(serde::Serialize)]
        struct ErrorResponse {
            error: String,
        }
        let status = match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        if status.is_server_error() {
            tracing::error!("{self}");
        }
        (
            status,
            axum::Json(ErrorResponse {
                error: self.to_string(),
            }),
        )
            .into_response()
    }
}

struct Inner {
    store: RwLock<Store>,
    dns_names: Arc<RwLock<HashSet<String>>>,
    pool: MySqlPool,
    fallback_image: SharedBytes,
    fallback_hash: String,
    init_lock: tokio::sync::Mutex<()>,
}

#[derive(Clone)]
struct AppState {
    key: Key,
    inner: Arc<Inner>,
}

impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Self {
        state.key.clone()
    }
}

fn parse_json<T: DeserializeOwned>(body: &Bytes) -> Result<T, Error> {
    serde_json::from_slice(body)
        .map_err(|_| Error::bad("failed to decode the request body as json"))
}

fn build_mysql_options() -> sqlx::mysql::MySqlConnectOptions {
    let mut options = sqlx::mysql::MySqlConnectOptions::new()
        .host("127.0.0.1")
        .port(3306)
        .username("isucon")
        .password("isucon")
        .database("isupipe");
    if let Ok(host) = std::env::var("ISUCON13_MYSQL_DIALCONFIG_ADDRESS") {
        options = options.host(&host);
    }
    if let Some(port) = std::env::var("ISUCON13_MYSQL_DIALCONFIG_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        options = options.port(port);
    }
    if let Ok(user) = std::env::var("ISUCON13_MYSQL_DIALCONFIG_USER") {
        options = options.username(&user);
    }
    if let Ok(password) = std::env::var("ISUCON13_MYSQL_DIALCONFIG_PASSWORD") {
        options = options.password(&password);
    }
    if let Ok(database) = std::env::var("ISUCON13_MYSQL_DIALCONFIG_DATABASE") {
        options = options.database(&database);
    }
    options
}

fn parse_ipv4(s: &str) -> Result<[u8; 4], Error> {
    s.parse::<Ipv4Addr>()
        .map(|ip| ip.octets())
        .map_err(|_| Error::internal(format!("invalid IPv4 address: {s}")))
}

fn etag_matches(header_value: &str, hash: &str) -> bool {
    header_value.split(',').any(|part| {
        let t = part.trim();
        let t = t.strip_prefix("W/").unwrap_or(t).trim();
        let t = t.trim_matches('"');
        t == hash || t == "*"
    })
}

struct Auth {
    user_id: i64,
}

async fn require_auth(jar: &SignedCookieJar) -> Result<Auth, Error> {
    let cookie = jar
        .get(DEFAULT_SESSION_ID_KEY)
        .ok_or_else(|| Error::forbidden("failed to get EXPIRES value from session"))?;
    let sess = CookieStore::new()
        .load_session(cookie.value().to_owned())
        .await
        .map_err(|_| Error::unauthorized("failed to get session"))?
        .ok_or_else(|| Error::forbidden("failed to get EXPIRES value from session"))?;
    let session_expires: i64 = sess
        .get(DEFAULT_SESSION_EXPIRES_KEY)
        .ok_or_else(|| Error::forbidden("failed to get EXPIRES value from session"))?;
    let user_id: i64 = sess
        .get(DEFAULT_USER_ID_KEY)
        .ok_or_else(|| Error::unauthorized("failed to get USERID value from session"))?;
    if now_unix() > session_expires {
        return Err(Error::unauthorized("session has expired"));
    }
    Ok(Auth { user_id })
}

fn rebuild_dns_names(store: &Store) -> HashSet<String> {
    let mut names = HashSet::new();
    if let Ok(zone) = store::parse_zone_names(&PathBuf::from(ZONE_PATH)) {
        names.extend(zone);
    }
    names.insert(ZONE.to_string());
    names.insert(format!("ns1.{ZONE}"));
    names.insert(format!("pipe.{ZONE}"));
    for user in store.users.values() {
        names.insert(format!("{}.{ZONE}", user.name.to_ascii_lowercase()));
    }
    names
}

async fn reload_memory(inner: &Inner) -> Result<(), Error> {
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
    let store = store::load_store(&inner.pool, inner.fallback_hash.clone())
        .await
        .map_err(|e| Error::internal(format!("failed to load state: {e}")))?;
    let names = rebuild_dns_names(&store);
    *inner.store.write() = store;
    *inner.dns_names.write() = names;
    tracing::info!("initialize loaded in {:?}", started.elapsed());
    Ok(())
}

#[derive(serde::Serialize)]
struct InitializeResponse {
    language: &'static str,
}

async fn initialize_handler(
    State(state): State<AppState>,
) -> Result<axum::Json<InitializeResponse>, Error> {
    let _guard = state.inner.init_lock.lock().await;
    reload_memory(&state.inner).await?;
    Ok(axum::Json(InitializeResponse { language: "rust" }))
}

#[derive(serde::Serialize)]
struct TagsResponse {
    tags: Vec<store::Tag>,
}

async fn get_tag_handler(State(state): State<AppState>) -> axum::Json<TagsResponse> {
    let store = state.inner.store.read();
    let tags = store
        .tag_order
        .iter()
        .filter_map(|id| store.tags.get(id).cloned())
        .collect();
    axum::Json(TagsResponse { tags })
}

async fn get_streamer_theme_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(username): Path<String>,
) -> Result<axum::Json<store::Theme>, Error> {
    require_auth(&jar).await?;
    let store = state.inner.store.read();
    let user_id = *store
        .users_by_name
        .get(&username)
        .ok_or_else(|| Error::not_found("not found user that has the given username"))?;
    let theme = store
        .users
        .get(&user_id)
        .map(|u| u.theme.clone())
        .ok_or_else(|| Error::not_found("not found user that has the given username"))?;
    Ok(axum::Json(theme))
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

async fn reserve_livestream_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    body: Bytes,
) -> Result<(StatusCode, axum::Json<store::Livestream>), Error> {
    let auth = require_auth(&jar).await?;
    let req: ReserveLivestreamRequest = parse_json(&body)?;
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
    Ok((StatusCode::CREATED, axum::Json(livestream)))
}

#[derive(serde::Deserialize)]
struct SearchLivestreamsQuery {
    #[serde(default)]
    tag: String,
    #[serde(default)]
    limit: String,
}

async fn search_livestreams_handler(
    State(state): State<AppState>,
    Query(q): Query<SearchLivestreamsQuery>,
) -> Result<axum::Json<Vec<store::Livestream>>, Error> {
    let store = state.inner.store.read();
    let mut ids: Vec<i64> = if q.tag.is_empty() {
        let mut ids: Vec<i64> = store.livestreams.keys().copied().collect();
        ids.sort_unstable_by(|a, b| b.cmp(a));
        if !q.limit.is_empty() {
            let limit: usize = q
                .limit
                .parse()
                .map_err(|_| Error::bad("limit query parameter must be integer"))?;
            ids.truncate(limit);
        }
        ids
    } else if let Some(tag_id) = store.tags_by_name.get(&q.tag) {
        store
            .livestreams_by_tag
            .get(tag_id)
            .cloned()
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    // Tag search is already livestream_id DESC.
    if !q.tag.is_empty() {
        ids.sort_unstable_by(|a, b| b.cmp(a));
        ids.dedup();
    }
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(ls) = store.fill_livestream(id) {
            out.push(ls);
        }
    }
    Ok(axum::Json(out))
}

async fn get_my_livestreams_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar,
) -> Result<axum::Json<Vec<store::Livestream>>, Error> {
    let auth = require_auth(&jar).await?;
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
    Ok(axum::Json(out))
}

async fn get_user_livestreams_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(username): Path<String>,
) -> Result<axum::Json<Vec<store::Livestream>>, Error> {
    require_auth(&jar).await?;
    let store = state.inner.store.read();
    let user_id = *store
        .users_by_name
        .get(&username)
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
    Ok(axum::Json(out))
}

async fn enter_livestream_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(livestream_id): Path<i64>,
) -> Result<StatusCode, Error> {
    let auth = require_auth(&jar).await?;
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
    Ok(StatusCode::OK)
}

async fn exit_livestream_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(livestream_id): Path<i64>,
) -> Result<StatusCode, Error> {
    let auth = require_auth(&jar).await?;
    let mut store = state.inner.store.write();
    if let Some(viewers) = store.viewers_by_ls.get_mut(&livestream_id) {
        let before = viewers.len();
        viewers.retain(|v| v.user_id != auth.user_id);
        let removed = (before - viewers.len()) as i64;
        if removed != 0 {
            store.add_viewer_stats(livestream_id, -removed);
        }
    }
    Ok(StatusCode::OK)
}

async fn get_livestream_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(livestream_id): Path<i64>,
) -> Result<axum::Json<store::Livestream>, Error> {
    require_auth(&jar).await?;
    let store = state.inner.store.read();
    let livestream = store
        .fill_livestream(livestream_id)
        .ok_or_else(|| Error::not_found("not found livestream that has the given id"))?;
    Ok(axum::Json(livestream))
}

async fn get_livecomment_reports_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(livestream_id): Path<i64>,
) -> Result<axum::Json<Vec<store::LivecommentReport>>, Error> {
    let auth = require_auth(&jar).await?;
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
    Ok(axum::Json(out))
}

#[derive(serde::Deserialize)]
struct LimitQuery {
    #[serde(default)]
    limit: String,
}

async fn get_livecomments_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(livestream_id): Path<i64>,
    Query(q): Query<LimitQuery>,
) -> Result<axum::Json<Vec<store::Livecomment>>, Error> {
    require_auth(&jar).await?;
    let limit = if q.limit.is_empty() {
        None
    } else {
        Some(
            q.limit
                .parse::<usize>()
                .map_err(|_| Error::bad("limit query parameter must be integer"))?,
        )
    };
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
    Ok(axum::Json(out))
}

async fn get_ngwords(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(livestream_id): Path<i64>,
) -> Result<axum::Json<Vec<NgWord>>, Error> {
    let auth = require_auth(&jar).await?;
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
    words.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    Ok(axum::Json(words))
}

#[derive(serde::Deserialize)]
struct PostLivecommentRequest {
    comment: String,
    tip: i64,
}

async fn post_livecomment_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(livestream_id): Path<i64>,
    body: Bytes,
) -> Result<(StatusCode, axum::Json<store::Livecomment>), Error> {
    let auth = require_auth(&jar).await?;
    let req: PostLivecommentRequest = parse_json(&body)?;
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
    Ok((StatusCode::CREATED, axum::Json(livecomment)))
}

async fn report_livecomment_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path((livestream_id, livecomment_id)): Path<(i64, i64)>,
    body: Bytes,
) -> Result<(StatusCode, axum::Json<store::LivecommentReport>), Error> {
    let _ = body;
    let auth = require_auth(&jar).await?;
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
    Ok((StatusCode::CREATED, axum::Json(report)))
}

#[derive(serde::Deserialize)]
struct ModerateRequest {
    ng_word: String,
}

#[derive(serde::Serialize)]
struct ModerateResponse {
    word_id: i64,
}

async fn moderate_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(livestream_id): Path<i64>,
    body: Bytes,
) -> Result<(StatusCode, axum::Json<ModerateResponse>), Error> {
    let auth = require_auth(&jar).await?;
    let req: ModerateRequest = parse_json(&body)?;
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
    Ok((
        StatusCode::CREATED,
        axum::Json(ModerateResponse { word_id }),
    ))
}

async fn get_reactions_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(livestream_id): Path<i64>,
    Query(q): Query<LimitQuery>,
) -> Result<axum::Json<Vec<store::Reaction>>, Error> {
    require_auth(&jar).await?;
    let limit = if q.limit.is_empty() {
        None
    } else {
        Some(
            q.limit
                .parse::<usize>()
                .map_err(|_| Error::bad("limit query parameter must be integer"))?,
        )
    };
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
    Ok(axum::Json(out))
}

#[derive(serde::Deserialize)]
struct PostReactionRequest {
    emoji_name: String,
}

async fn post_reaction_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(livestream_id): Path<i64>,
    body: Bytes,
) -> Result<(StatusCode, axum::Json<store::Reaction>), Error> {
    let auth = require_auth(&jar).await?;
    let req: PostReactionRequest = parse_json(&body)?;
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
    Ok((StatusCode::CREATED, axum::Json(reaction)))
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

#[derive(serde::Serialize)]
struct PostIconResponse {
    id: i64,
}

async fn get_icon_handler(
    State(state): State<AppState>,
    Path(username): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Error> {
    let store = state.inner.store.read();
    let user_id = *store
        .users_by_name
        .get(&username)
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

    if let Some(inm) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    {
        if etag_matches(inm, &hash) {
            return Ok((
                StatusCode::NOT_MODIFIED,
                [(header::ETAG, format!("\"{hash}\""))],
            )
                .into_response());
        }
    }

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/jpeg".to_string()),
            (header::ETAG, format!("\"{hash}\"")),
        ],
        body,
    )
        .into_response())
}

async fn post_icon_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    body: Bytes,
) -> Result<(StatusCode, axum::Json<PostIconResponse>), Error> {
    let auth = require_auth(&jar).await?;
    let req: PostIconRequest = parse_json(&body)?;
    let hash = sha256_hex(&req.image);
    let mut store = state.inner.store.write();
    let icon_id = store.next_icon_id;
    store.next_icon_id += 1;
    let user = store
        .users
        .get_mut(&auth.user_id)
        .ok_or_else(|| Error::not_found("not found user that has the userid in session"))?;
    user.icon = Some(SharedBytes::from(req.image));
    user.icon_hash = hash;
    user.icon_id = icon_id;
    Ok((
        StatusCode::CREATED,
        axum::Json(PostIconResponse { id: icon_id }),
    ))
}

async fn get_me_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar,
) -> Result<axum::Json<store::User>, Error> {
    let auth = require_auth(&jar).await?;
    let store = state.inner.store.read();
    let user = store
        .users
        .get(&auth.user_id)
        .ok_or_else(|| Error::not_found("not found user that has the userid in session"))?;
    Ok(axum::Json(user.to_public()))
}

async fn register_handler(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<(StatusCode, axum::Json<store::User>), Error> {
    let req: PostUserRequest = parse_json(&body)?;
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

    Ok((StatusCode::CREATED, axum::Json(public)))
}

async fn login_handler(
    State(state): State<AppState>,
    mut jar: SignedCookieJar,
    body: Bytes,
) -> Result<(SignedCookieJar, StatusCode), Error> {
    let req: LoginRequest = parse_json(&body)?;
    let (user_id, name, hashed) = {
        let store = state.inner.store.read();
        let user_id = *store
            .users_by_name
            .get(&req.username)
            .ok_or_else(|| Error::unauthorized("invalid username or password"))?;
        let user = store
            .users
            .get(&user_id)
            .ok_or_else(|| Error::unauthorized("invalid username or password"))?;
        (user.id, user.name.clone(), user.hashed_password.clone())
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

    let session_end_at = now_unix() + 3600;
    let session_id = Uuid::new_v4().to_string();
    let mut sess = async_session::Session::new();
    sess.insert(DEFAULT_SESSION_ID_KEY, session_id)
        .map_err(|e| Error::internal(format!("failed to save session: {e}")))?;
    sess.insert(DEFAULT_USER_ID_KEY, user_id)
        .map_err(|e| Error::internal(format!("failed to save session: {e}")))?;
    sess.insert(DEFAULT_USERNAME_KEY, name)
        .map_err(|e| Error::internal(format!("failed to save session: {e}")))?;
    sess.insert(DEFAULT_SESSION_EXPIRES_KEY, session_end_at)
        .map_err(|e| Error::internal(format!("failed to save session: {e}")))?;
    if let Some(cookie_value) = CookieStore::new()
        .store_session(sess)
        .await
        .map_err(|e| Error::internal(format!("failed to save session: {e}")))?
    {
        let cookie =
            axum_extra::extract::cookie::Cookie::build(DEFAULT_SESSION_ID_KEY, cookie_value)
                .domain("u.isucon.dev")
                .max_age(time::Duration::seconds(60000))
                .path("/")
                .finish();
        jar = jar.add(cookie);
    }
    Ok((jar, StatusCode::OK))
}

async fn get_user_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(username): Path<String>,
) -> Result<axum::Json<store::User>, Error> {
    require_auth(&jar).await?;
    let store = state.inner.store.read();
    let user_id = *store
        .users_by_name
        .get(&username)
        .ok_or_else(|| Error::not_found("not found user that has the given username"))?;
    let user = store
        .users
        .get(&user_id)
        .ok_or_else(|| Error::not_found("not found user that has the given username"))?;
    Ok(axum::Json(user.to_public()))
}

#[derive(serde::Serialize)]
struct UserStatistics {
    rank: i64,
    viewers_count: i64,
    total_reactions: i64,
    total_livecomments: i64,
    total_tip: i64,
    favorite_emoji: String,
}

async fn get_user_statistics_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(username): Path<String>,
) -> Result<axum::Json<UserStatistics>, Error> {
    require_auth(&jar).await?;
    let store = state.inner.store.read();
    let user_id = *store
        .users_by_name
        .get(&username)
        .ok_or_else(|| Error::bad("not found user that has the given username"))?;
    let user = store
        .users
        .get(&user_id)
        .ok_or_else(|| Error::bad("not found user that has the given username"))?;
    let rank = store
        .user_rank(&username)
        .ok_or_else(|| Error::bad("not found user that has the given username"))?;
    Ok(axum::Json(UserStatistics {
        rank,
        viewers_count: user.viewers_count,
        total_reactions: user.reaction_count,
        total_livecomments: user.livecomment_count,
        total_tip: user.tip_sum,
        favorite_emoji: user.favorite_emoji(),
    }))
}

#[derive(serde::Serialize)]
struct LivestreamStatistics {
    rank: i64,
    viewers_count: i64,
    total_reactions: i64,
    total_reports: i64,
    max_tip: i64,
}

async fn get_livestream_statistics_handler(
    State(state): State<AppState>,
    jar: SignedCookieJar,
    Path(livestream_id): Path<i64>,
) -> Result<axum::Json<LivestreamStatistics>, Error> {
    require_auth(&jar).await?;
    let store = state.inner.store.read();
    let ls = store
        .livestreams
        .get(&livestream_id)
        .ok_or_else(|| Error::bad("cannot get stats of not found livestream"))?;
    let rank = store
        .livestream_rank(livestream_id)
        .ok_or_else(|| Error::bad("cannot get stats of not found livestream"))?;
    Ok(axum::Json(LivestreamStatistics {
        rank,
        viewers_count: ls.viewers_count,
        total_reactions: ls.reaction_count,
        total_reports: ls.reports_count,
        max_tip: ls.max_tip,
    }))
}

#[derive(serde::Serialize)]
struct PaymentResult {
    total_tip: i64,
}

async fn get_payment_result(State(state): State<AppState>) -> axum::Json<PaymentResult> {
    let store = state.inner.store.read();
    axum::Json(PaymentResult {
        total_tip: store.total_tip,
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "info");
    }
    tracing_subscriber::fmt::init();

    let pool = MySqlPoolOptions::new()
        .max_connections(10)
        .connect_with(build_mysql_options())
        .await
        .expect("failed to connect db");

    const DEFAULT_SECRET: &[u8] = b"isucon13_session_cookiestore_defaultsecret";
    let secret = std::env::var("ISUCON13_SESSION_SECRETKEY")
        .map(|s| s.into_bytes())
        .unwrap_or_else(|_| DEFAULT_SECRET.to_owned());

    let powerdns_addr = std::env::var("ISUCON13_POWERDNS_SUBDOMAIN_ADDRESS")
        .expect("environ ISUCON13_POWERDNS_SUBDOMAIN_ADDRESS must be provided");
    let ipv4 = parse_ipv4(&powerdns_addr).expect("POWERDNS address must be IPv4");

    let (fallback_image, fallback_hash) = store::load_fallback_image().unwrap_or_else(|e| {
        tracing::warn!("failed to read {FALLBACK_IMAGE_PATH}: {e}");
        (SharedBytes::new(), String::new())
    });

    let dns_names = Arc::new(RwLock::new(HashSet::new()));
    let inner = Arc::new(Inner {
        store: RwLock::new(Store {
            fallback_hash: fallback_hash.clone(),
            ..Store::default()
        }),
        dns_names: dns_names.clone(),
        pool,
        fallback_image,
        fallback_hash,
        init_lock: tokio::sync::Mutex::new(()),
    });

    // Best-effort load so a restarted process can serve pretest before initialize.
    if let Err(err) = store::load_store(&inner.pool, inner.fallback_hash.clone())
        .await
        .map(|loaded| {
            let names = rebuild_dns_names(&loaded);
            *inner.store.write() = loaded;
            *inner.dns_names.write() = names;
        })
    {
        tracing::warn!("startup load skipped: {err}");
        *inner.dns_names.write() = rebuild_dns_names(&inner.store.read());
    }

    dns::start(dns_names, ipv4);

    let app = axum::Router::new()
        .route("/api/initialize", post(initialize_handler))
        .route("/api/tag", get(get_tag_handler))
        .route("/api/user/:username/theme", get(get_streamer_theme_handler))
        .route(
            "/api/livestream/reservation",
            post(reserve_livestream_handler),
        )
        .route("/api/livestream/search", get(search_livestreams_handler))
        .route("/api/livestream", get(get_my_livestreams_handler))
        .route(
            "/api/user/:username/livestream",
            get(get_user_livestreams_handler),
        )
        .route(
            "/api/livestream/:livestream_id",
            get(get_livestream_handler),
        )
        .route(
            "/api/livestream/:livestream_id/livecomment",
            get(get_livecomments_handler).post(post_livecomment_handler),
        )
        .route(
            "/api/livestream/:livestream_id/reaction",
            get(get_reactions_handler).post(post_reaction_handler),
        )
        .route(
            "/api/livestream/:livestream_id/report",
            get(get_livecomment_reports_handler),
        )
        .route("/api/livestream/:livestream_id/ngwords", get(get_ngwords))
        .route(
            "/api/livestream/:livestream_id/livecomment/:livecomment_id/report",
            post(report_livecomment_handler),
        )
        .route(
            "/api/livestream/:livestream_id/moderate",
            post(moderate_handler),
        )
        .route(
            "/api/livestream/:livestream_id/enter",
            post(enter_livestream_handler),
        )
        .route(
            "/api/livestream/:livestream_id/exit",
            delete(exit_livestream_handler),
        )
        .route("/api/register", post(register_handler))
        .route("/api/login", post(login_handler))
        .route("/api/user/me", get(get_me_handler))
        .route("/api/user/:username", get(get_user_handler))
        .route(
            "/api/user/:username/statistics",
            get(get_user_statistics_handler),
        )
        .route("/api/user/:username/icon", get(get_icon_handler))
        .route("/api/icon", post(post_icon_handler))
        .route(
            "/api/livestream/:livestream_id/statistics",
            get(get_livestream_statistics_handler),
        )
        .route("/api/payment", get(get_payment_result))
        .with_state(AppState {
            key: Key::derive_from(&secret),
            inner,
        })
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024));

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], LISTEN_PORT));
    tracing::info!("listening on {addr}");
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;
    Ok(())
}
