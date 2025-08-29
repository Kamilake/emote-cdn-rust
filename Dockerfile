# ---- Build stage
FROM rust:trixie AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo "fn main(){}" > src/main.rs && cargo build --release
COPY src ./src
RUN touch src/main.rs && cargo build --release

# ---- Runtime stage
FROM debian:trixie-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/emoji-resizer /usr/local/bin/emoji-resizer
ENV RUST_LOG=info
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/emoji-resizer"]
