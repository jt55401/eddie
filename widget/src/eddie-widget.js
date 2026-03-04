// SPDX-License-Identifier: GPL-3.0-only

// Eddie search widget
//
// Self-contained vanilla JS widget using Shadow DOM for style isolation.
// Embeds a search modal that communicates with a Web Worker running
// WASM-based semantic + keyword search.

"use strict";

(function () {
  const scriptEl = document.currentScript;
  if (!scriptEl) return;

  function parseOffsetPx(attrName) {
    const raw = scriptEl.getAttribute(attrName);
    if (raw == null || raw === "") return 0;
    const value = Number(raw);
    return Number.isFinite(value) ? Math.trunc(value) : 0;
  }

  function parseIntAttr(attrName, fallback) {
    const raw = scriptEl.getAttribute(attrName);
    if (raw == null || raw === "") return fallback;
    const value = Number.parseInt(raw, 10);
    return Number.isFinite(value) && value > 0 ? value : fallback;
  }

  function normalizeQaMode(raw) {
    const value = (raw || "").toLowerCase();
    if (value === "off" || value === "always" || value === "auto") {
      return value;
    }
    return "auto";
  }

  function normalizePosition(raw) {
    const value = (raw || "").toLowerCase();
    const allowed = new Set([
      "top-left",
      "top-right",
      "bottom-left",
      "bottom-right",
    ]);
    return allowed.has(value) ? value : "bottom-right";
  }

  const config = {
    indexUrl: scriptEl.getAttribute("data-index-url") || "/eddie/index.ed",
    position: normalizePosition(scriptEl.getAttribute("data-position")),
    theme: scriptEl.getAttribute("data-theme") || "auto",
    offsetY: parseOffsetPx("data-offset-y"),
    offsetX: parseOffsetPx("data-offset-x"),
    qaMode: normalizeQaMode(scriptEl.getAttribute("data-qa-mode")),
    qaSubject: (scriptEl.getAttribute("data-qa-subject") || "").toLowerCase(),
    resultTopK: parseIntAttr("data-top-k", 8),
    answerTopK: parseIntAttr("data-answer-top-k", 5),
  };

  const HEART_SPRITES = [
    [".11111.", "1222221", "1222221", ".12221."], // solid
    [".11111.", "12.2.21", "1222221", ".12.21."], // circuit
    [".13331.", "1344431", "1244421", ".12221."], // beveled
    [".13131.", "1344431", "12.4.21", ".12221."], // gear-ish
  ];

  const HEART_PALETTE = {
    "1": "#f2c94c",
    "2": "#e0b63f",
    "3": "#f8dda1",
    "4": "#b78e28",
  };

  // Resolve asset URLs relative to this script's location
  const scriptSrc = new URL(scriptEl.src, location.href);
  const baseUrl = scriptSrc.href.substring(0, scriptSrc.href.lastIndexOf("/") + 1);

  function resolveAsset(name) {
    return baseUrl + name;
  }

  // -- State --
  let worker = null;
  let searchRequestId = 0;
  let isOpen = false;
  let selectedIndex = -1;
  let currentResults = [];
  let currentAnswer = null;
  let engineState = "idle"; // idle | loading | ready | error
  let lastHeartIndex = -1;
  let activeRequestId = 0;
  let searchPending = false;

  // -- DOM setup --
  const host = document.createElement("div");
  host.id = "eddie-host";
  const shadow = host.attachShadow({ mode: "closed" });

  // -- Styles --
  const style = document.createElement("style");
  style.textContent = `
    :host {
      --sa-font: "IBM Plex Sans", -apple-system, BlinkMacSystemFont, sans-serif;
      --sa-font-mono: "IBM Plex Mono", "SF Mono", "Fira Code", monospace;
      --sa-bg: #ffffff;
      --sa-bg-elevated: #f6f6f6;
      --sa-text: #1a1a1a;
      --sa-text-muted: #6b6b6b;
      --sa-border: #e0e0e0;
      --sa-accent: #2563eb;
      --sa-accent-soft: rgba(37, 99, 235, 0.08);
      --sa-backdrop: rgba(0, 0, 0, 0.4);
      --sa-shadow: 0 16px 48px rgba(0, 0, 0, 0.12), 0 2px 8px rgba(0, 0, 0, 0.08);
      --sa-radius: 12px;
      --sa-radius-sm: 6px;
      --sa-trigger-size: 48px;

      all: initial;
      font-family: var(--sa-font);
      position: fixed;
      z-index: 999999;
    }

    @media (prefers-color-scheme: dark) {
      :host {
        --sa-bg: #1a1a1a;
        --sa-bg-elevated: #252525;
        --sa-text: #e8e8e8;
        --sa-text-muted: #999999;
        --sa-border: #333333;
        --sa-accent: #60a5fa;
        --sa-accent-soft: rgba(96, 165, 250, 0.1);
        --sa-backdrop: rgba(0, 0, 0, 0.6);
        --sa-shadow: 0 16px 48px rgba(0, 0, 0, 0.4), 0 2px 8px rgba(0, 0, 0, 0.2);
      }
    }

    *, *::before, *::after {
      box-sizing: border-box;
      margin: 0;
      padding: 0;
    }

    .sa-trigger {
      position: fixed;
      width: var(--sa-trigger-size);
      height: var(--sa-trigger-size);
      border-radius: 50%;
      border: 1px solid var(--sa-border);
      background: var(--sa-bg);
      color: var(--sa-text);
      cursor: pointer;
      display: flex;
      align-items: center;
      justify-content: center;
      box-shadow: 0 2px 12px rgba(0,0,0,0.1);
      transition: transform 0.15s ease, box-shadow 0.15s ease;
    }
    .sa-trigger:hover {
      transform: scale(1.06);
      box-shadow: 0 4px 16px rgba(0,0,0,0.15);
    }
    .sa-trigger:active {
      transform: scale(0.96);
    }
    .sa-trigger svg {
      width: 20px;
      height: 20px;
      stroke: currentColor;
      fill: none;
      stroke-width: 2;
      stroke-linecap: round;
      stroke-linejoin: round;
    }

    .sa-pos-bottom-right { right: 24px; bottom: 24px; }
    .sa-pos-bottom-left  { left: 24px; bottom: 24px; }
    .sa-pos-top-right    { right: 24px; top: 24px; }
    .sa-pos-top-left     { left: 24px; top: 24px; }

    .sa-backdrop {
      position: fixed;
      inset: 0;
      background: var(--sa-backdrop);
      display: none;
      align-items: flex-start;
      justify-content: center;
      padding-top: 12vh;
    }
    .sa-backdrop.sa-open {
      display: flex;
    }

    .sa-modal {
      background: var(--sa-bg);
      border: 1px solid var(--sa-border);
      border-radius: var(--sa-radius);
      box-shadow: var(--sa-shadow);
      width: 100%;
      max-width: 600px;
      max-height: 72vh;
      display: flex;
      flex-direction: column;
      overflow: hidden;
      animation: sa-slide-in 0.18s ease-out;
    }
    @keyframes sa-slide-in {
      from { opacity: 0; transform: translateY(-12px) scale(0.98); }
      to   { opacity: 1; transform: translateY(0) scale(1); }
    }

    .sa-header {
      display: flex;
      align-items: center;
      padding: 16px;
      gap: 12px;
      border-bottom: 1px solid var(--sa-border);
    }

    .sa-search-icon {
      flex-shrink: 0;
      width: 18px;
      height: 18px;
      stroke: var(--sa-text-muted);
      fill: none;
      stroke-width: 2;
      stroke-linecap: round;
      stroke-linejoin: round;
    }

    .sa-input {
      flex: 1;
      border: none;
      background: none;
      font-family: var(--sa-font);
      font-size: 16px;
      color: var(--sa-text);
      outline: none;
    }
    .sa-input::placeholder {
      color: var(--sa-text-muted);
    }

    .sa-close {
      flex-shrink: 0;
      width: 28px;
      height: 28px;
      border-radius: var(--sa-radius-sm);
      border: 1px solid var(--sa-border);
      background: var(--sa-bg-elevated);
      color: var(--sa-text-muted);
      cursor: pointer;
      display: flex;
      align-items: center;
      justify-content: center;
      font-family: var(--sa-font-mono);
      font-size: 11px;
      line-height: 1;
      transition: border-color 0.1s;
    }
    .sa-close:hover {
      border-color: var(--sa-text-muted);
    }

    .sa-heart {
      flex-shrink: 0;
      width: 14px;
      height: 8px;
      image-rendering: pixelated;
      image-rendering: crisp-edges;
      opacity: 0.95;
      display: block;
    }

    .sa-status {
      padding: 12px 16px;
      font-size: 13px;
      color: var(--sa-text-muted);
      display: none;
      align-items: center;
      gap: 10px;
      border-bottom: 1px solid var(--sa-border);
    }
    .sa-status.sa-visible {
      display: flex;
    }

    .sa-progress-bar {
      flex: 1;
      height: 3px;
      background: var(--sa-bg-elevated);
      border-radius: 2px;
      overflow: hidden;
    }
    .sa-progress-fill {
      height: 100%;
      background: var(--sa-accent);
      border-radius: 2px;
      width: 0%;
      transition: width 0.2s ease;
    }
    .sa-progress-indeterminate .sa-progress-fill {
      width: 40%;
      animation: sa-indeterminate 1.2s ease-in-out infinite;
    }
    @keyframes sa-indeterminate {
      0%   { transform: translateX(-100%); }
      100% { transform: translateX(350%); }
    }

    .sa-results {
      flex: 1;
      overflow-y: auto;
      list-style: none;
    }

    .sa-answer {
      display: none;
      border-bottom: 1px solid var(--sa-border);
      background: var(--sa-bg-elevated);
      padding: 12px 16px;
      gap: 6px;
      flex-direction: column;
    }
    .sa-answer.sa-visible {
      display: flex;
    }
    .sa-answer-label {
      font-size: 11px;
      letter-spacing: 0.08em;
      text-transform: uppercase;
      color: var(--sa-text-muted);
      font-family: var(--sa-font-mono);
    }
    .sa-answer-text {
      font-size: 14px;
      line-height: 1.45;
      color: var(--sa-text);
    }
    .sa-answer-cites {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
      margin-top: 2px;
    }
    .sa-answer-cite {
      font-size: 11px;
      color: var(--sa-accent);
      text-decoration: none;
      border: 1px solid var(--sa-border);
      border-radius: 999px;
      padding: 2px 8px;
    }
    .sa-answer-cite:hover {
      border-color: var(--sa-accent);
    }

    .sa-result {
      display: block;
      padding: 12px 16px;
      border-bottom: 1px solid var(--sa-border);
      cursor: pointer;
      text-decoration: none;
      color: inherit;
      transition: background 0.08s;
    }
    .sa-result:last-child {
      border-bottom: none;
    }
    .sa-result:hover,
    .sa-result[aria-selected="true"] {
      background: var(--sa-accent-soft);
    }
    .sa-result-title {
      font-size: 14px;
      font-weight: 600;
      color: var(--sa-text);
      margin-bottom: 2px;
    }
    .sa-result-url {
      font-family: var(--sa-font-mono);
      font-size: 11px;
      color: var(--sa-accent);
      margin-bottom: 4px;
    }
    .sa-result-section {
      font-size: 11px;
      color: var(--sa-text-muted);
      margin-bottom: 4px;
    }
    .sa-result-snippet {
      font-size: 13px;
      color: var(--sa-text-muted);
      line-height: 1.45;
    }

    .sa-empty {
      padding: 32px 16px;
      text-align: center;
      color: var(--sa-text-muted);
      font-size: 14px;
    }

    .sa-error {
      padding: 12px 16px;
      font-size: 13px;
      color: #dc2626;
      display: none;
    }
    .sa-error.sa-visible {
      display: block;
    }

    .sa-footer {
      padding: 8px 16px;
      border-top: 1px solid var(--sa-border);
      display: flex;
      align-items: center;
      justify-content: space-between;
      font-size: 11px;
      color: var(--sa-text-muted);
    }
    .sa-footer kbd {
      display: inline-block;
      padding: 1px 5px;
      font-family: var(--sa-font-mono);
      font-size: 10px;
      border: 1px solid var(--sa-border);
      border-radius: 3px;
      background: var(--sa-bg-elevated);
      margin: 0 2px;
    }

    .sa-brand {
      display: inline-flex;
      align-items: center;
      gap: 6px;
      letter-spacing: 0.08em;
      font-weight: 600;
    }

    .sa-brand-link {
      color: var(--sa-text-muted);
      text-decoration: none;
      border: 1px solid var(--sa-border);
      border-radius: 999px;
      padding: 2px 8px;
      transition: border-color 0.12s ease, color 0.12s ease, background 0.12s ease;
    }
    .sa-brand-link:hover {
      border-color: var(--sa-text-muted);
      color: var(--sa-text);
      background: var(--sa-bg-elevated);
    }
    .sa-brand-link:focus-visible {
      outline: 1px solid var(--sa-accent);
      outline-offset: 2px;
    }

    /* Mobile: bottom sheet */
    @media (max-width: 640px) {
      .sa-backdrop {
        padding-top: 0;
        align-items: flex-end;
      }
      .sa-modal {
        max-width: 100%;
        max-height: 85vh;
        border-radius: var(--sa-radius) var(--sa-radius) 0 0;
        animation-name: sa-slide-up;
      }
      @keyframes sa-slide-up {
        from { opacity: 0; transform: translateY(40px); }
        to   { opacity: 1; transform: translateY(0); }
      }
    }
  `;
  shadow.appendChild(style);

  // -- SVG helper --
  function createSearchSvg(className) {
    const ns = "http://www.w3.org/2000/svg";
    const svg = document.createElementNS(ns, "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    if (className) svg.setAttribute("class", className);
    const circle = document.createElementNS(ns, "circle");
    circle.setAttribute("cx", "11");
    circle.setAttribute("cy", "11");
    circle.setAttribute("r", "8");
    const line = document.createElementNS(ns, "line");
    line.setAttribute("x1", "21");
    line.setAttribute("y1", "21");
    line.setAttribute("x2", "16.65");
    line.setAttribute("y2", "16.65");
    svg.appendChild(circle);
    svg.appendChild(line);
    return svg;
  }

  // -- Trigger button --
  const trigger = document.createElement("button");
  trigger.className = `sa-trigger sa-pos-${config.position}`;
  trigger.setAttribute("aria-label", "Search");
  trigger.appendChild(createSearchSvg());
  trigger.addEventListener("click", openModal);
  applyTriggerOffsets();
  shadow.appendChild(trigger);

  // -- Backdrop --
  const backdrop = document.createElement("div");
  backdrop.className = "sa-backdrop";
  backdrop.addEventListener("click", (e) => {
    if (e.target === backdrop) closeModal();
  });
  shadow.appendChild(backdrop);

  // -- Modal --
  const modal = document.createElement("div");
  modal.className = "sa-modal";
  modal.setAttribute("role", "dialog");
  modal.setAttribute("aria-modal", "true");
  modal.setAttribute("aria-label", "Search");
  backdrop.appendChild(modal);

  // Header
  const header = document.createElement("div");
  header.className = "sa-header";
  header.appendChild(createSearchSvg("sa-search-icon"));
  modal.appendChild(header);

  const input = document.createElement("input");
  input.className = "sa-input";
  input.type = "text";
  input.setAttribute("role", "searchbox");
  input.setAttribute("aria-label", "Search query");
  input.placeholder = "Search\u2026";
  header.appendChild(input);

  const closeBtn = document.createElement("button");
  closeBtn.className = "sa-close";
  closeBtn.setAttribute("aria-label", "Close");
  closeBtn.textContent = "esc";
  closeBtn.addEventListener("click", closeModal);

  const heart = document.createElement("canvas");
  heart.className = "sa-heart";
  heart.width = 7;
  heart.height = 4;
  heart.setAttribute("aria-hidden", "true");
  drawHeartSprite(0);
  header.appendChild(closeBtn);

  // Status bar
  const status = document.createElement("div");
  status.className = "sa-status";
  modal.appendChild(status);

  const statusText = document.createElement("span");
  status.appendChild(statusText);

  const progressBar = document.createElement("div");
  progressBar.className = "sa-progress-bar";
  const progressFill = document.createElement("div");
  progressFill.className = "sa-progress-fill";
  progressBar.appendChild(progressFill);
  status.appendChild(progressBar);

  // Error area
  const errorEl = document.createElement("div");
  errorEl.className = "sa-error";
  modal.appendChild(errorEl);

  // Answer area (experimental factual mode)
  const answerEl = document.createElement("div");
  answerEl.className = "sa-answer";
  modal.appendChild(answerEl);

  // Results
  const resultsList = document.createElement("ul");
  resultsList.className = "sa-results";
  resultsList.setAttribute("role", "listbox");
  modal.appendChild(resultsList);

  // Footer (built with DOM, not innerHTML)
  const footer = document.createElement("div");
  footer.className = "sa-footer";

  const footerNav = document.createElement("span");
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
  footer.appendChild(footerNav);

  const footerBrandLink = document.createElement("a");
  footerBrandLink.className = "sa-brand-link";
  footerBrandLink.href = "https://github.com/jt55401/eddie";
  footerBrandLink.target = "_blank";
  footerBrandLink.rel = "noopener noreferrer";
  footerBrandLink.setAttribute("aria-label", "Eddie on GitHub (opens in a new tab)");

  const footerBrand = document.createElement("span");
  footerBrand.className = "sa-brand";
  footerBrand.appendChild(document.createTextNode("EDDIE"));
  footerBrand.appendChild(heart);
  footerBrandLink.appendChild(footerBrand);
  footer.appendChild(footerBrandLink);

  modal.appendChild(footer);

  // -- Keyboard handling --
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      if (selectedIndex >= 0 && selectedIndex < currentResults.length) {
        navigateToResult(currentResults[selectedIndex]);
      } else if (input.value.trim()) {
        doSearch(input.value.trim());
      }
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      moveSelection(1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      moveSelection(-1);
    } else if (e.key === "Escape") {
      e.preventDefault();
      closeModal();
    }
  });

  // Debounced search-as-you-type
  let searchTimer = null;
  input.addEventListener("input", () => {
    clearTimeout(searchTimer);
    const q = input.value.trim();
    if (q.length >= 2 && engineState === "ready") {
      searchTimer = setTimeout(() => doSearch(q), 200);
    } else if (q.length === 0) {
      clearResults();
    }
  });

  // Focus trap
  modal.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      closeModal();
      return;
    }
    if (e.key !== "Tab") return;

    const focusable = [input, closeBtn];
    const first = focusable[0];
    const last = focusable[focusable.length - 1];

    if (e.shiftKey) {
      if (shadow.activeElement === first) {
        e.preventDefault();
        last.focus();
      }
    } else {
      if (shadow.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    }
  });

  // -- Worker communication --
  function ensureWorker() {
    if (worker) return;

    worker = new Worker(resolveAsset("eddie-worker.js"));
    worker.onmessage = (e) => {
      const msg = e.data;

      if (msg.type === "status") {
        handleStatus(msg);
      } else if (msg.type === "search_result") {
        handleSearchResult(msg);
      } else if (msg.type === "error") {
        handleError(msg);
      }
    };

    engineState = "loading";
    worker.postMessage({
      type: "init",
      indexUrl: new URL(config.indexUrl, location.href).href,
      baseUrl: baseUrl,
    });
  }

  function handleStatus(msg) {
    const stateLabels = {
      loading_wasm: "Loading search engine\u2026",
      loading_index: "Loading index\u2026",
      checking_cache: "Checking model cache\u2026",
      downloading_model: "Downloading model\u2026",
      initializing: "Initializing\u2026",
      ready: "Ready",
    };

    if (msg.state === "ready") {
      engineState = "ready";
      showStatus(false);
      // If there's already a query waiting, run it
      if (input.value.trim().length >= 2) {
        doSearch(input.value.trim());
      }
      return;
    }

    if (msg.state === "error") {
      engineState = "error";
      showError(msg.error || "Failed to initialize");
      showStatus(false);
      return;
    }

    engineState = "loading";
    statusText.textContent = stateLabels[msg.state] || msg.state;
    showStatus(true);

    if (msg.state === "downloading_model" && msg.progress != null) {
      progressBar.classList.remove("sa-progress-indeterminate");
      progressFill.style.width = Math.round(msg.progress * 100) + "%";
      statusText.textContent =
        "Downloading " + (msg.file || "model") + "\u2026 " +
        Math.round(msg.progress * 100) + "%";
    } else {
      progressBar.classList.add("sa-progress-indeterminate");
      progressFill.style.width = "";
    }
  }

  function handleSearchResult(msg) {
    if (msg.requestId !== activeRequestId) return;
    currentResults = msg.results || [];
    currentAnswer = msg.answer || null;
    searchPending = false;
    maybeFinalizeQuery();
  }

  function maybeFinalizeQuery() {
    if (searchPending) {
      return;
    }

    selectedIndex = -1;
    showStatus(false);
    renderResults();
  }

  function handleError(msg) {
    if (msg.requestId && msg.requestId !== activeRequestId) return;
    showError(msg.error || "Search failed");
  }

  function doSearch(query) {
    if (!worker || engineState !== "ready") return;
    searchRequestId += 1;
    activeRequestId = searchRequestId;
    currentResults = [];
    currentAnswer = null;
    const answerMode = shouldUseAnswerMode(query);
    searchPending = true;

    if (answerMode) {
      statusText.textContent = "Searching and grounding answer...";
      progressBar.classList.add("sa-progress-indeterminate");
      progressFill.style.width = "";
      showStatus(true);
    }

    worker.postMessage({
      type: "search",
      requestId: activeRequestId,
      query: query,
      topK: config.resultTopK,
      answerTopK: config.answerTopK,
      answerMode: answerMode,
      qaSubject: config.qaSubject || "",
      mode: "hybrid",
    });
  }

  // -- Rendering --
  function renderResults() {
    resultsList.textContent = "";
    errorEl.classList.remove("sa-visible");
    renderAnswer();

    if (currentResults.length === 0 && input.value.trim()) {
      const empty = document.createElement("div");
      empty.className = "sa-empty";
      empty.textContent = "No results found.";
      resultsList.appendChild(empty);
      return;
    }

    currentResults.forEach((r, i) => {
      const li = document.createElement("a");
      li.className = "sa-result";
      li.href = r.url;
      li.setAttribute("role", "option");
      li.setAttribute("aria-selected", i === selectedIndex ? "true" : "false");

      const titleEl = document.createElement("div");
      titleEl.className = "sa-result-title";
      titleEl.textContent = r.title;
      li.appendChild(titleEl);

      const urlEl = document.createElement("div");
      urlEl.className = "sa-result-url";
      urlEl.textContent = r.url;
      li.appendChild(urlEl);

      if (
        r.section &&
        r.section !== "Semantic Segment" &&
        r.section !== "Summary Lane"
      ) {
        const sectionEl = document.createElement("div");
        sectionEl.className = "sa-result-section";
        sectionEl.textContent = r.section;
        li.appendChild(sectionEl);
      }

      if (r.snippet) {
        const snippetEl = document.createElement("div");
        snippetEl.className = "sa-result-snippet";
        snippetEl.textContent = r.snippet;
        li.appendChild(snippetEl);
      }

      li.addEventListener("click", (e) => {
        e.preventDefault();
        navigateToResult(r);
      });

      resultsList.appendChild(li);
    });
  }

  function moveSelection(delta) {
    if (currentResults.length === 0) return;

    selectedIndex += delta;
    if (selectedIndex < 0) selectedIndex = currentResults.length - 1;
    if (selectedIndex >= currentResults.length) selectedIndex = 0;

    const items = resultsList.querySelectorAll(".sa-result");
    items.forEach((el, i) => {
      el.setAttribute("aria-selected", i === selectedIndex ? "true" : "false");
    });

    // Scroll selected into view
    const selected = items[selectedIndex];
    if (selected) {
      selected.scrollIntoView({ block: "nearest" });
    }
  }

  function navigateToResult(r) {
    closeModal();
    window.location.href = r.url;
  }

  function clearResults() {
    currentResults = [];
    currentAnswer = null;
    selectedIndex = -1;
    searchPending = false;
    resultsList.textContent = "";
    answerEl.classList.remove("sa-visible");
  }

  function applyTriggerOffsets() {
    const baseInset = 24;
    const vertical = `${baseInset + config.offsetY}px`;
    const horizontal = `${baseInset + config.offsetX}px`;

    trigger.style.top = "";
    trigger.style.bottom = "";
    trigger.style.left = "";
    trigger.style.right = "";

    switch (config.position) {
      case "top-left":
        trigger.style.top = vertical;
        trigger.style.left = horizontal;
        break;
      case "top-right":
        trigger.style.top = vertical;
        trigger.style.right = horizontal;
        break;
      case "bottom-left":
        trigger.style.bottom = vertical;
        trigger.style.left = horizontal;
        break;
      default:
        trigger.style.bottom = vertical;
        trigger.style.right = horizontal;
        break;
    }
  }

  // -- Modal open/close --
  function openModal() {
    isOpen = true;
    rotateHeartSprite();
    backdrop.classList.add("sa-open");
    trigger.style.display = "none";
    input.value = "";
    clearResults();
    ensureWorker();
    // Focus after animation frame so the browser paints first
    requestAnimationFrame(() => input.focus());
  }

  function closeModal() {
    isOpen = false;
    backdrop.classList.remove("sa-open");
    trigger.style.display = "";
    trigger.focus();
  }

  function rotateHeartSprite() {
    if (HEART_SPRITES.length === 0) return;
    let idx = Math.floor(Math.random() * HEART_SPRITES.length);
    if (HEART_SPRITES.length > 1 && idx === lastHeartIndex) {
      idx = (idx + 1) % HEART_SPRITES.length;
    }
    lastHeartIndex = idx;
    drawHeartSprite(idx);
  }

  function drawHeartSprite(idx) {
    const sprite = HEART_SPRITES[idx];
    const ctx = heart.getContext("2d");
    if (!sprite || !ctx) return;
    ctx.clearRect(0, 0, heart.width, heart.height);

    for (let y = 0; y < sprite.length; y++) {
      const row = sprite[y];
      for (let x = 0; x < row.length; x++) {
        const key = row[x];
        if (key === ".") continue;
        const color = HEART_PALETTE[key];
        if (!color) continue;
        ctx.fillStyle = color;
        ctx.fillRect(x, y, 1, 1);
      }
    }
  }

  // -- Helpers --
  function showStatus(visible) {
    status.classList.toggle("sa-visible", visible);
  }

  function showError(message) {
    errorEl.textContent = message;
    errorEl.classList.add("sa-visible");
  }

  function shouldUseAnswerMode(query) {
    if (config.qaMode === "off") return false;
    if (config.qaMode === "always") return true;
    return looksFactualQuery(query);
  }

  function looksFactualQuery(query) {
    const q = query.toLowerCase().trim();
    if (!q) return false;
    if (q.includes("?")) return true;
    if (/^(who|what|when|where|why|how|does|do|is|are|can|could|should)\b/i.test(q)) {
      return true;
    }
    return q.split(/\s+/).length >= 5;
  }

  function renderAnswer() {
    answerEl.textContent = "";
    if (!currentAnswer || !currentAnswer.text) {
      answerEl.classList.remove("sa-visible");
      return;
    }

    const label = document.createElement("div");
    label.className = "sa-answer-label";
    label.textContent = "Experimental Answer";
    answerEl.appendChild(label);

    const text = document.createElement("div");
    text.className = "sa-answer-text";
    text.textContent = currentAnswer.text;
    answerEl.appendChild(text);

    if (currentAnswer.citations && currentAnswer.citations.length > 0) {
      const cites = document.createElement("div");
      cites.className = "sa-answer-cites";
      currentAnswer.citations.slice(0, 3).forEach((url) => {
        const a = document.createElement("a");
        a.className = "sa-answer-cite";
        a.href = url;
        a.textContent = "source";
        a.addEventListener("click", (e) => {
          e.preventDefault();
          navigateToResult({ url: url });
        });
        cites.appendChild(a);
      });
      answerEl.appendChild(cites);
    }

    answerEl.classList.add("sa-visible");
  }

  // -- Global keyboard shortcut: Ctrl+K or Cmd+K --
  document.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key === "k") {
      e.preventDefault();
      if (isOpen) {
        closeModal();
      } else {
        openModal();
      }
    }
  });

  // -- Mount --
  document.body.appendChild(host);
})();
