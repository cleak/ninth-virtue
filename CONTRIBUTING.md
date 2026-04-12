# Contributing

Small, focused pull requests are easiest to review.

## Ground Rules

- This is a Windows-only project that targets live DOSBox or DOSBox Staging sessions.
- Do not commit Ultima V game files, extracted assets, ROMs, or other third-party proprietary material.
- Do not commit secrets, credentials, or machine-specific private data.
- Keep reverse-engineering notes and code comments factual and reproducible.

## Development

Do not open or update a pull request with new build warnings.

From PowerShell, before opening or updating a pull request, run:

```powershell
cargo fmt --all -- --check
./scripts/check-no-build-warnings.ps1
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
```

If you change behavior, update tests or documentation where it is practical to do so.

## Licensing Of Contributions

This repository is dual-licensed under `MIT OR Apache-2.0`.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project by you shall be licensed as `MIT OR Apache-2.0`, without additional terms or conditions.

By submitting a contribution, you represent that you have the right to license it under those terms and that it does not knowingly bundle material that this repository cannot legally publish.
