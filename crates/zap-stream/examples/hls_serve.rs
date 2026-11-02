use std::net::SocketAddr;
use zap_stream::http::HlsRouter;
use zap_stream::stream_manager::StreamManager;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let base = args.next().expect("usage: hls_serve <base_dir> [addr]");
    let addr: SocketAddr = args
        .next()
        .unwrap_or_else(|| "0.0.0.0:8080".to_string())
        .parse()?;
    let router = HlsRouter::new(base, StreamManager::new("hls-serve".to_string()));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;
    Ok(())
}
