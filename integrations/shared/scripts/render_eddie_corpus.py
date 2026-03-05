#!/usr/bin/env python3
"""Render shared Eddie content corpus into CMS-specific files."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


def _q(text: str) -> str:
    return text.replace('"', '\\"')


def _yaml_list(values: list[str]) -> str:
    return "[" + ", ".join(f'\"{_q(v)}\"' for v in values) + "]"


def _toml_list(values: list[str]) -> str:
    return "[" + ", ".join(f'\"{_q(v)}\"' for v in values) + "]"


def _paragraphs(lines: list[str]) -> str:
    return "\n\n".join(lines).strip() + "\n"


def _load_corpus(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def _write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def render_hugo(site_root: Path, corpus: dict[str, Any]) -> None:
    about = corpus["about"]
    posts = corpus["posts"]

    about_text = (
        "+++\n"
        f"title = \"{_q(about['title'])}\"\n"
        f"description = \"{_q(about['description'])}\"\n"
        f"date = \"{about['date']}\"\n"
        f"tags = {_toml_list(about.get('tags', []))}\n"
        "+++\n\n"
        + _paragraphs(about["body"])
    )
    _write(site_root / "content" / "about" / "_index.md", about_text)

    for post in posts:
        post_text = (
            "+++\n"
            f"title = \"{_q(post['title'])}\"\n"
            f"date = \"{post['date']}\"\n"
            f"tags = {_toml_list(post.get('tags', []))}\n"
            "+++\n\n"
            + _paragraphs(post["body"])
        )
        _write(site_root / "content" / "posts" / f"{post['slug']}.md", post_text)


def render_astro(site_root: Path, corpus: dict[str, Any]) -> None:
    about = corpus["about"]
    posts = corpus["posts"]

    about_text = (
        "---\n"
        f"title: \"{_q(about['title'])}\"\n"
        f"description: \"{_q(about['description'])}\"\n"
        f"date: \"{about['date']}\"\n"
        f"tags: {_yaml_list(about.get('tags', []))}\n"
        "---\n\n"
        + _paragraphs(about["body"])
    )
    _write(site_root / "src" / "content" / "pages" / "about-eddie.md", about_text)

    for post in posts:
        post_text = (
            "---\n"
            f"title: \"{_q(post['title'])}\"\n"
            f"description: \"{_q(post['summary'])}\"\n"
            f"date: \"{post['date']}\"\n"
            f"tags: {_yaml_list(post.get('tags', []))}\n"
            "draft: false\n"
            "---\n\n"
            + _paragraphs(post["body"])
        )
        _write(site_root / "src" / "content" / "eddie" / f"{post['slug']}.md", post_text)


def render_docusaurus(site_root: Path, corpus: dict[str, Any]) -> None:
    about = corpus["about"]
    posts = corpus["posts"]

    about_text = (
        "---\n"
        f"title: \"{_q(about['title'])}\"\n"
        "slug: /eddie/about\n"
        f"description: \"{_q(about['description'])}\"\n"
        "---\n\n"
        + _paragraphs(about["body"])
    )
    _write(site_root / "docs" / "eddie" / "about.md", about_text)

    for post in posts:
        post_text = (
            "---\n"
            f"title: \"{_q(post['title'])}\"\n"
            f"slug: \"/eddie/{post['slug']}\"\n"
            f"date: \"{post['date']}\"\n"
            f"description: \"{_q(post['summary'])}\"\n"
            f"tags: {_yaml_list(post.get('tags', []))}\n"
            "---\n\n"
            + _paragraphs(post["body"])
        )
        _write(site_root / "docs" / "eddie" / f"{post['slug']}.md", post_text)


def render_mkdocs(site_root: Path, corpus: dict[str, Any]) -> None:
    about = corpus["about"]
    posts = corpus["posts"]

    about_text = f"# {about['title']}\n\n" + _paragraphs(about["body"])
    _write(site_root / "docs" / "eddie" / "about.md", about_text)

    for index, post in enumerate(posts, start=1):
        body = _paragraphs(post["body"]).rstrip()
        post_text = (
            f"# {post['title']}\n\n"
            f"_{post['date']} · {post['summary']}_\n\n"
            f"{body}\n"
        )
        _write(site_root / "docs" / "eddie" / f"{index:02d}-{post['slug']}.md", post_text)


def render_eleventy(site_root: Path, corpus: dict[str, Any]) -> None:
    about = corpus["about"]
    posts = corpus["posts"]

    about_text = (
        "---\n"
        "layout: layouts/page.njk\n"
        f"title: {_q(about['title'])}\n"
        "permalink: /about-eddie/\n"
        "---\n\n"
        + _paragraphs(about["body"])
    )
    _write(site_root / "src" / "about-eddie.md", about_text)

    for post in posts:
        tags = post.get("tags", []) + ["post", "eddie"]
        unique_tags: list[str] = []
        for tag in tags:
            if tag not in unique_tags:
                unique_tags.append(tag)
        tag_lines = "\n".join(f"  - {_q(tag)}" for tag in unique_tags)

        post_text = (
            "---\n"
            "layout: layouts/post.njk\n"
            f"title: \"{_q(post['title'])}\"\n"
            f"description: \"{_q(post['summary'])}\"\n"
            f"date: \"{post['date']}\"\n"
            "tags:\n"
            f"{tag_lines}\n"
            "---\n\n"
            + _paragraphs(post["body"])
        )
        _write(site_root / "src" / "posts" / f"{post['date']}-{post['slug']}.md", post_text)


def render_jekyll(site_root: Path, corpus: dict[str, Any]) -> None:
    about = corpus["about"]
    posts = corpus["posts"]

    about_text = (
        "---\n"
        "layout: page\n"
        f"title: {_q(about['title'])}\n"
        "permalink: /about-eddie/\n"
        "---\n\n"
        + _paragraphs(about["body"])
    )
    _write(site_root / "about-eddie.md", about_text)

    for post in posts:
        post_text = (
            "---\n"
            "layout: post\n"
            f"title: \"{_q(post['title'])}\"\n"
            f"description: \"{_q(post['summary'])}\"\n"
            f"tags: {_yaml_list(post.get('tags', []))}\n"
            "---\n\n"
            + _paragraphs(post["body"])
        )
        _write(site_root / "_posts" / f"{post['date']}-{post['slug']}.md", post_text)


RENDERERS = {
    "hugo": render_hugo,
    "astro": render_astro,
    "docusaurus": render_docusaurus,
    "mkdocs": render_mkdocs,
    "eleventy": render_eleventy,
    "jekyll": render_jekyll,
}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cms", required=True, choices=sorted(RENDERERS.keys()))
    parser.add_argument("--site-root", required=True)
    parser.add_argument(
        "--corpus-file",
        default=str(Path(__file__).resolve().parents[1] / "content" / "eddie-corpus.json"),
    )
    args = parser.parse_args()

    site_root = Path(args.site_root).resolve()
    corpus_file = Path(args.corpus_file).resolve()
    corpus = _load_corpus(corpus_file)

    RENDERERS[args.cms](site_root, corpus)


if __name__ == "__main__":
    main()
