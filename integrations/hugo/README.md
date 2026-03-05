# Hugo Integration E2E

This folder owns Hugo-specific plugin integration tests.

## Run

```bash
docker build -f integrations/hugo/tests/docker/Dockerfile -t eddie-hugo-e2e .
docker run --rm -v "$PWD":/repo eddie-hugo-e2e
```
