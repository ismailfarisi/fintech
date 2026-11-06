-- Ledger schema.
--
-- Money invariants are enforced by the database, not just the application:
--   * every ledger entry is balanced per asset (deferred constraint trigger)
--   * holdings can never go negative, and reservations never exceed holdings
--   * the ledger, trades and audit log are append-only (UPDATE/DELETE/TRUNCATE raise)
-- Amounts are NUMERIC (exact decimal). No floating point anywhere.

-- ---------------------------------------------------------------------------
-- Identity & sessions
-- ---------------------------------------------------------------------------

CREATE TABLE users (
    id                  UUID PRIMARY KEY,
    email               TEXT NOT NULL,
    email_normalized    TEXT NOT NULL UNIQUE,
    display_name        TEXT NOT NULL,
    password_hash       TEXT NOT NULL,
    failed_login_count  INTEGER NOT NULL DEFAULT 0 CHECK (failed_login_count >= 0),
    locked_until        TIMESTAMPTZ,
    disabled_at         TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (length(email) BETWEEN 3 AND 254),
    CHECK (length(display_name) BETWEEN 1 AND 100)
);

-- Only a SHA-256 of the session token is stored, so a database leak does not
-- leak usable sessions.
CREATE TABLE sessions (
    id              UUID PRIMARY KEY,
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash      BYTEA NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at      TIMESTAMPTZ NOT NULL,
    revoked_at      TIMESTAMPTZ,
    ip              TEXT,
    user_agent      TEXT
);
CREATE INDEX sessions_user_idx ON sessions (user_id) WHERE revoked_at IS NULL;

-- ---------------------------------------------------------------------------
-- Accounts
-- ---------------------------------------------------------------------------

CREATE TABLE accounts (
    id              UUID PRIMARY KEY,
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    name            TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    kind            TEXT NOT NULL CHECK (kind IN ('paper')),
    base_currency   TEXT NOT NULL DEFAULT 'USD' CHECK (base_currency = 'USD'),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    closed_at       TIMESTAMPTZ
);
CREATE INDEX accounts_user_idx ON accounts (user_id);

-- ---------------------------------------------------------------------------
-- Double-entry ledger
-- ---------------------------------------------------------------------------

CREATE TABLE ledger_entries (
    id              UUID PRIMARY KEY,
    account_id      UUID NOT NULL REFERENCES accounts(id),
    kind            TEXT NOT NULL CHECK (kind IN ('deposit', 'withdrawal', 'trade')),
    reference_id    UUID,
    description     TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ledger_entries_account_idx ON ledger_entries (account_id, created_at);

-- book: 'account'  = the customer's account (mirrored in `holdings`)
--       'external' = money entering/leaving the system (deposits, withdrawals)
--       'market'   = trade counterparty
--       'fees'     = fee income
CREATE TABLE postings (
    id              BIGSERIAL PRIMARY KEY,
    entry_id        UUID NOT NULL REFERENCES ledger_entries(id),
    account_id      UUID NOT NULL REFERENCES accounts(id),
    book            TEXT NOT NULL CHECK (book IN ('account', 'external', 'market', 'fees')),
    asset           TEXT NOT NULL CHECK (length(asset) BETWEEN 1 AND 32),
    amount          NUMERIC(38, 8) NOT NULL CHECK (amount <> 0)
);
CREATE INDEX postings_entry_idx ON postings (entry_id);
CREATE INDEX postings_account_asset_idx ON postings (account_id, asset) WHERE book = 'account';

CREATE FUNCTION check_entry_balanced() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM postings WHERE entry_id = NEW.entry_id
        GROUP BY asset HAVING sum(amount) <> 0
    ) THEN
        RAISE EXCEPTION 'ledger entry % is not balanced', NEW.entry_id
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NULL;
END $$;

-- Checked at COMMIT, after all postings of the entry are inserted.
CREATE CONSTRAINT TRIGGER postings_balanced
    AFTER INSERT ON postings
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION check_entry_balanced();

-- Current balances (a cache of `account`-book postings, kept in the same
-- transaction). `reserved` is held by open orders; `cost_basis` is the total
-- USD cost of the position for P&L.
CREATE TABLE holdings (
    account_id      UUID NOT NULL REFERENCES accounts(id),
    asset           TEXT NOT NULL CHECK (length(asset) BETWEEN 1 AND 32),
    quantity        NUMERIC(38, 8) NOT NULL DEFAULT 0 CHECK (quantity >= 0),
    reserved        NUMERIC(38, 8) NOT NULL DEFAULT 0 CHECK (reserved >= 0),
    cost_basis      NUMERIC(38, 8) NOT NULL DEFAULT 0 CHECK (cost_basis >= 0),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account_id, asset),
    CHECK (reserved <= quantity)
);

-- ---------------------------------------------------------------------------
-- Orders & trades
-- ---------------------------------------------------------------------------

CREATE TABLE orders (
    id              UUID PRIMARY KEY,
    account_id      UUID NOT NULL REFERENCES accounts(id),
    symbol          TEXT NOT NULL CHECK (length(symbol) BETWEEN 1 AND 32),
    side            TEXT NOT NULL CHECK (side IN ('buy', 'sell')),
    kind            TEXT NOT NULL CHECK (kind IN ('market', 'limit')),
    quantity        NUMERIC(38, 8) NOT NULL CHECK (quantity > 0),
    limit_price     NUMERIC(38, 8) CHECK (limit_price > 0),
    status          TEXT NOT NULL CHECK (status IN ('open', 'filled', 'cancelled')),
    -- Amount held in `holdings.reserved` while open (USD for buys, units for sells).
    reserved        NUMERIC(38, 8) NOT NULL DEFAULT 0 CHECK (reserved >= 0),
    fill_price      NUMERIC(38, 8),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    filled_at       TIMESTAMPTZ,
    cancelled_at    TIMESTAMPTZ,
    CHECK ((kind = 'limit') = (limit_price IS NOT NULL)),
    CHECK ((status = 'filled') = (fill_price IS NOT NULL AND filled_at IS NOT NULL)),
    CHECK (status = 'open' OR reserved = 0)
);
CREATE INDEX orders_account_idx ON orders (account_id, created_at DESC);
CREATE INDEX orders_open_symbol_idx ON orders (symbol, side, limit_price) WHERE status = 'open';

CREATE TABLE trades (
    id              UUID PRIMARY KEY,
    order_id        UUID NOT NULL UNIQUE REFERENCES orders(id),
    account_id      UUID NOT NULL REFERENCES accounts(id),
    ledger_entry_id UUID NOT NULL REFERENCES ledger_entries(id),
    symbol          TEXT NOT NULL,
    side            TEXT NOT NULL CHECK (side IN ('buy', 'sell')),
    quantity        NUMERIC(38, 8) NOT NULL CHECK (quantity > 0),
    price           NUMERIC(38, 8) NOT NULL CHECK (price > 0),
    gross           NUMERIC(38, 8) NOT NULL CHECK (gross >= 0),
    fee             NUMERIC(38, 8) NOT NULL DEFAULT 0 CHECK (fee >= 0),
    realized_pnl    NUMERIC(38, 8),
    executed_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((side = 'sell') = (realized_pnl IS NOT NULL))
);
CREATE INDEX trades_account_idx ON trades (account_id, executed_at DESC);

-- ---------------------------------------------------------------------------
-- Supporting tables
-- ---------------------------------------------------------------------------

-- Lets clients safely retry POSTs: the same key returns the original response.
CREATE TABLE idempotency_keys (
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    key             TEXT NOT NULL CHECK (length(key) BETWEEN 8 AND 128),
    request_hash    BYTEA NOT NULL,
    response_status SMALLINT NOT NULL,
    response_body   JSONB NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, key)
);

CREATE TABLE audit_log (
    id              BIGSERIAL PRIMARY KEY,
    at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    user_id         UUID,
    account_id      UUID,
    action          TEXT NOT NULL,
    ip              TEXT,
    user_agent      TEXT,
    details         JSONB NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX audit_log_user_idx ON audit_log (user_id, at DESC);

CREATE TABLE equity_snapshots (
    account_id      UUID NOT NULL REFERENCES accounts(id),
    at              TIMESTAMPTZ NOT NULL,
    equity          NUMERIC(38, 8) NOT NULL,
    PRIMARY KEY (account_id, at)
);

CREATE TABLE watchlist (
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    symbol          TEXT NOT NULL CHECK (length(symbol) BETWEEN 1 AND 32),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, symbol)
);

-- ---------------------------------------------------------------------------
-- Append-only enforcement
-- ---------------------------------------------------------------------------

CREATE FUNCTION forbid_mutation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION '% is append-only', TG_TABLE_NAME USING ERRCODE = 'insufficient_privilege';
END $$;

CREATE TRIGGER ledger_entries_append_only BEFORE UPDATE OR DELETE ON ledger_entries
    FOR EACH ROW EXECUTE FUNCTION forbid_mutation();
CREATE TRIGGER ledger_entries_no_truncate BEFORE TRUNCATE ON ledger_entries
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_mutation();
CREATE TRIGGER postings_append_only BEFORE UPDATE OR DELETE ON postings
    FOR EACH ROW EXECUTE FUNCTION forbid_mutation();
CREATE TRIGGER postings_no_truncate BEFORE TRUNCATE ON postings
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_mutation();
CREATE TRIGGER trades_append_only BEFORE UPDATE OR DELETE ON trades
    FOR EACH ROW EXECUTE FUNCTION forbid_mutation();
CREATE TRIGGER trades_no_truncate BEFORE TRUNCATE ON trades
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_mutation();
CREATE TRIGGER audit_log_append_only BEFORE UPDATE OR DELETE ON audit_log
    FOR EACH ROW EXECUTE FUNCTION forbid_mutation();
CREATE TRIGGER audit_log_no_truncate BEFORE TRUNCATE ON audit_log
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_mutation();
