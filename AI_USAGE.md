# AI Usage Disclosure (AI_USAGE.md)

This document describes how AI was used during the development of this project, including specific tools, manual decisions made independent of or against AI suggestions, and errors that required manual corrections.

## 1. AI Tools Used & Purpose

Initially, I used chatGPT to research about the various methods of building the project in order to have a better understanding of the technologies involved. Once the entire architecture was clear to me, I started using Antigravity IDE for the implementation of the project. 
I have used Antigravity IDE for the following tasks:
- Writing boilerplate code and following idiomatic Rust patterns
- Completing code based on the architectural decisions I have made
- Debugging and finding solutions to various problems
- Generating the integration tests
- Formulating Dockerfiles and docker-compose configurations.
- Writing a README.md based on the requirements.
- Writing the DESIGN.md based on the architectural decisions I have made.
- Added comments across the codebase to ensure clarity and understanding
- Designing and implementing request body validation for the idempotency key system to prevent key reuse with differing payloads.
---

## 2. Five Decisions Made Independently / Against AI Suggestions

### Decision 1: Removing PSP Reconciliation Complexity
- **AI Proposal**: The assistant initially proposed building a full background reconciliation worker in `invoice-service` and a corresponding `GET /payments/{id}` status-check endpoint in `mock-psp` to resolve timed-out payments (`tok_timeout`).
- **My Choice**: I chose to omit the reconciliation worker code from the codebase entirely and handle `tok_timeout` simply by timing out the HTTP client at 5 seconds, setting the attempt to `pending`, and returning `202 Accepted` with the invoice remaining `open`.
- **Reason**: The take-home assignment is designed to evaluate design judgment within a 4-6 hour window. I decided to keep the code footprint clean and minimal, avoiding unnecessary endpoints and workers, and instead fully documented the recovery and reconciliation worker design in the `DESIGN.md` document.

### Decision 2: Architectural Crate Refactoring for In-Process Integration Tests
- **AI Proposal**: The assistant initially generated all boilerplate code in `invoice-service/src/main.rs` as a single binary and suggested writing tests that would run against a running Docker container.
- **My Choice**: I refactored the project structure to split `invoice-service` into `src/lib.rs` and `src/main.rs`.
- **Reason**: This allowed us to write cargo integration tests that boot the real Axum router in-process on a random port (`127.0.0.1:0`). This makes the test suite completely self-contained, repeatable, and able to execute using a standard `cargo test` command without depending on external execution runners or pre-configured services.

### Decision 3: Dynamically Isolated Business Scope for Tests
- **AI Proposal**: The initial test template suggested performing payment attempts on a static pre-seeded business ID.
- **My Choice**: I changed the test initialization to call `POST /businesses` at the start of each integration test to create a completely new, unique business with its own API Key and auth headers.
- **Reason**: By isolating each test to a dynamically created business, we ensure that test executions never collide or pollute each other's customer and invoice records, even when running the test suite concurrently against the same Postgres database.

### Decision 4: Refactoring Mock PSP into Modules
- **AI Proposal**: Keep all mock-psp endpoint registration, server configuration, and payment simulation logic inside a single, flat `mock-psp/src/main.rs` file.
- **My Choice**: I chose to split out the route handler and its associated request/response types into a dedicated `mock-psp/src/handler.rs` module, keeping the entry point in `main.rs` clean.
- **Reason**: This keeps the server entry point focused strictly on initialization and port binding, matching the modular structure of the `invoice-service` and promoting cleaner software organization.

### Decision 5: Verifying Request Body for Cached Idempotency Key Reuses
- **AI Proposal**: The AI's initial implementation of the idempotency caching layer only verified that the `request_path` matched on a key hit, omitting checks on the request payload.
- **My Choice**: I chose to add full request body validation so that if a client reuses an idempotency key with a different body (e.g., changing the card token), the request is rejected with `400 Bad Request`.
- **Reason**: Standard idempotency key best practices require validating that a key is not reused across differing payloads to prevent erroneous transactions or cache poisoning.

---

## 3. Corrections Made to AI Suggestions

### SQLx Anonymous Record Type Mismatch
- **AI Proposal**: In the `list_invoices` handler, the AI initially wrote a single conditional expression:
  ```rust
  let rows = if let Some(status) = query.status {
      sqlx::query!("SELECT ... WHERE status = $2", ..., status).fetch_all(&db).await
  } else {
      sqlx::query!("SELECT ...", ...).fetch_all(&db).await
  }?;
  ```
- **The Issue**: In Rust, the `sqlx::query!` macro creates distinct anonymous record types for each macro expansion, even if the selected columns are identical. The compiler failed with a type mismatch because the `if` and `else` branches returned different anonymous types.
- **My Correction**: I resolved this by mapping the anonymous database records to the common `InvoiceResponse` struct *inside* each branch of the `if/else` block, returning a typed `Vec<InvoiceResponse>` instead.

### Database Migration Desynchronization
- **AI Proposal**: When the service failed to boot at startup because the `"businesses"` table already existed (caused by a dirty local Docker volume state), the AI suggested modifying the SQL migration to use `CREATE TABLE IF NOT EXISTS` to bypass the failure.
- **The Issue**: Altering the migration script masks a desynchronized migration registry (`_sqlx_migrations`), which can lead to untracked schema drifts in production and corrupt migration histories.
- **My Correction**: I chose to keep the migration script unmodified and instead clean up the database's schema state directly (dropping and recreating the `public` schema in Postgres) to ensure that the `_sqlx_migrations` table is correctly populated and synchronized with the actual database structure.

