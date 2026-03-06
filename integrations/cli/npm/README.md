# @jt55401/eddie-cli

`@jt55401/eddie-cli` exposes the `eddie` command for Node-based workflows.

On first run it downloads the native Eddie binary for this package version from:

- `https://github.com/jt55401/eddie/releases/tag/v<version>`

## Usage

```bash
npx -y @jt55401/eddie-cli@0.2.0 --help
npx -y @jt55401/eddie-cli@0.2.0 index --cms hugo --content-dir content --output static/eddie/index.ed
```

## Environment

- `EDDIE_CLI_CACHE_DIR`: override download/cache directory
- `EDDIE_CLI_VERSION`: override release version (defaults to package version)
