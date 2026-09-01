# 0110 Floating Trigger Button

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor, I see a small floating search button in the corner of
the page, and I can also open search with a keyboard shortcut, without
triggering any model download until I actually interact with it, and
without paying for the full widget just to have that button drawn.

## Key Fields/Parameters

- position: `data-position` (`top-left`, `top-right`, `bottom-left`, `bottom-right`; default `bottom-right`)
- icon: search magnifying glass
- z-index: high enough to float above site content
- size: ~48px circular button
- keyboard shortcut: Ctrl/Cmd+K opens the widget, compared case-insensitively on `e.key`; ignored while an editable element (input, textarea, contenteditable) already has focus, so it doesn't steal keystrokes from the site's own forms
- default renderer: `eddie-boot.js` (about 3.2 KB brotli), a separate script from `eddie-widget.js`, draws this button and installs the Ctrl/Cmd+K listener in its own closed Shadow DOM, reading the same `data-*` attributes (position, theme, offsets) the full widget reads; it is the script the module partial and every CMS integration put on the page by default (`loader = "boot"`, see [0110 Hugo Integration](../../0500-integration/0100-hugo/0110-hugo-integration.md))
- widget hand-over: a click, the shortcut, or `window.eddie.open()` injects `<script src="eddie-widget.js">` with the boot script's attributes copied over; hovering or focusing the trigger only preloads it; the full widget removes the boot trigger when it mounts and opens the modal if that is what the visitor asked for, so exactly one button exists at a time
- warm-up hand-over: `data-warm` (`auto` default, `off`, `always`) — `auto` also loads the full widget after `load` (idle callback) for a visitor who has opened search or accepted a model before on this browser (`localStorage` `eddie.search.used` / `eddie.search.consent`), unless Data Saver or `prefers-reduced-data` is set; `always` loads it for every visitor; `off` waits for interaction only
- `loader = "full"` (Hugo param) skips the boot script and puts `eddie-widget.js` on every page view instead; the button behaves identically either way

## Acceptance Criteria

- Button is visible on all pages where the widget is embedded.
- Clicking the button, or pressing Ctrl/Cmd+K outside an editable element, opens the search modal.
- Button position is configurable via `data-position`.
- Button does not interfere with the site's existing UI elements.
- No models are downloaded until the user interacts with the widget (opening it fetches only the index manifest, not any model).
- With the default loader, a plain page view fetches only `eddie-boot.js`; the full widget (`eddie-widget.js`) is not fetched until the visitor interacts with the trigger or shortcut, `window.eddie.open()` is called, or the `data-warm` warm-up condition above is met.
- A page that loads both the boot script and the full widget (e.g. `loader = "full"` plus a stray boot tag) mounts the button exactly once; the second mount is a no-op.

## Evidence

- `tests/integration/test_widget_trigger.js`
- `widget/src/lib/boot.js`, `widget/test/boot.test.js`
- `widget/README.md` — [Loader](../../../widget/README.md#loader)

## Linked Tickets

- (none yet)
