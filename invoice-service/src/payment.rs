use axum::{extract::{State, Path}, http::StatusCode, response::{IntoResponse, Response}, Json};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::AppState;
use crate::auth::AuthenticatedBusiness;
use crate::error::AppError;
use crate::webhook::queue_webhook;
use tracing::info;

#[derive(Serialize, Deserialize)]
pub struct PayInvoiceInput {
    pub card_token: String,
}

#[derive(Serialize)]
pub struct PayInvoiceResponse {
    pub status: String,
    pub payment_attempt_id: Uuid,
    pub psp_reference: Option<String>,
    pub error_code: Option<String>,
}

// Resolves a payment attempt by updating database statuses and queuing webhook events.
pub async fn resolve_payment_attempt(
    db: &sqlx::PgPool,
    business_id: Uuid,
    invoice_id: Uuid,
    payment_attempt_id: Uuid,
    status: &str,
    psp_ref: Option<String>,
    err_code: Option<String>,
    idempotency_key: &str,
    request_path: &str,
    request_body: Option<&str>,
) -> Result<Response, AppError> {
    let mut tx = db.begin().await.map_err(|e| AppError::Internal(e.to_string()))?;

    // Lock invoice FOR UPDATE
    let inv = sqlx::query!(
        "SELECT status FROM invoices WHERE id = $1 FOR UPDATE",
        invoice_id
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    // Double check state is still open
    if inv.status == "open" {
        // Update attempt
        sqlx::query!(
            "UPDATE payment_attempts SET status = $1, psp_reference = $2, error_code = $3 WHERE id = $4",
            status,
            psp_ref,
            err_code,
            payment_attempt_id
        )
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        let new_invoice_status = if status == "succeeded" {
            "paid"
        } else {
            "open"
        };

        sqlx::query!(
            "UPDATE invoices SET status = $1, updated_at = NOW() WHERE id = $2",
            new_invoice_status,
            invoice_id
        )
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        // Queue Webhook
        let event = if status == "succeeded" {
            "invoice.paid"
        } else {
            "invoice.payment_failed"
        };
        let webhook_payload = serde_json::json!({
            "invoice_id": invoice_id,
            "payment_attempt_id": payment_attempt_id,
            "status": status,
            "psp_reference": psp_ref,
            "error_code": err_code,
        });
        queue_webhook(&mut tx, business_id, event, webhook_payload)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;

        // Save in Idempotency
        let response_body = serde_json::json!({
            "status": status,
            "payment_attempt_id": payment_attempt_id,
            "psp_reference": psp_ref,
            "error_code": err_code
        });
        let response_body_str = response_body.to_string();

        sqlx::query!(
            r#"
            INSERT INTO idempotency_keys (idempotency_key, business_id, request_path, request_body, response_status, response_body)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
            idempotency_key,
            business_id,
            request_path,
            request_body,
            200i16,
            response_body_str
        )
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        tx.commit().await.map_err(|e| AppError::Internal(e.to_string()))?;

        Ok(Json(PayInvoiceResponse {
            status: status.to_string(),
            payment_attempt_id,
            psp_reference: psp_ref,
            error_code: err_code,
        })
        .into_response())
    } else {
        // Already resolved (e.g. concurrent race resolved first)
        tx.rollback().await.map_err(|e| AppError::Internal(e.to_string()))?;
        Err(AppError::Conflict("Invoice state already changed".to_string()))
    }
}

// Processes payment for an open invoice, enforces idempotency, and connects to the mock PSP.
pub async fn pay_invoice(
    State(state): State<AppState>,
    business: AuthenticatedBusiness,
    Path(invoice_id): Path<Uuid>,
    parts: axum::http::request::Parts,
    Json(input): Json<PayInvoiceInput>,
) -> Result<Response, AppError> {
    let idempotency_key = match parts
        .headers
        .get("Idempotency-Key")
        .and_then(|h| h.to_str().ok())
    {
        Some(k) => k.to_string(),
        None => return Err(AppError::BadRequest("Missing Idempotency-Key header".to_string())),
    };

    let request_path = parts.uri.path().to_string();
    let input_body_str = serde_json::to_string(&input).unwrap_or_default();

    // 1. Check idempotency table first
    let cached = sqlx::query!(
        "SELECT response_status, response_body, request_path, request_body FROM idempotency_keys WHERE business_id = $1 AND idempotency_key = $2",
        business.id,
        idempotency_key
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    if let Some(row) = cached {
        if row.request_path != request_path {
            return Err(AppError::BadRequest(
                "Idempotency key reused with different request parameters".to_string(),
            ));
        }
        if let Some(ref cached_body) = row.request_body {
            let current_val = serde_json::to_value(&input).unwrap_or_default();
            let cached_val: serde_json::Value = serde_json::from_str(cached_body).unwrap_or_default();
            if current_val != cached_val {
                return Err(AppError::BadRequest(
                    "Idempotency key reused with different request body".to_string(),
                ));
            }
        }
        let status = StatusCode::from_u16(row.response_status as u16)
            .unwrap_or(StatusCode::OK);
        let body_val: serde_json::Value = serde_json::from_str(&row.response_body)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        info!("Returning cached idempotency response for key: {}", idempotency_key);
        return Ok((status, Json(body_val)).into_response());
    }

    // 2. Phase 1: DB Reservation
    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Lock invoice FOR UPDATE
    let invoice = sqlx::query!(
        "SELECT status, amount_cents FROM invoices WHERE id = $1 AND business_id = $2 FOR UPDATE",
        invoice_id,
        business.id
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
    .ok_or_else(|| AppError::NotFound("Invoice not found".to_string()))?;

    // Validate state
    if invoice.status != "open" {
        return Err(AppError::UnprocessableEntity(format!(
            "Invoice is in '{}' state and cannot be paid",
            invoice.status
        )));
    }

    // Check for active pending payment attempts (within 5 minutes)
    let active_pending = sqlx::query!(
        r#"
        SELECT id, idempotency_key, card_token
        FROM payment_attempts
        WHERE invoice_id = $1 AND status = 'pending' AND created_at > NOW() - INTERVAL '5 minutes'
        "#,
        invoice_id
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    let payment_attempt_id: Uuid;
    let mut is_retry = false;

    if let Some(pending) = active_pending {
        // If it matches our current idempotency key, we allow retrying the external request
        if pending.idempotency_key.as_deref() == Some(&idempotency_key) {
            if pending.card_token != input.card_token {
                return Err(AppError::BadRequest(
                    "Idempotency key reused with different request body".to_string(),
                ));
            }
            payment_attempt_id = pending.id;
            is_retry = true;
            info!("Found matching pending attempt for idempotency key. Retrying PSP call.");
        } else {
            // Different idempotency key, block concurrent payment
            return Err(AppError::Conflict(
                "A payment attempt is currently in progress for this invoice".to_string(),
            ));
        }
    } else {
        // Create new attempt
        payment_attempt_id = Uuid::new_v4();
        sqlx::query!(
            r#"
            INSERT INTO payment_attempts (id, invoice_id, status, card_token, idempotency_key)
            VALUES ($1, $2, 'pending', $3, $4)
            "#,
            payment_attempt_id,
            invoice_id,
            input.card_token,
            idempotency_key
        )
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // 3. Phase 2: Call Mock PSP
    info!(
        "Calling Mock PSP for payment_attempt: {}, invoice: {}, retry: {}",
        payment_attempt_id, invoice_id, is_retry
    );

    let psp_url = format!("{}/payments", state.psp_url);
    let psp_payload = serde_json::json!({
        "payment_id": payment_attempt_id,
        "amount_cents": invoice.amount_cents,
        "card_token": input.card_token,
    });

    let psp_res = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        state.http_client.post(&psp_url).json(&psp_payload).send(),
    )
    .await;

    let return_pending = || {
        info!("PSP request timed out or returned network error. Leaving payment_attempt: {} as pending.", payment_attempt_id);
        Ok((
            StatusCode::ACCEPTED,
            Json(PayInvoiceResponse {
                status: "pending".to_string(),
                payment_attempt_id,
                psp_reference: None,
                error_code: None,
            }),
        )
            .into_response())
    };

    match psp_res {
        Ok(Ok(response)) => {
            let status = response.status();
            if status.is_success() {
                #[derive(Deserialize)]
                struct PspResponse {
                    status: String,
                    psp_ref: Option<Uuid>,
                    code: Option<String>,
                }
                if let Ok(res_body) = response.json::<PspResponse>().await {
                    if res_body.status == "succeeded" {
                        resolve_payment_attempt(
                            &state.db,
                            business.id,
                            invoice_id,
                            payment_attempt_id,
                            "succeeded",
                            res_body.psp_ref.map(|u| u.to_string()),
                            None,
                            &idempotency_key,
                            &request_path,
                            Some(&input_body_str),
                        )
                        .await
                    } else {
                        resolve_payment_attempt(
                            &state.db,
                            business.id,
                            invoice_id,
                            payment_attempt_id,
                            "failed",
                            None,
                            res_body.code,
                            &idempotency_key,
                            &request_path,
                            Some(&input_body_str),
                        )
                        .await
                    }
                } else {
                    return_pending()
                }
            } else {
                return_pending()
            }
        }
        _ => return_pending(),
    }
}
