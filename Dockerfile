ARG RUST_IMAGE=rust:1-bookworm
FROM ${RUST_IMAGE} AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY content ./content
RUN cargo build --locked --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install --yes --no-install-recommends openssh-client \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 portfolio \
    && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin portfolio \
    && mkdir /data \
    && chown 10001:10001 /data
COPY --from=builder /build/target/release/personal-portfolio-tui /usr/local/bin/personal-portfolio-tui
USER 10001:10001
WORKDIR /data
STOPSIGNAL SIGINT
EXPOSE 2222
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD ssh-keyscan -T 3 -p 2222 -t ed25519 127.0.0.1 2>/dev/null | grep -q ssh-ed25519
ENTRYPOINT ["/usr/local/bin/personal-portfolio-tui"]
CMD ["serve", "--bind", "0.0.0.0:2222", "--host-key", "/data/ssh_host_ed25519_key", "--max-sessions", "32", "--idle-timeout", "300", "--content-url", "https://www.aidanjeon.com/portfolio.json", "--max-per-ip", "3", "--max-session-secs", "1800"]
