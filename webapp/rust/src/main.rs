use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::AtomicI64;
use std::sync::Arc;

use bytes::Bytes;
use isupipe::store::{Store, FALLBACK_IMAGE_PATH};
use isupipe::{dns, handlers, http, mysql_options, rebuild_dns_names, AppState, Inner};
use parking_lot::RwLock;
use sqlx::mysql::MySqlPoolOptions;
use tokio::net::{TcpListener, UnixListener};

fn parse_ipv4(s: &str) -> [u8; 4] {
    s.parse::<Ipv4Addr>()
        .expect("POWERDNS address must be IPv4")
        .octets()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "info");
    }
    tracing_subscriber::fmt::init();

    let pool = MySqlPoolOptions::new()
        .max_connections(16)
        .connect_with(mysql_options())
        .await
        .expect("failed to connect db");

    let powerdns_addr = std::env::var("ISUCON13_POWERDNS_SUBDOMAIN_ADDRESS")
        .expect("environ ISUCON13_POWERDNS_SUBDOMAIN_ADDRESS must be provided");
    let ipv4 = parse_ipv4(&powerdns_addr);

    let (fallback_image, fallback_hash) = isupipe::store::load_fallback_image().unwrap_or_else(|e| {
        tracing::warn!("failed to read {FALLBACK_IMAGE_PATH}: {e}");
        (Bytes::new(), String::new())
    });

    let dns_names = Arc::new(RwLock::new(HashSet::new()));
    let inner = Arc::new(Inner {
        store: RwLock::new(Store {
            fallback_hash: fallback_hash.clone(),
            ..Store::default()
        }),
        shards: Inner::empty_shards(),
        dns_names: dns_names.clone(),
        pool,
        fallback_image,
        fallback_hash,
        init_lock: tokio::sync::Mutex::new(()),
        sessions: RwLock::new(Default::default()),
        next_livecomment_id: AtomicI64::new(1),
        next_reaction_id: AtomicI64::new(1),
        next_ng_word_id: AtomicI64::new(1),
        next_report_id: AtomicI64::new(1),
    });

    handlers::reload_on_boot(&inner).await;
    *dns_names.write() = rebuild_dns_names(&inner.store.read());

    dns::start(dns_names, ipv4);

    let state = AppState { inner };
    let sock_path = std::env::var("ISUCON13_HTTP_SOCK")
        .unwrap_or_else(|_| "/home/isucon/webapp/rust/isupipe.sock".to_string());
    let _ = std::fs::remove_file(&sock_path);
    let unix = UnixListener::bind(&sock_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o777));
    }
    let unix_state = state.clone();
    tokio::spawn(async move {
        if let Err(err) = http::serve_unix(unix, unix_state).await {
            tracing::error!("unix socket: {err}");
        }
    });
    tracing::info!("unix socket {sock_path}");

    let listener = TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], 8080))).await?;
    tracing::info!("listening on 0.0.0.0:8080 (custom HTTP/1.1)");
    http::serve(listener, state).await?;
    Ok(())
}
