use axum::{extract::{State, Path, Query}, http::StatusCode, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::AppState;
use crate::auth::AuthenticatedBusiness;
use crate::error::AppError;
use crate::webhook::queue_webhook;
use tracing::info;

#[derive(Deserialize)]
pub struct CreateInvoiceInput {
    pub customer_id: Uuid,
    pub due_date: DateTime<Utc>,
    pub items: Vec<CreateInvoiceItemInput>,
}

#[derive(Deserialize)]
pub struct CreateInvoiceItemInput {
    pub description: String,
    pub quantity: i32,
    pub unit_amount_cents: i64,
}

#[derive(Serialize)]
pub struct InvoiceItemResponse {
    pub id: Uuid,
    pub description: String,
    pub quantity: i32,
    pub unit_amount_cents: i64,
}

#[derive(Serialize)]
pub struct InvoiceResponse {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub status: String,
    pub amount_cents: i64,
    pub due_date: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub items: Option<Vec<InvoiceItemResponse>>,
}

#[derive(Deserialize)]
pub struct ListInvoicesQuery {
    pub status: Option<String>,
}

// Creates a new invoice with line items, calculates total amount, and queues creation webhook.
pub async fn create_invoice(
    State(state): State<AppState>,
    business: AuthenticatedBusiness,
    Json(input): Json<CreateInvoiceInput>,
) -> Result<(StatusCode, Json<InvoiceResponse>), AppError> {
    info!(
        "Creating invoice for customer: {} in business: {}",
        input.customer_id, business.name
    );

    // Verify customer exists and belongs to business
    let customer_exists = sqlx::query!(
        "SELECT 1 as exists FROM customers WHERE id = $1 AND business_id = $2",
        input.customer_id,
        business.id
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
    .is_some();

    if !customer_exists {
        return Err(AppError::BadRequest("Customer not found or invalid for business".to_string()));
    }

    if input.items.is_empty() {
        return Err(AppError::BadRequest("Invoice must contain at least one item".to_string()));
    }

    // Calculate total amount
    let mut total_cents: i64 = 0;
    for item in &input.items {
        if item.quantity <= 0 {
            return Err(AppError::BadRequest("Item quantity must be greater than zero".to_string()));
        }
        if item.unit_amount_cents < 0 {
            return Err(AppError::BadRequest("Item unit amount cannot be negative".to_string()));
        }
        total_cents = total_cents
            .checked_add(
                (item.quantity as i64)
                    .checked_mul(item.unit_amount_cents)
                    .ok_or_else(|| AppError::BadRequest("Integer overflow in calculation".to_string()))?,
            )
            .ok_or_else(|| AppError::BadRequest("Integer overflow in calculation".to_string()))?;
    }

    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Insert invoice
    let invoice_row = sqlx::query!(
        r#"
        INSERT INTO invoices (business_id, customer_id, status, amount_cents, due_date)
        VALUES ($1, $2, 'open', $3, $4)
        RETURNING id, customer_id, status, amount_cents, due_date, created_at, updated_at
        "#,
        business.id,
        input.customer_id,
        total_cents,
        input.due_date
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    // Insert line items
    let mut items_response = Vec::new();
    for item in input.items {
        let item_row = sqlx::query!(
            r#"
            INSERT INTO invoice_items (invoice_id, description, quantity, unit_amount_cents)
            VALUES ($1, $2, $3, $4)
            RETURNING id, description, quantity, unit_amount_cents
            "#,
            invoice_row.id,
            item.description,
            item.quantity,
            item.unit_amount_cents
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        items_response.push(InvoiceItemResponse {
            id: item_row.id,
            description: item_row.description,
            quantity: item_row.quantity,
            unit_amount_cents: item_row.unit_amount_cents,
        });
    }

    // Queue invoice.created webhook event
    let payload = serde_json::json!({
        "id": invoice_row.id,
        "customer_id": invoice_row.customer_id,
        "status": invoice_row.status,
        "amount_cents": invoice_row.amount_cents,
        "due_date": invoice_row.due_date,
    });
    queue_webhook(&mut tx, business.id, "invoice.created", payload)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(InvoiceResponse {
            id: invoice_row.id,
            customer_id: invoice_row.customer_id,
            status: invoice_row.status,
            amount_cents: invoice_row.amount_cents,
            due_date: invoice_row.due_date,
            created_at: invoice_row.created_at,
            updated_at: invoice_row.updated_at,
            items: Some(items_response),
        }),
    ))
}

// Retrieves details of a specific invoice including its line items.
pub async fn get_invoice(
    State(state): State<AppState>,
    business: AuthenticatedBusiness,
    Path(invoice_id): Path<Uuid>,
) -> Result<Json<InvoiceResponse>, AppError> {
    let invoice = sqlx::query!(
        r#"
        SELECT id, customer_id, status, amount_cents, due_date, created_at, updated_at
        FROM invoices WHERE id = $1 AND business_id = $2
        "#,
        invoice_id,
        business.id
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
    .ok_or_else(|| AppError::NotFound("Invoice not found".to_string()))?;

    let item_rows = sqlx::query!(
        r#"
        SELECT id, description, quantity, unit_amount_cents
        FROM invoice_items WHERE invoice_id = $1
        "#,
        invoice.id
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    let items = item_rows
        .into_iter()
        .map(|r| InvoiceItemResponse {
            id: r.id,
            description: r.description,
            quantity: r.quantity,
            unit_amount_cents: r.unit_amount_cents,
        })
        .collect();

    Ok(Json(InvoiceResponse {
        id: invoice.id,
        customer_id: invoice.customer_id,
        status: invoice.status,
        amount_cents: invoice.amount_cents,
        due_date: invoice.due_date,
        created_at: invoice.created_at,
        updated_at: invoice.updated_at,
        items: Some(items),
    }))
}

// Lists all invoices for the authenticated business, optionally filtered by status.
pub async fn list_invoices(
    State(state): State<AppState>,
    business: AuthenticatedBusiness,
    Query(query): Query<ListInvoicesQuery>,
) -> Result<Json<Vec<InvoiceResponse>>, AppError> {
    let invoices = if let Some(status) = query.status {
        let rows = sqlx::query!(
            r#"
            SELECT id, customer_id, status, amount_cents, due_date, created_at, updated_at
            FROM invoices WHERE business_id = $1 AND status = $2 ORDER BY created_at DESC
            "#,
            business.id,
            status
        )
        .fetch_all(&state.db)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        rows.into_iter()
            .map(|r| InvoiceResponse {
                id: r.id,
                customer_id: r.customer_id,
                status: r.status,
                amount_cents: r.amount_cents,
                due_date: r.due_date,
                created_at: r.created_at,
                updated_at: r.updated_at,
                items: None,
            })
            .collect()
    } else {
        let rows = sqlx::query!(
            r#"
            SELECT id, customer_id, status, amount_cents, due_date, created_at, updated_at
            FROM invoices WHERE business_id = $1 ORDER BY created_at DESC
            "#,
            business.id
        )
        .fetch_all(&state.db)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        rows.into_iter()
            .map(|r| InvoiceResponse {
                id: r.id,
                customer_id: r.customer_id,
                status: r.status,
                amount_cents: r.amount_cents,
                due_date: r.due_date,
                created_at: r.created_at,
                updated_at: r.updated_at,
                items: None,
            })
            .collect()
    };

    Ok(Json(invoices))
}

// Transition an open invoice status to 'void'.
pub async fn void_invoice(
    State(state): State<AppState>,
    business: AuthenticatedBusiness,
    Path(invoice_id): Path<Uuid>,
) -> Result<Json<InvoiceResponse>, AppError> {
    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let invoice = sqlx::query!(
        "SELECT id, status, customer_id, amount_cents, due_date, created_at, updated_at FROM invoices WHERE id = $1 AND business_id = $2 FOR UPDATE",
        invoice_id,
        business.id
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
    .ok_or_else(|| AppError::NotFound("Invoice not found".to_string()))?;

    if invoice.status == "paid" || invoice.status == "void" || invoice.status == "uncollectible" {
        return Err(AppError::UnprocessableEntity(format!(
            "Cannot void invoice in '{}' state",
            invoice.status
        )));
    }

    let updated = sqlx::query!(
        "UPDATE invoices SET status = 'void', updated_at = NOW() WHERE id = $1 RETURNING updated_at",
        invoice_id
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(InvoiceResponse {
        id: invoice.id,
        customer_id: invoice.customer_id,
        status: "void".to_string(),
        amount_cents: invoice.amount_cents,
        due_date: invoice.due_date,
        created_at: invoice.created_at,
        updated_at: updated.updated_at,
        items: None,
    }))
}

// Transition an open invoice status to 'uncollectible'.
pub async fn uncollectible_invoice(
    State(state): State<AppState>,
    business: AuthenticatedBusiness,
    Path(invoice_id): Path<Uuid>,
) -> Result<Json<InvoiceResponse>, AppError> {
    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let invoice = sqlx::query!(
        "SELECT id, status, customer_id, amount_cents, due_date, created_at, updated_at FROM invoices WHERE id = $1 AND business_id = $2 FOR UPDATE",
        invoice_id,
        business.id
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
    .ok_or_else(|| AppError::NotFound("Invoice not found".to_string()))?;

    if invoice.status == "paid" || invoice.status == "void" || invoice.status == "uncollectible" {
        return Err(AppError::UnprocessableEntity(format!(
            "Cannot mark invoice as uncollectible in '{}' state",
            invoice.status
        )));
    }

    let updated = sqlx::query!(
        "UPDATE invoices SET status = 'uncollectible', updated_at = NOW() WHERE id = $1 RETURNING updated_at",
        invoice_id
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(InvoiceResponse {
        id: invoice.id,
        customer_id: invoice.customer_id,
        status: "uncollectible".to_string(),
        amount_cents: invoice.amount_cents,
        due_date: invoice.due_date,
        created_at: invoice.created_at,
        updated_at: updated.updated_at,
        items: None,
    }))
}
