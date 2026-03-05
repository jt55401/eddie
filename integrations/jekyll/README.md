# Jekyll Integration (Issue #7)

This folder owns all Jekyll-specific integration code and tests.

## Run

```bash
docker build -f integrations/jekyll/tests/docker/Dockerfile -t eddie-jekyll-e2e .
docker run --rm -v "$PWD":/repo eddie-jekyll-e2e
```
