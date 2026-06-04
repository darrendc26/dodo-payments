use axum::{
    extract::FromRef,
    routing::{get, post},
    Router, response::IntoResponse, http::StatusCode
};
use sqlx::PgPool;

pub mod error;
pub mod auth;
pub mod business;
pub mod customer;
pub mod invoice;
pub mod payment;
pub mod webhook;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub http_client: reqwest::Client,
    pub psp_url: String,
}

impl FromRef<AppState> for PgPool {
    fn from_ref(state: &AppState) -> Self {
        state.db.clone()
    }
}

// GET /health
pub async fn health_check() -> impl IntoResponse {
    StatusCode::OK
}

// Router builder to share between main and tests
pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/businesses", post(business::create_business))
        .route(
            "/customers",
            post(customer::create_customer).get(customer::list_customers),
        )
        .route("/customers/:id", get(customer::get_customer))
        .route(
            "/invoices",
            post(invoice::create_invoice).get(invoice::list_invoices),
        )
        .route("/invoices/:id", get(invoice::get_invoice))
        .route("/invoices/:id/pay", post(payment::pay_invoice))
        .route("/invoices/:id/void", post(invoice::void_invoice))
        .route("/invoices/:id/uncollectible", post(invoice::uncollectible_invoice))
        .route(
            "/webhooks/endpoints",
            post(webhook::register_webhook_endpoint).get(webhook::list_webhook_endpoints),
        )
        .with_state(state)
}

// Re-export worker for convenience
pub use webhook::webhook_delivery_worker;
