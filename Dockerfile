# Build the release binary inside an isolated Rust image.
FROM rust:1-slim AS build
WORKDIR /app
COPY . .
RUN cargo build --release --locked

# Keep the runtime image minimal; the binary links libc only.
FROM debian:bookworm-slim
COPY --from=build /app/target/release/proxyforge /usr/local/bin/proxyforge
ENTRYPOINT ["proxyforge"]
