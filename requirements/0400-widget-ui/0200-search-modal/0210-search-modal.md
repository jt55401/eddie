# 0210 Search Modal

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor, I type a query into the search modal and see ranked
results update as I type, with page titles, snippets, and clickable links.

## Key Fields/Parameters

- elements: text input (a combobox), results list, close button, status region
- debounce: results update 200ms after the query reaches 2+ characters, not on submit and not on every single keystroke
- result item: page title, section heading, text snippet (≤ 180 chars), link
- keyboard: Escape closes the modal; the close button is labelled "Close (Esc)"; Tab reaches the input, results, citations (when an agent answer is shown), and footer in order
- accessibility: `aria-expanded`/`aria-controls`/`aria-activedescendant` on the combobox, `aria-live="polite"` status region for load/error/degraded states
- responsive: works on mobile and desktop
- theme: `data-theme` (`light`, `dark`, `auto`) applied to the modal root

## Acceptance Criteria

- Modal appears as an overlay panel (does not navigate away from the current page).
- Results update automatically as the user types (debounced), not only after an explicit submit.
- There are no separate "Search" / "Q&A" mode tabs — an inline answer blend (see [0420](../0400-qa-mode/0420-inline-retrieval-answer-blend.md)) and the agent answer card (see [0410](../0400-qa-mode/0410-qa-mode.md)) both render above the same result list.
- Each result is a clickable link to the source page.
- Escape key or the close button dismisses the modal.
- Modal is accessible: focus trap while open, ARIA labels and live region as above, full keyboard navigation through input, results, citations, and footer.
- A retry action is available after an initialization failure (index or model fetch).
- If the dense arm is unavailable, a "keyword-only results" notice is shown instead of silently returning worse results with no explanation.

## Evidence

- `tests/integration/test_search_modal.js`

## Linked Tickets

- (none yet)
