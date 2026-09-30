#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use console_app::{AppConfig, AppRole, AppState, DatabaseDependency, build_router};
use sqlx::PgPool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[sqlx::test(migrations = "../crates/platform/db/migrations")]
async fn websocket_route_is_mounted_and_auth_gated_at_upgrade(pool: PgPool) {
    let config = AppConfig::from_pairs([
        ("CONSOLE_APP_ROLE", AppRole::Api.to_string()),
        ("CONSOLE_HTTP_ADDR", "127.0.0.1:0".to_owned()),
    ])
    .unwrap();
    let state = AppState::new(config, DatabaseDependency::Postgres(pool)).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, build_router(state)).await.unwrap();
    });

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(
            format!(
                "GET /api/v1/ws HTTP/1.1\r\n\
                 Host: {addr}\r\n\
                 Connection: Upgrade\r\n\
                 Upgrade: websocket\r\n\
                 Sec-WebSocket-Version: 13\r\n\
                 Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                 \r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut buf = vec![0_u8; 4096];
    let bytes_read = stream.read(&mut buf).await.unwrap();
    server.abort();
    let response = String::from_utf8_lossy(&buf[..bytes_read]);

    assert_eq!(
        response.lines().next(),
        Some("HTTP/1.1 503 Service Unavailable"),
        "mounted WS route must fail closed before upgrade when JWT verification is not configured"
    );
}
