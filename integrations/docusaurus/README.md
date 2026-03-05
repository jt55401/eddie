# Docusaurus Integration (Issue #4)

This folder owns all Docusaurus-specific integration code and tests.

## Run

```bash
docker build -f integrations/docusaurus/tests/docker/Dockerfile -t eddie-docusaurus-e2e .
docker run --rm -v "$PWD":/repo eddie-docusaurus-e2e
```
