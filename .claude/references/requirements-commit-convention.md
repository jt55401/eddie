# Requirements Commit + Changelog Convention

## Commit Subject

Use conventional commits with requirements scope:

```text
docs(requirements-<area>): <summary>
```

Where `<area>` aligns to requirements folders:

- `0100-indexing-pipeline`
- `0200-search-runtime`
- `0300-qa-runtime`
- `0400-widget-ui`
- `0500-integration`
- `0600-configuration`

## Commit Body Footers

Preferred footers for extraction:

- `Req-Refs: <comma-separated requirement ids/files>`
- `Changelog: requirements`
- `Req-Summary: <single sentence for changelog>`

Example:

```text
docs(requirements-0200): add cosine similarity search requirement

Req-Refs: 0210-cosine-similarity-search.md
Changelog: requirements
Req-Summary: Added cosine similarity search requirement for WASM runtime.
```

## Changelog Output Mapping

The extraction script maps commit types to sections:

- `feat` -> `Added`
- `fix` -> `Fixed`
- `docs`, `chore`, `refactor`, `test` -> `Changed`

If `Req-Summary` is present, it is used as the changelog bullet text.
Otherwise the subject summary is used.

## Good Practices

- Keep summary outcome-focused, not activity-focused.
- Include user-visible intent (what behavior/expectation changed).
- Keep one logical requirement change per commit when possible.
- Always include `Req-Refs` for traceability.
