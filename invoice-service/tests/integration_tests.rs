use axum::{response::IntoResponse, routing::post, Json, Router};
use invoice_service::{create_router, AppState};
use reqwest::header::HeaderMap;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::net::TcpListener;
use uuid::Uuid;

// Spawn the invoice-service server on a random local port
async fn spawn_test_server(db: PgPool, psp_url: String) -> (String, PgPool) {
    let http_client = reqwest::Client::new();
    let state = AppState {
        db: db.clone(),
        http_client,
        psp_url,
    };
    let app = create_router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://{}", addr), db)
}

// Spawn a simple mock PSP mock server on a random local port to intercept PSP requests
async fn spawn_mock_psp(
    success_callback: Arc<dyn Fn(&str) + Send + Sync>,
) -> String {
    let handle_payments = move |Json(payload): Json<serde_json::Value>| {
        let success_cb = success_callback.clone();
        async move {
            let token = payload.get("card_token").and_then(|t| t.as_str()).unwrap_or("");
            success_cb(token);

            match token {
                "tok_success" => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    axum::response::Json(serde_json::json!({
                        "status": "succeeded",
                        "psp_ref": Uuid::new_v4()
                    })).into_response()
                }
                "tok_insufficient_funds" | "tok_card_declined" => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    axum::response::Json(serde_json::json!({
                        "status": "failed",
                        "code": token.replace("tok_", "")
                    })).into_response()
                }
                "tok_timeout" => {
                    // Sleep 10s (longer than the invoice-service client timeout of 5s)
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    axum::response::Json(serde_json::json!({
                        "status": "succeeded",
                        "psp_ref": Uuid::new_v4()
                    })).into_response()
                }
                "tok_network_error" => {
                    (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        "Simulated Network Error",
                    ).into_response()
                }
                _ => {
                    axum::response::Json(serde_json::json!({
                        "status": "succeeded",
                        "psp_ref": Uuid::new_v4()
                    })).into_response()
                }
            }
        }
    };

    let app = Router::new().route("/payments", post(handle_payments));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://{}", addr)
}

// Helper to create a new business and return its auth headers
async fn setup_test_business(server_url: &str) -> (HeaderMap, Uuid) {
    let client = reqwest::Client::new();
    
    // Create business
    let res = client
        .post(&format!("{}/businesses", server_url))
        .json(&serde_json::json!({
            "name": format!("Test Business {}", Uuid::new_v4())
        }))
        .send()
        .await
        .unwrap();
    
    assert!(res.status().is_success());
    let body: serde_json::Value = res.json().await.unwrap();
    let api_key = body.get("api_key").and_then(|k| k.as_str()).unwrap().to_string();
    let business_id = Uuid::parse_str(body.get("id").and_then(|id| id.as_str()).unwrap()).unwrap();

    let mut headers = HeaderMap::new();
    headers.insert(
        "Authorization",
        format!("Bearer {}", api_key).parse().unwrap(),
    );
    (headers, business_id)
}

// Helper to create a customer and return their ID
async fn create_test_customer(server_url: &str, headers: &HeaderMap) -> Uuid {
    let client = reqwest::Client::new();
    let res = client
        .post(&format!("{}/customers", server_url))
        .headers(headers.clone())
        .json(&serde_json::json!({
            "name": "Jane Doe",
            "email": format!("jane.doe.{}@example.com", Uuid::new_v4())
        }))
        .send()
        .await
        .unwrap();

    assert!(res.status().is_success());
    let body: serde_json::Value = res.json().await.unwrap();
    Uuid::parse_str(body.get("id").and_then(|id| id.as_str()).unwrap()).unwrap()
}

// Helper to create an invoice and return its ID
async fn create_test_invoice(server_url: &str, headers: &HeaderMap, customer_id: Uuid) -> Uuid {
    let client = reqwest::Client::new();
    let res = client
        .post(&format!("{}/invoices", server_url))
        .headers(headers.clone())
        .json(&serde_json::json!({
            "customer_id": customer_id,
            "due_date": "2026-12-31T23:59:59Z",
            "items": [
                {
                    "description": "Item 1",
                    "quantity": 2,
                    "unit_amount_cents": 500
                }
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), reqwest::StatusCode::CREATED);
    let body: serde_json::Value = res.json().await.unwrap();
    Uuid::parse_str(body.get("id").and_then(|id| id.as_str()).unwrap()).unwrap()
}

#[tokio::test]
async fn test_concurrency_payments() {
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgrespassword@localhost:5432/dodo_payments".to_string());
    let db = PgPool::connect(&db_url).await.unwrap();

    // Setup mock PSP to count calls
    let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let call_count_clone = call_count.clone();
    let psp_url = spawn_mock_psp(Arc::new(move |_token| {
        call_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    })).await;

    let (server_url, _) = spawn_test_server(db.clone(), psp_url).await;
    let (headers, _) = setup_test_business(&server_url).await;
    let customer_id = create_test_customer(&server_url, &headers).await;
    let invoice_id = create_test_invoice(&server_url, &headers, customer_id).await;

    // Fire 10 concurrent payments for the same invoice, each with a DIFFERENT idempotency key
    let client = reqwest::Client::new();
    let mut set = tokio::task::JoinSet::new();

    for i in 0..10 {
        let client_clone = client.clone();
        let server_url_clone = server_url.clone();
        let headers_clone = headers.clone();
        let invoice_id_clone = invoice_id.clone();
        let idemp_key = format!("idemp_concurrency_{}_{}", invoice_id, i);

        set.spawn(async move {
            client_clone
                .post(&format!("{}/invoices/{}/pay", server_url_clone, invoice_id_clone))
                .headers(headers_clone)
                .header("Idempotency-Key", idemp_key)
                .json(&serde_json::json!({
                    "card_token": "tok_success"
                }))
                .send()
                .await
        });
    }

    let mut successes = 0;
    let mut conflicts = 0;

    while let Some(res) = set.join_next().await {
        let resp = res.unwrap().unwrap();
        let status = resp.status();
        if status == reqwest::StatusCode::OK {
            successes += 1;
        } else if status == reqwest::StatusCode::CONFLICT {
            conflicts += 1;
        }
    }

    // Exactly 1 payment attempt must succeed, the rest must conflict (409)
    assert_eq!(successes, 1, "Exactly one payment attempt should succeed");
    assert_eq!(conflicts, 9, "The other nine payment attempts should conflict");
    
    // Check that mock PSP was only called exactly ONCE
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1, "Mock PSP should be called exactly once");

    // Check final state of invoice in DB is paid
    let inv_status: String = sqlx::query_scalar!("SELECT status FROM invoices WHERE id = $1", invoice_id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(inv_status, "paid", "Invoice should end up in paid status");

    // Check that only 1 payment attempt is marked succeeded in DB
    let succeeded_attempts: i64 = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM payment_attempts WHERE invoice_id = $1 AND status = 'succeeded'",
        invoice_id
    )
    .fetch_one(&db)
    .await
    .unwrap()
    .unwrap();
    assert_eq!(succeeded_attempts, 1, "Only one payment attempt should succeed in database");
}

#[tokio::test]
async fn test_idempotency_payments() {
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgrespassword@localhost:5432/dodo_payments".to_string());
    let db = PgPool::connect(&db_url).await.unwrap();

    let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let call_count_clone = call_count.clone();
    let psp_url = spawn_mock_psp(Arc::new(move |_token| {
        call_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    })).await;

    let (server_url, _) = spawn_test_server(db.clone(), psp_url).await;
    let (headers, _) = setup_test_business(&server_url).await;
    let customer_id = create_test_customer(&server_url, &headers).await;
    let invoice_id = create_test_invoice(&server_url, &headers, customer_id).await;

    let client = reqwest::Client::new();
    let idempotency_key = format!("idemp_key_{}", Uuid::new_v4());

    // First attempt
    let res1 = client
        .post(&format!("{}/invoices/{}/pay", server_url, invoice_id))
        .headers(headers.clone())
        .header("Idempotency-Key", &idempotency_key)
        .json(&serde_json::json!({
            "card_token": "tok_success"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res1.status(), reqwest::StatusCode::OK);
    let body1: serde_json::Value = res1.json().await.unwrap();

    // Second attempt with SAME idempotency key
    let res2 = client
        .post(&format!("{}/invoices/{}/pay", server_url, invoice_id))
        .headers(headers.clone())
        .header("Idempotency-Key", &idempotency_key)
        .json(&serde_json::json!({
            "card_token": "tok_success"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res2.status(), reqwest::StatusCode::OK);
    let body2: serde_json::Value = res2.json().await.unwrap();

    // Verify response bodies are identical
    assert_eq!(body1, body2, "Idempotency retry responses must match exactly");

    // Verify PSP was only called ONCE
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1, "Mock PSP should be called only once");

    // Verify only 1 payment attempt exists in DB
    let total_attempts: i64 = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM payment_attempts WHERE invoice_id = $1",
        invoice_id
    )
    .fetch_one(&db)
    .await
    .unwrap()
    .unwrap();
    assert_eq!(total_attempts, 1, "Only one payment attempt should be created in total");
}

#[tokio::test]
async fn test_psp_timeout_error_handling() {
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgrespassword@localhost:5432/dodo_payments".to_string());
    let db = PgPool::connect(&db_url).await.unwrap();

    let psp_url = spawn_mock_psp(Arc::new(|_token| {})).await;
    let (server_url, _) = spawn_test_server(db.clone(), psp_url).await;
    let (headers, _) = setup_test_business(&server_url).await;
    
    // 1. Timeout Test
    let customer_id = create_test_customer(&server_url, &headers).await;
    let invoice_id = create_test_invoice(&server_url, &headers, customer_id).await;

    let client = reqwest::Client::new();
    let start = std::time::Instant::now();
    let res = client
        .post(&format!("{}/invoices/{}/pay", server_url, invoice_id))
        .headers(headers.clone())
        .header("Idempotency-Key", format!("idemp_timeout_{}", invoice_id))
        .json(&serde_json::json!({
            "card_token": "tok_timeout" // Triggers 10s sleep at PSP, client times out at 5s
        }))
        .send()
        .await
        .unwrap();

    let elapsed = start.elapsed();
    
    // Must return immediately (around 5 seconds due to our client timeout, not hanging for 10s)
    assert!(elapsed.as_secs() >= 5 && elapsed.as_secs() < 8, "Should timeout around 5s");
    assert_eq!(res.status(), reqwest::StatusCode::ACCEPTED, "Must return 202 Accepted");
    
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body.get("status").unwrap().as_str().unwrap(), "pending", "Status must be pending");

    // Verify invoice remains open and payment attempt remains pending
    let invoice_status: String = sqlx::query_scalar!("SELECT status FROM invoices WHERE id = $1", invoice_id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(invoice_status, "open", "Invoice must remain open on timeout");

    let attempt_status: String = sqlx::query_scalar!(
        "SELECT status FROM payment_attempts WHERE invoice_id = $1 ORDER BY created_at DESC LIMIT 1",
        invoice_id
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(attempt_status, "pending", "Payment attempt must remain pending on timeout");

    // 2. Network Error Test
    let invoice_id_net = create_test_invoice(&server_url, &headers, customer_id).await;
    let res_net = client
        .post(&format!("{}/invoices/{}/pay", server_url, invoice_id_net))
        .headers(headers.clone())
        .header("Idempotency-Key", format!("idemp_net_{}", invoice_id_net))
        .json(&serde_json::json!({
            "card_token": "tok_network_error"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res_net.status(), reqwest::StatusCode::ACCEPTED, "Must return 202 Accepted");
    
    let body_net: serde_json::Value = res_net.json().await.unwrap();
    assert_eq!(body_net.get("status").unwrap().as_str().unwrap(), "pending", "Status must be pending");

    // Verify invoice remains open and attempt remains pending
    let invoice_status_net: String = sqlx::query_scalar!("SELECT status FROM invoices WHERE id = $1", invoice_id_net)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(invoice_status_net, "open", "Invoice must remain open on network error");
}

#[tokio::test]
async fn test_idempotency_different_body() {
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgrespassword@localhost:5432/dodo_payments".to_string());
    let db = PgPool::connect(&db_url).await.unwrap();

    let psp_url = spawn_mock_psp(Arc::new(|_token| {})).await;
    let (server_url, _) = spawn_test_server(db.clone(), psp_url).await;
    let (headers, _) = setup_test_business(&server_url).await;
    let customer_id = create_test_customer(&server_url, &headers).await;
    let invoice_id1 = create_test_invoice(&server_url, &headers, customer_id).await;

    let client = reqwest::Client::new();
    let idempotency_key = format!("idemp_diff_body_{}", Uuid::new_v4());

    // 1. First attempt with card_token = tok_success
    let res1 = client
        .post(&format!("{}/invoices/{}/pay", server_url, invoice_id1))
        .headers(headers.clone())
        .header("Idempotency-Key", &idempotency_key)
        .json(&serde_json::json!({
            "card_token": "tok_success"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res1.status(), reqwest::StatusCode::OK);

    // 2. Second attempt with SAME key but DIFFERENT card_token
    let res2 = client
        .post(&format!("{}/invoices/{}/pay", server_url, invoice_id1))
        .headers(headers.clone())
        .header("Idempotency-Key", &idempotency_key)
        .json(&serde_json::json!({
            "card_token": "tok_card_declined"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res2.status(), reqwest::StatusCode::BAD_REQUEST);
    let body2: serde_json::Value = res2.json().await.unwrap();
    assert!(
        body2.get("error").unwrap().get("message").unwrap().as_str().unwrap().contains("Idempotency key reused with different request body"),
        "Error message should mention different request body"
    );
}

