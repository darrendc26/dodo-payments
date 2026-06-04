use axum::{extract::{State, Path}, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::AppState;
use crate::auth::AuthenticatedBusiness;
use crate::error::AppError;
use tracing::info;

#[derive(Deserialize)]
pub struct CreateCustomerInput {
    pub name: String,
    pub email: String,
}

#[derive(Serialize)]
pub struct CustomerResponse {
    pub id: Uuid,
    pub name: String,
    pub email: String,
    pub created_at: DateTime<Utc>,
}

// Creates a new customer for the authenticated business.
pub async fn create_customer(
    State(state): State<AppState>,
    business: AuthenticatedBusiness,
    Json(input): Json<CreateCustomerInput>,
) -> Result<Json<CustomerResponse>, AppError> {
    info!(
        "Creating customer: {} for business: {}",
        input.email, business.name
    );

    let res = sqlx::query!(
        "INSERT INTO customers (business_id, name, email) VALUES ($1, $2, $3) RETURNING id, name, email, created_at",
        business.id,
        input.name,
        input.email
    )
    .fetch_one(&state.db)
    .await;

    match res {
        Ok(row) => Ok(Json(CustomerResponse {
            id: row.id,
            name: row.name,
            email: row.email,
            created_at: row.created_at,
        })),
        Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
            Err(AppError::Conflict("Customer email already exists for this business".to_string()))
        }
        Err(e) => Err(AppError::Internal(e.to_string())),
    }
}

// Retrieves all customers belonging to the authenticated business.
pub async fn list_customers(
    State(state): State<AppState>,
    business: AuthenticatedBusiness,
) -> Result<Json<Vec<CustomerResponse>>, AppError> {
    let rows = sqlx::query!(
        "SELECT id, name, email, created_at FROM customers WHERE business_id = $1 ORDER BY created_at DESC",
        business.id
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    let customers = rows
        .into_iter()
        .map(|r| CustomerResponse {
            id: r.id,
            name: r.name,
            email: r.email,
            created_at: r.created_at,
        })
        .collect();

    Ok(Json(customers))
}

// Fetches a single customer by ID, ensuring it belongs to the authenticated business.
pub async fn get_customer(
    State(state): State<AppState>,
    business: AuthenticatedBusiness,
    Path(customer_id): Path<Uuid>,
) -> Result<Json<CustomerResponse>, AppError> {
    let row = sqlx::query!(
        "SELECT id, name, email, created_at FROM customers WHERE id = $1 AND business_id = $2",
        customer_id,
        business.id
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
    .ok_or_else(|| AppError::NotFound("Customer not found".to_string()))?;

    Ok(Json(CustomerResponse {
        id: row.id,
        name: row.name,
        email: row.email,
        created_at: row.created_at,
    }))
}
