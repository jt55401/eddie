# Eleventy Integration (Issue #6)

This folder owns all Eleventy-specific integration code and tests.

## Run

```bash
docker build -f integrations/eleventy/tests/docker/Dockerfile -t eddie-eleventy-e2e .
docker run --rm -v "$PWD":/repo eddie-eleventy-e2e
```
