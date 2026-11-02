pub mod dns;
pub mod handlers;
pub mod http;
pub mod store;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicI64;
use std::sync::Arc;

use bytes::Bytes;
use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use sqlx::MySqlPool;
use store::{LsShard, Store, SHARDS};

pub const SESSION_COOKIE: &str = "SESSIONID";
pub const BCRYPT_COST: u32 = 4;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    BadRequest(std::borrow::Cow<'static, str>),
    #[error("{0}")]
    Unauthorized(std::borrow::Cow<'static, str>),
    #[error("{0}")]
    Forbidden(std::borrow::Cow<'static, str>),
    #[error("{0}")]
    NotFound(std::borrow::Cow<'static, str>),
    #[error("{0}")]
    Internal(std::borrow::Cow<'static, str>),
}

impl Error {
    pub fn bad(msg: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        Self::BadRequest(msg.into())
    }
    pub fn unauthorized(msg: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        Self::Unauthorized(msg.into())
    }
    pub fn forbidden(msg: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        Self::Forbidden(msg.into())
    }
    pub fn not_found(msg: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        Self::NotFound(msg.into())
    }
    pub fn internal(msg: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        Self::Internal(msg.into())
    }
}

pub struct SessionRec {
    pub user_id: i64,
    pub expires: i64,
}

pub struct Inner {
    pub store: RwLock<Store>,
    pub shards: Vec<RwLock<LsShard>>,
    pub dns_names: Arc<RwLock<HashSet<String>>>,
    pub pool: MySqlPool,
    pub fallback_image: Bytes,
    pub fallback_hash: String,
    pub init_lock: tokio::sync::Mutex<()>,
    pub sessions: RwLock<HashMap<String, SessionRec>>,
    pub next_livecomment_id: AtomicI64,
    pub next_reaction_id: AtomicI64,
    pub next_ng_word_id: AtomicI64,
    pub next_report_id: AtomicI64,
}

impl Inner {
    pub fn empty_shards() -> Vec<RwLock<LsShard>> {
        (0..SHARDS).map(|_| RwLock::new(LsShard::default())).collect()
    }

    pub fn install_shards(&self, shards: Vec<LsShard>) {
        for (i, shard) in shards.into_iter().enumerate() {
            *self.shards[i].write() = shard;
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<Inner>,
}

pub fn parse_json<T: DeserializeOwned>(body: &[u8]) -> Result<T, Error> {
    serde_json::from_slice(body).map_err(|_| Error::bad("failed to decode the request body as json"))
}

pub fn json_bytes<T: serde::Serialize>(v: &T) -> Result<Vec<u8>, Error> {
    serde_json::to_vec(v).map_err(|e| Error::internal(format!("json encode: {e}")))
}

pub struct Auth {
    pub user_id: i64,
}

pub fn require_auth(state: &AppState, cookie: Option<&str>) -> Result<Auth, Error> {
    let sid = cookie.ok_or_else(|| Error::forbidden("failed to get EXPIRES value from session"))?;
    let sessions = state.inner.sessions.read();
    let sess = sessions
        .get(sid)
        .ok_or_else(|| Error::forbidden("failed to get EXPIRES value from session"))?;
    if store::now_unix() > sess.expires {
        return Err(Error::unauthorized("session has expired"));
    }
    Ok(Auth {
        user_id: sess.user_id,
    })
}

pub fn create_session(state: &AppState, user_id: i64) -> String {
    let sid = uuid::Uuid::new_v4().to_string();
    state.inner.sessions.write().insert(
        sid.clone(),
        SessionRec {
            user_id,
            expires: store::now_unix() + 3600,
        },
    );
    sid
}

pub fn etag_matches(header_value: &str, hash: &str) -> bool {
    header_value.split(',').any(|part| {
        let t = part.trim();
        let t = t.strip_prefix("W/").unwrap_or(t).trim();
        let t = t.trim_matches('"');
        t == hash || t == "*"
    })
}

pub fn rebuild_dns_names(store: &Store) -> HashSet<String> {
    let mut names = HashSet::new();
    if let Ok(zone) = store::parse_zone_names(&std::path::PathBuf::from(store::ZONE_PATH)) {
        names.extend(zone);
    }
    names.insert(store::ZONE.to_string());
    names.insert(format!("ns1.{}", store::ZONE));
    names.insert(format!("pipe.{}", store::ZONE));
    for user in store.users.values() {
        names.insert(format!("{}.{}", user.name.to_ascii_lowercase(), store::ZONE));
    }
    names
}

pub fn mysql_options() -> sqlx::mysql::MySqlConnectOptions {
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
