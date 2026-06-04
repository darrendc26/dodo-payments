use invoice_service::{create_router, AppState, webhook_delivery_worker};
use sqlx::PgPool;
use std::net::SocketAddr;
use tracing::info;

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Fetch config from environment
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgrespassword@localhost:5432/dodo_payments".to_string());
    let psp_url = std::env::var("PSP_URL").unwrap_or_else(|_| "http://localhost:8081".to_string());
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());

    info!("Connecting to PostgreSQL database: {}", db_url);
    let db = PgPool::connect(&db_url)
        .await
        .expect("Failed to connect to database");

    info!("Running database migrations...");
    sqlx::migrate!("./migrations")
        .run(&db)
        .await
        .expect("Failed to run migrations");
    info!("Database migrations completed successfully.");

    let http_client = reqwest::Client::new();

    // Spawn outbox webhook delivery worker
    let db_clone = db.clone();
    let client_clone = http_client.clone();
    tokio::spawn(async move {
        webhook_delivery_worker(db_clone, client_clone).await;
    });

    let state = AppState {
        db,
        http_client,
        psp_url,
    };

    let app = create_router(state);

    let addr: SocketAddr = format!("0.0.0.0:{}", port)
        .parse()
        .expect("Invalid address");

    info!("Starting Invoice & Payment Service on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
