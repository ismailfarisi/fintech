# syntax=docker/dockerfile:1
# Multi-stage build: frontend -> backend -> minimal non-root runtime image.

FROM node:22-alpine AS web
WORKDIR /web
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY frontend/ ./
RUN npm run build

FROM rust:1.97-slim-trixie AS build
WORKDIR /src
COPY backend/ ./
# Migrations are embedded into the binary at compile time.
RUN cargo build --release --locked && strip target/release/fintech-backend

FROM debian:trixie-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --no-create-home --shell /usr/sbin/nologin ledger
WORKDIR /app
COPY --from=build /src/target/release/fintech-backend /app/ledger
COPY --from=build /src/markets.toml /app/markets.toml
COPY --from=web /web/dist /app/static
ENV BIND_ADDR=0.0.0.0:8080 \
    STATIC_DIR=/app/static \
    MARKETS_CONFIG=/app/markets.toml \
    LOG_FORMAT=json
USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["/app/ledger"]
