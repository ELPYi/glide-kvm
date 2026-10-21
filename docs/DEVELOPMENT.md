# Development

```
cargo test             # 110+ tests: unit, headless UI, end-to-end over real UDP
cargo clippy --all-targets
cargo build --release  # a "dev" build: never updates itself
scripts/package.ps1    # builds dist/glide/
```

Every push to `master` is linted, tested, built and published as a GitHub
release by [CI](../.github/workflows/release.yml), which sets `GLIDE_VERSION` and `GLIDE_UPDATE_REPO` (read by `build.rs`). Only builds with both set update themselves.

[ARCHITECTURE.md](ARCHITECTURE.md) explains the module layout, threads, protocol and testing
strategy, and includes a short Rust primer.

Building locally on Windows with the GNU toolchain needs MinGW; see
[ARCHITECTURE.md](ARCHITECTURE.md#building-on-windows).
