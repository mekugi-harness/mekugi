# GitHub Actions workflows

- `release.yml` validates pull request and main trees. It builds, smoke-tests
  and packages the Cargo native runtime on Linux amd64, Linux arm64 and macOS
  arm64, along with the terminal and browser clients. Tags publish releases.
- `ci-image.yml` builds the Linux CI image used by validation and packaging.
- `sandbox-kit.yml` checks and packages the sandbox kit.
