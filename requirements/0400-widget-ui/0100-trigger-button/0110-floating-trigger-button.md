# 0110 Floating Trigger Button

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor, I see a small floating search button in the corner of the page that opens the search modal when clicked.

## Key Fields/Parameters

- position: bottom-right corner (configurable)
- icon: search magnifying glass
- z-index: high enough to float above site content
- size: ~48px circular button

## Acceptance Criteria

- Button is visible on all pages where the widget is embedded.
- Clicking the button opens the search modal.
- Button position is configurable (bottom-right, bottom-left).
- Button does not interfere with the site's existing UI elements.
- No models are downloaded until the user interacts with the widget.

## Evidence

- `tests/integration/test_widget_trigger.js`

## Linked Tickets

- (none yet)
