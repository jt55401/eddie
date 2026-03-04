#!/usr/bin/env bash
set -euo pipefail

SITE_DIR="${1:?usage: install.sh <docusaurus-site-dir>}"
WIDGET_SRC="${2:-/repo/widget/src/eddie-widget.js}"

mkdir -p "$SITE_DIR/static/eddie"
cp "$WIDGET_SRC" "$SITE_DIR/static/eddie/eddie-widget.js"

mkdir -p "$SITE_DIR/src/theme"
cat > "$SITE_DIR/src/theme/Root.js" <<'ROOT'
import React from 'react';
import Head from '@docusaurus/Head';

export default function Root({children}) {
  return (
    <>
      <Head>
        <script defer src="/eddie/eddie-widget.js" />
      </Head>
      {children}
    </>
  );
}
ROOT
