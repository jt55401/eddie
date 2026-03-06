#!/usr/bin/env bash
set -euo pipefail

CMS="${1:?usage: start-cms-demo.sh <cms>}"
REPO_ROOT="/repo"
WORKDIR="/tmp/${CMS}-gallery"
SITE_ROOT="$WORKDIR/site"
INSTALL_SOURCE="${EDDIE_INSTALL_SOURCE:-local}"
PACKAGE_VERSION="${EDDIE_PACKAGE_VERSION:-}"

rm -rf "$WORKDIR"
mkdir -p "$WORKDIR"

build_eddie() {
  cd "$REPO_ROOT"
  cargo build --release >/tmp/${CMS}-cargo.log 2>&1 || {
    cat /tmp/${CMS}-cargo.log >&2
    return 1
  }
}

npm_package_spec() {
  local package_name="$1"
  if [[ -n "$PACKAGE_VERSION" ]]; then
    printf "%s@%s" "$package_name" "$PACKAGE_VERSION"
  else
    printf "%s" "$package_name"
  fi
}

install_eddie_for_cms() {
  local cms="$1"
  local site_root="$2"

  case "$INSTALL_SOURCE" in
    local)
      bash "$REPO_ROOT/integrations/$cms/plugin/install.sh" "$site_root"
      ;;
    registry)
      case "$cms" in
        hugo|astro|docusaurus|eleventy)
          npx -y "$(npm_package_spec "@jt55401/eddie-$cms")" "$site_root" "$REPO_ROOT/dist"
          ;;
        mkdocs)
          local spec="eddie-mkdocs"
          if [[ -n "$PACKAGE_VERSION" ]]; then
            spec="${spec}==${PACKAGE_VERSION}"
          fi
          pip3 install --break-system-packages --no-cache-dir "$spec" >/tmp/${CMS}-pip-install-plugin.log 2>&1 || {
            cat /tmp/${CMS}-pip-install-plugin.log >&2
            exit 1
          }
          eddie-mkdocs-install "$site_root" "$REPO_ROOT/dist"
          ;;
        jekyll)
          if [[ -n "$PACKAGE_VERSION" ]]; then
            gem install eddie-jekyll -v "$PACKAGE_VERSION" --no-document >/tmp/${CMS}-gem-install-plugin.log 2>&1 || {
              cat /tmp/${CMS}-gem-install-plugin.log >&2
              exit 1
            }
          else
            gem install eddie-jekyll --no-document >/tmp/${CMS}-gem-install-plugin.log 2>&1 || {
              cat /tmp/${CMS}-gem-install-plugin.log >&2
              exit 1
            }
          fi
          eddie-jekyll-install "$site_root" "$REPO_ROOT/dist"
          ;;
        *)
          echo "Unsupported CMS for registry install: $cms" >&2
          exit 2
          ;;
      esac
      ;;
    *)
      echo "Unsupported EDDIE_INSTALL_SOURCE: $INSTALL_SOURCE" >&2
      exit 2
      ;;
  esac
}

case "$CMS" in
  hugo)
    hugo new site "$SITE_ROOT" >/dev/null
    cat > "$SITE_ROOT/hugo.toml" <<'TOML'
baseURL = "http://example.org/"
languageCode = "en-us"
title = "Eddie Search Logbook"
TOML
    mkdir -p "$SITE_ROOT/layouts/_default"
    cat > "$SITE_ROOT/layouts/_default/baseof.html" <<'HTML'
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>{{ if .Title }}{{ .Title }} | {{ end }}{{ .Site.Title }}</title>
  <style>
    :root {
      --bg: #f7f6f2;
      --surface: #ffffff;
      --text: #0f1720;
      --muted: #516170;
      --line: #e3e5ea;
      --accent: #0b6e4f;
      --accent-2: #1f8d68;
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      font-family: "Space Grotesk", "Segoe UI", sans-serif;
      color: var(--text);
      background:
        radial-gradient(circle at 10% -20%, #d9efe7 0%, transparent 44%),
        radial-gradient(circle at 100% 0%, #f5d9d6 0%, transparent 38%),
        var(--bg);
    }
    .shell { max-width: 1080px; margin: 0 auto; padding: 24px; }
    header {
      display: flex;
      justify-content: space-between;
      align-items: baseline;
      gap: 12px;
      margin-bottom: 24px;
    }
    .brand { margin: 0; font-size: clamp(1.4rem, 2vw, 2rem); letter-spacing: -0.02em; }
    .tagline { margin: 0; color: var(--muted); font-size: 0.95rem; }
    .grid {
      display: grid;
      grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
      gap: 14px;
    }
    .card {
      background: var(--surface);
      border: 1px solid var(--line);
      border-radius: 14px;
      padding: 14px;
      box-shadow: 0 8px 20px rgba(12, 34, 56, 0.06);
    }
    .card h2 { margin: 0 0 6px; font-size: 1.04rem; line-height: 1.25; }
    .card h2 a { color: inherit; text-decoration: none; }
    .card p { margin: 0 0 8px; color: var(--muted); line-height: 1.45; }
    .meta { font-size: 0.82rem; color: var(--accent); font-weight: 700; text-transform: uppercase; letter-spacing: 0.03em; }
    a { color: var(--accent-2); }
  </style>
  {{ block "head" . }}{{ end }}
</head>
<body>
  <div class="shell">
    <header>
      <h1 class="brand">{{ .Site.Title }}</h1>
      <p class="tagline">Eddie indexing reports from the static frontier</p>
    </header>
    {{ block "main" . }}{{ end }}
  </div>
</body>
</html>
HTML
    cat > "$SITE_ROOT/layouts/index.html" <<'HTML'
{{ define "main" }}
<section class="grid">
  {{ range first 12 (where .Site.RegularPages "Section" "posts") }}
  <article class="card">
    <h2><a href="{{ .RelPermalink }}">{{ .Title }}</a></h2>
    <p>{{ .Summary }}</p>
    <div class="meta">{{ .Date.Format "January 2, 2006" }}</div>
  </article>
  {{ end }}
</section>
{{ end }}
HTML
    cat > "$SITE_ROOT/layouts/_default/single.html" <<'HTML'
{{ define "main" }}
<article class="card">
  <h2>{{ .Title }}</h2>
  <div class="meta">{{ .Date.Format "January 2, 2006" }}</div>
  <p>{{ .Content }}</p>
</article>
{{ end }}
HTML
    cat > "$SITE_ROOT/layouts/_default/list.html" <<'HTML'
{{ define "main" }}
<section class="grid">
  {{ range .Pages }}
  <article class="card">
    <h2><a href="{{ .RelPermalink }}">{{ .Title }}</a></h2>
    <p>{{ .Summary }}</p>
  </article>
  {{ end }}
</section>
{{ end }}
HTML
    bash "$REPO_ROOT/integrations/hugo/tests/docker/seed-content.sh" "$SITE_ROOT"
    install_eddie_for_cms "hugo" "$SITE_ROOT"
    build_eddie
    "$REPO_ROOT/target/release/eddie" index --cms hugo --content-dir "$SITE_ROOT/content" --output "$SITE_ROOT/static/eddie/index.ed"
    cd "$SITE_ROOT"
    hugo >/tmp/${CMS}-build.log 2>&1 || { cat /tmp/${CMS}-build.log >&2; exit 1; }
    exec hugo server --bind 0.0.0.0 --port 1313
    ;;

  astro)
    npm create astro@latest "$SITE_ROOT" -- --template blog --yes --no-install >/tmp/${CMS}-template.log 2>&1 || { cat /tmp/${CMS}-template.log >&2; exit 1; }
    bash "$REPO_ROOT/integrations/astro/tests/docker/seed-content.sh" "$SITE_ROOT"
    cd "$SITE_ROOT"
    npm install >/tmp/${CMS}-npm-install.log 2>&1 || { cat /tmp/${CMS}-npm-install.log >&2; exit 1; }
    install_eddie_for_cms "astro" "$SITE_ROOT"
    build_eddie
    CONTENT_DIR="$SITE_ROOT/src/content"
    if [[ ! -d "$CONTENT_DIR" ]]; then CONTENT_DIR="$SITE_ROOT/src/pages"; fi
    "$REPO_ROOT/target/release/eddie" index --cms astro --content-dir "$CONTENT_DIR" --output "$SITE_ROOT/public/eddie/index.ed"
    cd "$SITE_ROOT"
    exec npm run dev -- --host 0.0.0.0 --port 4321
    ;;

  docusaurus)
    npx create-docusaurus@latest "$SITE_ROOT" classic --javascript --package-manager npm --skip-install >/tmp/${CMS}-template.log 2>&1 || { cat /tmp/${CMS}-template.log >&2; exit 1; }
    bash "$REPO_ROOT/integrations/docusaurus/tests/docker/seed-content.sh" "$SITE_ROOT"
    cd "$SITE_ROOT"
    npm install >/tmp/${CMS}-npm-install.log 2>&1 || { cat /tmp/${CMS}-npm-install.log >&2; exit 1; }
    install_eddie_for_cms "docusaurus" "$SITE_ROOT"
    build_eddie
    "$REPO_ROOT/target/release/eddie" index --cms docusaurus --content-dir "$SITE_ROOT/docs" --output "$SITE_ROOT/static/eddie/index.ed"
    cd "$SITE_ROOT"
    npm run build >/tmp/${CMS}-build.log 2>&1 || { cat /tmp/${CMS}-build.log >&2; exit 1; }
    exec npm run serve -- --host 0.0.0.0 --port 3000
    ;;

  mkdocs)
    git clone --depth 1 https://github.com/squidfunk/mkdocs-material.git "$SITE_ROOT"
    bash "$REPO_ROOT/integrations/mkdocs/tests/docker/seed-content.sh" "$SITE_ROOT"
    if [[ -f "$SITE_ROOT/requirements.txt" ]]; then
      pip3 install --break-system-packages --no-cache-dir -r "$SITE_ROOT/requirements.txt" >/tmp/${CMS}-pip.log 2>&1 || { cat /tmp/${CMS}-pip.log >&2; exit 1; }
    fi
    pip3 install --break-system-packages --no-cache-dir mkdocs mkdocs-material mkdocs-minify-plugin >/tmp/${CMS}-pip-extra.log 2>&1 || { cat /tmp/${CMS}-pip-extra.log >&2; exit 1; }
    install_eddie_for_cms "mkdocs" "$SITE_ROOT"
    build_eddie
    "$REPO_ROOT/target/release/eddie" index --cms mkdocs --content-dir "$SITE_ROOT/docs" --output "$SITE_ROOT/docs/eddie/index.ed"
    cd "$SITE_ROOT"
    mkdocs build >/tmp/${CMS}-build.log 2>&1 || { cat /tmp/${CMS}-build.log >&2; exit 1; }
    cd "$SITE_ROOT/site"
    exec python3 -m http.server 8000 --bind 0.0.0.0
    ;;

  eleventy)
    git clone --depth 1 https://github.com/11ty/eleventy-base-blog.git "$SITE_ROOT"
    bash "$REPO_ROOT/integrations/eleventy/tests/docker/seed-content.sh" "$SITE_ROOT"
    cd "$SITE_ROOT"
    npm install >/tmp/${CMS}-npm-install.log 2>&1 || { cat /tmp/${CMS}-npm-install.log >&2; exit 1; }
    install_eddie_for_cms "eleventy" "$SITE_ROOT"
    build_eddie
    "$REPO_ROOT/target/release/eddie" index --cms eleventy --content-dir "$SITE_ROOT/src" --output "$SITE_ROOT/public/eddie/index.ed"
    cd "$SITE_ROOT"
    npm run build >/tmp/${CMS}-build.log 2>&1 || { cat /tmp/${CMS}-build.log >&2; exit 1; }
    cd "$SITE_ROOT/_site"
    exec python3 -m http.server 8080 --bind 0.0.0.0
    ;;

  jekyll)
    jekyll new "$SITE_ROOT" --force >/tmp/${CMS}-template.log 2>&1 || { cat /tmp/${CMS}-template.log >&2; exit 1; }
    bash "$REPO_ROOT/integrations/jekyll/tests/docker/seed-content.sh" "$SITE_ROOT"
    cd "$SITE_ROOT"
    bundle install >/tmp/${CMS}-bundle-install.log 2>&1 || { cat /tmp/${CMS}-bundle-install.log >&2; exit 1; }
    install_eddie_for_cms "jekyll" "$SITE_ROOT"
    build_eddie
    "$REPO_ROOT/target/release/eddie" index --cms jekyll --content-dir "$SITE_ROOT" --output "$SITE_ROOT/assets/eddie/index.ed"
    cd "$SITE_ROOT"
    bundle exec jekyll build >/tmp/${CMS}-build.log 2>&1 || { cat /tmp/${CMS}-build.log >&2; exit 1; }
    cd "$SITE_ROOT/_site"
    exec python3 -m http.server 4000 --bind 0.0.0.0
    ;;

  *)
    echo "Unsupported CMS: $CMS" >&2
    exit 2
    ;;
esac
