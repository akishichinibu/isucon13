//! CPU pprof on :6060. Not exposed through nginx.
//!
//!   curl -sS 'http://127.0.0.1:6060/debug/pprof/profile?seconds=30' -o cpu.svg

use std::io::Write as _;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub async fn serve() {
    let listener = match TcpListener::bind("0.0.0.0:6060").await {
        Ok(l) => l,
        Err(err) => {
            tracing::error!("pprof bind 6060: {err}");
            return;
        }
    };
    tracing::info!("pprof on 0.0.0.0:6060");
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        tokio::spawn(handle(stream));
    }
}

async fn handle(mut stream: tokio::net::TcpStream) {
    let mut buf = vec![0u8; 2048];
    let n = stream.read(&mut buf).await.unwrap_or(0);
    let req = String::from_utf8_lossy(&buf[..n]);
    let line = req.lines().next().unwrap_or("");
    if !line.starts_with("GET /debug/pprof/") {
        let _ = write_http(&mut stream, 404, "text/plain", b"not found").await;
        return;
    }
    let seconds = parse_seconds(line).clamp(1, 60);
    let result = tokio::task::spawn_blocking(move || sample_flamegraph(seconds)).await;
    match result {
        Ok(Ok(body)) => {
            let _ = write_http(&mut stream, 200, "image/svg+xml", &body).await;
        }
        Ok(Err(err)) => {
            let msg = err.into_bytes();
            let _ = write_http(&mut stream, 500, "text/plain", &msg).await;
        }
        Err(err) => {
            let msg = format!("join: {err}").into_bytes();
            let _ = write_http(&mut stream, 500, "text/plain", &msg).await;
        }
    }
}

fn parse_seconds(line: &str) -> u64 {
    line.split(['?', '&', ' '])
        .find_map(|part| part.strip_prefix("seconds="))
        .and_then(|s| s.trim_end_matches(|c: char| !c.is_ascii_digit()).parse().ok())
        .unwrap_or(30)
}

fn sample_flamegraph(seconds: u64) -> Result<Vec<u8>, String> {
    let guard = pprof::ProfilerGuardBuilder::default()
        .frequency(99)
        .blocklist(&["libc", "libgcc", "pthread", "vdso"])
        .build()
        .map_err(|e| format!("profiler: {e}"))?;
    std::thread::sleep(Duration::from_secs(seconds));
    let report = guard.report().build().map_err(|e| format!("report: {e}"))?;
    let mut body = Vec::new();
    report
        .flamegraph(&mut body)
        .map_err(|e| format!("flamegraph: {e}"))?;
    Ok(body)
}

async fn write_http(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let mut head = Vec::with_capacity(128);
    let _ = write!(
        &mut head,
        "HTTP/1.1 {status} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(&head).await?;
    stream.write_all(body).await?;
    Ok(())
}