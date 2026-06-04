use axum::{http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use tracing::info;
use uuid::Uuid;

#[derive(Deserialize, Debug)]
pub struct PspPaymentRequest {
    pub payment_id: Uuid,
    pub amount_cents: i64,
    pub card_token: String,
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum PspPaymentResponse {
    Success { status: String, psp_ref: Uuid },
    Failed { status: String, code: String },
}

// Processes payment requests with simulated delays and outcomes based on card tokens.
pub async fn handle_payment(
    Json(payload): Json<PspPaymentRequest>,
) -> Result<Json<PspPaymentResponse>, (StatusCode, String)> {
    info!("Mock PSP received payment request: {:?}", payload);

    match payload.card_token.as_str() {
        "tok_success" => {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            Ok(Json(PspPaymentResponse::Success {
                status: "succeeded".to_string(),
                psp_ref: Uuid::new_v4(),
            }))
        }
        "tok_insufficient_funds" => {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            Ok(Json(PspPaymentResponse::Failed {
                status: "failed".to_string(),
                code: "insufficient_funds".to_string(),
            }))
        }
        "tok_card_declined" => {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            Ok(Json(PspPaymentResponse::Failed {
                status: "failed".to_string(),
                code: "card_declined".to_string(),
            }))
        }
        "tok_timeout" => {
            info!("tok_timeout received, sleeping for 30 seconds...");
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            info!("tok_timeout sleep complete, returning success");
            Ok(Json(PspPaymentResponse::Success {
                status: "succeeded".to_string(),
                psp_ref: Uuid::new_v4(),
            }))
        }
        "tok_network_error" => {
            info!("tok_network_error received, returning 500");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal Server Error (Simulated network error)".to_string(),
            ))
        }
        _ => {
            // Default to success
            Ok(Json(PspPaymentResponse::Success {
                status: "succeeded".to_string(),
                psp_ref: Uuid::new_v4(),
            }))
        }
    }
}
