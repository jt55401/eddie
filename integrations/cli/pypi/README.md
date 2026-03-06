# jt55401-eddie-cli

`jt55401-eddie-cli` exposes the `eddie` command for Python-based workflows.

On first run it downloads the native Eddie binary for this package version from:

- `https://github.com/jt55401/eddie/releases/tag/v<version>`

## Usage

```bash
python -m pip install jt55401-eddie-cli==0.2.0
eddie --help
eddie index --cms mkdocs --content-dir docs --output docs/eddie/index.ed
```

## Environment

- `EDDIE_CLI_CACHE_DIR`: override download/cache directory
- `EDDIE_CLI_VERSION`: override release version (defaults to package version)
