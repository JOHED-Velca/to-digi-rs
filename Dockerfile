FROM rust:1-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates mdbtools \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/to-digi-rs /usr/local/bin/to-digi-rs

WORKDIR /work
ENTRYPOINT ["/usr/local/bin/to-digi-rs"]
