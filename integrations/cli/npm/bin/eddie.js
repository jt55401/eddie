#!/usr/bin/env node

const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const https = require('node:https');
const { spawnSync } = require('node:child_process');

const pkg = require('../package.json');

function resolveAsset(platform, arch) {
  if (platform === 'linux' && arch === 'x64') return 'eddie-linux-amd64';
  if (platform === 'linux' && arch === 'arm64') return 'eddie-linux-arm64';
  if (platform === 'darwin' && arch === 'x64') return 'eddie-darwin-amd64';
  if (platform === 'darwin' && arch === 'arm64') return 'eddie-darwin-arm64';
  if (platform === 'win32' && arch === 'x64') return 'eddie-windows-amd64.exe';
  if (platform === 'win32' && arch === 'arm64') return 'eddie-windows-arm64.exe';
  return null;
}

function downloadFile(url, dest, redirects = 0) {
  if (redirects > 5) {
    throw new Error(`Too many redirects while downloading ${url}`);
  }

  return new Promise((resolve, reject) => {
    const request = https.get(
      url,
      {
        headers: {
          'User-Agent': '@jt55401/eddie-cli',
          Accept: 'application/octet-stream'
        }
      },
      (response) => {
        if (
          response.statusCode &&
          response.statusCode >= 300 &&
          response.statusCode < 400 &&
          response.headers.location
        ) {
          response.resume();
          const nextUrl = new URL(response.headers.location, url).toString();
          downloadFile(nextUrl, dest, redirects + 1).then(resolve).catch(reject);
          return;
        }

        if (response.statusCode !== 200) {
          response.resume();
          reject(new Error(`Download failed (${response.statusCode}): ${url}`));
          return;
        }

        const out = fs.createWriteStream(dest, { mode: 0o755 });
        response.pipe(out);
        out.on('finish', () => out.close(resolve));
        out.on('error', reject);
      }
    );

    request.on('error', reject);
  });
}

async function ensureBinary(version) {
  const asset = resolveAsset(process.platform, process.arch);
  if (!asset) {
    throw new Error(
      `Unsupported platform for Eddie CLI: ${process.platform}/${process.arch}. ` +
        'No release asset mapping is configured.'
    );
  }

  const cacheRoot =
    process.env.EDDIE_CLI_CACHE_DIR || path.join(os.homedir(), '.cache', 'eddie-cli');
  const versionDir = path.join(cacheRoot, version);
  const binName = process.platform === 'win32' ? 'eddie.exe' : 'eddie';
  const binPath = path.join(versionDir, binName);

  if (fs.existsSync(binPath)) {
    fs.chmodSync(binPath, 0o755);
    return binPath;
  }

  fs.mkdirSync(versionDir, { recursive: true });
  const tempPath = `${binPath}.tmp`;
  const url = `https://github.com/jt55401/eddie/releases/download/v${version}/${asset}`;

  process.stderr.write(`Downloading Eddie CLI ${version} (${asset})...\n`);
  try {
    await downloadFile(url, tempPath);
    fs.renameSync(tempPath, binPath);
    fs.chmodSync(binPath, 0o755);
  } finally {
    if (fs.existsSync(tempPath)) {
      fs.unlinkSync(tempPath);
    }
  }

  return binPath;
}

async function main() {
  const version = process.env.EDDIE_CLI_VERSION || pkg.version;
  const binPath = await ensureBinary(version);
  const result = spawnSync(binPath, process.argv.slice(2), { stdio: 'inherit' });

  if (result.error) {
    process.stderr.write(`${result.error.message}\n`);
    process.exit(1);
  }

  process.exit(result.status ?? 1);
}

main().catch((err) => {
  process.stderr.write(`${err.message}\n`);
  process.exit(1);
});
