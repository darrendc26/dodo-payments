use axum::{
    routing::post,
    Router,
};
use std::net::SocketAddr;
use tracing::info;

mod handler;

use handler::handle_payment;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let app = Router::new().route("/payments", post(handle_payment));

    let port = std::env::var("PORT").unwrap_or_else(|_| "8081".to_string());
    let addr: SocketAddr = format!("0.0.0.0:{}", port)
        .parse()
        .expect("Invalid address");

    info!("Starting mock PSP server on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
