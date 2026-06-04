# Invoice & Payment Service (Dodo Payments Take-Home)

This repository contains a Cargo-workspace based Rust implementation of a minimal Invoice & Payment Service. It is built using Axum, PostgreSQL, SQLx, and Docker.

## Demo Video
Please find the unscripted video walkthrough here:
**[Loom Walkthrough Video](https://loom.com/share/placeholder-video-link-dodo-payments-darren)**

---

## Architecture Overview

The system consists of two primary services and a database running inside Docker Compose:
1. **`invoice-service`**: Exposes the main API for managing businesses, customers, invoices, and payments, along with an outbox webhook delivery worker.
2. **`mock-psp`**: Intercepts payment processor transactions and simulates outcomes based on card tokens.
3. **`db`**: A PostgreSQL 15 database instance storing schemas and states.

---

## Setup & Run Instructions

To run the application, database, and mock PSP, simply execute:

```bash
docker compose up --build
```

This brings up:
- `invoice-service` on port `8080`
- `mock-psp` on port `8081`
- `postgres` on port `5432`

Migrations are executed automatically on application startup.

### Running Integration Tests
To run the automated integration tests locally (requires a running postgres database):
```bash
DATABASE_URL=postgres://postgres:postgrespassword@localhost:5432/dodo_payments cargo test
```

---

## API curl Examples

> [!NOTE]
> All API Keys and UUIDs (such as customer and invoice IDs) in the examples below are **placeholders**. You must replace them with the actual keys and IDs returned by the preceding steps.

### 1. Create a Business
Register a new business to generate your live API key:

```bash
curl -X POST http://localhost:8080/businesses \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Darren's Store"
  }'
```

**Expected Response**:
```json
{
  "id": "00000000-0000-0000-0000-000000000001",
  "name": "Darren's Store",
  "api_key": "[ENCRYPTION_KEY]"
}
```

---

### 2. Create a Customer
Create a new customer under your business:

```bash
curl -X POST http://localhost:8080/customers \
  -H "Authorization: Bearer <your_business_api_key>" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Alice Developer",
    "email": "alice.dev@example.com"
  }'
```

**Expected Response**:
```json
{
  "id": "e93fca1a-e8d1-4475-8025-a4b5952db509",
  "name": "Alice Developer",
  "email": "alice.dev@example.com",
  "created_at": "2026-06-03T14:31:00Z"
}
```

---

### 3. Create an Invoice
Create an invoice with line items (the server computes the total cents automatically):

```bash
curl -X POST http://localhost:8080/invoices \
  -H "Authorization: Bearer <your_business_api_key>" \
  -H "Content-Type: application/json" \
  -d '{
    "customer_id": "<your_customer_id>",
    "due_date": "2026-12-31T23:59:59Z",
    "items": [
      {
        "description": "Premium Subscription Monthly",
        "quantity": 1,
        "unit_amount_cents": 2900
      },
      {
        "description": "Setup Fee",
        "quantity": 2,
        "unit_amount_cents": 1000
      }
    ]
  }'
```

**Expected Response**:
```json
{
  "id": "76495dbf-1c4c-4ebc-8822-0ef3d7c57d76",
  "customer_id": "e93fca1a-e8d1-4475-8025-a4b5952db509",
  "status": "open",
  "amount_cents": 4900,
  "due_date": "2026-12-31T23:59:59Z",
  "created_at": "2026-06-03T14:31:05Z",
  "updated_at": "2026-06-03T14:31:05Z",
  "items": [
    {
      "id": "402eb0ab-f018-47bc-ad3e-862d7c07da01",
      "description": "Premium Subscription Monthly",
      "quantity": 1,
      "unit_amount_cents": 2900
    },
    {
      "id": "e446ef4c-473d-4c31-97b7-6f81e285d89f",
      "description": "Setup Fee",
      "quantity": 2,
      "unit_amount_cents": 1000
    }
  ]
}
```

---

### 4. Attempt Payment (Success Case)
Attempt paying the invoice using `tok_success` card token (uses `Idempotency-Key` header):

```bash
curl -X POST http://localhost:8080/invoices/<your_invoice_id>/pay \
  -H "Authorization: Bearer <your_business_api_key>" \
  -H "Idempotency-Key: idemp_success_example_1" \
  -H "Content-Type: application/json" \
  -d '{
    "card_token": "tok_success"
  }'
```

**Expected Response**:
```json
{
  "status": "succeeded",
  "payment_attempt_id": "847253bd-9ca1-482a-aef2-bc3251bdca01",
  "psp_reference": "0e59a84f-770f-488f-9bc3-cd7253ad509e",
  "error_code": null
}
```

---

### 5. Attempt Payment (Failure/Decline Case)
Attempt paying another invoice (create one first) using `tok_card_declined` token:

```bash
curl -X POST http://localhost:8080/invoices/<your_invoice_id>/pay \
  -H "Authorization: Bearer <your_business_api_key>" \
  -H "Idempotency-Key: idemp_decline_example_1" \
  -H "Content-Type: application/json" \
  -d '{
    "card_token": "tok_card_declined"
  }'
```

**Expected Response**:
```json
{
  "status": "failed",
  "payment_attempt_id": "905bd0c1-da42-498c-8fef-97b7cbcd253c",
  "psp_reference": null,
  "error_code": "card_declined"
}
```

---

### 6. Register Webhook Endpoint
Configure a URL to receive signed webhook events (`invoice.created`, `invoice.paid`, etc.):

```bash
curl -X POST http://localhost:8080/webhooks/endpoints \
  -H "Authorization: Bearer <your_business_api_key>" \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://httpbin.org/post"
  }'
```

**Expected Response**:
```json
{
  "id": "e4587db1-0941-4cfa-9bc3-cd4852abdc76",
  "url": "https://httpbin.org/post",
  "secret": "whsec_3b290941bd564c4f9e79bb7d11f71df4",
  "created_at": "2026-06-03T14:32:00Z"
}
```

