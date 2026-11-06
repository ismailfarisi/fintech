# Security

This document covers the threat model, the controls in place, and what still needs doing before handling real money or exposing the app publicly.

## Reporting

Please report vulnerabilities privately to the maintainer rather than opening a public issue.

## Controls

### Authentication & sessions
- **Passwords:**
  - Hashed with Argon2id (m=19 MiB, t=2, p=1, the OWASP baseline) on a blocking thread pool.
  - Policy follows NIST SP 800-63B: 12–128 characters, no single repeated character, must not contain the email name, small common-password list. No composition rules.
- **Timing and enumeration:**
  - Unknown emails still run a full Argon2 verification against a dummy hash, so response time doesn't reveal whether an account exists.
  - Login failures return one generic message.
- **Brute force:**
  - Per-IP limit: 10 attempts/minute.
  - Per-email limit: 5 per 5 minutes.
  - Account lockout: 15 minutes after 5 consecutive failures.
  - All attempts are audited.
- **Sessions:**
  - 256-bit random tokens from the OS CSPRNG. Only their SHA-256 is stored, so a database leak can't be replayed.
  - The cookie is `__Host-ledger_session`, with `HttpOnly`, `Secure`, `SameSite=Strict` and `Path=/`.
  - Expiry: 30-minute idle timeout and 12-hour absolute lifetime (both configurable).
  - Changing the password signs out all other sessions. Users can see and revoke their sessions.

### Request hardening
- **CSRF:**
  - The `SameSite=Strict` cookie, plus an Origin check: state-changing requests must carry an allowed `Origin`/`Referer`.
  - Cookie-bearing requests with neither header are refused, and so are requests with duplicate `Origin` headers.
- **WebSocket hijacking:** the upgrade requires a valid session, an allowed `Origin` and ownership of the account. The session is re-validated every minute.
- **Rate limits:** 20 requests/second per IP (burst 120) on the API.
  - `X-Forwarded-For` is only trusted when `TRUST_PROXY_HEADERS=true`.
  - The limiter is in-memory, i.e. per instance.
- **Limits:** 16 KiB request bodies and a 30-second request timeout. Paging is capped at 200 items, the watchlist at 100 symbols, and WebSocket messages at 4 KiB.
- **Strict input:** unknown JSON fields are rejected. Money must be a decimal string (floats are rejected), with bounds on quantities, prices and amounts.
- **Headers:**
  - A strict CSP (`default-src 'self'`, no inline scripts, `frame-ancestors 'none'`), plus HSTS, `nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy`, `Permissions-Policy` and COOP/CORP.
  - API responses also carry `Cache-Control: no-store`.
- **Errors:** a stable `{error: {code, message}}` shape. Internal errors are logged with a request id and returned as a generic 500.

### Authorization & data
- **Tenant isolation:** every account-scoped route calls `get_owned(user, account)`, and the repositories filter by owner. Someone else's account returns **404**, indistinguishable from one that doesn't exist. Integration tests check this for every route.
- **SQL injection:** all SQL is static with bound parameters. sqlx 0.9 refuses dynamically built query strings.
- **Integrity:**
  - A balanced double-entry ledger, non-negative balances, and append-only ledger, trades and audit log, all enforced by the database (see `docs/ARCHITECTURE.md`).
  - Idempotency keys prevent duplicate execution.
- **Audit trail:** sign-ups, logins (successful, failed and blocked), password and session changes, orders, fills and cash movements are recorded with IP and user agent. Users can see their own log.

### Secrets & operations
- Configuration comes from the environment (`.env` in development, never committed). Secrets are wrapped in a `Secret<T>` type that never prints, and passwords live in a redacted `Password` type.
- The container runs as a non-root user (UID 10001) with a read-only filesystem, no capabilities and `no-new-privileges`. Compose binds ports to loopback only.
- **CI checks:**
  - `cargo clippy -D warnings` and `cargo fmt`
  - integration tests against Postgres
  - `cargo deny`: RustSec advisories, license allow-list, crates.io as the only source
  - `npm audit`

## Before going to production

- [ ] Serve only over HTTPS (TLS at a reverse proxy). Keep `COOKIE_SECURE=true` and set `PUBLIC_ORIGIN` to the https URL.
- [ ] Use a dedicated database role with least privilege, and TLS to the database (`?sslmode=verify-full`). Take backups.
- [ ] Add a shared rate limiter (proxy or Redis) when running more than one instance, and replace the in-process event bus.
- [ ] Add email verification and password reset, and offer TOTP 2FA (required before real money).
- [ ] Pin GitHub Actions to commit SHAs and enable Dependabot or Renovate.
- [ ] Add centralised log shipping and alerting on `auth.login_blocked`, 5xx rates and reconciliation mismatches.
- [ ] Get an external penetration test.

## Out of scope today

Real-money execution, KYC/AML, and regulatory reporting. The app trades paper money against live prices.
