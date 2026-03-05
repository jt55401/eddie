# Shared Eddie Demo Content

This folder contains the reusable Eddie demo corpus used by CMS integration tests.

- `content/eddie-corpus.json` is the source of truth.
- `scripts/render_eddie_corpus.py` renders framework-specific files for Hugo, Astro, Docusaurus, MkDocs, Eleventy, and Jekyll.

Use it in test seed scripts:

```bash
python3 /repo/integrations/shared/scripts/render_eddie_corpus.py \
  --cms <hugo|astro|docusaurus|mkdocs|eleventy|jekyll> \
  --site-root /path/to/site
```
