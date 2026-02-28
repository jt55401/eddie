#!/usr/bin/env python3
"""Extract requirements-related conventional commits into markdown changelog entries."""

from __future__ import annotations

import argparse
import datetime as dt
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


CC_RE = re.compile(
    r"^(?P<type>feat|fix|docs|chore|refactor|test)(?:\((?P<scope>[^)]+)\))?(?P<breaking>!)?: (?P<summary>.+)$"
)


@dataclass
class CommitEntry:
    sha: str
    ctype: str
    scope: str
    summary: str
    req_refs: str
    req_summary: str


def run_git_log(git_range: str) -> str:
    cmd = ["git", "log", "--pretty=format:%h%x1f%s%x1f%b%x1e", git_range]
    try:
        res = subprocess.run(cmd, check=True, text=True, capture_output=True)
    except subprocess.CalledProcessError as exc:
        print(exc.stderr.strip() or str(exc), file=sys.stderr)
        raise
    return res.stdout


def parse_footer(body: str, key: str) -> str:
    pattern = re.compile(rf"^{re.escape(key)}:\s*(.+)$", re.MULTILINE)
    match = pattern.search(body)
    return match.group(1).strip() if match else ""


def is_requirements_commit(scope: str, body: str) -> bool:
    normalized = scope.lower()
    if "requirement" in normalized or normalized.startswith("req"):
        return True
    if parse_footer(body, "Changelog").lower() == "requirements":
        return True
    if parse_footer(body, "Req-Refs"):
        return True
    return False


def collect_entries(raw: str) -> list[CommitEntry]:
    entries: list[CommitEntry] = []
    for record in raw.split("\x1e"):
        record = record.strip()
        if not record:
            continue
        parts = record.split("\x1f")
        if len(parts) < 3:
            continue
        sha, subject, body = parts[0].strip(), parts[1].strip(), parts[2].strip()
        cc_match = CC_RE.match(subject)
        if not cc_match:
            continue
        ctype = cc_match.group("type")
        scope = (cc_match.group("scope") or "").strip()
        summary = cc_match.group("summary").strip()
        if not is_requirements_commit(scope, body):
            continue
        req_refs = parse_footer(body, "Req-Refs")
        req_summary = parse_footer(body, "Req-Summary")
        entries.append(CommitEntry(sha=sha, ctype=ctype, scope=scope, summary=summary, req_refs=req_refs, req_summary=req_summary))
    return entries


def section_name(ctype: str) -> str:
    if ctype == "feat":
        return "Added"
    if ctype == "fix":
        return "Fixed"
    return "Changed"


def render_markdown(entries: list[CommitEntry], as_of: dt.date) -> str:
    if not entries:
        return ""
    buckets: dict[str, list[CommitEntry]] = {"Added": [], "Fixed": [], "Changed": []}
    for entry in entries:
        buckets[section_name(entry.ctype)].append(entry)
    lines: list[str] = [f"### {as_of.isoformat()}"]
    for section in ("Added", "Fixed", "Changed"):
        if not buckets[section]:
            continue
        lines.append(f"#### {section}")
        for entry in buckets[section]:
            text = entry.req_summary or entry.summary
            suffix_parts = [f"`{entry.sha}`"]
            if entry.req_refs:
                suffix_parts.append(f"Req-Refs: {entry.req_refs}")
            lines.append(f"- {text} ({'; '.join(suffix_parts)})")
        lines.append("")
    if lines and lines[-1] == "":
        lines.pop()
    return "\n".join(lines)


def ensure_changelog_file(path: Path) -> None:
    if path.exists():
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    template = "# Requirements Changelog\n\n## Unreleased\n\n"
    path.write_text(template, encoding="utf-8")


def apply_to_changelog(path: Path, snippet: str) -> None:
    ensure_changelog_file(path)
    content = path.read_text(encoding="utf-8")
    marker = "## Unreleased"
    idx = content.find(marker)
    if idx == -1:
        content = content.rstrip() + "\n\n## Unreleased\n\n"
        idx = content.find(marker)
    insert_pos = content.find("\n", idx)
    if insert_pos == -1:
        insert_pos = len(content)
    insert_pos += 1
    updated = content[:insert_pos] + "\n" + snippet + "\n" + content[insert_pos:]
    path.write_text(updated, encoding="utf-8")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--range", default="HEAD", help="Git revision range to inspect")
    parser.add_argument("--changelog-file", default="requirements/CHANGELOG.md", help="Target changelog file")
    parser.add_argument("--apply", action="store_true", help="Insert into changelog file")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        raw = run_git_log(args.range)
    except subprocess.CalledProcessError:
        return 2
    entries = collect_entries(raw)
    snippet = render_markdown(entries, dt.date.today())
    if not snippet:
        print("No requirements-related conventional commits found in range.")
        return 0
    if args.apply:
        changelog_path = Path(args.changelog_file)
        apply_to_changelog(changelog_path, snippet)
        print(f"Updated {changelog_path} with requirements changelog section.")
        return 0
    print(snippet)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
