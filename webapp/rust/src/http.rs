//! HTTP/1.1 state machine + isupipe route match. No framework.

use std::io::Write as _;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, UnixListener};

use crate::{handlers, AppState, Error};

const MAX_HEADER: usize = 16 * 1024;
const MAX_BODY: usize = 10 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Delete,
}

#[derive(Debug)]
pub enum Route {
    PostInitialize,
    GetTag,
    GetUserTheme { username: String },
    PostReservation,
    GetSearch,
    GetMyLivestreams,
    GetUserLivestreams { username: String },
    GetLivestream { id: i64 },
    GetLivecomments { id: i64 },
    PostLivecomment { id: i64 },
    GetReactions { id: i64 },
    PostReaction { id: i64 },
    GetReports { id: i64 },
    GetNgwords { id: i64 },
    PostReport { livestream_id: i64, comment_id: i64 },
    PostModerate { id: i64 },
    PostEnter { id: i64 },
    DeleteExit { id: i64 },
    PostRegister,
    PostLogin,
    GetMe,
    GetUser { username: String },
    GetUserStats { username: String },
    GetIcon { username: String },
    PostIcon,
    GetLivestreamStats { id: i64 },
    GetPayment,
}

pub struct HttpResp {
    pub status: u16,
    pub body: Vec<u8>,
    pub set_cookie: Option<String>,
    pub content_type: Option<String>,
    pub etag: Option<String>,
}

impl HttpResp {
    pub fn json(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            body,
            set_cookie: None,
            content_type: Some("application/json".into()),
            etag: None,
        }
    }
    pub fn empty(status: u16) -> Self {
        Self {
            status,
            body: Vec::new(),
            set_cookie: None,
            content_type: None,
            etag: None,
        }
    }
    pub fn bytes(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Self {
            status,
            body,
            set_cookie: None,
            content_type: Some(content_type.into()),
            etag: None,
        }
    }
    pub fn cookie(mut self, value: String) -> Self {
        self.set_cookie = Some(value);
        self
    }
    pub fn with_etag(mut self, hash: String) -> Self {
        self.etag = Some(hash);
        self
    }
}

impl From<Error> for HttpResp {
    fn from(err: Error) -> Self {
        let status = match &err {
            Error::BadRequest(_) => 400,
            Error::Unauthorized(_) => 401,
            Error::Forbidden(_) => 403,
            Error::NotFound(_) => 404,
            Error::Internal(_) => 500,
        };
        let message = err.to_string();
        if status >= 500 {
            tracing::error!("{message}");
        }
        let body = serde_json::json!({ "error": message }).to_string().into_bytes();
        HttpResp::json(status, body)
    }
}

pub struct Parsed {
    pub route: Route,
    pub query: Vec<(String, String)>,
    pub keepalive: bool,
    pub session: Option<String>,
    pub if_none_match: Option<String>,
    pub body: Vec<u8>,
}

pub fn query_str<'a>(q: &'a [(String, String)], key: &str) -> &'a str {
    q.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .unwrap_or("")
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_i64(s: &[u8]) -> Option<i64> {
    std::str::from_utf8(s).ok()?.parse().ok()
}

fn match_route(method: Method, path: &[u8]) -> Option<Route> {
    let path = path.strip_suffix(b"/").unwrap_or(path);
    match (method, path) {
        (Method::Post, b"/api/initialize") => return Some(Route::PostInitialize),
        (Method::Get, b"/api/tag") => return Some(Route::GetTag),
        (Method::Post, b"/api/livestream/reservation") => return Some(Route::PostReservation),
        (Method::Get, b"/api/livestream/search") => return Some(Route::GetSearch),
        (Method::Get, b"/api/livestream") => return Some(Route::GetMyLivestreams),
        (Method::Post, b"/api/register") => return Some(Route::PostRegister),
        (Method::Post, b"/api/login") => return Some(Route::PostLogin),
        (Method::Get, b"/api/user/me") => return Some(Route::GetMe),
        (Method::Post, b"/api/icon") => return Some(Route::PostIcon),
        (Method::Get, b"/api/payment") => return Some(Route::GetPayment),
        _ => {}
    }
    let segs: Vec<&[u8]> = path
        .split(|c| *c == b'/')
        .filter(|s| !s.is_empty())
        .collect();
    match (method, segs.as_slice()) {
        (Method::Get, [b"api", b"user", name, b"theme"]) => Some(Route::GetUserTheme {
            username: String::from_utf8_lossy(name).into_owned(),
        }),
        (Method::Get, [b"api", b"user", name, b"livestream"]) => Some(Route::GetUserLivestreams {
            username: String::from_utf8_lossy(name).into_owned(),
        }),
        (Method::Get, [b"api", b"user", name, b"statistics"]) => Some(Route::GetUserStats {
            username: String::from_utf8_lossy(name).into_owned(),
        }),
        (Method::Get, [b"api", b"user", name, b"icon"]) => Some(Route::GetIcon {
            username: String::from_utf8_lossy(name).into_owned(),
        }),
        (Method::Get, [b"api", b"user", name]) => Some(Route::GetUser {
            username: String::from_utf8_lossy(name).into_owned(),
        }),
        (Method::Get, [b"api", b"livestream", id, b"livecomment"]) => Some(Route::GetLivecomments {
            id: parse_i64(id)?,
        }),
        (Method::Post, [b"api", b"livestream", id, b"livecomment"]) => Some(Route::PostLivecomment {
            id: parse_i64(id)?,
        }),
        (Method::Get, [b"api", b"livestream", id, b"reaction"]) => Some(Route::GetReactions {
            id: parse_i64(id)?,
        }),
        (Method::Post, [b"api", b"livestream", id, b"reaction"]) => Some(Route::PostReaction {
            id: parse_i64(id)?,
        }),
        (Method::Get, [b"api", b"livestream", id, b"report"]) => Some(Route::GetReports {
            id: parse_i64(id)?,
        }),
        (Method::Get, [b"api", b"livestream", id, b"ngwords"]) => Some(Route::GetNgwords {
            id: parse_i64(id)?,
        }),
        (Method::Post, [b"api", b"livestream", id, b"livecomment", cid, b"report"]) => {
            Some(Route::PostReport {
                livestream_id: parse_i64(id)?,
                comment_id: parse_i64(cid)?,
            })
        }
        (Method::Post, [b"api", b"livestream", id, b"moderate"]) => Some(Route::PostModerate {
            id: parse_i64(id)?,
        }),
        (Method::Post, [b"api", b"livestream", id, b"enter"]) => Some(Route::PostEnter {
            id: parse_i64(id)?,
        }),
        (Method::Delete, [b"api", b"livestream", id, b"exit"]) => Some(Route::DeleteExit {
            id: parse_i64(id)?,
        }),
        (Method::Get, [b"api", b"livestream", id, b"statistics"]) => Some(Route::GetLivestreamStats {
            id: parse_i64(id)?,
        }),
        (Method::Get, [b"api", b"livestream", id]) => Some(Route::GetLivestream {
            id: parse_i64(id)?,
        }),
        _ => None,
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""),
                16,
            ) {
                out.push(v);
                i += 3;
                continue;
            }
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_query(q: &[u8]) -> Vec<(String, String)> {
    if q.is_empty() {
        return Vec::new();
    }
    String::from_utf8_lossy(q)
        .split('&')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            Some((percent_decode(k), percent_decode(v)))
        })
        .collect()
}

fn cookie_value(header: &str, name: &str) -> Option<String> {
    for part in header.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix(name) {
            if let Some(val) = rest.strip_prefix('=') {
                return Some(val.to_string());
            }
        }
    }
    None
}

fn parse_headers(
    raw: &[u8],
) -> Result<(Route, Vec<(String, String)>, bool, Option<String>, Option<String>, usize), Error> {
    let text = std::str::from_utf8(raw).map_err(|_| Error::bad("invalid header utf8"))?;
    let mut lines = text.split("\r\n");
    let req = lines.next().ok_or_else(|| Error::bad("empty request"))?;
    let mut parts = req.splitn(3, ' ');
    let method = match parts.next() {
        Some("GET") => Method::Get,
        Some("POST") => Method::Post,
        Some("DELETE") => Method::Delete,
        _ => return Err(Error::bad("method")),
    };
    let target = parts.next().ok_or_else(|| Error::bad("path"))?;
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.as_bytes(), parse_query(q.as_bytes())),
        None => (target.as_bytes(), Vec::new()),
    };
    let route = match_route(method, path).ok_or_else(|| Error::not_found("not found"))?;

    let mut content_length = 0usize;
    let mut keepalive = true;
    let mut cookie = String::new();
    let mut if_none_match = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.parse().unwrap_or(0);
        } else if name.eq_ignore_ascii_case("connection") {
            keepalive = !value.eq_ignore_ascii_case("close");
        } else if name.eq_ignore_ascii_case("cookie") {
            if !cookie.is_empty() {
                cookie.push(';');
            }
            cookie.push_str(value);
        } else if name.eq_ignore_ascii_case("if-none-match") {
            if_none_match = Some(value.to_string());
        }
    }
    let session = cookie_value(&cookie, crate::SESSION_COOKIE);
    Ok((
        route,
        query,
        keepalive,
        session,
        if_none_match,
        content_length,
    ))
}

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

async fn write_resp<S: AsyncWrite + Unpin>(
    stream: &mut S,
    resp: HttpResp,
    keepalive: bool,
) -> std::io::Result<()> {
    let mut head = Vec::with_capacity(256);
    let _ = write!(
        &mut head,
        "HTTP/1.1 {} {}\r\n",
        resp.status,
        status_text(resp.status)
    );
    if let Some(ct) = resp.content_type {
        let _ = write!(&mut head, "Content-Type: {ct}\r\n");
    }
    if let Some(etag) = resp.etag {
        let _ = write!(&mut head, "ETag: \"{etag}\"\r\n");
    }
    let _ = write!(&mut head, "Content-Length: {}\r\n", resp.body.len());
    if let Some(cookie) = resp.set_cookie {
        let _ = write!(
            &mut head,
            "Set-Cookie: {cookie}; Path=/; Domain=u.isucon.dev; Max-Age=60000\r\n"
        );
    }
    if keepalive {
        head.extend_from_slice(b"Connection: keep-alive\r\n");
    } else {
        head.extend_from_slice(b"Connection: close\r\n");
    }
    head.extend_from_slice(b"\r\n");
    stream.write_all(&head).await?;
    if resp.status != 204 && resp.status != 304 && !resp.body.is_empty() {
        stream.write_all(&resp.body).await?;
    }
    stream.flush().await?;
    Ok(())
}

async fn dispatch(state: AppState, req: Parsed) -> HttpResp {
    let out = match req.route {
        Route::PostInitialize => handlers::initialize(&state).await,
        Route::GetTag => handlers::get_tag(&state),
        Route::GetUserTheme { username } => handlers::get_theme(&state, req.session.as_deref(), &username),
        Route::PostReservation => {
            handlers::reserve(&state, req.session.as_deref(), &req.body)
        }
        Route::GetSearch => handlers::search(&state, &req.query),
        Route::GetMyLivestreams => handlers::my_livestreams(&state, req.session.as_deref()),
        Route::GetUserLivestreams { username } => {
            handlers::user_livestreams(&state, req.session.as_deref(), &username)
        }
        Route::GetLivestream { id } => handlers::get_livestream(&state, req.session.as_deref(), id),
        Route::GetLivecomments { id } => {
            handlers::get_livecomments(&state, req.session.as_deref(), id, &req.query)
        }
        Route::PostLivecomment { id } => {
            handlers::post_livecomment(&state, req.session.as_deref(), id, &req.body)
        }
        Route::GetReactions { id } => {
            handlers::get_reactions(&state, req.session.as_deref(), id, &req.query)
        }
        Route::PostReaction { id } => {
            handlers::post_reaction(&state, req.session.as_deref(), id, &req.body)
        }
        Route::GetReports { id } => handlers::get_reports(&state, req.session.as_deref(), id),
        Route::GetNgwords { id } => handlers::get_ngwords(&state, req.session.as_deref(), id),
        Route::PostReport {
            livestream_id,
            comment_id,
        } => handlers::post_report(&state, req.session.as_deref(), livestream_id, comment_id),
        Route::PostModerate { id } => {
            handlers::moderate(&state, req.session.as_deref(), id, &req.body)
        }
        Route::PostEnter { id } => handlers::enter(&state, req.session.as_deref(), id),
        Route::DeleteExit { id } => handlers::exit(&state, req.session.as_deref(), id),
        Route::PostRegister => handlers::register(&state, &req.body).await,
        Route::PostLogin => handlers::login(&state, &req.body).await,
        Route::GetMe => handlers::me(&state, req.session.as_deref()),
        Route::GetUser { username } => handlers::get_user(&state, req.session.as_deref(), &username),
        Route::GetUserStats { username } => {
            handlers::user_stats(&state, req.session.as_deref(), &username)
        }
        Route::GetIcon { username } => {
            handlers::get_icon(&state, &username, req.if_none_match.as_deref())
        }
        Route::PostIcon => handlers::post_icon(&state, req.session.as_deref(), &req.body).await,
        Route::GetLivestreamStats { id } => {
            handlers::livestream_stats(&state, req.session.as_deref(), id)
        }
        Route::GetPayment => handlers::payment(&state),
    };
    match out {
        Ok(r) => r,
        Err(e) => HttpResp::from(e),
    }
}

async fn handle_conn<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S, state: AppState) {
    let mut buf = vec![0u8; 8192];
    let mut filled = 0usize;
    loop {
        let header_end = loop {
            if let Some(pos) = find_header_end(&buf[..filled]) {
                break pos;
            }
            if filled >= MAX_HEADER {
                return;
            }
            if filled == buf.len() {
                buf.resize((buf.len() * 2).min(MAX_HEADER + MAX_BODY), 0);
            }
            match tokio::time::timeout(Duration::from_secs(65), stream.read(&mut buf[filled..])).await
            {
                Ok(Ok(0)) | Err(_) => return,
                Ok(Ok(n)) => filled += n,
                Ok(Err(_)) => return,
            }
        };
        let parsed_headers = match parse_headers(&buf[..header_end]) {
            Ok(h) => h,
            Err(e) => {
                let _ = write_resp(&mut stream, HttpResp::from(e), false).await;
                return;
            }
        };
        let (route, query, keepalive, session, if_none_match, content_length) = parsed_headers;
        if content_length > MAX_BODY {
            return;
        }
        let body_start = header_end + 4;
        let need = body_start + content_length;
        while filled < need {
            if filled == buf.len() {
                buf.resize((buf.len() * 2).max(need).min(MAX_HEADER + MAX_BODY), 0);
            }
            match stream.read(&mut buf[filled..]).await {
                Ok(0) => return,
                Ok(n) => filled += n,
                Err(_) => return,
            }
        }
        let body = buf[body_start..need].to_vec();
        let leftover = filled - need;
        if leftover > 0 {
            buf.copy_within(need..filled, 0);
        }
        filled = leftover;

        let req = Parsed {
            route,
            query,
            keepalive,
            session,
            if_none_match,
            body,
        };
        let resp = dispatch(state.clone(), req).await;
        if write_resp(&mut stream, resp, keepalive).await.is_err() {
            return;
        }
        if !keepalive {
            return;
        }
    }
}

pub async fn serve(listener: TcpListener, state: AppState) -> std::io::Result<()> {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let _ = stream.set_nodelay(true);
                let state = state.clone();
                tokio::spawn(async move {
                    handle_conn(stream, state).await;
                });
            }
            Err(err) => {
                tracing::error!("accept: {err}");
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }
    }
}

pub async fn serve_unix(listener: UnixListener, state: AppState) -> std::io::Result<()> {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let state = state.clone();
                tokio::spawn(async move {
                    handle_conn(stream, state).await;
                });
            }
            Err(err) => {
                tracing::error!("unix accept: {err}");
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }
    }
}
