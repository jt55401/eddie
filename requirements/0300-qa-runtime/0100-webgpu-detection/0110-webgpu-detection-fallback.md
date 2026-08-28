# 0110 WebGPU and Device Gating

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As the widget, I check whether the visitor's device can actually run the
agent before offering it, so a capable-looking but too-weak device doesn't
get an unusable download.

## Key Fields/Parameters

- gate: a WebGPU adapter exists (`navigator.gpu`, `requestAdapter()` succeeds) **and** `adapter.limits.maxBufferSize >= 1 GiB` **and** `navigator.connection.saveData` is not set
- `data-agent-mode="off"` skips detection entirely and never renders the Ask affordance
- fallback: when the gate fails, the Ask affordance is simply not rendered — search works identically without it, no error messages

## Acceptance Criteria

- Device gating runs when the widget is first opened, before any agent model is fetched.
- When the gate fails, the Ask affordance is not rendered and no console errors appear.
- Search functionality is completely unaffected regardless of whether the agent gate passes.
- `data-agent-mode="off"` skips detection and disables the agent regardless of device capability.
- A device with a WebGPU adapter but under the buffer-size floor is treated the same as no WebGPU at all (search-only, no error).

## Evidence

- `tests/integration/test_webgpu_detection.js`

## Linked Tickets

- (none yet)
