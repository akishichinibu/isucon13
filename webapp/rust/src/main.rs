use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use bytes::Bytes;
use isupipe::store::{Store, FALLBACK_IMAGE_PATH};
use isupipe::{dns, handlers, http, mysql_options, rebuild_dns_names, AppState, Inner};
use parking_lot::RwLock;
use sqlx::mysql::MySqlPoolOptions;
use tokio::net::TcpListener;

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
        dns_names: dns_names.clone(),
        pool,
        fallback_image,
        fallback_hash,
        init_lock: tokio::sync::Mutex::new(()),
        sessions: RwLock::new(Default::default()),
    });

    handlers::reload_on_boot(&inner).await;
    *dns_names.write() = rebuild_dns_names(&inner.store.read());

    dns::start(dns_names, ipv4);

    let state = AppState { inner };
    let listener = TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], 8080))).await?;
    tracing::info!("listening on 0.0.0.0:8080 (custom HTTP/1.1)");
    http::serve(listener, state).await?;
    Ok(())
}
