# Astro Integration (Issue #3)

This folder owns all Astro-specific integration code and tests.

## What this adds

- `plugin/install.sh`: integrates Eddie assets into an Astro template site.
- `tests/docker/Dockerfile`: deterministic test environment.
- `tests/docker/run-e2e.sh`: full E2E flow:
  1. Downloads a public Astro template project.
  2. Integrates Eddie plugin assets/snippet.
  3. Builds Eddie and indexes template content.
  4. Boots Astro dev server and verifies the site is reachable.

## Run

```bash
docker build -f integrations/astro/tests/docker/Dockerfile -t eddie-astro-e2e .
docker run --rm -v "$PWD":/repo eddie-astro-e2e
```
