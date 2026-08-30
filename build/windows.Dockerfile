FROM rust:1.89.0-bookworm AS builder
RUN apt-get update && apt-get install -y --no-install-recommends mingw-w64 ca-certificates && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-pc-windows-gnu
WORKDIR /src
COPY . .
RUN cargo build --release -p whale-widget-win --target x86_64-pc-windows-gnu

FROM scratch AS export
COPY --from=builder /src/target/x86_64-pc-windows-gnu/release/cpa-whale.exe /cpa-whale-windows-x64.exe
