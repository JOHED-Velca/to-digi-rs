FROM rust:1-bookworm AS builder

ARG TO_DIGI_RS_RELEASE_IMAGE=""
ARG TO_DIGI_RS_GIT_REVISION=""
ENV TO_DIGI_RS_RELEASE_IMAGE=${TO_DIGI_RS_RELEASE_IMAGE}
ENV TO_DIGI_RS_GIT_REVISION=${TO_DIGI_RS_GIT_REVISION}

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY deploy ./deploy
COPY profiles ./profiles

RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates mdbtools \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/to-digi-rs /usr/local/bin/to-digi-rs

WORKDIR /work
ENTRYPOINT ["/usr/local/bin/to-digi-rs"]
