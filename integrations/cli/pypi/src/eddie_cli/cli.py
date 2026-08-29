"""Launcher CLI that downloads and executes Eddie release binaries."""

from __future__ import annotations

import hashlib
import importlib.metadata
import os
import platform
import re
import stat
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path

# Asset names match the eddie release matrix: eddie-<os>-<arch>[.exe].
# Only the platforms release.yml actually builds are listed here.
_SUPPORTED_PLATFORMS = (
    "eddie-linux-x86_64, eddie-linux-aarch64, eddie-macos-x86_64, "
    "eddie-macos-aarch64, and eddie-windows-x86_64.exe"
)


def resolve_asset() -> str:
    system = platform.system().lower()
    machine = platform.machine().lower()

    if system == "linux" and machine in {"x86_64", "amd64"}:
        return "eddie-linux-x86_64"
    if system == "linux" and machine in {"aarch64", "arm64"}:
        return "eddie-linux-aarch64"
    if system == "darwin" and machine in {"x86_64", "amd64"}:
        return "eddie-macos-x86_64"
    if system == "darwin" and machine in {"arm64", "aarch64"}:
        return "eddie-macos-aarch64"
    if system == "windows" and machine in {"x86_64", "amd64"}:
        return "eddie-windows-x86_64.exe"

    raise RuntimeError(
        f"Unsupported platform for Eddie CLI: {system}/{machine}. "
        f"Eddie releases {_SUPPORTED_PLATFORMS}. Build from source for other platforms."
    )


def package_version() -> str:
    return os.environ.get("EDDIE_CLI_VERSION") or importlib.metadata.version(
        "jt55401-eddie-cli"
    )


def cache_root() -> Path:
    """OS cache directory convention, overridable with EDDIE_CLI_CACHE_DIR."""
    root = os.environ.get("EDDIE_CLI_CACHE_DIR")
    if root:
        return Path(root)

    system = platform.system().lower()
    if system == "darwin":
        return Path.home() / "Library" / "Caches" / "eddie-cli"
    if system == "windows":
        local_app_data = os.environ.get("LOCALAPPDATA")
        base = Path(local_app_data) if local_app_data else Path.home() / "AppData" / "Local"
        return base / "eddie-cli" / "Cache"
    return Path(os.environ.get("XDG_CACHE_HOME") or (Path.home() / ".cache")) / "eddie-cli"


_SHA_LINE_RE = re.compile(r"^([0-9a-f]{64})[\s*]+(.+)$")


def parse_sha256sums(text: str) -> dict[str, str]:
    """Parses `sha256sum * > SHA256SUMS` output into {filename: hex digest}."""
    sums: dict[str, str] = {}
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if len(line) < 66:
            continue
        match = _SHA_LINE_RE.match(line)
        if not match:
            continue
        digest, filename = match.group(1).lower(), match.group(2).strip()
        if filename:
            sums[filename] = digest
    return sums


def _sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def ensure_binary(version: str) -> Path:
    asset = resolve_asset()
    is_windows = asset.endswith(".exe")
    binary_name = "eddie.exe" if is_windows else "eddie"
    version_dir = cache_root() / version
    binary_path = version_dir / binary_name

    if binary_path.exists():
        binary_path.chmod(binary_path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
        return binary_path

    version_dir.mkdir(parents=True, exist_ok=True)
    release_base = f"https://github.com/jt55401/eddie/releases/download/v{version}"
    asset_url = f"{release_base}/{asset}"
    sums_url = f"{release_base}/SHA256SUMS"

    with tempfile.NamedTemporaryFile(dir=version_dir, delete=False) as tmp:
        temp_path = Path(tmp.name)

    try:
        print(f"Downloading Eddie CLI {version} ({asset})...", file=sys.stderr)
        with urllib.request.urlopen(sums_url) as response:
            sums_text = response.read().decode("utf-8", errors="replace")

        expected = parse_sha256sums(sums_text).get(asset)
        if not expected:
            raise RuntimeError(f"SHA256SUMS for v{version} has no entry for {asset}.")

        with urllib.request.urlopen(asset_url) as response, temp_path.open("wb") as out:
            out.write(response.read())

        actual = _sha256_of(temp_path)
        if actual != expected:
            raise RuntimeError(
                f"Checksum mismatch for {asset}: expected {expected}, got {actual}. "
                "Refusing to install a corrupted or tampered binary."
            )

        temp_path.chmod(temp_path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
        temp_path.replace(binary_path)
    finally:
        if temp_path.exists():
            temp_path.unlink()

    return binary_path


def main() -> int:
    version = package_version()
    binary = ensure_binary(version)
    result = subprocess.run([os.fspath(binary), *sys.argv[1:]], check=False)
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
