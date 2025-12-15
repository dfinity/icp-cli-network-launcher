FROM rust:1.90.0-slim-trixie AS chef
RUN apt-get update && apt-get install -y jq curl
WORKDIR /app
RUN cargo install cargo-chef --version 0.1.73 --locked
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN ./package.sh out
FROM debian:trixie-slim AS runtime
RUN apt-get update && apt-get install -y ca-certificates
WORKDIR /app
COPY --from=builder /app/out ./
STOPSIGNAL SIGINT
EXPOSE 4942/tcp 4943/tcp
ENTRYPOINT ["/app/icp-cli-network-launcher", "--status-dir=/app/status", \
    "--config-port", "4942", "--gateway-port", "4943", "--bind", "0.0.0.0"]
