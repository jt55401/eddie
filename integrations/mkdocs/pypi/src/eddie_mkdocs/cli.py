"""CLI wrapper for the Eddie MkDocs installer script."""

from __future__ import annotations

import argparse
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
        default="/repo/dist",
        help="Directory containing Eddie runtime assets",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    script_path = resources.files("eddie_mkdocs").joinpath("scripts/install.sh")

    result = subprocess.run(
        [
            "bash",
            os.fspath(script_path),
            os.path.abspath(args.site_dir),
            os.path.abspath(args.asset_root),
        ],
        check=False,
    )
    return result.returncode


if __name__ == "__main__":
    sys.exit(main())
