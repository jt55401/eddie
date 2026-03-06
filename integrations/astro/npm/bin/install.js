#!/usr/bin/env node

const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const pkg = require('../package.json');

const [, , siteDirArg, assetRootArg] = process.argv;

if (!siteDirArg) {
  console.error('usage: eddie-astro-install <site-dir> [asset-root]');
  process.exit(1);
}

const packageRoot = path.resolve(__dirname, '..');
const scriptPath = path.join(packageRoot, 'scripts', 'install.sh');
const siteDir = path.resolve(siteDirArg);

if (!fs.existsSync(scriptPath)) {
  console.error(`Missing installer script: ${scriptPath}`);
  process.exit(1);
}

const env = {
  ...process.env,
  EDDIE_RELEASE_VERSION: process.env.EDDIE_RELEASE_VERSION || pkg.version
};
const args = [scriptPath, siteDir];
if (assetRootArg) {
  args.push(path.resolve(assetRootArg));
}

const result = spawnSync('bash', args, { stdio: 'inherit', env });
if (result.error) {
  console.error(result.error.message);
  process.exit(1);
}
process.exit(result.status ?? 1);
