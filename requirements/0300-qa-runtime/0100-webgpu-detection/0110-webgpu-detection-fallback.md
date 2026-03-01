# 0110 WebGPU Detection and Fallback

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As the widget, I detect whether the browser supports WebGPU and show or hide the Ask button accordingly.

## Key Fields/Parameters

- detection: `navigator.gpu` API check (also verify `requestAdapter()` succeeds)
- fallback: Ask button not rendered — search works identically without it
- no error: graceful degradation, no console errors on older browsers

## Acceptance Criteria

- WebGPU detection runs when the modal is first opened.
- When WebGPU is unavailable, the Ask button is simply not rendered (no error messages).
- Search functionality is completely unaffected regardless of WebGPU support.
- No console errors on browsers without WebGPU.
- When `data-qa-enabled="false"`, detection is skipped and Ask button is not rendered.

## Evidence

- `tests/integration/test_webgpu_detection.js`

## Linked Tickets

- (none yet)
