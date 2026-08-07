# Build with the repository root as the context, so the workspace resolves:
#
#     docker build -f crates/server/Dockerfile -t feed-server .
#
# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
#
# Pinned to the machine doing the building rather than the machine being built
# for, so that building an arm64 image on an amd64 host compiles natively and
# cross-links, instead of running the entire Rust toolchain under emulation.
# That is the difference between a couple of minutes and most of an hour.
#
# Cross-compiling needs a linker for the target and the target's C library
# headers — not everything here is Rust all the way down, since some
# cryptography dependencies build a little C.
FROM --platform=$BUILDPLATFORM rust:1-slim-bookworm AS builder

# Supplied by the builder. TARGETARCH is the architecture being built for.
ARG TARGETARCH

WORKDIR /build

# Docker and Rust have different names for the same architectures.
RUN case "$TARGETARCH" in \
        amd64) echo x86_64-unknown-linux-gnu > /target ;; \
        arm64) echo aarch64-unknown-linux-gnu > /target ;; \
        *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac \
    && rustup target add "$(cat /target)"

# A cross-linker and the target's headers, needed only when the target differs
# from the host. libc6-dev-arm64-cross is not optional: without it the C parts of
# the cryptography crates fail to compile against headers that aren't there.
RUN if [ "$TARGETARCH" = "arm64" ]; then \
        apt-get update \
        && apt-get install -y --no-install-recommends \
            gcc-aarch64-linux-gnu \
            libc6-dev-arm64-cross \
        && rm -rf /var/lib/apt/lists/*; \
    fi

ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc

# Only what the server needs. The desktop app is a member of the same workspace,
# so its manifest has to be here for the workspace to resolve — but its source
# does not, and leaving it out keeps its dependencies (a bundled SQLCipher and
# OpenSSL, which take a long time to compile) out of this image entirely.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY src-tauri/Cargo.toml ./src-tauri/Cargo.toml

# Cargo works out what a package contains by looking at its files, so the app's
# manifest needs something to point at. This stub is never built.
RUN mkdir -p src-tauri/src && echo 'fn main() {}' > src-tauri/src/main.rs

RUN cargo build --release --target "$(cat /target)" -p feed-server \
    && cp "target/$(cat /target)/release/feed-server" /feed-server

# ---------------------------------------------------------------------------
# Run
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim

# Nothing else is needed. The server speaks its own encrypted protocol rather
# than TLS, and its dependencies are pure Rust, so there are no certificates or
# system libraries to install beyond what the base image already has.

# The user the server runs as. It defaults to 1000 because that is the first
# account created on most Linux hosts, so a directory you make yourself is one
# the container can already write to. If your account is not 1000, build with
#
#     docker build --build-arg UID=$(id -u) --build-arg GID=$(id -g) ...
#
# Getting this wrong is not subtle: the server exits immediately, unable to write
# its identity.
ARG UID=1000
ARG GID=1000

RUN groupadd --gid $GID feed \
    && useradd --system --uid $UID --gid $GID feed \
    && mkdir /data \
    && chown $UID:$GID /data

COPY --from=builder /feed-server /usr/local/bin/feed-server

USER feed

# Everything the server keeps lives here, so this is the directory to mount:
# the identity, and the configuration if you supply one.
WORKDIR /data
VOLUME ["/data"]

EXPOSE 4001

ENTRYPOINT ["feed-server"]
CMD ["/data/feed-server.toml"]
