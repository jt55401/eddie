#!/usr/bin/env python3
"""Heuristic conflict detection for requirements docs.

Checks for:
- broken relative markdown links inside requirements/
- duplicate story titles
- duplicate story ids within an area
- endpoint overlap that likely indicates duplicate/conflicting requirements
- missing required story sections (warning)
"""

from __future__ import annotations

import argparse
import itertools
import re
import subprocess
import sys
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path


RE_LINK = re.compile(r"\[[^\]]+\]\(([^)]+)\)")
RE_TITLE = re.compile(r"^#\s+(.+?)\s*$", re.MULTILINE)
RE_SECTION = re.compile(r"^##\s+(.+?)\s*$", re.MULTILINE)
RE_ENDPOINT_TICK = re.compile(r"`(/[^`\s]+)`")

STOPWORDS = {
    "the", "and", "for", "with", "from", "that", "this", "into",
    "when", "then", "must", "should", "will", "user", "users",
}

REQUIRED_SECTIONS = [
    "User Story",
    "Key Fields/Parameters",
    "Acceptance Criteria",
    "Evidence",
    "Linked Tickets",
]


@dataclass
class Story:
    path: Path
    area: str
    story_id: str
    title: str
    sections: dict[str, str]
    endpoints: set[str]


def run_git_changed(range_expr: str, root: Path) -> set[Path]:
    cmd = ["git", "diff", "--name-only", range_expr, "--", str(root)]
    proc = subprocess.run(cmd, capture_output=True, text=True, check=False)
    if proc.returncode != 0:
        return set()
    out = set()
    for line in proc.stdout.splitlines():
        p = Path(line.strip())
        if p.suffix == ".md":
            out.add(p)
    return out


def split_sections(text: str) -> dict[str, str]:
    matches = list(RE_SECTION.finditer(text))
    if not matches:
        return {}
    sections: dict[str, str] = {}
    for idx, m in enumerate(matches):
        title = m.group(1).strip()
        start = m.end()
        end = matches[idx + 1].start() if idx + 1 < len(matches) else len(text)
        sections[title] = text[start:end].strip()
    return sections


def normalize_title(title: str) -> str:
    return re.sub(r"[^a-z0-9]+", "", title.lower())


def tokenize(text: str) -> set[str]:
    words = re.findall(r"[a-zA-Z][a-zA-Z0-9_-]{2,}", text.lower())
    return {w for w in words if w not in STOPWORDS}


def jaccard(a: set[str], b: set[str]) -> float:
    if not a or not b:
        return 0.0
    union = a | b
    if not union:
        return 0.0
    return len(a & b) / len(union)


def parse_story(path: Path, root: Path) -> Story | None:
    rel = path.relative_to(root)
    parts = rel.parts
    if len(parts) < 3:
        return None

    text = path.read_text(encoding="utf-8", errors="replace")
    tm = RE_TITLE.search(text)
    title = tm.group(1).strip() if tm else path.stem

    sections = split_sections(text)
    key_fields = sections.get("Key Fields/Parameters", "")

    def _clean_endpoint(raw: str) -> str:
        return raw.strip().rstrip("`.,);")

    endpoints = {_clean_endpoint(ep) for ep in RE_ENDPOINT_TICK.findall(key_fields)}
    endpoints = {ep for ep in endpoints if ep}

    story_id_match = re.match(r"^(\d{4})-", path.stem)
    story_id = story_id_match.group(1) if story_id_match else "0000"

    return Story(
        path=path,
        area=parts[0],
        story_id=story_id,
        title=title,
        sections=sections,
        endpoints=endpoints,
    )


def gather_markdown_files(root: Path) -> list[Path]:
    files: list[Path] = []
    for p in root.rglob("*.md"):
        if not p.is_file():
            continue
        name = p.name
        if name.startswith("._") or name in {".DS_Store", "._.DS_Store"}:
            continue
        files.append(p)
    return sorted(files)


def check_broken_links(files: list[Path]) -> list[str]:
    errors: list[str] = []
    for f in files:
        text = f.read_text(encoding="utf-8", errors="replace")
        for target in RE_LINK.findall(text):
            target = target.strip()
            if not target or target.startswith(("http://", "https://", "mailto:", "#")):
                continue
            target_no_anchor = target.split("#", 1)[0]
            if not target_no_anchor:
                continue
            resolved = (f.parent / target_no_anchor).resolve()
            if not resolved.exists():
                errors.append(f"BROKEN LINK: {f} -> {target}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description="Requirements conflict checker")
    parser.add_argument("--root", default="requirements", help="Requirements root directory")
    parser.add_argument("--range", default="HEAD~1..HEAD", help="Git range for --changed-only mode")
    parser.add_argument("--changed-only", action="store_true", help="Only check stories changed in git range")
    parser.add_argument("--strict-warnings", action="store_true", help="Exit non-zero on warnings")
    args = parser.parse_args()

    root = Path(args.root)
    if not root.exists():
        print(f"Root not found: {root}", file=sys.stderr)
        return 2

    all_md = gather_markdown_files(root)
    link_errors = check_broken_links(all_md)

    stories: list[Story] = []
    for p in all_md:
        rel = p.relative_to(root)
        if rel.name in {"0000-README.md", "CHANGELOG.md"}:
            continue
        if rel.name == "0000-high-level-requirements.md":
            continue
        story = parse_story(p, root)
        if story is not None:
            stories.append(story)

    changed_set: set[Path] = set()
    if args.changed_only:
        changed_paths = run_git_changed(args.range, root)
        changed_set = {Path.cwd() / p for p in changed_paths}

    errors: list[str] = []
    warnings: list[str] = []
    errors.extend(link_errors)

    # Duplicate story IDs within same area
    by_area_id: dict[tuple[str, str], list[Story]] = defaultdict(list)
    for s in stories:
        by_area_id[(s.area, s.story_id)].append(s)
    for (area, sid), group in by_area_id.items():
        if len(group) > 1:
            paths = ", ".join(str(s.path) for s in group)
            errors.append(f"DUPLICATE STORY ID: {area}/{sid} in {paths}")

    # Duplicate titles
    by_title: dict[str, list[Story]] = defaultdict(list)
    for s in stories:
        by_title[normalize_title(s.title)].append(s)
    for key, group in by_title.items():
        if key and len(group) > 1:
            paths = ", ".join(str(s.path) for s in group)
            errors.append(f"DUPLICATE TITLE: '{group[0].title}' in {paths}")

    # Missing required sections
    for s in stories:
        missing = [sec for sec in REQUIRED_SECTIONS if sec not in s.sections]
        if missing:
            warnings.append(f"MISSING SECTIONS: {s.path} missing {', '.join(missing)}")

    # Endpoint overlap
    endpoint_map: dict[str, list[Story]] = defaultdict(list)
    for s in stories:
        for ep in s.endpoints:
            endpoint_map[ep].append(s)

    for ep, group in endpoint_map.items():
        if len(group) < 2:
            continue
        if args.changed_only and changed_set:
            if not any((s.path.resolve() in {p.resolve() for p in changed_set}) for s in group):
                continue
        paths = ", ".join(str(s.path) for s in group)
        warnings.append(f"ENDPOINT OVERLAP: {ep} appears in {len(group)} stories -> {paths}")

        for a, b in itertools.combinations(group, 2):
            acc_a = a.sections.get("Acceptance Criteria", "")
            acc_b = b.sections.get("Acceptance Criteria", "")
            sim = jaccard(tokenize(acc_a), tokenize(acc_b))
            if sim >= 0.75:
                errors.append(
                    f"POSSIBLE DUPLICATE REQUIREMENTS: {a.path} and {b.path} "
                    f"share endpoint {ep} with high acceptance similarity ({sim:.2f})"
                )

    if errors:
        print("Errors:")
        for e in errors:
            print(f"- {e}")

    if warnings:
        print("Warnings:")
        for w in warnings:
            print(f"- {w}")

    if errors:
        return 1
    if warnings and args.strict_warnings:
        return 1

    print("Requirements conflict check passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
