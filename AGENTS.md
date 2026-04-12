# AGENTS.md

Instructions for coding agents working in this repository.

## Pull Request Checks

- Do not introduce new build warnings.
- Always run `./scripts/check-no-build-warnings.ps1` before creating or updating a PR.
- Before creating or updating a PR, also run `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets --all-features -- -D warnings`, and `cargo test --locked`.
