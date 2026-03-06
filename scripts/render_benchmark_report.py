#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
#
# Convenience wrapper for benchmark report rendering.
#
# Usage:
#   python3 scripts/render_benchmark_report.py .bench/results/<run_id>

from __future__ import annotations

import argparse
from pathlib import Path

from benchmark_suite import render_markdown_report


def main() -> int:
    parser = argparse.ArgumentParser(description="Render benchmark markdown report from CSV outputs")
    parser.add_argument("run_dir", help="Benchmark run directory")
    parser.add_argument("--output", default="", help="Optional markdown output path")
    args = parser.parse_args()

    run_dir = Path(args.run_dir).resolve()
    out = Path(args.output).resolve() if args.output else None
    report = render_markdown_report(run_dir=run_dir, output_path=out)
    print(report)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
