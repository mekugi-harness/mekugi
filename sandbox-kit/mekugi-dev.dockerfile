# syntax=docker/dockerfile:1

# Ubuntu 26.04 LTS development environment workload for Mekugi (Go + Rust/V8 +
# Bun/Node + Just + uv + linters + Docker Engine), built for Docker Sandboxes
# v3 kits.
#
# Built directly from digest-pinned Ubuntu 26.04 LTS supplying the sbx@1
# platform floor (bash, non-root `agent` UID/GID 1000 with passwordless sudo,
# git, CA store, /etc/sandbox-persistent.sh) and Docker Engine from Docker's
# Ubuntu repository.
ARG BASE_IMAGE=ubuntu:26.04@sha256:d66b366dac61b575bee2aef07e9c26e8070096fbe84a3d4d3eb0da069b5d76db
FROM ${BASE_IMAGE}

ARG BASE_IMAGE
ARG TARGETARCH
ARG GO_VERSION=1.27.3
ARG RUST_TOOLCHAIN=1.99.1
ARG NODE_VERSION=26.13.1
ARG BUN_VERSION=1.4.4
ARG JUST_VERSION=1.51.0
ARG GOLANGCI_LINT_VERSION=2.14.0
ARG DEADCODE_VERSION=0.51.0
ARG SCCACHE_VERSION=0.19.0
ARG NEXTEST_VERSION=0.9.152
ARG UV_VERSION=0.13.10

# 1. Platform floor, Docker Engine, and Mekugi native/Go build dependencies.
#    Matches .github/workflows/release.yml and docs/contributing/development.md:
#    - build-essential, clang, libclang-dev, cmake: CGO (tree-sitter), BoringSSL, bindgen
#    - pkg-config, libcap-dev, libssl-dev, mold: codex-rs / codex-bwrap / static OpenSSL / fast linking
#    - python3, python3-venv: runtime/build.py and scripts/format.py
#    - ripgrep: interpreter/query test fixtures and code search
#    - docker-ce + plugins: optional DinD via dockerd@1
RUN set -eux; \
    export DEBIAN_FRONTEND=noninteractive; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
      bash \
      build-essential \
      ca-certificates \
      clang \
      cmake \
      curl \
      git \
      iproute2 \
      iptables \
      jq \
      libcap-dev \
      libclang-dev \
      libssl-dev \
      mold \
      pkg-config \
      procps \
      python3 \
      python3-venv \
      ripgrep \
      sudo \
      unzip \
      xz-utils; \
    install -m 0755 -d /etc/apt/keyrings; \
    curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc; \
    chmod 0644 /etc/apt/keyrings/docker.asc; \
    codename="$(. /etc/os-release && printf '%s' "${VERSION_CODENAME}")"; \
    printf 'deb [arch=%s signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/ubuntu %s stable\n' \
      "$(dpkg --print-architecture)" "${codename}" > /etc/apt/sources.list.d/docker.list; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
      containerd.io \
      docker-buildx-plugin \
      docker-ce \
      docker-ce-cli \
      docker-compose-plugin; \
    rm -rf /var/lib/apt/lists/*

# 2. Go toolchain (CGO-enabled, matches Dockerfile and satisfies mekugi-tui/go.work).
RUN set -eux; \
    case "${TARGETARCH}" in \
      amd64|arm64) goarch="${TARGETARCH}" ;; \
      *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    curl -fsSL "https://go.dev/dl/go${GO_VERSION}.linux-${goarch}.tar.gz" -o /tmp/go.tar.gz; \
    rm -rf /usr/local/go; \
    tar -C /usr/local -xzf /tmp/go.tar.gz; \
    rm -f /tmp/go.tar.gz; \
    /usr/local/go/bin/go version | grep -q "go${GO_VERSION}"

# 3. Node.js LTS (required for Vite builds and node --test browser fixtures).
RUN set -eux; \
    case "${TARGETARCH}" in \
      amd64) nodearch="x64" ;; \
      arm64) nodearch="arm64" ;; \
      *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    node_prefix="node-v${NODE_VERSION}-linux-${nodearch}"; \
    curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/${node_prefix}.tar.xz" -o /tmp/node.tar.xz; \
    tar -C /usr/local --strip-components=1 -xJf /tmp/node.tar.xz \
      "${node_prefix}/bin" \
      "${node_prefix}/include" \
      "${node_prefix}/lib" \
      "${node_prefix}/share"; \
    rm -f /tmp/node.tar.xz; \
    /usr/local/bin/node --version | grep -q "v${NODE_VERSION}"

# 4. Bun (matches Dockerfile and release.yml; required for mekugi-webui).
RUN set -eux; \
    case "${TARGETARCH}" in \
      amd64) bunarch="x64" ;; \
      arm64) bunarch="aarch64" ;; \
      *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    curl -fsSL "https://github.com/oven-sh/bun/releases/download/bun-v${BUN_VERSION}/bun-linux-${bunarch}.zip" -o /tmp/bun.zip; \
    unzip -q /tmp/bun.zip -d /tmp/bun; \
    install -m 0755 "/tmp/bun/bun-linux-${bunarch}/bun" /usr/local/bin/bun; \
    ln -sf /usr/local/bin/bun /usr/local/bin/bunx; \
    rm -rf /tmp/bun /tmp/bun.zip; \
    /usr/local/bin/bun --version | grep -q "${BUN_VERSION}"

# 5. Just, golangci-lint, sccache, cargo-nextest, and uv (pinned multi-arch binaries).
RUN set -eux; \
    case "${TARGETARCH}" in \
      amd64) rust_triple="x86_64-unknown-linux-musl"; gnu_triple="x86_64-unknown-linux-gnu"; goarch="amd64" ;; \
      arm64) rust_triple="aarch64-unknown-linux-musl"; gnu_triple="aarch64-unknown-linux-gnu"; goarch="arm64" ;; \
      *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    curl -fsSL "https://github.com/casey/just/releases/download/${JUST_VERSION}/just-${JUST_VERSION}-${rust_triple}.tar.gz" \
      | tar -xz -C /usr/local/bin just; \
    /usr/local/bin/just --version | grep -q "${JUST_VERSION}"; \
    curl -fsSL "https://github.com/golangci/golangci-lint/releases/download/v${GOLANGCI_LINT_VERSION}/golangci-lint-${GOLANGCI_LINT_VERSION}-linux-${goarch}.tar.gz" \
      | tar -xz -C /usr/local/bin --strip-components=1 "golangci-lint-${GOLANGCI_LINT_VERSION}-linux-${goarch}/golangci-lint"; \
    /usr/local/bin/golangci-lint version | grep -q "${GOLANGCI_LINT_VERSION}"; \
    curl -fsSL "https://github.com/mozilla/sccache/releases/download/v${SCCACHE_VERSION}/sccache-v${SCCACHE_VERSION}-${rust_triple}.tar.gz" \
      | tar -xz -C /usr/local/bin --strip-components=1 "sccache-v${SCCACHE_VERSION}-${rust_triple}/sccache"; \
    /usr/local/bin/sccache --version | grep -q "${SCCACHE_VERSION}"; \
    curl -fsSL "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-${NEXTEST_VERSION}/cargo-nextest-${NEXTEST_VERSION}-${gnu_triple}.tar.gz" \
      | tar -xz -C /usr/local/bin cargo-nextest; \
    /usr/local/bin/cargo-nextest --version | grep -q "${NEXTEST_VERSION}"; \
    curl -fsSL "https://github.com/astral-sh/uv/releases/download/${UV_VERSION}/uv-${rust_triple}.tar.gz" \
      | tar -xz -C /usr/local/bin --strip-components=1 "uv-${rust_triple}/uv" "uv-${rust_triple}/uvx"; \
    /usr/local/bin/uv --version | grep -q "${UV_VERSION}"

# 6. Rename Ubuntu's uid/gid 1000 account to `agent` (matching sbx@1 and agent mixins)
#    and prepare cache directories, proxy CA dir, and idempotent persistent shell env.
RUN <<'SCRIPT'
set -eux
usermod --login agent --home /home/agent --move-home ubuntu
groupmod --new-name agent ubuntu
usermod --shell /bin/bash --append --groups docker,sudo agent
printf 'agent ALL=(ALL) NOPASSWD:ALL\n' > /etc/sudoers.d/agent
chmod 0440 /etc/sudoers.d/agent
mkdir -p \
  /home/agent/workspace \
  /home/agent/.local/bin \
  /home/agent/.local/state \
  /home/agent/.config \
  /home/agent/.cache/sccache \
  /home/agent/.cache/uv \
  /home/agent/.bun/install/cache \
  /home/agent/go/bin \
  /home/agent/go/pkg/mod \
  /home/agent/.cargo/registry \
  /home/agent/.cargo/git \
  /home/agent/.rustup \
  /usr/local/share/ca-certificates
chown -R agent:agent /home/agent
cat > /etc/sandbox-persistent.sh <<'EOF'
export GOPATH=/home/agent/go
export GOBIN=/home/agent/go/bin
export CARGO_HOME=/home/agent/.cargo
export RUSTUP_HOME=/home/agent/.rustup
for _mekugi_dir in /usr/local/go/bin /home/agent/.cargo/bin /home/agent/go/bin /home/agent/.local/bin; do
  case ":$PATH:" in
    *":${_mekugi_dir}:"*) ;;
    *) PATH="${_mekugi_dir}:$PATH" ;;
  esac
done
unset _mekugi_dir
export PATH
EOF
chmod 0644 /etc/sandbox-persistent.sh
printf '. /etc/sandbox-persistent.sh\n' > /etc/profile.d/sandbox-persistent.sh
SCRIPT

# 7. Agent-owned toolchains: Go deadcode + pinned Rustup toolchain (matches codex-rs/rust-toolchain.toml).
USER agent
ENV HOME=/home/agent \
    GOPATH=/home/agent/go \
    GOBIN=/home/agent/go/bin \
    CARGO_HOME=/home/agent/.cargo \
    RUSTUP_HOME=/home/agent/.rustup \
    CGO_ENABLED=1 \
    CARGO_NET_GIT_FETCH_WITH_CLI=true \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=-fuse-ld=mold" \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=-fuse-ld=mold" \
    RUST_MIN_STACK=8388608 \
    RUSTC_WRAPPER=sccache \
    SCCACHE_ALLOW_INCREMENTAL=1 \
    SCCACHE_DIR=/home/agent/.cache/sccache \
    NO_PROXY="localhost,127.0.0.1,::1,172.17.0.0/16" \
    no_proxy="localhost,127.0.0.1,::1,172.17.0.0/16" \
    BASH_ENV=/etc/sandbox-persistent.sh \
    PATH="/home/agent/.local/bin:/home/agent/go/bin:/home/agent/.cargo/bin:/usr/local/go/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

RUN go install "golang.org/x/tools/cmd/deadcode@v${DEADCODE_VERSION}" \
 && command -v deadcode \
 && go clean -cache -modcache

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --profile minimal --default-toolchain "${RUST_TOOLCHAIN}" \
 && rustup component add clippy rustfmt rust-src \
 && rustc --version | grep -q "${RUST_TOOLCHAIN}" \
 && cargo --version

# Transition: runtimes predating dockerd@1 start the engine for this label.
LABEL org.opencontainers.image.source="https://github.com/mekugi-harness/mekugi"
LABEL org.opencontainers.image.licenses="MIT AND Apache-2.0"
LABEL com.docker.sandboxes.start-docker="true"
LABEL com.docker.sandboxes.flavor="mekugi-dev"
LABEL com.docker.sandboxes.base="${BASE_IMAGE}"

WORKDIR /home/agent/workspace
ENTRYPOINT ["bash"]
CMD ["-l"]
