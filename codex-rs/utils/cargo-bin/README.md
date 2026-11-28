# Cargo test binaries and resources

- `cargo_bin` resolves the `CARGO_BIN_EXE_*` path for a Cargo-built test
  binary. It falls back to Cargo's target directory when that variable is
  unavailable.
- `find_resource!` locates a test resource relative to the calling crate's
  `CARGO_MANIFEST_DIR`.
- `repo_root` finds the repository containing this helper crate.
