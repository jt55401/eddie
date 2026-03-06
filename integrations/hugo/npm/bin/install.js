#!/usr/bin/env node

const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const [, , siteDirArg, assetRootArg] = process.argv;

if (!siteDirArg) {
  console.error('usage: eddie-hugo-install <site-dir> [asset-root]');
  process.exit(1);
}

const packageRoot = path.resolve(__dirname, '..');
const scriptPath = path.join(packageRoot, 'scripts', 'install.sh');
const siteDir = path.resolve(siteDirArg);
const assetRoot = assetRootArg ? path.resolve(assetRootArg) : '/repo/dist';

if (!fs.existsSync(scriptPath)) {
  console.error(`Missing installer script: ${scriptPath}`);
  process.exit(1);
}

const result = spawnSync('bash', [scriptPath, siteDir, assetRoot], { stdio: 'inherit' });
if (result.error) {
  console.error(result.error.message);
  process.exit(1);
}
process.exit(result.status ?? 1);
