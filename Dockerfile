# Builds the WASM client and the server into one image.
FROM rust:1.98-bookworm AS build
RUN rustup target add wasm32-unknown-unknown
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY .cargo .cargo
COPY core core
COPY server server
COPY web web
# wasm-bindgen must match the version pinned in Cargo.lock.
RUN cargo install wasm-bindgen-cli --locked --version \
    "$(grep -A1 '^name = "wasm-bindgen"$' Cargo.lock | sed -n 's/^version = "\(.*\)"$/\1/p')"
RUN web/build.sh
RUN cargo build --release --locked -p mm-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home mm
COPY --from=build /src/target/release/mm-server /usr/local/bin/mm-server
COPY --from=build /src/web/dist /app/web
USER mm
ENV STATIC_DIR=/app/web LISTEN=0.0.0.0:8080 RUST_LOG=info,sqlx=warn
EXPOSE 8080
CMD ["mm-server"]
