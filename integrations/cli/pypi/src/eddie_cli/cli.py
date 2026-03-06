"""Launcher CLI that downloads and executes Eddie release binaries."""

from __future__ import annotations

import importlib.metadata
import os
import platform
import stat
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path


def resolve_asset() -> str:
    system = platform.system().lower()
    machine = platform.machine().lower()

    if system == "linux" and machine in {"x86_64", "amd64"}:
        return "eddie-linux-amd64"
    if system == "linux" and machine in {"aarch64", "arm64"}:
        return "eddie-linux-arm64"
    if system == "darwin" and machine in {"x86_64", "amd64"}:
        return "eddie-darwin-amd64"
    if system == "darwin" and machine in {"arm64", "aarch64"}:
        return "eddie-darwin-arm64"
    if system == "windows" and machine in {"x86_64", "amd64"}:
        return "eddie-windows-amd64.exe"
    if system == "windows" and machine in {"arm64", "aarch64"}:
        return "eddie-windows-arm64.exe"

    raise RuntimeError(
        f"Unsupported platform for Eddie CLI: {system}/{machine}. "
        "No release asset mapping is configured."
    )


def package_version() -> str:
    return os.environ.get("EDDIE_CLI_VERSION") or importlib.metadata.version(
        "jt55401-eddie-cli"
    )


def cache_root() -> Path:
    root = os.environ.get("EDDIE_CLI_CACHE_DIR")
    if root:
        return Path(root)
    return Path.home() / ".cache" / "eddie-cli"


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
    url = f"https://github.com/jt55401/eddie/releases/download/v{version}/{asset}"

    with tempfile.NamedTemporaryFile(dir=version_dir, delete=False) as tmp:
        temp_path = Path(tmp.name)

    try:
        print(f"Downloading Eddie CLI {version} ({asset})...", file=sys.stderr)
        with urllib.request.urlopen(url) as response, temp_path.open("wb") as out:
            out.write(response.read())
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
