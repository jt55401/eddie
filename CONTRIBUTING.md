# Contributing

Thanks for improving Eddie.

## Contribution Flow

1. Fork the repository.
2. Create a branch for your change.
3. Run local checks:
   - `cargo build --all-targets`
   - `cargo test`
   - `python3 .claude/scripts/check_requirements_conflicts.py --root requirements`
4. Open a pull request against upstream.

## Legal And Attribution

- By submitting a contribution, you agree your change is provided under the repository license (`GPL-3.0-only`).
- Keep existing copyright, license, and notice text intact.
- Do not present your fork as the official upstream project. See `TRADEMARKS.md`.
