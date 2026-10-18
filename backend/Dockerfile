# syntax=docker/dockerfile:1.7
FROM rust:1.95-bookworm AS builder

WORKDIR /app
ARG CREWDOCK_GIT_SHA=unknown
ARG CREWDOCK_BUILD_TIME=unknown
ENV CREWDOCK_GIT_SHA=${CREWDOCK_GIT_SHA}
ENV CREWDOCK_BUILD_TIME=${CREWDOCK_BUILD_TIME}
COPY Cargo.toml Cargo.lock ./
COPY backend ./backend
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo clean -p crewdock-api \
    && cargo build --locked --release -p crewdock-api \
    && cp /app/target/release/crewdock-api /usr/local/bin/crewdock-api

FROM debian:bookworm-slim

WORKDIR /app
ARG DEBIAN_MIRROR=http://deb.debian.org/debian
ARG DEBIAN_SECURITY_MIRROR=http://deb.debian.org/debian-security
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
RUN sed -i \
        -e "s|http://deb.debian.org/debian-security|${DEBIAN_SECURITY_MIRROR}|g" \
        -e "s|http://deb.debian.org/debian|${DEBIAN_MIRROR}|g" \
        /etc/apt/sources.list.d/debian.sources \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        fonts-noto-cjk \
        libreoffice-writer \
        poppler-utils \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/bin/crewdock-api /usr/local/bin/crewdock-api
COPY --from=builder /app/backend/migrations ./migrations

EXPOSE 16888
CMD ["crewdock-api"]
