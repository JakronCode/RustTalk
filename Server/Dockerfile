# syntax=docker/dockerfile:1@sha256:4edf897a3ffa55b89f906fc8cc78afdb3f1834cc9c7083565e611a8a7d5fe99e
# RustTalk server image. Build from the repository root:
#   docker build -f Server/Dockerfile -t rusttalk-server .
# Multi-platform: docker buildx build --platform linux/amd64,linux/arm64 -f Server/Dockerfile .

# The builder runs on the build machine and cross-compiles for the target, so no emulation is needed.
# Debian 13 (trixie) here and in the runtime image: same glibc.
FROM --platform=$BUILDPLATFORM rust:1.99.1-slim-trixie@sha256:f6c4a35d21dbb3c89d7a8d056c193e94fc09037f2b58dd858b09c39a47dd02d7 AS build
ARG TARGETARCH
ARG BUILDARCH
RUN set -eu; \
    case "$TARGETARCH" in amd64) arch=x86_64 ;; arm64) arch=aarch64 ;; *) echo "unsupported platform: $TARGETARCH" >&2; exit 1 ;; esac; \
    echo "$arch-unknown-linux-gnu" > /rust-target; \
    if [ "$TARGETARCH" != "$BUILDARCH" ]; then \
        apt-get update; \
        apt-get install -y --no-install-recommends "gcc-$(echo "$arch" | tr _ -)-linux-gnu" "libc6-dev-$TARGETARCH-cross"; \
        rm -rf /var/lib/apt/lists/*; \
    fi; \
    rustup target add "$arch-unknown-linux-gnu"

WORKDIR /src
# Only what the server needs. The client stays a workspace member (Cargo.lock lists it)
# but gets an empty stub, so none of its native dependencies are fetched or built.
COPY Cargo.toml Cargo.lock ./
COPY shared/Cargo.toml shared/
COPY shared/src shared/src
COPY Server/Cargo.toml Server/
COPY Server/src Server/src
COPY Client/Windows/Cargo.toml Client/Windows/
RUN mkdir -p Client/Windows/src && echo 'fn main() {}' > Client/Windows/src/main.rs

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target,id=rusttalk-target-$TARGETARCH \
    set -eu; \
    target="$(cat /rust-target)"; \
    if [ "$TARGETARCH" != "$BUILDARCH" ]; then \
        cc="${target%%-*}-linux-gnu-gcc"; \
        export "CARGO_TARGET_$(echo "$target" | tr a-z- A-Z_)_LINKER=$cc" "CC_$(echo "$target" | tr - _)=$cc"; \
    fi; \
    cargo build --locked --release -p rusttalk-server --bin rusttalk-server --target "$target"; \
    mkdir -p /out/data; \
    cp "target/$target/release/rusttalk-server" /out/rusttalk-server

# Distroless: glibc + libgcc + CA certificates, no shell, no package manager, user 65532.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:cc66d222f0a7ec996b78a99fe76a969bc80b539a5016ad866c14d1949bc6d0ab
LABEL org.opencontainers.image.title="rusttalk-server" \
      org.opencontainers.image.description="RustTalk voice chat server" \
      org.opencontainers.image.source="https://github.com/rusttalkcrew/RustTalk"
# Root-owned program (the server cannot replace itself); the state volume belongs to the server user.
COPY --from=build --chmod=755 /out/rusttalk-server /usr/local/bin/rusttalk-server
COPY --from=build --chown=65532:65532 --chmod=700 /out/data /data
USER 65532:65532
WORKDIR /data
ENV RUSTTALK_CONTAINER=1 \
    RUSTTALK_ADMIN_PASSWORD_FILE=/run/secrets/rusttalk_admin_password
VOLUME ["/data"]
EXPOSE 7331/tcp 7331/udp
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --start-interval=2s --retries=3 \
    CMD ["/usr/local/bin/rusttalk-server", "--healthcheck", "/data/server.toml"]
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/rusttalk-server"]
CMD ["/data/server.toml"]
