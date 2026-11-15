# Contributing to Ferrix

Thanks for your interest! Ferrix is in early development; read
[`docs/architecture.md`](docs/architecture.md) and the [ADRs](docs/adr/) first.

## Getting started

```sh
cargo xtask fetch-pdfium   # downloads the pinned, checksum-verified PDFium build into vendor/
cargo test --workspace
cargo run -p ferrix-cli -- render some.pdf --pages 1 --dpi 144 -o page.png
```

Any editor with rust-analyzer works; RustRover opens the workspace from the root `Cargo.toml`.

## Ground rules

- **Layering:** `ferrix-types` → `ferrix-engine` → backends → core → app. Dependencies point down
  only; nothing above `ferrix-engine` may use PDFium types.
- **No `unsafe`** in first-party crates, except reviewed FFI modules (engine backends,
  `ferrix-print`), each `unsafe` block with a `// SAFETY:` comment.
- **Untrusted input:** every PDF is hostile. Validate sizes and indices; never `unwrap` on data
  derived from a document.
- **Tests:** new engine behavior needs a test in `crates/ferrix-engine-pdfium/tests/`, using
  hand-written fixtures in `tests/fixtures/` rather than committed binary PDFs where possible.
- Before pushing: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
- Significant design changes start as an ADR in `docs/adr/`.

## Releasing

1. Bump `version` in the root `Cargo.toml`, merge to `main`.
2. Tag the merge commit and push the tag: `git tag v0.1.0 && git push origin v0.1.0`.
3. The [Release workflow](.github/workflows/release.yml) builds a universal macOS `.dmg`, Windows
   x64/arm64 `.zip`s and Linux x64/arm64 `.tar.gz`s, then creates a **draft** release with
   `SHA256SUMS` and build-provenance attestations. Review the draft on GitHub and publish it.

`dist/package.sh <version>` builds the same package for the host OS into `out/`.

## Licensing

Ferrix is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
Unless you explicitly state otherwise, any contribution you submit for inclusion is dual-licensed
as above, without additional terms or conditions. New dependencies must use a license allowed by
[`deny.toml`](deny.toml).
