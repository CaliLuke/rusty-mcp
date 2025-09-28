use rustymcp::{api, config, logging, processing};
use std::sync::Arc;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    config::init_config();
    logging::init_tracing();
    let app = api::create_router(Arc::new(processing::ProcessingService::new().await));

    let (listener, port) = bind_listener().await.expect("Failed to bind listener");
    tracing::info!("Listening on http://0.0.0.0:{}", port);
    axum::serve(listener, app).await.unwrap();
}

async fn bind_listener() -> Result<(TcpListener, u16), std::io::Error> {
    use std::net::Ipv4Addr;

    let config = config::get_config();
    if let Some(port) = config.server_port {
        return TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))
            .await
            .map(|listener| (listener, port));
    }

    const PORT_RANGE: std::ops::RangeInclusive<u16> = 4100..=4199;
    for port in PORT_RANGE {
        match TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).await {
            Ok(listener) => {
                tracing::debug!(port, "Bound server port");
                return Ok((listener, port));
            }
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
                tracing::debug!(port, "Port already in use; trying next");
                continue;
            }
            Err(err) => return Err(err),
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AddrNotAvailable,
        "No available port found in range 4100-4199",
    ))
}
