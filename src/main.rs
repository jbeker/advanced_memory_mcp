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

    /// Directory for per-user JSONL data files
    #[arg(long, env = "MCP_DATA_DIR", required = true)]
    data_dir: PathBuf,

    /// Path to tokens.json config file
    #[arg(long, env = "MCP_TOKEN_CONFIG", required = true)]
    token_config: PathBuf,

    /// Path to entity_types.json config file
    #[arg(long, env = "MCP_TYPE_CONFIG")]
    type_config: Option<PathBuf>,

    /// Transport protocol ("http" or "streamable-http"; both are the MCP
    /// streamable HTTP transport. "sse" is no longer supported.)
    #[arg(long, env = "MCP_TRANSPORT", default_value = "http")]
    transport: String,
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

    match args.transport.as_str() {
        "http" | "streamable-http" => {}
        "sse" => anyhow::bail!(
            "the 'sse' transport was removed; use 'http' (streamable HTTP) instead"
        ),
        other => anyhow::bail!("unknown transport '{other}'; use 'http' or 'streamable-http'"),
    }

    let tokens = TokenConfig::load(&args.token_config).map_err(anyhow::Error::msg)?;
    let aliases = load_aliases(args.type_config.as_ref())?;
    std::fs::create_dir_all(&args.data_dir)?;

    tracing::info!("{SERVER_NAME} v{} (rust)", env!("CARGO_PKG_VERSION"));
    tracing::info!(
        "Transport: streamable-http | Host: {} | Port: {}",
        args.host,
        args.port
    );
    tracing::info!("Data dir: {}", args.data_dir.display());
    tracing::info!("Type aliases loaded: {}", aliases.len());

    let state = Arc::new(AppState {
        stores: StoreMap::new(args.data_dir.clone()),
        tokens,
        aliases,
    });

    let ct = tokio_util::sync::CancellationToken::new();
    let mcp_state = state.clone();
    let mcp_service = StreamableHttpService::new(
        move || Ok(MemoryServer::new(mcp_state.clone())),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_stateful_mode(false)
            .with_cancellation_token(ct.child_token()),
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
        .merge(advanced_memory_mcp::webui::router(state.clone(), ui_secret));
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
