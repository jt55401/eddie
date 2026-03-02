# 0210 Widget Theming

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site owner, I can customize the widget's appearance to match my site's design.

## Key Fields/Parameters

- config: CSS custom properties (variables) for colors, fonts, border-radius
- options: `theme: "light" | "dark" | "auto"` (auto follows `prefers-color-scheme`)
- options: `position: "bottom-right" | "bottom-left"`
- override: site owners can target `.eddie-*` CSS classes

## Acceptance Criteria

- The widget uses CSS custom properties for all visual values.
- Light and dark themes are built in; auto-detection is the default.
- Site owners can override styles via standard CSS specificity.
- The widget's CSS does not leak into or conflict with the host site's styles.

## Evidence

- `tests/integration/test_theming.js`

## Linked Tickets

- (none yet)
