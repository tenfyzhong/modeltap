FROM rust:1.85-bookworm AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY benches ./benches
RUN cargo build --release --locked

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home modeltap

COPY --from=builder /app/target/release/modeltap /usr/local/bin/modeltap

USER modeltap
EXPOSE 2080
ENTRYPOINT ["modeltap"]
CMD ["--help"]
