# Mekugi Dev Sandbox Kit

A [Docker Sandboxes v3](https://docs.docker.com/ai/sandboxes/customize/)
workload kit (`schemaVersion: "3"`, `kind: workload`) providing a pinned Ubuntu
26.04 LTS Linux environment for building, testing, linting, and formatting
**Mekugi** (`mekugi-tui`, `mekugi-webui`, and the native Rust runtime in
`codex-rs`).

Published multi-architecture (`linux/amd64`, `linux/arm64`) to GitHub Container
Registry at `ghcr.io/mekugi-harness/mekugi-dev` by
[`.github/workflows/sandbox-kit.yml`](../.github/workflows/sandbox-kit.yml).

## What is included

Built on digest-pinned `ubuntu:26.04` (non-root `agent` UID/GID 1000
with passwordless `sudo`, Docker Engine) with toolchains matching
[`.github/workflows/release.yml`](../.github/workflows/release.yml),
[`Dockerfile`](../Dockerfile), and repository manifests:

- **Go 1.27.3** (CGO enabled), `golangci-lint` 2.14.0, `deadcode` 0.51.0
- **Rust 1.99.1** via `rustup` (`clippy`, `rustfmt`, `rust-src` matching [`codex-rs/rust-toolchain.toml`](../codex-rs/rust-toolchain.toml)), `mold` linker, `sccache` 0.19.0 (`RUSTC_WRAPPER=sccache`), `cargo-nextest` 0.9.152
- **Node.js 26.13.1** (LTS) and **Bun 1.4.4**
- **Just 1.51.0**, **Python 3**, **`uv` 0.13.10** (for `just fmt` locked `ruff`), `ripgrep`, `clang`/`gcc`/`cmake`, `pkg-config`, `libcap-dev`, `libssl-dev`
- **Capabilities**: default compute (`4` vCPU, `8g` RAM via `resources@1`), optional Docker-in-Docker (`dockerd@1`, `privileged@1`), optional persistent cache volumes (`volume@1` for Go modules, Cargo registry/git, Bun, `sccache`, and `uv`), and a scoped `network-policy@1` egress allowlist (Go, Cargo/rustup, GitHub/prebuilt V8, npm/Bun, PyPI, Ubuntu/Docker apt, Docker Hub, and GHCR)

Browser-based WebUI tests (`MEKUGI_TEST_WEBUI_BROWSER=1`) require Chrome and
`agent-browser`, which are not bundled in this image.

## Run

Requires the [`sbx` CLI](https://docs.docker.com/ai/sandboxes/install/).

Run the published GHCR kit against the current repository checkout (`.`):

```sh
sbx run ghcr.io/mekugi-harness/mekugi-dev:latest .
```

Or run on Docker Agentic Platform cloud sandboxes:

```sh
sbx --cloud run ghcr.io/mekugi-harness/mekugi-dev:latest
```

Compose with a v3 agent mixin (which brings the coding agent binary, credentials,
and provider egress):

```sh
sbx run ghcr.io/mekugi-harness/mekugi-dev:latest --kit sbx/codex-mixin:latest .
```

Increase compute when compiling the native Rust runtime:

```sh
sbx run ghcr.io/mekugi-harness/mekugi-dev:latest --cpus 8 --memory 16g .
```

Or override the kit's `memory` argument directly with `--kit-arg memory=16g`.

To test local edits to `sandbox-kit/` before publishing, pass the directory
directly from the repository root:

```sh
sbx run ./sandbox-kit .
```

## Build and publish

[`.github/workflows/sandbox-kit.yml`](../.github/workflows/sandbox-kit.yml)
validates `linux/amd64` and `linux/arm64` builds on pull requests and publishes
`ghcr.io/mekugi-harness/mekugi-dev` (`latest`, the `version` in
`mekugi-dev.yaml`, and `sha-<commit>`) on pushes to `main` and manual workflow
dispatches.

To build and push manually, use a `docker-container` Buildx builder so
multi-platform exports preserve the `vnd.docker.sandbox.kit.descriptor` manifest
annotation (the classic `docker` driver can drop manifest annotations):

```sh
docker buildx create --name kit-builder --driver docker-container --bootstrap
gh auth token | docker login ghcr.io -u "$(gh api user --jq .login)" --password-stdin
docker buildx build --builder kit-builder ./sandbox-kit \
  --file ./sandbox-kit/mekugi-dev.yaml \
  --platform linux/amd64,linux/arm64 \
  --provenance=mode=min \
  --tag ghcr.io/mekugi-harness/mekugi-dev:1.0.0 \
  --tag ghcr.io/mekugi-harness/mekugi-dev:latest \
  --push
```

Validate a published OCI artifact with
`kit-tck validate ghcr.io/mekugi-harness/mekugi-dev:latest`.

Override any pinned toolchain version at build time with `--build-arg`, using
the kit argument name declared in `mekugi-dev.yaml`:

```sh
docker buildx build --builder kit-builder ./sandbox-kit \
  --file ./sandbox-kit/mekugi-dev.yaml \
  --platform linux/amd64,linux/arm64 \
  --provenance=mode=min \
  --build-arg goVersion=1.27.3 \
  --build-arg rustToolchain=1.99.1 \
  --tag ghcr.io/mekugi-harness/mekugi-dev:1.0.0 \
  --push
```

See [`docs/contributing/development.md`](../docs/contributing/development.md)
for Mekugi build, test, and lint workflows.
