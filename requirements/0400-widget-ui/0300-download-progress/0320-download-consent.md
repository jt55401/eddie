# 0320 Model Download Consent

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor, I'm asked before the widget downloads anything sizeable,
so a multi-hundred-megabyte fetch never starts on my connection without my
say-so.

## Key Fields/Parameters

- shown before the first fetch of any model (search's dense lane, or the agent's LLM), stating the approximate download size
- skipped when that model is already cached (repeat visits don't re-prompt)
- skipped, and the download itself deferred, when `navigator.connection.saveData` is set
- the visitor's choice is remembered in `localStorage` so it isn't asked again for the same model
- copy is overridable via `data-consent-text` (falls back to the widget's built-in copy when empty)

## Acceptance Criteria

- No model download starts before the visitor has seen and accepted a consent prompt naming its approximate size.
- A cached model never re-prompts.
- `navigator.connection.saveData` suppresses the prompt and the download; the widget falls back to whichever arms need no model (BM25, and sparse if the index has it).
- `data-consent-text` replaces the prompt copy when set; the widget's default copy is used otherwise.

## Evidence

- `tests/integration/test_download_consent.js`

## Linked Tickets

- (none yet)
