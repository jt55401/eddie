# 0220 Multi-Platform Release

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a user on any of Eddie's five supported platforms, I can install and run
the CLI without building from source, and trust that the binary I
downloaded hasn't been corrupted or tampered with.

## Key Fields/Parameters

- release matrix (native builds, no cross-compilation): `eddie-linux-x86_64` (`ubuntu-22.04`), `eddie-linux-aarch64` (`ubuntu-24.04-arm`), `eddie-macos-aarch64` (`macos-14`), `eddie-macos-x86_64` (`macos-15-intel`), `eddie-windows-x86_64.exe` (`windows-2022`)
- every tagged release also publishes `SHA256SUMS` covering every asset
- launcher packages (`integrations/cli/{npm,gem,pypi}`) resolve the caller's platform/arch to one of the five asset names, download the pinned version matching the package's own version (never `latest`), fetch `SHA256SUMS` alongside it, and verify the downloaded binary's digest before marking it executable
- launchers cache the verified binary under an OS-appropriate cache directory (`~/.cache/eddie-cli` on Linux, `~/Library/Caches/eddie-cli` on macOS, `%LOCALAPPDATA%\eddie-cli\Cache` on Windows), overridable via `EDDIE_CLI_CACHE_DIR`/`EDDIE_CLI_VERSION`

## Acceptance Criteria

- Every platform a launcher package claims to support (its `resolveAsset`/`resolve_asset` mapping) has a matching asset actually published by `release.yml`.
- A launcher aborts and deletes the temporary file, without marking anything executable, when the downloaded binary's SHA-256 doesn't match `SHA256SUMS`.
- A launcher fails with a clear, actionable message (naming the platforms that are supported) on any platform/arch combination it doesn't recognize, rather than attempting a download that's guaranteed to 404.
- CUDA-accelerated builds are not part of this release matrix; `cargo build --release --features cuda` against a local CUDA toolchain is the documented path for that build.

## Evidence

- `.github/workflows/release.yml`
- `integrations/cli/npm/test/eddie.test.js`
- `docs/guides/github-actions.md`

## Linked Tickets

- (none yet)
