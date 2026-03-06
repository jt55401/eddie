# eddie-cli

`eddie-cli` exposes the `eddie` command for Ruby workflows.

On first run it downloads the native Eddie binary for this gem version from:

- `https://github.com/jt55401/eddie/releases/tag/v<version>`

## Usage

```bash
gem install eddie-cli -v 0.2.1
eddie --help
eddie index --cms jekyll --content-dir . --output assets/eddie/index.ed
```

## Environment

- `EDDIE_CLI_CACHE_DIR`: override download/cache directory
- `EDDIE_CLI_VERSION`: override release version (defaults to gem version)
