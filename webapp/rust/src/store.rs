use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::ops::Bound;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use sha2::{Digest, Sha256};
use sqlx::MySqlPool;

pub const TERM_START_AT: i64 = 1_700_874_000;
pub const TERM_END_AT: i64 = 1_732_496_400;
pub const FALLBACK_IMAGE_PATH: &str = "../img/NoImage.jpg";
pub const ZONE_PATH: &str = "../pdns/u.isucon.dev.zone";
pub const ZONE: &str = "u.isucon.dev";

#[derive(Clone, Debug, serde::Serialize)]
pub struct Tag {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Theme {
    pub id: i64,
    pub dark_mode: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct User {
    pub id: i64,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub theme: Theme,
    pub icon_hash: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Livestream {
    pub id: i64,
    pub owner: User,
    pub title: String,
    pub description: String,
    pub playlist_url: String,
    pub thumbnail_url: String,
    pub tags: Vec<Tag>,
    pub start_at: i64,
    pub end_at: i64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Livecomment {
    pub id: i64,
    pub user: User,
    pub livestream: Livestream,
    pub comment: String,
    pub tip: i64,
    pub created_at: i64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct LivecommentReport {
    pub id: i64,
    pub reporter: User,
    pub livecomment: Livecomment,
    pub created_at: i64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Reaction {
    pub id: i64,
    pub emoji_name: String,
    pub user: User,
    pub livestream: Livestream,
    pub created_at: i64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct NgWord {
    pub id: i64,
    pub user_id: i64,
    pub livestream_id: i64,
    pub word: String,
    pub created_at: i64,
}

#[derive(Debug)]
pub struct UserRec {
    pub id: i64,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub hashed_password: String,
    pub theme: Theme,
    pub icon_hash: String,
    pub icon: Option<Bytes>,
    pub icon_id: i64,
    pub livestream_ids: Vec<i64>,
    pub reaction_count: AtomicI64,
    pub tip_sum: AtomicI64,
    pub livecomment_count: AtomicI64,
    pub viewers_count: AtomicI64,
    pub emoji_counts: parking_lot::Mutex<HashMap<String, i64>>,
}

impl UserRec {
    pub fn score(&self) -> i64 {
        self.reaction_count.load(Ordering::Relaxed) + self.tip_sum.load(Ordering::Relaxed)
    }

    pub fn to_public(&self) -> User {
        User {
            id: self.id,
            name: self.name.clone(),
            display_name: self.display_name.clone(),
            description: self.description.clone(),
            theme: self.theme.clone(),
            icon_hash: self.icon_hash.clone(),
        }
    }

    pub fn favorite_emoji(&self) -> String {
        self.emoji_counts
            .lock()
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then_with(|| a.0.cmp(b.0)))
            .map(|(name, _)| name.clone())
            .unwrap_or_default()
    }
}

#[derive(Debug)]
pub struct LivestreamRec {
    pub id: i64,
    pub user_id: i64,
    pub title: String,
    pub description: String,
    pub playlist_url: String,
    pub thumbnail_url: String,
    pub start_at: i64,
    pub end_at: i64,
    pub tag_ids: Vec<i64>,
    pub reaction_count: AtomicI64,
    pub tip_sum: AtomicI64,
    pub max_tip: AtomicI64,
    pub viewers_count: AtomicI64,
    pub reports_count: AtomicI64,
}

impl LivestreamRec {
    pub fn score(&self) -> i64 {
        self.reaction_count.load(Ordering::Relaxed) + self.tip_sum.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Clone)]
pub struct LivecommentRec {
    pub id: i64,
    pub user_id: i64,
    pub livestream_id: i64,
    pub comment: Arc<str>,
    pub tip: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct ReactionRec {
    pub id: i64,
    pub emoji_name: Arc<str>,
    pub user_id: i64,
    pub livestream_id: i64,
    pub created_at: i64,
}

#[derive(Clone)]
pub struct ThinComment {
    pub id: i64,
    pub user_id: i64,
    pub comment: Arc<str>,
    pub tip: i64,
    pub created_at: i64,
}

#[derive(Clone)]
pub struct ThinReaction {
    pub id: i64,
    pub emoji_name: Arc<str>,
    pub user_id: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct ReportRec {
    pub id: i64,
    pub user_id: i64,
    #[allow(dead_code)]
    pub livestream_id: i64,
    pub livecomment_id: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct ViewerRec {
    pub user_id: i64,
    #[allow(dead_code)]
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct SlotRec {
    pub start_at: i64,
    pub end_at: i64,
    pub slot: i64,
}

pub const SHARDS: usize = 32;

pub fn shard_idx(livestream_id: i64) -> usize {
    (livestream_id as u64 % SHARDS as u64) as usize
}

#[derive(Debug, Default)]
pub struct LsShard {
    pub comments: HashMap<i64, LivecommentRec>,
    pub comments_by_ls: HashMap<i64, Vec<i64>>,
    pub reactions: HashMap<i64, ReactionRec>,
    pub reactions_by_ls: HashMap<i64, Vec<i64>>,
    pub ng_words: HashMap<i64, NgWord>,
    pub ng_by_ls: HashMap<i64, Vec<i64>>,
    pub reports: HashMap<i64, ReportRec>,
    pub reports_by_ls: HashMap<i64, Vec<i64>>,
    pub viewers_by_ls: HashMap<i64, Vec<ViewerRec>>,
}

impl LsShard {
    pub fn max_tip(&self, livestream_id: i64) -> i64 {
        self.comments_by_ls
            .get(&livestream_id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.comments.get(id).map(|c| c.tip))
            .max()
            .unwrap_or(0)
    }

    pub fn moderate(&mut self, livestream_id: i64) -> Vec<i64> {
        let words: Vec<String> = self
            .ng_by_ls
            .get(&livestream_id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.ng_words.get(id).map(|w| w.word.clone()))
            .collect();
        if words.is_empty() {
            return Vec::new();
        }
        let ids = self
            .comments_by_ls
            .get(&livestream_id)
            .cloned()
            .unwrap_or_default();
        let mut keep = Vec::with_capacity(ids.len());
        let mut removed_tips = Vec::new();
        for id in ids {
            let hit = self
                .comments
                .get(&id)
                .map(|c| words.iter().any(|w| c.comment.contains(w.as_str())))
                .unwrap_or(false);
            if hit {
                if let Some(c) = self.comments.remove(&id) {
                    removed_tips.push(c.tip);
                }
            } else {
                keep.push(id);
            }
        }
        self.comments_by_ls.insert(livestream_id, keep);
        removed_tips
    }
}

#[derive(Debug, Default)]
pub struct RankState {
    pub total_tip: i64,
    ls_score: HashMap<i64, i64>,
    ls_index: BTreeSet<(i64, i64)>,
    user_score: HashMap<i64, i64>,
    user_name: HashMap<i64, String>,
    user_index: BTreeSet<(i64, String)>,
}

impl RankState {
    pub fn from_store(store: &Store) -> Self {
        let mut rank = RankState::default();
        for ls in store.livestreams.values() {
            let score = ls.score();
            rank.ls_score.insert(ls.id, score);
            rank.ls_index.insert((score, ls.id));
            rank.total_tip += ls.tip_sum.load(Ordering::Relaxed);
        }
        for user in store.users.values() {
            let score = user.score();
            rank.user_score.insert(user.id, score);
            rank.user_index.insert((score, user.name.clone()));
            rank.user_name.insert(user.id, user.name.clone());
        }
        rank
    }

    pub fn insert_user(&mut self, user_id: i64, name: String) {
        self.user_name.insert(user_id, name.clone());
        self.user_score.insert(user_id, 0);
        self.user_index.insert((0, name));
    }

    pub fn insert_ls(&mut self, livestream_id: i64) {
        self.ls_score.insert(livestream_id, 0);
        self.ls_index.insert((0, livestream_id));
    }

    pub fn bump_ls(&mut self, livestream_id: i64, delta: i64) {
        let old = self.ls_score.get(&livestream_id).copied().unwrap_or(0);
        self.ls_index.remove(&(old, livestream_id));
        let new = old + delta;
        self.ls_score.insert(livestream_id, new);
        self.ls_index.insert((new, livestream_id));
    }

    pub fn bump_user(&mut self, user_id: i64, delta: i64) {
        let old = self.user_score.get(&user_id).copied().unwrap_or(0);
        if let Some(name) = self.user_name.get(&user_id) {
            self.user_index.remove(&(old, name.clone()));
            self.user_index.insert((old + delta, name.clone()));
        }
        self.user_score.insert(user_id, old + delta);
    }

    pub fn add_tip(&mut self, livestream_id: i64, owner_id: i64, tip: i64) {
        self.total_tip += tip;
        self.bump_ls(livestream_id, tip);
        self.bump_user(owner_id, tip);
    }

    pub fn add_reaction(&mut self, livestream_id: i64, owner_id: i64) {
        self.bump_ls(livestream_id, 1);
        self.bump_user(owner_id, 1);
    }

    pub fn remove_tip(&mut self, livestream_id: i64, owner_id: i64, tip: i64) {
        self.total_tip -= tip;
        self.bump_ls(livestream_id, -tip);
        self.bump_user(owner_id, -tip);
    }

    pub fn user_rank(&self, user_id: i64) -> Option<i64> {
        let score = *self.user_score.get(&user_id)?;
        let name = self.user_name.get(&user_id)?;
        let better = self
            .user_index
            .range((Bound::Excluded((score, name.clone())), Bound::Unbounded))
            .count();
        Some(better as i64 + 1)
    }

    pub fn livestream_rank(&self, livestream_id: i64) -> Option<i64> {
        let score = *self.ls_score.get(&livestream_id)?;
        let better = self
            .ls_index
            .range((Bound::Excluded((score, livestream_id)), Bound::Unbounded))
            .count();
        Some(better as i64 + 1)
    }
}

fn push_i64(out: &mut Vec<u8>, n: i64) {
    let _ = write!(out, "{n}");
}

fn push_json_str(out: &mut Vec<u8>, s: &str) {
    if let Ok(buf) = serde_json::to_vec(s) {
        out.extend_from_slice(&buf);
    } else {
        out.extend_from_slice(b"\"\"");
    }
}

pub fn assemble_comment(id: i64, user: &[u8], ls: &[u8], comment: &str, tip: i64, created_at: i64) -> Vec<u8> {
    let mut out = Vec::with_capacity(user.len() + ls.len() + comment.len() + 80);
    push_comment(&mut out, id, user, ls, comment, tip, created_at);
    out
}

fn push_comment(out: &mut Vec<u8>, id: i64, user: &[u8], ls: &[u8], comment: &str, tip: i64, created_at: i64) {
    out.extend_from_slice(b"{\"id\":");
    push_i64(out, id);
    out.extend_from_slice(b",\"user\":");
    out.extend_from_slice(user);
    out.extend_from_slice(b",\"livestream\":");
    out.extend_from_slice(ls);
    out.extend_from_slice(b",\"comment\":");
    push_json_str(out, comment);
    out.extend_from_slice(b",\"tip\":");
    push_i64(out, tip);
    out.extend_from_slice(b",\"created_at\":");
    push_i64(out, created_at);
    out.push(b'}');
}

pub fn assemble_comments(rows: &[ThinComment], users: &HashMap<i64, Bytes>, ls: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + rows.len() * (ls.len() + 96));
    out.push(b'[');
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        let user = users.get(&row.user_id).map(|b| b.as_ref()).unwrap_or(b"{}");
        push_comment(&mut out, row.id, user, ls, row.comment.as_ref(), row.tip, row.created_at);
    }
    out.push(b']');
    out
}

pub fn assemble_reaction(
    id: i64,
    emoji_name: &str,
    user: &[u8],
    ls: &[u8],
    created_at: i64,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(user.len() + ls.len() + emoji_name.len() + 64);
    push_reaction(&mut out, id, emoji_name, user, ls, created_at);
    out
}

fn push_reaction(out: &mut Vec<u8>, id: i64, emoji_name: &str, user: &[u8], ls: &[u8], created_at: i64) {
    out.extend_from_slice(b"{\"id\":");
    push_i64(out, id);
    out.extend_from_slice(b",\"emoji_name\":");
    push_json_str(out, emoji_name);
    out.extend_from_slice(b",\"user\":");
    out.extend_from_slice(user);
    out.extend_from_slice(b",\"livestream\":");
    out.extend_from_slice(ls);
    out.extend_from_slice(b",\"created_at\":");
    push_i64(out, created_at);
    out.push(b'}');
}

pub fn assemble_reactions(rows: &[ThinReaction], users: &HashMap<i64, Bytes>, ls: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + rows.len() * (ls.len() + 80));
    out.push(b'[');
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        let user = users.get(&row.user_id).map(|b| b.as_ref()).unwrap_or(b"{}");
        push_reaction(&mut out, row.id, row.emoji_name.as_ref(), user, ls, row.created_at);
    }
    out.push(b']');
    out
}

pub fn assemble_json_array<'a, I>(items: I) -> Vec<u8>
where
    I: IntoIterator<Item = &'a [u8]>,
{
    let mut out = vec![b'['];
    let mut first = true;
    for item in items {
        if !first {
            out.push(b',');
        }
        first = false;
        out.extend_from_slice(item);
    }
    out.push(b']');
    out
}

#[derive(Debug, Default)]
pub struct Store {
    pub users: HashMap<i64, UserRec>,
    pub users_by_name: HashMap<String, i64>,
    pub livestreams: HashMap<i64, LivestreamRec>,
    pub tags: HashMap<i64, Tag>,
    pub tags_by_name: HashMap<String, i64>,
    pub tag_order: Vec<i64>,
    pub livestreams_by_tag: HashMap<i64, Vec<i64>>,
    /// Newest livestream id first. Search without a tag walks this prefix.
    pub livestream_ids_desc: Vec<i64>,
    pub slots: Vec<SlotRec>,
    pub next_user_id: i64,
    pub next_theme_id: i64,
    pub next_icon_id: i64,
    pub next_livestream_id: i64,
    pub next_livecomment_id: i64,
    pub next_reaction_id: i64,
    pub next_ng_word_id: i64,
    pub next_report_id: i64,
    pub cached_ls_json: HashMap<i64, Bytes>,
    pub cached_user_json: HashMap<i64, Bytes>,
    #[allow(dead_code)]
    pub fallback_hash: String,
}

impl Store {
    fn build_livestream(&self, id: i64) -> Option<Livestream> {
        let ls = self.livestreams.get(&id)?;
        let owner = self.users.get(&ls.user_id)?.to_public();
        let tags = ls
            .tag_ids
            .iter()
            .filter_map(|tid| self.tags.get(tid).cloned())
            .collect();
        Some(Livestream {
            id: ls.id,
            owner,
            title: ls.title.clone(),
            description: ls.description.clone(),
            playlist_url: ls.playlist_url.clone(),
            thumbnail_url: ls.thumbnail_url.clone(),
            tags,
            start_at: ls.start_at,
            end_at: ls.end_at,
        })
    }

    pub fn fill_livestream(&self, id: i64) -> Option<Livestream> {
        self.build_livestream(id)
    }

    pub fn cache_user_json(&mut self, user_id: i64) {
        let Some(user) = self.users.get(&user_id) else {
            return;
        };
        if let Ok(buf) = serde_json::to_vec(&user.to_public()) {
            self.cached_user_json.insert(user_id, Bytes::from(buf));
        }
    }

    pub fn cache_livestream(&mut self, id: i64) {
        let Some(ls) = self.build_livestream(id) else {
            return;
        };
        if let Ok(buf) = serde_json::to_vec(&ls) {
            self.cached_ls_json.insert(id, Bytes::from(buf));
        }
    }

    pub fn invalidate_owner_cache(&mut self, user_id: i64) {
        self.cache_user_json(user_id);
        let ids = self
            .users
            .get(&user_id)
            .map(|u| u.livestream_ids.clone())
            .unwrap_or_default();
        for id in ids {
            self.cache_livestream(id);
        }
    }

    pub fn user_json(&self, user_id: i64) -> Option<Bytes> {
        if let Some(buf) = self.cached_user_json.get(&user_id) {
            return Some(buf.clone());
        }
        let user = self.users.get(&user_id)?;
        serde_json::to_vec(&user.to_public())
            .ok()
            .map(Bytes::from)
    }

    pub fn ls_json(&self, livestream_id: i64) -> Option<Bytes> {
        if let Some(buf) = self.cached_ls_json.get(&livestream_id) {
            return Some(buf.clone());
        }
        let ls = self.build_livestream(livestream_id)?;
        serde_json::to_vec(&ls).ok().map(Bytes::from)
    }

    pub fn add_comment_stats(&self, livestream_id: i64, tip: i64) {
        if let Some(ls) = self.livestreams.get(&livestream_id) {
            ls.tip_sum.fetch_add(tip, Ordering::Relaxed);
            ls.max_tip.fetch_max(tip, Ordering::Relaxed);
            if let Some(owner) = self.users.get(&ls.user_id) {
                owner.tip_sum.fetch_add(tip, Ordering::Relaxed);
                owner.livecomment_count.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn remove_comment_stats(&self, livestream_id: i64, tip: i64, new_max_tip: i64) {
        if let Some(ls) = self.livestreams.get(&livestream_id) {
            ls.tip_sum.fetch_sub(tip, Ordering::Relaxed);
            ls.max_tip.store(new_max_tip, Ordering::Relaxed);
            if let Some(owner) = self.users.get(&ls.user_id) {
                owner.tip_sum.fetch_sub(tip, Ordering::Relaxed);
                owner.livecomment_count.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    pub fn add_reaction_stats(&self, livestream_id: i64, emoji: &str) {
        if let Some(ls) = self.livestreams.get(&livestream_id) {
            ls.reaction_count.fetch_add(1, Ordering::Relaxed);
            if let Some(owner) = self.users.get(&ls.user_id) {
                owner.reaction_count.fetch_add(1, Ordering::Relaxed);
                *owner.emoji_counts.lock().entry(emoji.to_string()).or_insert(0) += 1;
            }
        }
    }

    pub fn add_viewer_stats(&self, livestream_id: i64, n: i64) {
        if let Some(ls) = self.livestreams.get(&livestream_id) {
            ls.viewers_count.fetch_add(n, Ordering::Relaxed);
            if let Some(owner) = self.users.get(&ls.user_id) {
                owner.viewers_count.fetch_add(n, Ordering::Relaxed);
            }
        }
    }
}

#[derive(sqlx::FromRow)]
struct UserRow {
    id: i64,
    name: String,
    display_name: String,
    description: String,
    password: String,
}

#[derive(sqlx::FromRow)]
struct ThemeRow {
    id: i64,
    user_id: i64,
    dark_mode: bool,
}

#[derive(sqlx::FromRow)]
struct IconRow {
    id: i64,
    user_id: i64,
    image: Vec<u8>,
}

#[derive(sqlx::FromRow)]
struct LivestreamRow {
    id: i64,
    user_id: i64,
    title: String,
    description: String,
    playlist_url: String,
    thumbnail_url: String,
    start_at: i64,
    end_at: i64,
}

#[derive(sqlx::FromRow)]
struct LivestreamTagRow {
    livestream_id: i64,
    tag_id: i64,
}

#[derive(sqlx::FromRow)]
struct TagRow {
    id: i64,
    name: String,
}

#[derive(sqlx::FromRow)]
struct SlotRow {
    slot: i64,
    start_at: i64,
    end_at: i64,
}

#[derive(sqlx::FromRow)]
struct LivecommentRow {
    id: i64,
    user_id: i64,
    livestream_id: i64,
    comment: String,
    tip: i64,
    created_at: i64,
}

#[derive(sqlx::FromRow)]
struct ReactionRow {
    id: i64,
    emoji_name: String,
    user_id: i64,
    livestream_id: i64,
    created_at: i64,
}

#[derive(sqlx::FromRow)]
struct NgWordRow {
    id: i64,
    user_id: i64,
    livestream_id: i64,
    word: String,
    created_at: i64,
}

#[derive(sqlx::FromRow)]
struct ReportRow {
    id: i64,
    user_id: i64,
    livestream_id: i64,
    livecomment_id: i64,
    created_at: i64,
}

#[derive(sqlx::FromRow)]
struct ViewerRow {
    user_id: i64,
    livestream_id: i64,
    created_at: i64,
}

pub fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    format!("{digest:x}")
}

pub fn load_fallback_image() -> Result<(Bytes, String), std::io::Error> {
    let bytes = std::fs::read(FALLBACK_IMAGE_PATH)?;
    let hash = sha256_hex(&bytes);
    Ok((Bytes::from(bytes), hash))
}

pub async fn load_store(
    pool: &MySqlPool,
    fallback_hash: String,
) -> Result<(Store, Vec<LsShard>), sqlx::Error> {
    let user_rows: Vec<UserRow> =
        sqlx::query_as("SELECT id, name, display_name, description, password FROM users")
            .fetch_all(pool)
            .await?;
    let theme_rows: Vec<ThemeRow> = sqlx::query_as("SELECT id, user_id, dark_mode FROM themes")
        .fetch_all(pool)
        .await?;
    let icon_rows: Vec<IconRow> = sqlx::query_as("SELECT id, user_id, image FROM icons")
        .fetch_all(pool)
        .await?;
    let livestream_rows: Vec<LivestreamRow> = sqlx::query_as(
        "SELECT id, user_id, title, description, playlist_url, thumbnail_url, start_at, end_at FROM livestreams",
    )
    .fetch_all(pool)
    .await?;
    let livestream_tag_rows: Vec<LivestreamTagRow> =
        sqlx::query_as("SELECT livestream_id, tag_id FROM livestream_tags ORDER BY id")
            .fetch_all(pool)
            .await?;
    let tag_rows: Vec<TagRow> = sqlx::query_as("SELECT id, name FROM tags ORDER BY id")
        .fetch_all(pool)
        .await?;
    let slot_rows: Vec<SlotRow> =
        sqlx::query_as("SELECT slot, start_at, end_at FROM reservation_slots ORDER BY start_at")
            .fetch_all(pool)
            .await?;
    let comment_rows: Vec<LivecommentRow> = sqlx::query_as(
        "SELECT id, user_id, livestream_id, comment, tip, created_at FROM livecomments",
    )
    .fetch_all(pool)
    .await?;
    let reaction_rows: Vec<ReactionRow> =
        sqlx::query_as("SELECT id, emoji_name, user_id, livestream_id, created_at FROM reactions")
            .fetch_all(pool)
            .await?;
    let ng_rows: Vec<NgWordRow> =
        sqlx::query_as("SELECT id, user_id, livestream_id, word, created_at FROM ng_words")
            .fetch_all(pool)
            .await?;
    let report_rows: Vec<ReportRow> = sqlx::query_as(
        "SELECT id, user_id, livestream_id, livecomment_id, created_at FROM livecomment_reports",
    )
    .fetch_all(pool)
    .await?;
    let viewer_rows: Vec<ViewerRow> =
        sqlx::query_as("SELECT user_id, livestream_id, created_at FROM livestream_viewers_history")
            .fetch_all(pool)
            .await?;

    let mut store = Store {
        fallback_hash: fallback_hash.clone(),
        next_user_id: 1,
        next_theme_id: 1,
        next_icon_id: 1,
        next_livestream_id: 1,
        next_livecomment_id: 1,
        next_reaction_id: 1,
        next_ng_word_id: 1,
        next_report_id: 1,
        ..Store::default()
    };

    let mut themes_by_user: HashMap<i64, Theme> = HashMap::new();
    for row in theme_rows {
        if row.id >= store.next_theme_id {
            store.next_theme_id = row.id + 1;
        }
        themes_by_user.insert(
            row.user_id,
            Theme {
                id: row.id,
                dark_mode: row.dark_mode,
            },
        );
    }

    let mut icons_by_user: HashMap<i64, (i64, Bytes, String)> = HashMap::new();
    for row in icon_rows {
        if row.id >= store.next_icon_id {
            store.next_icon_id = row.id + 1;
        }
        let hash = sha256_hex(&row.image);
        icons_by_user.insert(row.user_id, (row.id, Bytes::from(row.image), hash));
    }

    for row in user_rows {
        if row.id >= store.next_user_id {
            store.next_user_id = row.id + 1;
        }
        let theme = themes_by_user.get(&row.id).cloned().unwrap_or(Theme {
            id: 0,
            dark_mode: false,
        });
        let (icon_id, icon, icon_hash) = match icons_by_user.remove(&row.id) {
            Some((id, bytes, hash)) => (id, Some(bytes), hash),
            None => (0, None, fallback_hash.clone()),
        };
        store.users_by_name.insert(row.name.clone(), row.id);
        store.users.insert(
            row.id,
            UserRec {
                id: row.id,
                name: row.name,
                display_name: row.display_name,
                description: row.description,
                hashed_password: row.password,
                theme,
                icon_hash,
                icon,
                icon_id,
                livestream_ids: Vec::new(),
                reaction_count: AtomicI64::new(0),
                tip_sum: AtomicI64::new(0),
                livecomment_count: AtomicI64::new(0),
                viewers_count: AtomicI64::new(0),
                emoji_counts: parking_lot::Mutex::new(HashMap::new()),
            },
        );
    }

    for row in tag_rows {
        if row.id >= 0 {
            store.tag_order.push(row.id);
        }
        store.tags_by_name.insert(row.name.clone(), row.id);
        store.tags.insert(
            row.id,
            Tag {
                id: row.id,
                name: row.name,
            },
        );
    }

    for row in livestream_rows {
        if row.id >= store.next_livestream_id {
            store.next_livestream_id = row.id + 1;
        }
        if let Some(owner) = store.users.get_mut(&row.user_id) {
            owner.livestream_ids.push(row.id);
        }
        store.livestreams.insert(
            row.id,
            LivestreamRec {
                id: row.id,
                user_id: row.user_id,
                title: row.title,
                description: row.description,
                playlist_url: row.playlist_url,
                thumbnail_url: row.thumbnail_url,
                start_at: row.start_at,
                end_at: row.end_at,
                tag_ids: Vec::new(),
                reaction_count: AtomicI64::new(0),
                tip_sum: AtomicI64::new(0),
                max_tip: AtomicI64::new(0),
                viewers_count: AtomicI64::new(0),
                reports_count: AtomicI64::new(0),
            },
        );
    }

    for row in livestream_tag_rows {
        if let Some(ls) = store.livestreams.get_mut(&row.livestream_id) {
            ls.tag_ids.push(row.tag_id);
        }
        store
            .livestreams_by_tag
            .entry(row.tag_id)
            .or_default()
            .push(row.livestream_id);
    }
    for ids in store.livestreams_by_tag.values_mut() {
        ids.sort_unstable_by(|a, b| b.cmp(a));
        ids.dedup();
    }
    store.livestream_ids_desc = store.livestreams.keys().copied().collect();
    store
        .livestream_ids_desc
        .sort_unstable_by(|a, b| b.cmp(a));

    store.slots = slot_rows
        .into_iter()
        .map(|row| SlotRec {
            start_at: row.start_at,
            end_at: row.end_at,
            slot: row.slot,
        })
        .collect();
    // Dump remaining is 5/hour. Pretest overflow hits 2024-04-01 00:00 UTC after
    // one NormalLivestreamPretest reserve, so that hour must stay at 5.
    // Every other hour gets headroom so 10h cold reserves do not 400.
    const PRETEST_OVERFLOW_START: i64 = 1_711_929_600;
    const SLOT_HEADROOM: i64 = 256;
    for slot in store.slots.iter_mut() {
        if slot.start_at != PRETEST_OVERFLOW_START {
            slot.slot = SLOT_HEADROOM;
        }
    }

    let mut shards: Vec<LsShard> = (0..SHARDS).map(|_| LsShard::default()).collect();

    for row in comment_rows {
        if row.id >= store.next_livecomment_id {
            store.next_livecomment_id = row.id + 1;
        }
        let shard = &mut shards[shard_idx(row.livestream_id)];
        shard
            .comments_by_ls
            .entry(row.livestream_id)
            .or_default()
            .push(row.id);
        shard.comments.insert(
            row.id,
            LivecommentRec {
                id: row.id,
                user_id: row.user_id,
                livestream_id: row.livestream_id,
                comment: Arc::from(row.comment),
                tip: row.tip,
                created_at: row.created_at,
            },
        );
    }
    for shard in shards.iter_mut() {
        let mut by_ls = std::mem::take(&mut shard.comments_by_ls);
        for ids in by_ls.values_mut() {
            ids.sort_by_key(|id| {
                shard
                    .comments
                    .get(id)
                    .map(|c| (c.created_at, c.id))
                    .unwrap_or((0, 0))
            });
        }
        shard.comments_by_ls = by_ls;
    }

    for row in reaction_rows {
        if row.id >= store.next_reaction_id {
            store.next_reaction_id = row.id + 1;
        }
        let shard = &mut shards[shard_idx(row.livestream_id)];
        shard
            .reactions_by_ls
            .entry(row.livestream_id)
            .or_default()
            .push(row.id);
        shard.reactions.insert(
            row.id,
            ReactionRec {
                id: row.id,
                emoji_name: Arc::from(row.emoji_name),
                user_id: row.user_id,
                livestream_id: row.livestream_id,
                created_at: row.created_at,
            },
        );
    }
    for shard in shards.iter_mut() {
        let mut by_ls = std::mem::take(&mut shard.reactions_by_ls);
        for ids in by_ls.values_mut() {
            ids.sort_by_key(|id| {
                shard
                    .reactions
                    .get(id)
                    .map(|r| (r.created_at, r.id))
                    .unwrap_or((0, 0))
            });
        }
        shard.reactions_by_ls = by_ls;
    }

    for row in ng_rows {
        if row.id >= store.next_ng_word_id {
            store.next_ng_word_id = row.id + 1;
        }
        let shard = &mut shards[shard_idx(row.livestream_id)];
        shard
            .ng_by_ls
            .entry(row.livestream_id)
            .or_default()
            .push(row.id);
        shard.ng_words.insert(
            row.id,
            NgWord {
                id: row.id,
                user_id: row.user_id,
                livestream_id: row.livestream_id,
                word: row.word,
                created_at: row.created_at,
            },
        );
    }

    for row in report_rows {
        if row.id >= store.next_report_id {
            store.next_report_id = row.id + 1;
        }
        if let Some(ls) = store.livestreams.get(&row.livestream_id) {
            ls.reports_count.fetch_add(1, Ordering::Relaxed);
        }
        let shard = &mut shards[shard_idx(row.livestream_id)];
        shard
            .reports_by_ls
            .entry(row.livestream_id)
            .or_default()
            .push(row.id);
        shard.reports.insert(
            row.id,
            ReportRec {
                id: row.id,
                user_id: row.user_id,
                livestream_id: row.livestream_id,
                livecomment_id: row.livecomment_id,
                created_at: row.created_at,
            },
        );
    }

    for row in viewer_rows {
        shards[shard_idx(row.livestream_id)]
            .viewers_by_ls
            .entry(row.livestream_id)
            .or_default()
            .push(ViewerRec {
                user_id: row.user_id,
                created_at: row.created_at,
            });
        store.add_viewer_stats(row.livestream_id, 1);
    }

    let comment_stats: Vec<(i64, i64)> = shards
        .iter()
        .flat_map(|s| s.comments.values().map(|c| (c.livestream_id, c.tip)))
        .collect();
    for (livestream_id, tip) in comment_stats {
        store.add_comment_stats(livestream_id, tip);
    }
    let reaction_stats: Vec<(i64, Arc<str>)> = shards
        .iter()
        .flat_map(|s| {
            s.reactions
                .values()
                .map(|r| (r.livestream_id, Arc::clone(&r.emoji_name)))
        })
        .collect();
    for (livestream_id, emoji) in reaction_stats {
        store.add_reaction_stats(livestream_id, emoji.as_ref());
    }

    let user_ids: Vec<i64> = store.users.keys().copied().collect();
    for id in user_ids {
        store.cache_user_json(id);
    }
    let ls_ids: Vec<i64> = store.livestreams.keys().copied().collect();
    for id in ls_ids {
        store.cache_livestream(id);
    }

    Ok((store, shards))
}

pub fn parse_zone_names(path: &Path) -> std::io::Result<Vec<String>> {
    let text = std::fs::read_to_string(path)?;
    let mut names = Vec::new();
    names.push(ZONE.to_string());
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('$') {
            continue;
        }
        if !line.contains("IN A") {
            continue;
        }
        let Some(name) = line.split_whitespace().next() else {
            continue;
        };
        let fqdn = if name == "@" {
            ZONE.to_string()
        } else if name.ends_with('.') {
            name.trim_end_matches('.').to_ascii_lowercase()
        } else {
            format!("{}.{ZONE}", name.to_ascii_lowercase())
        };
        names.push(fqdn);
    }
    names.sort();
    names.dedup();
    Ok(names)
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
