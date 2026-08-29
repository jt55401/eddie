# 0600 High-Level Requirements — Configuration

[Requirements Home](../0000-README.md)

There is no config file. Indexing behavior is configured via CLI flags;
widget behavior is configured via `data-*` attributes on its `<script>`
tag (or `[params.eddie]` in `hugo.toml`, which the Hugo Module partial
turns into those same attributes). Sensible defaults mean zero config
works out of the box.

## Story Index

- [0110 Embedding Model Selection](0100-model-selection/0110-embedding-model-selection.md)
- [0120 Agent Model Selection](0100-model-selection/0120-llm-model-selection.md)
- [0210 Widget Theming](0200-theming/0210-widget-theming.md)
