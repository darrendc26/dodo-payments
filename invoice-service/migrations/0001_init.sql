-- Enable UUID extension
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- 1. Businesses
CREATE TABLE businesses (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name VARCHAR(255) NOT NULL,
    api_key_hash VARCHAR(64) NOT NULL UNIQUE, -- SHA-256 hash of API key
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 2. Customers
CREATE TABLE customers (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    business_id UUID NOT NULL REFERENCES businesses(id) ON DELETE CASCADE,
    name VARCHAR(255) NOT NULL,
    email VARCHAR(255) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (business_id, email)
);
CREATE INDEX idx_customers_business_id ON customers(business_id);

-- 3. Invoices
CREATE TABLE invoices (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    business_id UUID NOT NULL REFERENCES businesses(id) ON DELETE CASCADE,
    customer_id UUID NOT NULL REFERENCES customers(id) ON DELETE RESTRICT,
    status VARCHAR(50) NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'open', 'paid', 'void', 'uncollectible')),
    amount_cents BIGINT NOT NULL CHECK (amount_cents >= 0),
    due_date TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_invoices_business_id_status ON invoices(business_id, status);
CREATE INDEX idx_invoices_customer_id ON invoices(customer_id);

-- 4. Invoice Items
CREATE TABLE invoice_items (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    invoice_id UUID NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    description VARCHAR(255) NOT NULL,
    quantity INTEGER NOT NULL CHECK (quantity > 0),
    unit_amount_cents BIGINT NOT NULL CHECK (unit_amount_cents >= 0)
);
CREATE INDEX idx_invoice_items_invoice_id ON invoice_items(invoice_id);

-- 5. Payment Attempts
CREATE TABLE payment_attempts (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    invoice_id UUID NOT NULL REFERENCES invoices(id) ON DELETE RESTRICT,
    status VARCHAR(50) NOT NULL CHECK (status IN ('pending', 'succeeded', 'failed')),
    card_token VARCHAR(255) NOT NULL,
    psp_reference VARCHAR(255) UNIQUE,
    error_code VARCHAR(100),
    idempotency_key VARCHAR(255), -- linked idempotency key for retry mapping
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_payment_attempts_invoice_id ON payment_attempts(invoice_id);

-- 6. Idempotency Keys
CREATE TABLE idempotency_keys (
    idempotency_key VARCHAR(255) NOT NULL,
    business_id UUID NOT NULL REFERENCES businesses(id) ON DELETE CASCADE,
    request_path VARCHAR(255) NOT NULL,
    request_body TEXT,
    response_status SMALLINT NOT NULL,
    response_body TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (business_id, idempotency_key)
);

-- 7. Webhook Endpoints
CREATE TABLE webhook_endpoints (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    business_id UUID NOT NULL REFERENCES businesses(id) ON DELETE CASCADE,
    url VARCHAR(1024) NOT NULL,
    secret VARCHAR(255) NOT NULL, -- Signing key
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_webhook_endpoints_business_id ON webhook_endpoints(business_id);

-- 8. Webhook Deliveries (Transactional Outbox)
CREATE TABLE webhook_deliveries (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    business_id UUID NOT NULL REFERENCES businesses(id) ON DELETE CASCADE,
    endpoint_id UUID NOT NULL REFERENCES webhook_endpoints(id) ON DELETE CASCADE,
    event_type VARCHAR(100) NOT NULL,
    payload JSONB NOT NULL,
    status VARCHAR(50) NOT NULL CHECK (status IN ('pending', 'succeeded', 'failed')),
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_webhook_deliveries_status_next_attempt ON webhook_deliveries(status, next_attempt_at);

-- Seed default business for testing
-- The API Key is 'dodo_sk_test_123456789'
-- SHA-256 hash is '74da007ee7e75d9ec1a66770149b71360cddde795beb4f3c51daaf4aaef85be4'
INSERT INTO businesses (id, name, api_key_hash) VALUES (
    '00000000-0000-0000-0000-000000000001',
    'Dodo Store Inc.',
    '74da007ee7e75d9ec1a66770149b71360cddde795beb4f3c51daaf4aaef85be4'
) ON CONFLICT DO NOTHING;
