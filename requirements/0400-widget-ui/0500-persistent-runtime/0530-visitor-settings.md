# 0530 Visitor Settings Panel

[Requirements Home](../../0000-README.md) | [Area Overview](../0000-high-level-requirements.md)

## User Story

As a site visitor, I can see what the search is about to download onto my
device and choose something lighter (or nothing), pick how good an answer
model to run, decide whether either is loaded before I ask for it, and delete
what has already been downloaded -- and my choices are remembered on this
browser without me having to make them again on every page.

## Key Fields/Parameters

- a gear button in the modal header (`aria-label="Settings"`, `aria-expanded`) opens a panel in the modal; Esc closes the panel before it closes the modal; opening the download-consent card closes the panel
- four preferences in one `localStorage` entry, `eddie.settings`, as a JSON object of string fields (`widget/src/lib/settings.js`):
  - `searchLane`: `"none"` (keyword + sparse, no model download) or a dense lane id from the index manifest
  - `agentLevel`: `"off"`, `"light"` (Qwen3.5-0.8B), `"quality"` (Qwen3.5-2B), or the site's pinned WebLLM model id
  - `warm`: a `data-warm` value; `persist`: a `data-persist` value
- **the site's `data-*` config is the ceiling.** `settingsChoices` offers a lane only when the index carries it, the host can run it (not in `hostSkippedLanes`, and a `webgpu-onnx` lane only with an adapter) and `data-dense-runtime` allows it; the agent only when `data-agent-mode` is not `off` and an adapter exists; `warm` and `persist` only up to the site's rung on the ladders `off < auto < always` and `off < auto`. `"none"` and `"off"` are always offered, so a visitor can always choose less
- `data-dense-runtime` gains the value `off`: keyword and sparse only, no dense lane, for a site that wants no model downloads at all
- `effectiveConfig` maps the preferences onto the widget config (`denseRuntime`, `laneId`, `agentMode`, `agentModel`, `warm`, `persist`) and ignores any preference no longer on offer, so a stale one degrades to the site default
- engine support: `init` and `cache_check` accept `laneId`, which `chooseDenseLanes` narrows the candidate list to (an unrunnable pinned lane is no dense arm, never a silent swap to another model); lane choice runs on every init, not only the first; `denseRuntime: "off"` yields no candidates and its `degraded` entry is not reported as a failure
- storage: the panel shows `navigator.storage.estimate().usage` (the model cache, WebLLM's weights, service-worker caches; not the HTTP cache) and deletes it with the engine's `cache_clear` message when a transport exists, or `indexedDB.deleteDatabase("eddie-models")` when none does, plus every Cache Storage entry whose name names WebLLM
- changing the search model re-initialises the engine on the tier the new lane needs and writes `eddie.search.tier`, so the next page registers that tier; a service worker whose ready engine runs a different lane than the visitor asked for is not adopted

## Acceptance Criteria

- The panel lists exactly the lanes the index carries and this browser can run, with the running one selected; with nothing running yet it selects the least-eager option, never the heaviest.
- Choosing "Keyword only" drops the dense arm, downloads no model, and shows no "the dense model isn't available" notice: the visitor asked for this.
- A choice survives a reload of the same page and applies on other pages of the site, including when a service worker is still running the previous lane.
- Choosing a lane after having chosen "Keyword only" loads that lane, moving to the service worker tier it needs.
- Turning answers off hides the Ask button; switching level drops a loaded agent so the next Ask uses the new model.
- "Delete downloads" frees the downloaded weights and the panel's figure drops to match.
- "Use this site's defaults" clears the stored preferences and restores the site's behaviour without a reload.
- A site that sets `data-agent-mode="off"`, `data-dense-runtime="off"` or `data-warm="off"` offers no way to switch those back on.

## Evidence

- `widget/src/lib/settings.js`, `widget/test/settings.test.js`
- `widget/src/eddie-widget.js` — the gear, the panel, `pickSearchLane` / `pickAgentLevel` / `clearDownloads`
- `widget/src/lib/lanes.js` (`chooseDenseLanes` `off` and `laneId`), `widget/test/lanes.test.js`
- `widget/src/lib/search-engine.js` — `laneId` on `init`/`cache_check`, `chooseLanes`, `reportedDegraded`, `cache_clear`
- `widget/README.md` — [Settings (the gear)](../../../widget/README.md#settings-the-gear)

## Linked Tickets

- (none yet)
