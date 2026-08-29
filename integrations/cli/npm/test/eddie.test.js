'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');

const { resolveAsset, cacheRoot, parseSha256Sums } = require('../bin/eddie.js');

test('resolveAsset maps every released platform/arch pair', () => {
  assert.equal(resolveAsset('linux', 'x64'), 'eddie-linux-x86_64');
  assert.equal(resolveAsset('linux', 'arm64'), 'eddie-linux-aarch64');
  assert.equal(resolveAsset('darwin', 'x64'), 'eddie-macos-x86_64');
  assert.equal(resolveAsset('darwin', 'arm64'), 'eddie-macos-aarch64');
  assert.equal(resolveAsset('win32', 'x64'), 'eddie-windows-x86_64.exe');
});

test('resolveAsset returns null for platforms release.yml does not build', () => {
  assert.equal(resolveAsset('win32', 'arm64'), null);
  assert.equal(resolveAsset('freebsd', 'x64'), null);
  assert.equal(resolveAsset('linux', 'ia32'), null);
});

test('cacheRoot honors an explicit override on every OS', () => {
  const env = { EDDIE_CLI_CACHE_DIR: '/custom/cache' };
  assert.equal(cacheRoot(env, 'linux', '/home/x'), '/custom/cache');
  assert.equal(cacheRoot(env, 'darwin', '/home/x'), '/custom/cache');
  assert.equal(cacheRoot(env, 'win32', '/home/x'), '/custom/cache');
});

test('cacheRoot uses the macOS Caches convention', () => {
  assert.equal(
    cacheRoot({}, 'darwin', '/Users/x'),
    path.join('/Users/x', 'Library', 'Caches', 'eddie-cli')
  );
});

test('cacheRoot uses LOCALAPPDATA on Windows', () => {
  assert.equal(
    cacheRoot({ LOCALAPPDATA: 'C:\\Users\\x\\AppData\\Local' }, 'win32', 'C:\\Users\\x'),
    path.join('C:\\Users\\x\\AppData\\Local', 'eddie-cli', 'Cache')
  );
});

test('cacheRoot falls back to homedir/AppData/Local on Windows without LOCALAPPDATA', () => {
  assert.equal(
    cacheRoot({}, 'win32', 'C:\\Users\\x'),
    path.join('C:\\Users\\x', 'AppData', 'Local', 'eddie-cli', 'Cache')
  );
});

test('cacheRoot honors XDG_CACHE_HOME on Linux, else ~/.cache', () => {
  assert.equal(
    cacheRoot({ XDG_CACHE_HOME: '/xdg/cache' }, 'linux', '/home/x'),
    path.join('/xdg/cache', 'eddie-cli')
  );
  assert.equal(
    cacheRoot({}, 'linux', '/home/x'),
    path.join('/home/x', '.cache', 'eddie-cli')
  );
});

test('parseSha256Sums reads standard `sha256sum *` output (binary mode)', () => {
  const hashA = 'a'.repeat(64);
  const hashB = 'b'.repeat(64);
  const text = `${hashA} *eddie-linux-x86_64\n${hashB} *eddie-macos-aarch64\n`;
  const sums = parseSha256Sums(text);
  assert.equal(sums.get('eddie-linux-x86_64'), hashA);
  assert.equal(sums.get('eddie-macos-aarch64'), hashB);
  assert.equal(sums.size, 2);
});

test('parseSha256Sums reads text-mode output (two spaces) and ignores blank lines', () => {
  const hash = 'c'.repeat(64);
  const text = `\n${hash}  eddie-windows-x86_64.exe\n\n`;
  const sums = parseSha256Sums(text);
  assert.equal(sums.get('eddie-windows-x86_64.exe'), hash);
  assert.equal(sums.size, 1);
});

test('parseSha256Sums ignores malformed lines', () => {
  const sums = parseSha256Sums('not-a-hash eddie-linux-x86_64\n');
  assert.equal(sums.size, 0);
});
