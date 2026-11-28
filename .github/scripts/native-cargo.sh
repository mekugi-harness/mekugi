#!/usr/bin/env bash
set -euo pipefail

export CARGO_INCREMENTAL=0
# Use system Git for V8's nested Git dependencies instead of libgit2.
export CARGO_NET_GIT_FETCH_WITH_CLI=true
export RUSTC_WRAPPER=sccache
export CARGO_BUILD_JOBS=2
if [[ "$RUNNER_OS" == Linux ]]; then
  export CARGO_BUILD_JOBS=4
  export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS='-C link-arg=-fuse-ld=mold'
  export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS='-C link-arg=-fuse-ld=mold'
  # Allow rustup to install a newer toolchain if the checkout bumps the pin.
  export RUSTUP_AUTO_INSTALL=1
fi

exec python3 runtime/build.py "$@"
