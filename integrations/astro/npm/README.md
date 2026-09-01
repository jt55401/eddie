# @jt55401/eddie-astro

Install helper for integrating Eddie into astro sites.

## Usage

```bash
npx @jt55401/eddie-astro /path/to/site /path/to/eddie/dist
```

Arguments:

- First argument: CMS site root directory
- Second argument: Eddie runtime asset directory (a built `dist/`, or an
  installed package's `assets/`) containing every file named in that
  directory's `assets.list` manifest, plus the manifest itself. The current
  file list lives at `widget/assets.list` in the
  [Eddie repo](https://github.com/jt55401/eddie/blob/main/widget/assets.list)
  (boot loader, full widget, page-worker fallbacks, lite/dense WASM +
  glue, and the tiered service workers).
