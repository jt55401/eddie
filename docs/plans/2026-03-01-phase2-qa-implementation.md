# Phase 2: Q&A via WebLLM — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add an "Ask" button to the search widget that generates AI answers from search results using WebLLM (WebGPU).

**Architecture:** Widget detects WebGPU → renders Ask button → on click, worker fetches context chunks via WASM, loads WebLLM, streams LLM answer back to widget. No tabs, no mode switching. Search stays instant. Ask is an explicit action. Falls back to search-only when no WebGPU.

**Tech Stack:** WebLLM (`@mlc-ai/web-llm`) via dynamic import, existing WASM search engine (unchanged), vanilla JS widget with Shadow DOM.

---

## Context

Phase 1 is complete and merged to `main`. The widget has a search modal with instant search-as-you-type via a Web Worker running WASM search. The worker uses `importScripts()` for the WASM glue and communicates via `postMessage`. The index (SAGI v3) stores full chunk texts.

**Key files:**
- `src/wasm.rs` — WASM exports: `init_engine()`, `search_query()`
- `widget/src/worker.js` — Web Worker: loads WASM, downloads embedding model, handles search
- `widget/src/static-agent-widget.js` — Shadow DOM widget: trigger button, modal, search results
- `widget/build.sh` — Builds WASM, copies files to `dist/`

**Design doc:** `docs/plans/2026-03-01-phase2-qa-design.md`

---

## Task 0: Add `get_context_chunks()` WASM export

**Files:**
- Modify: `src/wasm.rs`

The existing `search_query()` deduplicates results by URL and truncates snippets to ~150 chars. For RAG context, we need full chunk texts without deduplication (multiple chunks from the same page may all be relevant).

**Step 1: Add `ContextChunk` struct and `get_context_chunks` function to `src/wasm.rs`**

Add after the `truncate_snippet` function (line 181):

```rust
#[derive(Serialize)]
struct ContextChunk {
    title: String,
    url: String,
    section: Option<String>,
    text: String,
    score: f64,
}

/// Return top-k chunks with full text for RAG context.
///
/// Unlike `search_query`, this does NOT deduplicate by URL and does NOT
/// truncate text — it returns complete chunk content for LLM prompting.
#[wasm_bindgen]
pub fn get_context_chunks(query: &str, top_k: usize) -> Result<JsValue, JsValue> {
    ENGINE.with(|cell| {
        let borrow = cell.borrow();
        let engine = borrow
            .as_ref()
            .ok_or_else(|| JsValue::from_str("engine not initialized"))?;

        let fetch_k = top_k * 3;

        let query_vecs = engine
            .embedder
            .embed_batch(&[query])
            .map_err(|e| JsValue::from_str(&format!("embedding failed: {}", e)))?;
        let semantic_results = search(&engine.index, &query_vecs[0], fetch_k);
        let bm25_results = engine.index.bm25.search(query, fetch_k);

        let semantic_pairs: Vec<(usize, f32)> = semantic_results
            .iter()
            .map(|r| (r.chunk_index, r.score))
            .collect();

        let hybrid = hybrid_rrf(&semantic_pairs, &bm25_results, fetch_k);

        let results: Vec<ContextChunk> = hybrid
            .into_iter()
            .take(top_k)
            .map(|(chunk_idx, score)| {
                let meta = &engine.index.metadata[chunk_idx];
                let text = if chunk_idx < engine.index.texts.len() {
                    engine.index.texts[chunk_idx].clone()
                } else {
                    String::new()
                };

                ContextChunk {
                    title: meta.title.clone(),
                    url: meta.url.clone(),
                    section: meta.section.clone(),
                    text,
                    score,
                }
            })
            .collect();

        serde_wasm_bindgen::to_value(&results)
            .map_err(|e| JsValue::from_str(&format!("serialization failed: {}", e)))
    })
}
```

**Step 2: Verify native build**

Run: `cargo check`
Expected: compiles without errors

**Step 3: Verify WASM build**

Run: `cargo check --target wasm32-unknown-unknown --lib`
Expected: compiles without errors

**Step 4: Run tests**

Run: `cargo test`
Expected: all 37 tests pass (no new tests needed — this is a thin wrapper over existing search logic)

**Step 5: Rebuild WASM binary**

Run: `bash widget/build.sh`
Expected: builds successfully, all 4 files in `dist/`

**Step 6: Commit**

```bash
git add src/wasm.rs
git commit -m "feat(wasm): add get_context_chunks export for RAG context"
```

---

## Task 1: Widget — WebGPU detection, configuration, and Ask button

**Files:**
- Modify: `widget/src/static-agent-widget.js`

**Step 1: Add configuration for Q&A**

In the `config` object (line 15-19), add Q&A settings:

```javascript
const config = {
    indexUrl: scriptEl.getAttribute("data-index-url") || "/static-agent-index.bin",
    position: scriptEl.getAttribute("data-position") || "bottom-right",
    theme: scriptEl.getAttribute("data-theme") || "auto",
    qaEnabled: scriptEl.getAttribute("data-qa-enabled") !== "false",
    qaModel: scriptEl.getAttribute("data-qa-model") || "Qwen2.5-0.5B-Instruct-q4f16_1-MLC",
};
```

**Step 2: Add WebGPU detection state and function**

In the state section (around line 35), add:

```javascript
let hasWebGPU = false;
let askState = "idle"; // idle | loading | generating | complete | error
```

Add a detection function after the state variables:

```javascript
async function detectWebGPU() {
    if (!config.qaEnabled) return false;
    if (!navigator.gpu) return false;
    try {
        const adapter = await navigator.gpu.requestAdapter();
        return adapter !== null;
    } catch {
        return false;
    }
}
```

**Step 3: Add Ask button to header**

After the `closeBtn` creation (line 406-407), add the Ask button:

```javascript
const askBtn = document.createElement("button");
askBtn.className = "sa-ask";
askBtn.setAttribute("aria-label", "Ask AI");
askBtn.textContent = "Ask";
askBtn.style.display = "none"; // Hidden until WebGPU detected
askBtn.addEventListener("click", doAsk);
header.appendChild(askBtn);
```

**Step 4: Add CSS for Ask button**

In the style block, add before the `/* Mobile: bottom sheet */` comment:

```css
.sa-ask {
    flex-shrink: 0;
    padding: 5px 14px;
    border-radius: var(--sa-radius-sm);
    border: 1px solid var(--sa-accent);
    background: var(--sa-accent);
    color: #fff;
    font-family: var(--sa-font);
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: opacity 0.12s, background 0.12s;
    white-space: nowrap;
}
.sa-ask:hover {
    opacity: 0.9;
}
.sa-ask:disabled {
    opacity: 0.5;
    cursor: not-allowed;
}
.sa-ask.sa-loading {
    pointer-events: none;
    opacity: 0.7;
}
```

**Step 5: Add Shift+Enter shortcut**

In the `input` keydown handler (line 460-478), modify the Enter case:

```javascript
if (e.key === "Enter") {
    e.preventDefault();
    if (e.shiftKey && hasWebGPU) {
        doAsk();
    } else if (selectedIndex >= 0 && selectedIndex < currentResults.length) {
        navigateToResult(currentResults[selectedIndex]);
    } else if (input.value.trim()) {
        doSearch(input.value.trim());
    }
}
```

**Step 6: Run WebGPU detection on modal open**

In `openModal()` (line 692), add WebGPU detection:

```javascript
async function openModal() {
    isOpen = true;
    backdrop.classList.add("sa-open");
    trigger.style.display = "none";
    input.value = "";
    clearResults();
    ensureWorker();
    requestAnimationFrame(() => input.focus());

    // Detect WebGPU on first open
    if (!hasWebGPU && config.qaEnabled) {
        hasWebGPU = await detectWebGPU();
        if (hasWebGPU) {
            askBtn.style.display = "";
        }
    }
}
```

**Step 7: Add placeholder `doAsk` function**

Add a stub after `doSearch`:

```javascript
function doAsk() {
    if (!worker || engineState !== "ready") return;
    if (askState === "loading" || askState === "generating") return;
    const query = input.value.trim();
    if (!query) return;
    // Will be implemented in Task 4
    console.log("Ask:", query);
}
```

**Step 8: Update footer keyboard hints**

Update the footer keys array to include the Shift+Enter hint when WebGPU is available. Replace the existing `keys` array construction (lines 440-449) with:

```javascript
const keys = [
    ["\u2191", ""],
    ["\u2193", " navigate "],
    ["enter", " open"],
];
keys.forEach(([key, after]) => {
    const kbd = document.createElement("kbd");
    kbd.textContent = key;
    footerNav.appendChild(kbd);
    if (after) footerNav.appendChild(document.createTextNode(after));
});
```

No changes to footer yet — the Shift+Enter hint will be added dynamically when WebGPU is detected (in the `openModal` function, after setting `askBtn.style.display`). Add after the `askBtn.style.display = "";` line:

```javascript
// Add shift+enter hint to footer
const shiftHint = document.createElement("span");
shiftHint.className = "sa-shift-hint";
const shiftKbd = document.createElement("kbd");
shiftKbd.textContent = "shift+enter";
shiftHint.appendChild(document.createTextNode(" "));
shiftHint.appendChild(shiftKbd);
shiftHint.appendChild(document.createTextNode(" ask"));
footerNav.appendChild(shiftHint);
```

**Step 9: Verify**

Open the widget test page in a WebGPU-capable browser. Confirm:
- Ask button appears next to close button
- Shift+Enter logs to console
- Ask button NOT visible if WebGPU unavailable (test in Firefox without WebGPU)
- Search works as before

**Step 10: Commit**

```bash
git add widget/src/static-agent-widget.js
git commit -m "feat(widget): add Ask button with WebGPU detection and Shift+Enter shortcut"
```

---

## Task 2: Widget — Answer card DOM and styles

**Files:**
- Modify: `widget/src/static-agent-widget.js`

**Step 1: Add answer card DOM elements**

After the `errorEl` creation (line 426-427) and BEFORE the results list, add the answer card:

```javascript
// Answer card (for Q&A)
const answerCard = document.createElement("div");
answerCard.className = "sa-answer";
modal.appendChild(answerCard);

const answerHeader = document.createElement("div");
answerHeader.className = "sa-answer-header";
answerHeader.textContent = "AI Answer";
answerCard.appendChild(answerHeader);

const answerProgress = document.createElement("div");
answerProgress.className = "sa-answer-progress";
answerCard.appendChild(answerProgress);

const answerProgressFill = document.createElement("div");
answerProgressFill.className = "sa-answer-progress-fill";
answerProgress.appendChild(answerProgressFill);

const answerProgressText = document.createElement("div");
answerProgressText.className = "sa-answer-progress-text";
answerCard.appendChild(answerProgressText);

const answerText = document.createElement("div");
answerText.className = "sa-answer-text";
answerCard.appendChild(answerText);

const answerSources = document.createElement("div");
answerSources.className = "sa-answer-sources";
answerCard.appendChild(answerSources);

const answerError = document.createElement("div");
answerError.className = "sa-answer-error";
answerCard.appendChild(answerError);
```

**Step 2: Add CSS for answer card**

Add to the style block, before `/* Mobile: bottom sheet */`:

```css
.sa-answer {
    display: none;
    border-bottom: 1px solid var(--sa-border);
    padding: 14px 16px;
    background: var(--sa-accent-soft);
}
.sa-answer.sa-visible {
    display: block;
}

.sa-answer-header {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--sa-accent);
    margin-bottom: 8px;
}

.sa-answer-progress {
    display: none;
    height: 3px;
    background: var(--sa-bg-elevated);
    border-radius: 2px;
    overflow: hidden;
    margin-bottom: 8px;
}
.sa-answer-progress.sa-visible {
    display: block;
}
.sa-answer-progress-fill {
    height: 100%;
    background: var(--sa-accent);
    border-radius: 2px;
    width: 0%;
    transition: width 0.3s ease;
}

.sa-answer-progress-text {
    display: none;
    font-size: 12px;
    color: var(--sa-text-muted);
    margin-bottom: 8px;
}
.sa-answer-progress-text.sa-visible {
    display: block;
}

.sa-answer-text {
    font-size: 14px;
    line-height: 1.55;
    color: var(--sa-text);
    white-space: pre-wrap;
}
.sa-answer-text:empty {
    display: none;
}

.sa-answer-cursor::after {
    content: "\u258C";
    animation: sa-blink 0.6s step-end infinite;
    color: var(--sa-accent);
}
@keyframes sa-blink {
    50% { opacity: 0; }
}

.sa-answer-sources {
    margin-top: 10px;
    font-size: 12px;
    color: var(--sa-text-muted);
}
.sa-answer-sources:empty {
    display: none;
}
.sa-answer-sources a {
    color: var(--sa-accent);
    text-decoration: none;
    margin-right: 12px;
}
.sa-answer-sources a:hover {
    text-decoration: underline;
}

.sa-answer-error {
    display: none;
    font-size: 13px;
    color: #dc2626;
}
.sa-answer-error.sa-visible {
    display: block;
}
```

**Step 3: Add answer card helper functions**

Add these after the existing `showError` function:

```javascript
function showAnswer(visible) {
    answerCard.classList.toggle("sa-visible", visible);
}

function resetAnswer() {
    answerText.textContent = "";
    answerText.classList.remove("sa-answer-cursor");
    answerSources.textContent = "";
    answerProgress.classList.remove("sa-visible");
    answerProgressText.classList.remove("sa-visible");
    answerProgressFill.style.width = "0%";
    answerError.textContent = "";
    answerError.classList.remove("sa-visible");
    showAnswer(false);
    askState = "idle";
}

function showAnswerDownloading(text, progress) {
    showAnswer(true);
    answerProgress.classList.add("sa-visible");
    answerProgressText.classList.add("sa-visible");
    answerProgressText.textContent = text;
    if (progress != null) {
        answerProgressFill.style.width = Math.round(progress * 100) + "%";
    }
}

function appendAnswerToken(token) {
    answerText.classList.add("sa-answer-cursor");
    answerText.textContent += token;
    // Auto-scroll answer into view
    answerCard.scrollIntoView({ block: "nearest", behavior: "smooth" });
}

function showAnswerComplete(sources) {
    answerText.classList.remove("sa-answer-cursor");
    answerProgress.classList.remove("sa-visible");
    answerProgressText.classList.remove("sa-visible");

    if (sources && sources.length > 0) {
        const label = document.createTextNode("Sources: ");
        answerSources.appendChild(label);
        sources.forEach((s, i) => {
            const a = document.createElement("a");
            a.href = s.url;
            a.textContent = "[" + (i + 1) + "] " + s.title;
            a.addEventListener("click", (e) => {
                e.preventDefault();
                closeModal();
                window.location.href = s.url;
            });
            answerSources.appendChild(a);
        });
    }
    askState = "complete";
}

function showAnswerError(message) {
    answerProgress.classList.remove("sa-visible");
    answerProgressText.classList.remove("sa-visible");
    answerText.classList.remove("sa-answer-cursor");
    answerError.textContent = message;
    answerError.classList.add("sa-visible");
    askState = "error";
}
```

**Step 4: Clear answer on new search**

In the `clearResults` function, add `resetAnswer()`:

```javascript
function clearResults() {
    currentResults = [];
    selectedIndex = -1;
    resultsList.textContent = "";
    resetAnswer();
}
```

**Step 5: Verify**

Open widget in browser, confirm:
- No visual changes (answer card is hidden by default)
- Search still works
- No console errors

**Step 6: Commit**

```bash
git add widget/src/static-agent-widget.js
git commit -m "feat(widget): add answer card DOM and styles for Q&A display"
```

---

## Task 3: Worker — context and LLM message handlers

**Files:**
- Modify: `widget/src/worker.js`

This is the largest task. The worker gets three new capabilities:
1. A `get_context` message handler (calls the new WASM export)
2. Lazy loading of WebLLM via dynamic `import()`
3. A `ask` message handler that fetches context, loads LLM, and streams the answer

**Step 1: Add LLM state variables**

After the existing state section (line 17-18), add:

```javascript
let llmEngine = null;
let llmLoading = false;
```

**Step 2: Add `get_context` and `ask` handlers to `onmessage`**

In the `onmessage` handler (line 21-53), add two new cases after the `search` handler:

```javascript
  } else if (msg.type === "get_context") {
    try {
      if (!initialized) {
        throw new Error("Engine not initialized");
      }
      const chunks = wasm_bindgen.get_context_chunks(
        msg.query,
        msg.topK || 5
      );
      self.postMessage({
        type: "context_result",
        requestId: msg.requestId,
        chunks: chunks,
      });
    } catch (err) {
      self.postMessage({
        type: "error",
        requestId: msg.requestId,
        error: err.message || String(err),
      });
    }
  } else if (msg.type === "ask") {
    handleAsk(msg).catch((err) => {
      self.postMessage({
        type: "answer_error",
        requestId: msg.requestId,
        error: err.message || String(err),
      });
    });
  }
```

**Step 3: Add the `handleAsk` function**

Add at the bottom of worker.js, before the `// -- Utilities --` section:

```javascript
// -- LLM (WebLLM) --

async function handleAsk(msg) {
  if (!initialized) {
    throw new Error("Engine not initialized");
  }

  const requestId = msg.requestId;
  const query = msg.query;
  const modelId = msg.modelId || "Qwen2.5-0.5B-Instruct-q4f16_1-MLC";
  const topK = msg.topK || 5;

  // 1. Get context chunks from WASM search
  self.postMessage({ type: "ask_status", requestId, state: "searching" });
  const chunks = wasm_bindgen.get_context_chunks(query, topK);

  if (!chunks || chunks.length === 0) {
    self.postMessage({
      type: "answer_complete",
      requestId,
      answer: "No relevant content found to answer this question.",
      sources: [],
    });
    return;
  }

  // 2. Initialize WebLLM engine (lazy, first use only)
  if (!llmEngine) {
    await initLLM(modelId, requestId);
  }

  // 3. Build RAG prompt
  const context = chunks
    .map(
      (c, i) =>
        `[${i + 1}] "${c.title}" (${c.url})${c.section ? " > " + c.section : ""}\n${c.text}`
    )
    .join("\n\n");

  const messages = [
    {
      role: "system",
      content:
        "You are a helpful assistant for a website. Answer the user's question based ONLY on the provided search results. Be concise (1-3 sentences). Cite sources by their number (e.g. [1], [2]). If the search results don't contain the answer, say so.",
    },
    {
      role: "user",
      content: `Search results:\n\n${context}\n\nQuestion: ${query}`,
    },
  ];

  // 4. Stream the answer
  self.postMessage({ type: "ask_status", requestId, state: "generating" });

  const completion = await llmEngine.chat.completions.create({
    messages,
    stream: true,
    temperature: 0.3,
    max_tokens: 256,
  });

  let fullAnswer = "";
  for await (const chunk of completion) {
    const token = chunk.choices[0]?.delta?.content || "";
    if (token) {
      fullAnswer += token;
      self.postMessage({ type: "answer_token", requestId, token });
    }
  }

  // 5. Send completion with source metadata
  const sources = chunks.map((c) => ({
    title: c.title,
    url: c.url,
  }));

  self.postMessage({
    type: "answer_complete",
    requestId,
    answer: fullAnswer,
    sources,
  });
}

async function initLLM(modelId, requestId) {
  if (llmLoading) {
    throw new Error("LLM is already loading");
  }
  llmLoading = true;

  try {
    self.postMessage({
      type: "ask_status",
      requestId,
      state: "loading_llm",
      progress: 0,
      text: "Loading AI model\u2026",
    });

    // Dynamic import of WebLLM
    const webllm = await import(
      "https://esm.run/@mlc-ai/web-llm@0.2"
    );

    llmEngine = await webllm.CreateMLCEngine(modelId, {
      initProgressCallback: (progress) => {
        self.postMessage({
          type: "ask_status",
          requestId,
          state: "loading_llm",
          progress: progress.progress,
          text: progress.text,
        });
      },
    });

    self.postMessage({
      type: "ask_status",
      requestId,
      state: "llm_ready",
    });
  } finally {
    llmLoading = false;
  }
}
```

**Step 4: Verify syntax**

Run a quick syntax check (no browser needed):

```bash
node --check widget/src/worker.js
```

Expected: no syntax errors (note: the dynamic `import()` and `for await` are valid syntax even though the worker-specific APIs won't be available in Node)

**Step 5: Commit**

```bash
git add widget/src/worker.js
git commit -m "feat(worker): add WebLLM integration with streaming answer generation"
```

---

## Task 4: Widget — Ask flow orchestration

**Files:**
- Modify: `widget/src/static-agent-widget.js`

**Step 1: Replace the stub `doAsk` function**

Replace the placeholder `doAsk` with the full implementation:

```javascript
function doAsk() {
    if (!worker || engineState !== "ready") return;
    if (askState === "loading" || askState === "generating") return;
    const query = input.value.trim();
    if (!query) return;

    askState = "loading";
    resetAnswer();
    showAnswer(true);
    showAnswerDownloading("Preparing\u2026", null);
    askBtn.disabled = true;
    askBtn.classList.add("sa-loading");
    askBtn.textContent = "\u2026";

    searchRequestId++;
    worker.postMessage({
        type: "ask",
        requestId: searchRequestId,
        query: query,
        modelId: config.qaModel,
        topK: 5,
    });
}
```

**Step 2: Add LLM message handlers to `worker.onmessage`**

In the `ensureWorker` function (line 519-541), add handlers for the new message types. In the `worker.onmessage` callback, add:

```javascript
} else if (msg.type === "ask_status") {
    handleAskStatus(msg);
} else if (msg.type === "answer_token") {
    handleAnswerToken(msg);
} else if (msg.type === "answer_complete") {
    handleAnswerComplete(msg);
} else if (msg.type === "answer_error") {
    handleAnswerError(msg);
}
```

**Step 3: Add the handler functions**

Add after the existing `handleError` function:

```javascript
function handleAskStatus(msg) {
    if (msg.state === "searching") {
        showAnswerDownloading("Searching\u2026", null);
    } else if (msg.state === "loading_llm") {
        const text = msg.text || "Loading AI model\u2026";
        showAnswerDownloading(text, msg.progress);
    } else if (msg.state === "llm_ready") {
        answerProgress.classList.remove("sa-visible");
        answerProgressText.classList.remove("sa-visible");
    } else if (msg.state === "generating") {
        askState = "generating";
        answerProgress.classList.remove("sa-visible");
        answerProgressText.classList.remove("sa-visible");
    }
}

function handleAnswerToken(msg) {
    appendAnswerToken(msg.token);
}

function handleAnswerComplete(msg) {
    showAnswerComplete(msg.sources);
    askBtn.disabled = false;
    askBtn.classList.remove("sa-loading");
    askBtn.textContent = "Ask";
}

function handleAnswerError(msg) {
    showAnswerError(msg.error || "Couldn't generate an answer.");
    askBtn.disabled = false;
    askBtn.classList.remove("sa-loading");
    askBtn.textContent = "Ask";
    askState = "error";
}
```

**Step 4: Verify syntax**

```bash
node --check widget/src/static-agent-widget.js
```

Expected: no syntax errors

**Step 5: Commit**

```bash
git add widget/src/static-agent-widget.js
git commit -m "feat(widget): wire up Ask button to worker LLM generation with streaming"
```

---

## Task 5: Build pipeline update

**Files:**
- Modify: `widget/build.sh`

**Step 1: Update build.sh**

The only change needed is to copy the updated worker.js and widget.js to dist/. The build script already does this. No WebLLM JS needs to be bundled — it's loaded via dynamic `import()` from CDN at runtime.

Verify the build script still works:

```bash
bash widget/build.sh
```

Expected: builds successfully, all 4 files in `dist/`

**Step 2: Copy dist files to test directory**

```bash
cp dist/* /tmp/static-agent-test/
```

**Step 3: Commit (if any build.sh changes were needed)**

```bash
git add widget/build.sh
git commit -m "chore: update build pipeline for Q&A support"
```

---

## Task 6: End-to-end integration test

**Step 1: Re-index the Hugo site (if not already v3 with texts)**

```bash
cargo run -- index \
  --content-dir /home/jason/Documents/Personal/jasongrey-com-hugo/jasongrey/content \
  --output /tmp/static-agent-test/static-agent-index.bin
```

**Step 2: Copy dist files to test directory**

```bash
cp dist/* /tmp/static-agent-test/
```

**Step 3: Serve locally**

```bash
python3 -m http.server 8765 --directory /tmp/static-agent-test --bind 0.0.0.0
```

**Step 4: Test in a WebGPU-capable browser**

Open `http://<ip>:8765/` and verify:

1. Floating search button appears
2. Click → modal opens, search works as before
3. **Ask button visible** next to the close button
4. Type "Rust programming" → search results appear instantly
5. Click **Ask** → answer card appears with "Preparing..."
6. First time: model download progress shown (~350MB)
7. After download: streaming answer appears with blinking cursor
8. Answer completes with "Sources: [1] Title [2] Title" links
9. Click a source link → navigates to the page
10. **Shift+Enter** triggers Ask from keyboard
11. New search query → answer card clears, new Ask works
12. **Escape** closes modal cleanly

**Step 5: Test without WebGPU**

Open in a browser without WebGPU (or use Chrome with `--disable-gpu`):

1. Modal opens, search works
2. Ask button is NOT visible
3. No console errors

**Step 6: Test with Q&A disabled**

Modify the test HTML:
```html
<script src="/static-agent-widget.js"
        data-index-url="/static-agent-index.bin"
        data-qa-enabled="false"
        defer></script>
```

1. Modal opens, search works
2. Ask button is NOT visible (even with WebGPU)

**Step 7: Commit**

```bash
git add -A
git commit -m "feat: Phase 2 Q&A integration complete"
```

---

## Dependency graph

```
Task 0 (WASM export) ─→ Task 3 (worker LLM) ─→ Task 4 (widget orchestration)
                                                        │
Task 1 (Ask button + detection) ──────────────────→ Task 4
                                                        │
Task 2 (answer card DOM) ─────────────────────────→ Task 4
                                                        │
                                                   Task 5 (build)
                                                        │
                                                   Task 6 (integration test)
```

Tasks 0, 1, and 2 can be developed in parallel. Task 3 depends on Task 0. Task 4 depends on Tasks 1, 2, and 3. Tasks 5 and 6 are sequential after Task 4.
