use axum::{extract::State, http::StatusCode, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Transaction};
use uuid::Uuid;
use crate::AppState;
use crate::auth::AuthenticatedBusiness;
use crate::error::AppError;
use tracing::{error, info};

#[derive(Deserialize)]
pub struct CreateWebhookEndpointInput {
    pub url: String,
}

#[derive(Serialize)]
pub struct WebhookEndpointResponse {
    pub id: Uuid,
    pub url: String,
    pub secret: String,
    pub created_at: DateTime<Utc>,
}

// Queues a pending webhook delivery record inside the provided database transaction.
pub async fn queue_webhook(
    tx: &mut Transaction<'_, sqlx::Postgres>,
    business_id: Uuid,
    event_type: &str,
    payload: serde_json::Value,
) -> Result<(), sqlx::Error> {
    let endpoints = sqlx::query!(
        "SELECT id FROM webhook_endpoints WHERE business_id = $1",
        business_id
    )
    .fetch_all(&mut **tx)
    .await?;

    for ep in endpoints {
        sqlx::query!(
            r#"
            INSERT INTO webhook_deliveries (business_id, endpoint_id, event_type, payload, status)
            VALUES ($1, $2, $3, $4, 'pending')
            "#,
            business_id,
            ep.id,
            event_type,
            payload
        )
        .execute(&mut **tx)
        .await?;
    }

    Ok(())
}

// Registers a new webhook destination URL and generates a signing secret for secure payloads.
pub async fn register_webhook_endpoint(
    State(state): State<AppState>,
    business: AuthenticatedBusiness,
    Json(input): Json<CreateWebhookEndpointInput>,
) -> Result<(StatusCode, Json<WebhookEndpointResponse>), AppError> {
    info!(
        "Registering webhook url: {} for business: {}",
        input.url, business.name
    );

    let secret = format!("whsec_{}", Uuid::new_v4().simple());

    let row = sqlx::query!(
        r#"
        INSERT INTO webhook_endpoints (business_id, url, secret)
        VALUES ($1, $2, $3)
        RETURNING id, url, secret, created_at
        "#,
        business.id,
        input.url,
        secret
    )
    .fetch_one(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(WebhookEndpointResponse {
            id: row.id,
            url: row.url,
            secret: row.secret,
            created_at: row.created_at,
        }),
    ))
}

// Retrieves all registered webhook endpoints for the authenticated business.
pub async fn list_webhook_endpoints(
    State(state): State<AppState>,
    business: AuthenticatedBusiness,
) -> Result<Json<Vec<WebhookEndpointResponse>>, AppError> {
    let rows = sqlx::query!(
        "SELECT id, url, secret, created_at FROM webhook_endpoints WHERE business_id = $1 ORDER BY created_at DESC",
        business.id
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    let endpoints = rows
        .into_iter()
        .map(|r| WebhookEndpointResponse {
            id: r.id,
            url: r.url,
            secret: r.secret,
            created_at: r.created_at,
        })
        .collect();

    Ok(Json(endpoints))
}

// Background worker that polls pending webhook deliveries and POSTs signed requests to receiver endpoints.
pub async fn webhook_delivery_worker(db: PgPool, client: reqwest::Client) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        interval.tick().await;

        let deliveries = match sqlx::query!(
            r#"
            SELECT wd.id, wd.event_type, wd.payload, wd.attempts, we.url, we.secret
            FROM webhook_deliveries wd
            JOIN webhook_endpoints we ON wd.endpoint_id = we.id
            WHERE wd.status = 'pending' OR (wd.status = 'failed' AND wd.next_attempt_at <= NOW() AND wd.attempts < 5)
            ORDER BY wd.created_at ASC
            LIMIT 10
            "#
        )
        .fetch_all(&db)
        .await
        {
            Ok(d) => d,
            Err(e) => {
                error!("Failed to fetch webhook deliveries: {}", e);
                continue;
            }
        };

        for delivery in deliveries {
            let db_clone = db.clone();
            let client_clone = client.clone();
            tokio::spawn(async move {
                info!("Attempting to deliver webhook: {:?}", delivery.id);
                let body_str = serde_json::to_string(&delivery.payload).unwrap();
                let timestamp = Utc::now().timestamp();
                let payload_to_sign = format!("{}.{}", timestamp, body_str);

                use hmac::{Hmac, Mac};
                use sha2::Sha256;
                let mut mac = Hmac::<Sha256>::new_from_slice(delivery.secret.as_bytes()).unwrap();
                mac.update(payload_to_sign.as_bytes());
                let signature = hex::encode(mac.finalize().into_bytes());
                let signature_header = format!("t={},v1={}", timestamp, signature);

                let res = client_clone
                    .post(&delivery.url)
                    .header("Content-Type", "application/json")
                    .header("X-Dodo-Signature", signature_header)
                    .body(body_str)
                    .send()
                    .await;

                let success = match res {
                    Ok(resp) => resp.status().is_success(),
                    Err(_) => false,
                };

                let next_attempts = delivery.attempts + 1;
                if success {
                    info!("Successfully delivered webhook: {:?}", delivery.id);
                    let _ = sqlx::query!(
                        "UPDATE webhook_deliveries SET status = 'succeeded', attempts = $1 WHERE id = $2",
                        next_attempts,
                        delivery.id
                    )
                    .execute(&db_clone)
                    .await;
                } else if next_attempts >= 5 {
                    info!("Webhook delivery exhausted retries: {:?}", delivery.id);
                    let _ = sqlx::query!(
                        "UPDATE webhook_deliveries SET status = 'failed', attempts = $1, next_attempt_at = NULL WHERE id = $2",
                        next_attempts,
                        delivery.id
                    )
                    .execute(&db_clone)
                    .await;
                } else {
                    // Exponential backoff
                    let delay_secs = match next_attempts {
                        1 => 15,
                        2 => 60,
                        3 => 300,
                        4 => 1800,
                        _ => 7200,
                    };
                    info!("Webhook delivery failed, scheduling retry {} in {}s for {:?}", next_attempts, delay_secs, delivery.id);
                    let next_run = Utc::now() + chrono::Duration::seconds(delay_secs);
                    let _ = sqlx::query!(
                        "UPDATE webhook_deliveries SET status = 'failed', attempts = $1, next_attempt_at = $2 WHERE id = $3",
                        next_attempts,
                        next_run,
                        delivery.id
                    )
                    .execute(&db_clone)
                    .await;
                }
            });
        }
    }
}
