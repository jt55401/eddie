# MkDocs Integration (Issue #5)

This folder owns all MkDocs-specific integration code and tests.

## Run

```bash
docker build -f integrations/mkdocs/tests/docker/Dockerfile -t eddie-mkdocs-e2e .
docker run --rm -v "$PWD":/repo eddie-mkdocs-e2e
```
