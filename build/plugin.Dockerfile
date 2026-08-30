FROM rust:1.89.0-bookworm AS builder
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config gcc libc6-dev ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo test --workspace --exclude whale-widget-win
RUN cargo build --release -p cpa-whale-plugin -p cpa-whale-admin

FROM scratch AS export
COPY --from=builder /src/target/release/libcpa_whale_plugin.so /cpa-whale-plugin-linux-amd64.so
COPY --from=builder /src/target/release/cpa-whale-admin /cpa-whale-admin-linux-amd64
