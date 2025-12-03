FROM rust:slim AS builder
RUN apt-get update && apt-get install -y jq curl
WORKDIR /pocket-ic-launcher
COPY . .
RUN ./package.sh out
FROM debian AS runtime
RUN apt-get update && apt-get install -y ca-certificates
WORKDIR /app
COPY --from=builder /pocket-ic-launcher/out ./
STOPSIGNAL SIGINT
EXPOSE 4942/tcp 4943/tcp
ENTRYPOINT ["/app/pocket-ic-launcher", "--nns", "--ii", "--artificial-delay-ms=10", "--status-dir=/app/status", \
    "--config-port", "4942", "--gateway-port", "4943", "--bind", "0.0.0.0"]
