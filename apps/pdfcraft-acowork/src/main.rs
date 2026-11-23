//! ACowork-hosted PdfCraft process (ADR-116 generic runtime, ADR-118 dual canvas).
//!
//! Speaks the Gateway generic-supervisor CLI contract (`--host` / `--port` /
//! `--port-file` / `--gateway-health-url` / `--log-level`) and serves:
//! - `GET /health` — supervisor liveness
//! - `GET /ui/*` — static web UI already packed beside the binary, with an
//!   injected ACowork theme/openFile bridge (upstream `index.html` is not edited)
//! - `POST /mcp` — `pdfcraft-mcp` confined to `X-MCP-Workspace-Root`
//!
//! The UI directory is resolved as: `--ui-dir` → sibling `../ui` of this
//! binary (package layout) → `./ui`.

mod fonts;
mod mcp;
mod ui;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use axum::Router;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use clap::Parser;
use tower_http::services::{ServeDir, ServeFile};

use crate::mcp::McpHub;

#[derive(Parser, Debug)]
#[command(
    name = "pdfcraft-acowork",
    about = "ACowork hosted PdfCraft process (loopback HTTP, ADR-116). Not an ArtCraft product."
)]
struct Cli {
    /// Loopback bind address (Gateway always passes 127.0.0.1)
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Preferred listen port; on conflict, try up to +20
    #[arg(long, default_value = "18094")]
    port: u16,

    /// Write the bound port here for the Gateway supervisor handshake
    #[arg(long)]
    port_file: Option<PathBuf>,

    /// Gateway /health URL — self-exit after sustained failure (ADR-018)
    #[arg(long)]
    gateway_health_url: Option<String>,

    #[arg(long, default_value = "10000")]
    gateway_health_interval_ms: u64,

    #[arg(long, default_value = "300000")]
    gateway_health_timeout_ms: u64,

    /// Override the directory that backs `/ui/`
    #[arg(long)]
    ui_dir: Option<PathBuf>,

    #[arg(long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    init_logging(&cli.log_level);

    let ui_dir = resolve_ui_dir(cli.ui_dir.as_deref());
    ensure_placeholder_ui(&ui_dir);
    log::info!("serving PdfCraft UI from {}", ui_dir.display());

    if let Some(url) = cli.gateway_health_url.clone() {
        spawn_gateway_health_watchdog(
            url,
            Duration::from_millis(cli.gateway_health_interval_ms),
            Duration::from_millis(cli.gateway_health_timeout_ms),
        );
    }

    match McpHub::spawn() {
        Ok(h) => h.install(),
        Err(e) => {
            log::error!("{e}");
            std::process::exit(1);
        }
    };
    let app = build_router(&ui_dir);

    let listener = match bind_with_fallback(&cli.host, cli.port).await {
        Ok(l) => l,
        Err(e) => {
            log::error!("bind failed: {e}");
            std::process::exit(1);
        }
    };
    let addr = match listener.local_addr() {
        Ok(a) => a,
        Err(e) => {
            log::error!("local_addr failed: {e}");
            std::process::exit(1);
        }
    };
    log::info!("pdfcraft-acowork listening on {addr}");

    if let Some(path) = &cli.port_file {
        if let Err(e) = std::fs::write(path, addr.port().to_string()) {
            log::error!("failed to write port file {}: {e}", path.display());
            std::process::exit(1);
        }
        log::info!("port file written to {} ({})", path.display(), addr.port());
    }

    if let Err(e) = axum::serve(listener, app).await {
        log::error!("server exited: {e}");
        std::process::exit(1);
    }
}

fn init_logging(level: &str) {
    let mut builder = env_logger::Builder::new();
    builder.parse_filters(level);
    builder.format_timestamp_secs();
    let _ = builder.try_init();
}

fn resolve_ui_dir(explicit: Option<&Path>) -> PathBuf {
    if let Some(p) = explicit {
        return p.to_path_buf();
    }
    if let Ok(exe) = std::env::current_exe() {
        let sibling = ui_dir_beside_binary(&exe);
        if let Ok(canon) = sibling.canonicalize()
            && canon.is_dir()
        {
            return canon;
        }
        if sibling.is_dir() {
            return sibling;
        }
    }
    PathBuf::from("ui")
}

/// Package layout is `{version}/bin/<binary>` next to `{version}/ui/`.
fn ui_dir_beside_binary(exe: &Path) -> PathBuf {
    match exe.parent() {
        Some(bin_dir) => bin_dir.join("..").join("ui"),
        None => PathBuf::from("ui"),
    }
}

fn build_router(ui_dir: &Path) -> Router {
    let index = ui_dir.join("index.html");
    let ui_service = ServeDir::new(ui_dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(index));
    Router::new()
        .route("/health", get(health))
        .route("/mcp", post(mcp::mcp_handler))
        // The document's own typefaces, read from this machine: the app loads what
        // the file names instead of the host shipping every system font.
        .route("/fonts", get(fonts::list))
        .route("/fonts/{family}", get(fonts::serve))
        .nest_service("/ui", ui_service)
        .layer(axum::middleware::from_fn(ui::ui_response_layer))
}

fn ensure_placeholder_ui(ui_dir: &Path) {
    let index = ui_dir.join("index.html");
    if index.is_file() {
        return;
    }
    if let Err(e) = std::fs::create_dir_all(ui_dir) {
        log::warn!("could not create ui dir {}: {e}", ui_dir.display());
        return;
    }
    if let Err(e) = std::fs::write(&index, PLACEHOLDER_HTML) {
        log::warn!("could not write placeholder {}: {e}", index.display());
    }
}

async fn bind_with_fallback(host: &str, start: u16) -> Result<tokio::net::TcpListener, String> {
    let ip: std::net::IpAddr = host
        .parse()
        .map_err(|e| format!("invalid --host {host:?}: {e}"))?;
    let max = start.saturating_add(20);
    let mut port = start;
    loop {
        let addr = SocketAddr::new(ip, port);
        match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => return Ok(l),
            Err(e) if port < max => {
                log::warn!("port {port} occupied ({e}) — trying next");
                port += 1;
            }
            Err(e) => return Err(format!("could not bind {start}..={max}: {e}")),
        }
    }
}

async fn health() -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "status": "ok",
        "app": "pdfcraft",
        "service": "pdfcraft-acowork",
    }))
}

pub(crate) fn apply_asset_headers(path: &str, response: &mut Response) {
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    if let Some(kind) = asset_content_type(path) {
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    }
    // HTML must revalidate on every load: a heuristic-cached index (or the SPA fallback's
    // HTML served under a missing asset's URL) otherwise survives a reinstall and keeps
    // feeding superseded hashes / HTML to the wasm loader.
    if headers
        .get(header::CONTENT_TYPE)
        .is_some_and(|v| v.to_str().unwrap_or("").starts_with("text/html"))
    {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
}

fn asset_content_type(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?;
    if ext.eq_ignore_ascii_case("wasm") {
        Some("application/wasm")
    } else if ext.eq_ignore_ascii_case("js") || ext.eq_ignore_ascii_case("mjs") {
        Some("text/javascript")
    } else {
        None
    }
}

const PLACEHOLDER_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>PdfCraft — ACowork</title>
  <style>
    body { margin: 0; min-height: 100vh; display: grid; place-items: center;
      font: 15px/1.5 system-ui, sans-serif; background: #0d0e10; color: #e8eaed; }
    main { max-width: 36rem; padding: 2rem; }
    h1 { font-size: 1.4rem; font-weight: 600; margin: 0 0 .5rem; }
    p { color: #9aa3b2; margin: 0 0 .75rem; }
    code { color: #f0a92e; }
  </style>
</head>
<body>
  <main>
    <h1>PdfCraft (ACowork)</h1>
    <p>ACowork hosted adaptation of PdfCraft by the ArtCraft team. Not an ArtCraft product.</p>
    <p>Placeholder UI. Replace this directory with a <code>trunk build --release</code>
       output from <code>apps/pdfcraft-web</code> before publishing a Release.</p>
  </main>
</body>
</html>
"#;

fn spawn_gateway_health_watchdog(health_url: String, interval: Duration, timeout: Duration) {
    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .no_proxy()
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                log::error!("gateway health client: {e}");
                return;
            }
        };
        let mut last_success = Instant::now();
        loop {
            tokio::time::sleep(interval).await;
            let healthy = match client.get(&health_url).send().await {
                Ok(resp) => resp.status().is_success(),
                Err(_) => false,
            };
            if healthy {
                last_success = Instant::now();
            } else if last_success.elapsed() >= timeout {
                log::error!(
                    "Gateway unreachable for {}s (limit {}s) — self-exiting (ADR-018)",
                    last_success.elapsed().as_secs(),
                    timeout.as_secs()
                );
                std::process::exit(0);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_ui_sits_beside_the_binary() {
        let exe = Path::new("versions/0.21.1/bin/pdfcraft-acowork.exe");
        let ui = ui_dir_beside_binary(exe);
        assert_eq!(ui, Path::new("versions/0.21.1/bin/../ui"));
    }

    #[test]
    fn script_and_wasm_types_are_explicit() {
        assert_eq!(
            asset_content_type("/ui/pdfcraft-web-abc.js"),
            Some("text/javascript")
        );
        assert_eq!(
            asset_content_type("/ui/pdfcraft-web-abc_bg.wasm"),
            Some("application/wasm")
        );
        assert_eq!(asset_content_type("/ui/index.html"), None);
    }
}
