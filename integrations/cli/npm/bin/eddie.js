#!/usr/bin/env node

const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const https = require('node:https');
const { spawnSync } = require('node:child_process');

const pkg = require('../package.json');

// Asset names match the eddie release matrix: eddie-<os>-<arch>[.exe].
// Only the platforms release.yml actually builds are listed here.
function resolveAsset(platform, arch) {
  if (platform === 'linux' && arch === 'x64') return 'eddie-linux-x86_64';
  if (platform === 'linux' && arch === 'arm64') return 'eddie-linux-aarch64';
  if (platform === 'darwin' && arch === 'x64') return 'eddie-macos-x86_64';
  if (platform === 'darwin' && arch === 'arm64') return 'eddie-macos-aarch64';
  if (platform === 'win32' && arch === 'x64') return 'eddie-windows-x86_64.exe';
  return null;
}

// OS cache directory convention, overridable with EDDIE_CLI_CACHE_DIR.
function cacheRoot(env = process.env, platform = process.platform, home = os.homedir()) {
  if (env.EDDIE_CLI_CACHE_DIR) return env.EDDIE_CLI_CACHE_DIR;
  if (platform === 'darwin') return path.join(home, 'Library', 'Caches', 'eddie-cli');
  if (platform === 'win32') {
    return path.join(env.LOCALAPPDATA || path.join(home, 'AppData', 'Local'), 'eddie-cli', 'Cache');
  }
  return path.join(env.XDG_CACHE_HOME || path.join(home, '.cache'), 'eddie-cli');
}

// Parses `sha256sum * > SHA256SUMS` output into a Map of filename -> hex digest.
function parseSha256Sums(text) {
  const sums = new Map();
  for (const rawLine of text.split('\n')) {
    const line = rawLine.trim();
    if (line.length < 66) continue;
    const hash = line.slice(0, 64).toLowerCase();
    if (!/^[0-9a-f]{64}$/.test(hash)) continue;
    const filename = line.slice(64).replace(/^[\s*]+/, '').trim();
    if (filename) sums.set(filename, hash);
  }
  return sums;
}

function sha256File(filePath) {
  return new Promise((resolve, reject) => {
    const hash = crypto.createHash('sha256');
    const stream = fs.createReadStream(filePath);
    stream.on('error', reject);
    stream.on('data', (chunk) => hash.update(chunk));
    stream.on('end', () => resolve(hash.digest('hex')));
  });
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

        const out = fs.createWriteStream(dest);
        response.pipe(out);
        out.on('finish', () => out.close(resolve));
        out.on('error', reject);
      }
    );

    request.on('error', reject);
  });
}

function downloadText(url) {
  return new Promise((resolve, reject) => {
    const request = https.get(
      url,
      { headers: { 'User-Agent': '@jt55401/eddie-cli' } },
      (response) => {
        if (
          response.statusCode &&
          response.statusCode >= 300 &&
          response.statusCode < 400 &&
          response.headers.location
        ) {
          response.resume();
          downloadText(new URL(response.headers.location, url).toString())
            .then(resolve)
            .catch(reject);
          return;
        }
        if (response.statusCode !== 200) {
          response.resume();
          reject(new Error(`Download failed (${response.statusCode}): ${url}`));
          return;
        }
        let body = '';
        response.setEncoding('utf8');
        response.on('data', (chunk) => (body += chunk));
        response.on('end', () => resolve(body));
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
        'Eddie releases eddie-linux-x86_64, eddie-linux-aarch64, eddie-macos-x86_64, ' +
        'eddie-macos-aarch64, and eddie-windows-x86_64.exe. Build from source for other platforms.'
    );
  }

  const cacheDir = cacheRoot();
  const versionDir = path.join(cacheDir, version);
  const binName = process.platform === 'win32' ? 'eddie.exe' : 'eddie';
  const binPath = path.join(versionDir, binName);

  if (fs.existsSync(binPath)) {
    fs.chmodSync(binPath, 0o755);
    return binPath;
  }

  fs.mkdirSync(versionDir, { recursive: true });
  const releaseBase = `https://github.com/jt55401/eddie/releases/download/v${version}`;
  const assetUrl = `${releaseBase}/${asset}`;
  const sumsUrl = `${releaseBase}/SHA256SUMS`;
  const tempPath = `${binPath}.tmp`;

  process.stderr.write(`Downloading Eddie CLI ${version} (${asset})...\n`);
  try {
    const [, sumsText] = await Promise.all([
      downloadFile(assetUrl, tempPath),
      downloadText(sumsUrl)
    ]);

    const expected = parseSha256Sums(sumsText).get(asset);
    if (!expected) {
      throw new Error(`SHA256SUMS for v${version} has no entry for ${asset}.`);
    }

    const actual = await sha256File(tempPath);
    if (actual !== expected) {
      throw new Error(
        `Checksum mismatch for ${asset}: expected ${expected}, got ${actual}. ` +
          'Refusing to install a corrupted or tampered binary.'
      );
    }

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

module.exports = { resolveAsset, cacheRoot, parseSha256Sums, sha256File };

if (require.main === module) {
  main().catch((err) => {
    process.stderr.write(`${err.message}\n`);
    process.exit(1);
  });
}
