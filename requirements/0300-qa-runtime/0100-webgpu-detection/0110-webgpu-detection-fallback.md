# 0110 WebGPU Detection and Fallback

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As the widget, I detect whether the browser supports WebGPU and show or hide the Q&A option accordingly.

## Key Fields/Parameters

- detection: `navigator.gpu` API check
- fallback: Q&A tab/button hidden or shows "requires a modern browser with WebGPU"
- no error: graceful degradation, search always works

## Acceptance Criteria

- WebGPU detection runs before showing the Q&A option.
- When WebGPU is unavailable, Q&A is hidden or shows an informative message.
- Search functionality is unaffected regardless of WebGPU support.
- No console errors on browsers without WebGPU.

## Evidence

- `tests/integration/test_webgpu_detection.js`

## Linked Tickets

- (none yet)
