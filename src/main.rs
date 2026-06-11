use advanced_memory_mcp::mcp::{AppState, MemoryServer, SERVER_NAME};
use advanced_memory_mcp::store::StoreMap;
use advanced_memory_mcp::tokens::TokenConfig;
use clap::Parser;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Advanced Memory MCP Server (Rust implementation).
///
/// Same flags and environment variables as the Python version, except the
/// deprecated `sse` transport is no longer supported and authentication
/// always requires a Bearer header (no MEMORY_TOKEN fallback).
#[derive(Parser, Debug)]
#[command(name = "advanced-memory-mcp", version)]
struct Args {
    /// Host to bind to
    #[arg(long, env = "MCP_HOST", default_value = "0.0.0.0")]
    host: String,

    /// Port to listen on
    #[arg(long, env = "MCP_PORT", default_value_t = 8765)]
    port: u16,

    /// Directory for per-user JSONL data files (required unless --health-check)
    #[arg(long, env = "MCP_DATA_DIR")]
    data_dir: Option<PathBuf>,

    /// Path to tokens.json config file (required unless --health-check)
    #[arg(long, env = "MCP_TOKEN_CONFIG")]
    token_config: Option<PathBuf>,

    /// Path to entity_types.json config file
    #[arg(long, env = "MCP_TYPE_CONFIG")]
    type_config: Option<PathBuf>,

    /// Transport protocol ("http" or "streamable-http"; both are the MCP
    /// streamable HTTP transport. "sse" is no longer supported.)
    #[arg(long, env = "MCP_TRANSPORT", default_value = "http")]
    transport: String,

    /// Comma-separated Host headers to accept (DNS-rebinding protection),
    /// e.g. "memory.example.com,localhost". Empty (the default) accepts any
    /// Host, matching the Python implementation; MCP calls are still
    /// bearer-token authenticated either way.
    #[arg(long, env = "MCP_ALLOWED_HOSTS", value_delimiter = ',', num_args = 0..)]
    allowed_hosts: Vec<String>,

    /// Probe the running server's /health endpoint and exit 0/1.
    /// Used as the Docker HEALTHCHECK command.
    #[arg(long)]
    health_check: bool,
}

fn load_aliases(path: Option<&PathBuf>) -> anyhow::Result<HashMap<String, String>> {
    let Some(path) = path else {
        return Ok(HashMap::new());
    };
    if !path.exists() {
        // Python silently skips a missing type config; match that.
        return Ok(HashMap::new());
    }
    let raw = std::fs::read_to_string(path)?;
    #[derive(serde::Deserialize)]
    struct TypeConfig {
        #[serde(default)]
        aliases: HashMap<String, String>,
    }
    let config: TypeConfig = serde_json::from_str(&raw)?;
    Ok(config.aliases)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();

    if args.health_check {
        // A 0.0.0.0 bind address isn't connectable; probe loopback instead.
        let host = if args.host == "0.0.0.0" { "127.0.0.1" } else { &args.host };
        return match advanced_memory_mcp::health::probe(
            host,
            args.port,
            std::time::Duration::from_secs(3),
        ) {
            Ok(()) => Ok(()),
            Err(e) => {
                eprintln!("health check failed: {e}");
                std::process::exit(1);
            }
        };
    }

    let Some(data_dir) = args.data_dir else {
        anyhow::bail!("--data-dir is required (or set MCP_DATA_DIR)");
    };
    let Some(token_config) = args.token_config else {
        anyhow::bail!("--token-config is required (or set MCP_TOKEN_CONFIG)");
    };

    match args.transport.as_str() {
        "http" | "streamable-http" => {}
        "sse" => anyhow::bail!(
            "the 'sse' transport was removed; use 'http' (streamable HTTP) instead"
        ),
        other => anyhow::bail!("unknown transport '{other}'; use 'http' or 'streamable-http'"),
    }

    let tokens = TokenConfig::load(&token_config).map_err(anyhow::Error::msg)?;
    let aliases = load_aliases(args.type_config.as_ref())?;
    std::fs::create_dir_all(&data_dir)?;

    tracing::info!("{SERVER_NAME} v{} (rust)", env!("CARGO_PKG_VERSION"));
    tracing::info!(
        "Transport: streamable-http | Host: {} | Port: {}",
        args.host,
        args.port
    );
    tracing::info!("Data dir: {}", data_dir.display());
    tracing::info!("Type aliases loaded: {}", aliases.len());

    let state = Arc::new(AppState {
        stores: StoreMap::new(data_dir.clone()),
        tokens,
        aliases,
    });

    let allowed_hosts: Vec<String> = args
        .allowed_hosts
        .iter()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .collect();
    if allowed_hosts.is_empty() {
        tracing::info!("Host header validation: disabled (set MCP_ALLOWED_HOSTS to enable)");
    } else {
        tracing::info!("Host header validation: {}", allowed_hosts.join(", "));
    }

    let ct = tokio_util::sync::CancellationToken::new();
    let mcp_state = state.clone();
    let mut mcp_config = StreamableHttpServerConfig::default()
        .with_stateful_mode(false)
        .with_cancellation_token(ct.child_token());
    mcp_config = if allowed_hosts.is_empty() {
        // rmcp defaults to localhost-only, which rejects any reverse-proxied
        // or remote deployment; empty means accept any Host, like Python.
        mcp_config.disable_allowed_hosts()
    } else {
        mcp_config.with_allowed_hosts(allowed_hosts)
    };
    let mcp_service = StreamableHttpService::new(
        move || Ok(MemoryServer::new(mcp_state.clone())),
        LocalSessionManager::default().into(),
        mcp_config,
    );

    let ui_secret = match std::env::var("MEMORY_UI_SECRET") {
        Ok(secret) if !secret.is_empty() => secret,
        _ => {
            tracing::warn!(
                "MEMORY_UI_SECRET not set; using ephemeral secret. \
                 UI sessions will be invalidated when the server restarts."
            );
            use rand::Rng;
            let bytes: [u8; 32] = rand::rng().random();
            use base64::Engine;
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
        }
    };

    let router = axum::Router::new()
        .nest_service("/mcp", mcp_service)
        .merge(advanced_memory_mcp::webui::router(state.clone(), ui_secret))
        .merge(advanced_memory_mcp::health::router());
    tracing::info!("Web UI available at /ui/");

    let listener = tokio::net::TcpListener::bind((args.host.as_str(), args.port)).await?;
    tracing::info!("listening on {}", listener.local_addr()?);
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            ct.cancel();
        })
        .await?;
    Ok(())
}
