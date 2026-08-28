# Contributing

Thanks for improving Eddie.

## Contribution Flow

1. Fork the repository.
2. Install the pinned Rust toolchain (`rust-toolchain.toml`, currently `1.93.1` with the `wasm32-unknown-unknown` target) — `rustup` picks it up automatically.
3. Create a branch for your change.
4. Run local checks:
   - `cargo fmt --all -- --check`
   - `cargo clippy --all-targets --locked -- -D warnings`
   - `cargo test --locked`
   - `cargo build --target wasm32-unknown-unknown --lib --locked`
   - `python3 .claude/scripts/check_requirements_conflicts.py --root requirements`
   - If you touched `widget/` or `integrations/`: `bash widget/build.sh` and `node --test integrations/cli/npm/test/`
5. Open a pull request against upstream.

## Legal And Attribution

- By submitting a contribution, you agree your change is provided under the repository license (`GPL-3.0-only`).
- Keep existing copyright, license, and notice text intact.
- Do not present your fork as the official upstream project. See `TRADEMARKS.md`.
