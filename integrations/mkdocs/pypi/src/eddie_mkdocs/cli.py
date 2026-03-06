"""CLI wrapper for the Eddie MkDocs installer script."""

from __future__ import annotations

import argparse
import importlib.metadata
import os
import subprocess
import sys
from importlib import resources


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="eddie-mkdocs-install",
        description="Install Eddie assets/snippet into an MkDocs site",
    )
    parser.add_argument("site_dir", help="Path to the MkDocs site root")
    parser.add_argument(
        "asset_root",
        nargs="?",
        help="Directory containing Eddie runtime assets",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    script_path = resources.files("eddie_mkdocs").joinpath("scripts/install.sh")
    env = os.environ.copy()
    env.setdefault("EDDIE_RELEASE_VERSION", importlib.metadata.version("eddie-mkdocs"))
    cmd = [
        "bash",
        os.fspath(script_path),
        os.path.abspath(args.site_dir),
    ]
    if args.asset_root:
        cmd.append(os.path.abspath(args.asset_root))

    result = subprocess.run(
        cmd,
        check=False,
        env=env,
    )
    return result.returncode


if __name__ == "__main__":
    sys.exit(main())
