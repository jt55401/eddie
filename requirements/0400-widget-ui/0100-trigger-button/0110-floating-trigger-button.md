# 0110 Floating Trigger Button

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor, I see a small floating search button in the corner of
the page, and I can also open search with a keyboard shortcut, without
triggering any model download until I actually interact with it.

## Key Fields/Parameters

- position: `data-position` (`top-left`, `top-right`, `bottom-left`, `bottom-right`; default `bottom-right`)
- icon: search magnifying glass
- z-index: high enough to float above site content
- size: ~48px circular button
- keyboard shortcut: Ctrl/Cmd+K opens the widget, compared case-insensitively on `e.key`; ignored while an editable element (input, textarea, contenteditable) already has focus, so it doesn't steal keystrokes from the site's own forms

## Acceptance Criteria

- Button is visible on all pages where the widget is embedded.
- Clicking the button, or pressing Ctrl/Cmd+K outside an editable element, opens the search modal.
- Button position is configurable via `data-position`.
- Button does not interfere with the site's existing UI elements.
- No models are downloaded until the user interacts with the widget (opening it fetches only the index manifest, not any model).

## Evidence

- `tests/integration/test_widget_trigger.js`

## Linked Tickets

- (none yet)
