# 0210 Search Modal

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor, I type a query into the search modal and see ranked results with page titles, snippets, and clickable links.

## Key Fields/Parameters

- elements: text input, results list, close button, mode tabs (Search / Q&A)
- result item: page title, section heading, text snippet (~150 chars), link
- keyboard: Escape closes modal, Enter submits search
- responsive: works on mobile and desktop

## Acceptance Criteria

- Modal appears as an overlay panel (does not navigate away from the current page).
- Results update after the user submits a query (not on every keystroke).
- Each result is a clickable link to the source page.
- Escape key or close button dismisses the modal.
- Modal is accessible (focus trap, ARIA labels, keyboard navigation).

## Evidence

- `tests/integration/test_search_modal.js`

## Linked Tickets

- (none yet)
