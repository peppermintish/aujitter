FROM rust:1.98.1-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY .cargo .cargo
COPY src src
COPY web web
RUN cargo build --release --locked --no-default-features --bin aujitter

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates iputils-ping && rm -rf /var/lib/apt/lists/*
RUN groupadd --gid 10001 aujitter && useradd --uid 10001 --gid 10001 --no-create-home aujitter && mkdir /data && chown 10001:10001 /data
COPY --from=builder /src/target/release/aujitter /usr/local/bin/aujitter
USER 10001:10001
ENV AUJITTER_CONTAINER=1 AUJITTER_DB=/data/history.sqlite3 AUJITTER_CONFIG=/data/settings.json
EXPOSE 9876
VOLUME ["/data"]
HEALTHCHECK --interval=30s --timeout=6s --start-period=15s --retries=3 CMD ["aujitter", "status", "--json"]
ENTRYPOINT ["aujitter"]
CMD ["run", "--bind", "0.0.0.0:9876"]
