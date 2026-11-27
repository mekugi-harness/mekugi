# syntax=docker/dockerfile:1

# Pre-baked Ubuntu 24.04 LTS CI base image for Mekugi (glibc 2.39 compatible).
# Includes all Linux toolchains, linters, native build deps, Chrome/agent-browser
# (amd64), and pre-warmed Bun, Go, Cargo (including rusty_v8 git submodules), and
# prebuilt V8 artifact caches so CI jobs skip setup/install/fetch steps.
ARG BASE_IMAGE=ubuntu:24.04@sha256:562111b88caa79c71391c0c4d3c2114a7d5c35915c20561fd893cb1e07929e6b
FROM ${BASE_IMAGE}

ARG TARGETARCH
ARG GO_VERSION=1.27.3
ARG RUST_TOOLCHAIN=1.99.1
ARG NODE_VERSION=26.13.1
ARG BUN_VERSION=1.4.5
ARG JUST_VERSION=1.51.0
ARG GOLANGCI_LINT_VERSION=2.14.0
ARG DEADCODE_VERSION=0.51.0
ARG SCCACHE_VERSION=0.19.0
ARG NEXTEST_VERSION=0.9.152
ARG AGENT_BROWSER_VERSION=0.45.0

# 1. System build deps, ripgrep, mold, GitHub CLI, and Chrome (amd64).
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
      gnupg \
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
      xz-utils \
      zstd; \
    install -m 0755 -d /etc/apt/keyrings; \
    curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
      -o /etc/apt/keyrings/githubcli-archive-keyring.gpg; \
    chmod go+r /etc/apt/keyrings/githubcli-archive-keyring.gpg; \
    printf 'deb [arch=%s signed-by=/etc/apt/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main\n' \
      "$(dpkg --print-architecture)" > /etc/apt/sources.list.d/github-cli.list; \
    apt-get update; \
    apt-get install -y --no-install-recommends gh; \
    rm -rf /var/lib/apt/lists/*

# 1b. Google Chrome Stable on amd64 (for packaged browser E2E tests).
RUN set -eux; \
    if [ "${TARGETARCH}" = "amd64" ]; then \
      export DEBIAN_FRONTEND=noninteractive; \
      apt-get update; \
      curl -fsSL https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb \
        -o /tmp/google-chrome.deb; \
      apt-get install -y --no-install-recommends /tmp/google-chrome.deb; \
      rm -rf /tmp/google-chrome.deb /var/lib/apt/lists/*; \
      printf '#!/bin/sh\nexec /usr/bin/google-chrome --no-sandbox "$@"\n' > /usr/local/bin/google-chrome-ci; \
      chmod 0755 /usr/local/bin/google-chrome-ci; \
    fi

# 2. Go toolchain (CGO-enabled, satisfies mekugi-tui/go.work).
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

# 3. Node.js LTS + agent-browser (for packaged browser E2E tests on amd64).
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
    /usr/local/bin/node --version | grep -q "v${NODE_VERSION}"; \
    if [ "${TARGETARCH}" = "amd64" ]; then \
      npm install --global "agent-browser@${AGENT_BROWSER_VERSION}"; \
      npm cache clean --force; \
      command -v agent-browser; \
    fi

# 4. Bun (for mekugi-webui).
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

# 5. Pinned CLI tools: just, golangci-lint, sccache, cargo-nextest, deadcode.
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
    GOBIN=/usr/local/bin /usr/local/go/bin/go install "golang.org/x/tools/cmd/deadcode@v${DEADCODE_VERSION}"; \
    /usr/local/go/bin/go clean -cache -modcache; \
    command -v deadcode

ENV GOPATH=/opt/go \
    GOMODCACHE=/opt/go/pkg/mod \
    GOBIN=/usr/local/bin \
    CARGO_HOME=/opt/cargo \
    RUSTUP_HOME=/opt/rustup \
    RUSTUP_AUTO_INSTALL=0 \
    BUN_INSTALL_CACHE_DIR=/opt/bun/cache \
    CODEX_V8_CACHE_ROOT=/opt/v8-artifacts \
    CGO_ENABLED=1 \
    CARGO_NET_GIT_FETCH_WITH_CLI=true \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=-fuse-ld=mold" \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=-fuse-ld=mold" \
    RUST_MIN_STACK=8388608 \
    PATH="/opt/cargo/bin:/usr/local/go/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

# 6. Pinned Rust toolchain (matches codex-rs/rust-toolchain.toml; RUSTUP_AUTO_INSTALL=0 prevents re-install).
RUN set -eux; \
    mkdir -p /opt/cargo /opt/rustup /opt/go/pkg/mod /opt/bun/cache /opt/v8-artifacts; \
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --profile minimal --default-toolchain "${RUST_TOOLCHAIN}"; \
    rustup component add clippy rustfmt rust-src; \
    rustc --version | grep -q "${RUST_TOOLCHAIN}"; \
    cargo --version

# 7. Pre-warm Bun dependency cache.
RUN --mount=type=bind,source=mekugi-webui/app/package.json,target=/tmp/bun-warm/package.json \
    --mount=type=bind,source=mekugi-webui/app/bun.lock,target=/tmp/bun-warm/bun.lock \
    set -eux; \
    mkdir -p /tmp/bun-work; \
    cp /tmp/bun-warm/package.json /tmp/bun-warm/bun.lock /tmp/bun-work/; \
    cd /tmp/bun-work && bun install --frozen-lockfile --no-progress --no-summary; \
    rm -rf /tmp/bun-work

# 8. Pre-warm Go module cache for the workspace.
RUN --mount=type=bind,source=.,target=/tmp/repo,readonly \
    set -eux; \
    mkdir -p /tmp/go-warm; \
    (cd /tmp/repo && find mekugi-tui mekugi-webui -not -path '*/testdata/*' \( -name go.work -o -name go.work.sum -o -name go.mod -o -name go.sum \) -print0 | tar --null -T - -cf - | tar -xf - -C /tmp/go-warm); \
    for go_mod in /tmp/go-warm/mekugi-webui/go.mod $(find /tmp/go-warm/mekugi-tui -name go.mod); do \
      (cd "$(dirname "$go_mod")" && GOWORK=/tmp/go-warm/mekugi-tui/go.work go mod download); \
    done; \
    rm -rf /tmp/go-warm

# 9. Pre-warm Cargo registry, git repos (including rusty_v8 + Chromium submodules), and prebuilt V8 artifacts.
RUN --mount=type=bind,source=.,target=/tmp/repo,readonly \
    set -eux; \
    cd /tmp/repo/codex-rs; \
    cargo fetch --locked; \
    cargo metadata --locked --format-version 1 >/dev/null; \
    CODEX_REPO_ROOT=/tmp/repo python3 -c "import subprocess, sys; from pathlib import Path; sys.path.insert(0, '/tmp/repo/scripts'); from codex_package.targets import TARGET_SPECS; from codex_package.v8 import fetch_codex_v8_artifacts; v = subprocess.check_output(['rustc', '-vV'], text=True); t = next(l.removeprefix('host: ') for l in v.splitlines() if l.startswith('host: ')); fetch_codex_v8_artifacts(TARGET_SPECS[t], cache_root=Path('/opt/v8-artifacts'))"

LABEL org.opencontainers.image.source="https://github.com/mekugi-harness/mekugi"
LABEL org.opencontainers.image.description="Pre-baked Ubuntu 24.04 CI base image for Mekugi"
LABEL org.opencontainers.image.licenses="MIT AND Apache-2.0"
