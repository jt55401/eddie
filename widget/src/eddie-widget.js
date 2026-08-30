// SPDX-License-Identifier: GPL-3.0-only

// Eddie search widget
//
// Self-contained vanilla JS widget in a closed Shadow DOM. Talks to the
// search engine for retrieval and, on demand, to the agent for a streamed,
// cited answer. Both engines live either in the Eddie service worker
// (eddie-sw.js, persistent across navigations; see "Persistent engines" in
// widget/README.md) or, as the fallback, in page-side workers
// (eddie-worker.js, eddie-agent-worker.js). The transports in lib/transport.js
// hide the difference.
//
// widget/build.sh concatenates widget/src/lib/*.js ahead of this file inside
// one IIFE, so the pure helpers are available as `EddieLib` without leaking a
// global.

"use strict";

(function () {
  const scriptEl = document.currentScript;
  if (!scriptEl) return;
  const lib = EddieLib;

  const config = lib.parseWidgetConfig((name) => scriptEl.getAttribute(name));
  const scriptHref = new URL(scriptEl.src, location.href).href;
  const baseUrl = lib.baseUrlOf(scriptHref);
  const indexUrl = config.indexUrl
    ? new URL(config.indexUrl, location.href).href
    : new URL("index.ed", baseUrl).href;
  const version = lib.versionOf(scriptHref) || lib.versionOf(indexUrl);
  const siteName = config.qaSubject || location.hostname;
  const reducedMotion = !!(window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches);
  const saveData = !!(navigator.connection && navigator.connection.saveData);

  const HEART_SPRITES = [
    [".11111.", "1222221", "1222221", ".12221."], // solid
    [".11111.", "12.2.21", "1222221", ".12.21."], // circuit
    [".13331.", "1344431", "1244421", ".12221."], // beveled
    [".13131.", "1344431", "12.4.21", ".12221."], // gear-ish
  ];
  const HEART_PALETTE = { 1: "#f2c94c", 2: "#e0b63f", 3: "#f8dda1", 4: "#b78e28" };
  const HIDDEN_SECTIONS = /^(summary|summary lane|semantic segment)$/i;
  const AGENT_CONSENT_KEY = "eddie.agent.consent";
  const SEARCH_CONSENT_KEY = lib.SEARCH_CONSENT_KEY;
  // How long a modal open waits for the transport decision (service worker
  // registration + hello) before starting a page-side worker instead.
  const TRANSPORT_WAIT_MS = 3000;
  // Longest a search-engine request may stay unanswered over the service
  // worker before the widget checks whether the worker is still alive.
  const SW_CALL_TIMEOUT_MS = 60000;

  // -- State --
  let search = null; // search transport (lib/transport.js), null until needed
  let workerState = "idle"; // idle | loading | index_ready | awaiting_consent | ready | error | dead
  let searchable = false;
  let initSent = false; // an init went to the current search transport
  let requestSeq = 0;
  let activeSearchId = 0;
  let isOpen = false;
  let selectedIndex = -1;
  let currentResults = [];
  let lastHeartIndex = -1;
  let consentDeclined = false;
  let pendingConsentLane = null; // lane id of the consent card being shown
  let lastDegradedNotice = null;
  let lastAnnouncedQuarter = -1;
  let manifestInfo = null;
  let searchTimer = null;
  let lastSearchedQuery = "";
  let initWaiters = []; // resolve() once the index is loaded (index_ready or ready)
  let transportPlan = null; // Promise<{kind, registration?, hello?, swSearch?}>
  let searchCreating = null; // Promise while ensureSearchTransport runs
  let forcePageWorkers = false; // after a dead service-worker engine: this page uses page workers
  let pageGpu = null; // null (unknown) | false | { maxBufferSize, hasF16 }
  let pageGpuProbe = null;

  let agentInfo = null; // null (unknown) | false | { maxBufferSize, hasF16 }
  let agent = null; // agent transport, null until the first Ask
  let agentCreating = null;
  let agentModel = null; // { id, base, sizeBytes }
  let agentLoaded = false;
  let agentLoading = null; // Promise while a load is in flight
  let agentRun = null; // { requestId, question, text, aborted }
  let agentPendingLoad = null; // { resolve, reject }

  // -- DOM setup --
  const host = document.createElement("div");
  host.id = "eddie-host";
  host.dataset.theme = config.theme;
  host.dataset.state = workerState;
  const shadow = host.attachShadow({ mode: "closed" });

  /** The worker state is mirrored on the host as `data-state` (readable by the page). */
  function setWorkerState(next) {
    workerState = next;
    host.dataset.state = next;
  }

  const DARK_TOKENS = `
      --sa-bg: #1a1a1a;
      --sa-bg-elevated: #252525;
      --sa-text: #e8e8e8;
      --sa-text-muted: #a3a3a3;
      --sa-border: #333333;
      --sa-accent: #60a5fa;
      --sa-accent-soft: rgba(96, 165, 250, 0.12);
      --sa-backdrop: rgba(0, 0, 0, 0.6);
      --sa-shadow: 0 16px 48px rgba(0, 0, 0, 0.4), 0 2px 8px rgba(0, 0, 0, 0.2);
      --sa-error: #f87171;
      --sa-error-bg: rgba(248, 113, 113, 0.1);
  `;

  const style = document.createElement("style");
  style.textContent = `
    :host {
      --sa-font: "IBM Plex Sans", -apple-system, BlinkMacSystemFont, sans-serif;
      --sa-font-mono: "IBM Plex Mono", "SF Mono", "Fira Code", monospace;
      --sa-bg: #ffffff;
      --sa-bg-elevated: #f6f6f6;
      --sa-text: #1a1a1a;
      --sa-text-muted: #5f5f5f;
      --sa-border: #e0e0e0;
      --sa-accent: #2563eb;
      --sa-accent-soft: rgba(37, 99, 235, 0.08);
      --sa-backdrop: rgba(0, 0, 0, 0.4);
      --sa-shadow: 0 16px 48px rgba(0, 0, 0, 0.12), 0 2px 8px rgba(0, 0, 0, 0.08);
      --sa-error: #b91c1c;
      --sa-error-bg: rgba(185, 28, 28, 0.06);
      --sa-radius: 12px;
      --sa-radius-sm: 6px;
      --sa-trigger-size: 48px;

      all: initial;
      font-family: var(--sa-font);
      position: fixed;
      z-index: 999999;
    }
    :host([data-theme="dark"]) { ${DARK_TOKENS} }
    @media (prefers-color-scheme: dark) {
      :host([data-theme="auto"]) { ${DARK_TOKENS} }
    }

    *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }

    .sa-visually-hidden {
      position: absolute !important;
      width: 1px; height: 1px;
      padding: 0; margin: -1px;
      overflow: hidden;
      clip: rect(0, 0, 0, 0);
      white-space: nowrap;
      border: 0;
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
    .sa-trigger:hover { transform: scale(1.06); box-shadow: 0 4px 16px rgba(0,0,0,0.15); }
    .sa-trigger:active { transform: scale(0.96); }
    .sa-trigger:focus-visible, .sa-close:focus-visible, .sa-btn:focus-visible,
    .sa-result:focus-visible, .sa-cite:focus-visible, .sa-answer-cite:focus-visible {
      outline: 2px solid var(--sa-accent);
      outline-offset: 2px;
    }
    .sa-trigger svg { width: 20px; height: 20px; stroke: currentColor; fill: none; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }

    .sa-backdrop {
      position: fixed;
      inset: 0;
      background: var(--sa-backdrop);
      display: none;
      align-items: flex-start;
      justify-content: center;
      padding-top: 12vh;
    }
    .sa-backdrop.sa-open { display: flex; }

    .sa-modal {
      background: var(--sa-bg);
      color: var(--sa-text);
      border: 1px solid var(--sa-border);
      border-radius: var(--sa-radius);
      box-shadow: var(--sa-shadow);
      width: 100%;
      max-width: 620px;
      max-height: 76vh;
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
      padding: 12px 16px;
      gap: 10px;
      border-bottom: 1px solid var(--sa-border);
    }
    .sa-search-icon { flex-shrink: 0; width: 18px; height: 18px; stroke: var(--sa-text-muted); fill: none; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }
    .sa-input {
      flex: 1;
      min-width: 0;
      border: none;
      background: none;
      font-family: var(--sa-font);
      font-size: 16px;
      color: var(--sa-text);
      outline: none;
    }
    .sa-input::placeholder { color: var(--sa-text-muted); }

    .sa-btn {
      flex-shrink: 0;
      border-radius: var(--sa-radius-sm);
      border: 1px solid var(--sa-border);
      background: var(--sa-bg-elevated);
      color: var(--sa-text);
      cursor: pointer;
      font-family: var(--sa-font);
      font-size: 12px;
      padding: 5px 10px;
      line-height: 1.2;
      transition: border-color 0.1s;
    }
    .sa-btn:hover { border-color: var(--sa-text-muted); }
    .sa-btn:disabled { opacity: 0.5; cursor: default; }
    .sa-btn-primary { background: var(--sa-accent); border-color: var(--sa-accent); color: #fff; }
    .sa-btn-primary:hover { filter: brightness(1.08); border-color: var(--sa-accent); }
    .sa-ask { font-weight: 600; }
    .sa-ask[hidden] { display: none; }

    .sa-close {
      flex-shrink: 0;
      height: 28px;
      min-width: 28px;
      padding: 0 6px;
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
    .sa-close:hover { border-color: var(--sa-text-muted); }

    .sa-heart { flex-shrink: 0; width: 14px; height: 8px; image-rendering: pixelated; image-rendering: crisp-edges; opacity: 0.95; display: block; }

    .sa-status {
      padding: 10px 16px;
      font-size: 13px;
      color: var(--sa-text-muted);
      display: none;
      align-items: center;
      gap: 10px;
      border-bottom: 1px solid var(--sa-border);
    }
    .sa-status.sa-visible { display: flex; }
    .sa-status-text { flex-shrink: 0; }
    .sa-progress-bar { flex: 1; height: 3px; background: var(--sa-bg-elevated); border-radius: 2px; overflow: hidden; }
    .sa-progress-fill { height: 100%; background: var(--sa-accent); border-radius: 2px; width: 0%; transition: width 0.2s ease; }
    .sa-progress-indeterminate .sa-progress-fill { width: 40%; animation: sa-indeterminate 1.2s ease-in-out infinite; }
    @keyframes sa-indeterminate {
      0%   { transform: translateX(-100%); }
      100% { transform: translateX(350%); }
    }

    .sa-card {
      display: none;
      border-bottom: 1px solid var(--sa-border);
      background: var(--sa-bg-elevated);
      padding: 12px 16px;
      flex-direction: column;
      gap: 8px;
      font-size: 13px;
      line-height: 1.45;
      color: var(--sa-text);
    }
    .sa-card.sa-visible { display: flex; }
    .sa-card-label {
      font-size: 11px;
      letter-spacing: 0.08em;
      text-transform: uppercase;
      color: var(--sa-text-muted);
      font-family: var(--sa-font-mono);
      display: flex;
      align-items: center;
      gap: 8px;
      flex-wrap: wrap;
    }
    .sa-card-label .sa-model { text-transform: none; letter-spacing: 0; }
    .sa-card-actions { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; }
    .sa-muted { color: var(--sa-text-muted); }

    .sa-error {
      padding: 10px 16px;
      font-size: 13px;
      color: var(--sa-error);
      background: var(--sa-error-bg);
      border-bottom: 1px solid var(--sa-border);
      display: none;
      align-items: center;
      gap: 10px;
      flex-wrap: wrap;
    }
    .sa-error.sa-visible { display: flex; }
    .sa-error .sa-btn { color: var(--sa-text); }

    .sa-notice {
      padding: 6px 16px;
      font-size: 12px;
      color: var(--sa-text-muted);
      border-bottom: 1px solid var(--sa-border);
      display: none;
    }
    .sa-notice.sa-visible { display: block; }

    .sa-answer-text:not(.sa-visible), .sa-sources:not(.sa-visible), .sa-answer-progress:not(.sa-visible) { display: none; }
    .sa-answer-text { font-size: 14px; line-height: 1.5; color: var(--sa-text); white-space: pre-wrap; }
    .sa-answer-text.sa-nohit { color: var(--sa-text-muted); font-style: italic; }
    .sa-answer-cite { color: var(--sa-accent); text-decoration: none; font-family: var(--sa-font-mono); font-size: 12px; }
    .sa-answer-cite:hover { text-decoration: underline; }
    .sa-caret { display: inline-block; width: 0.5em; height: 1em; vertical-align: text-bottom; background: var(--sa-accent); animation: sa-blink 1s steps(2) infinite; margin-left: 2px; }
    @keyframes sa-blink { to { opacity: 0; } }
    .sa-sources { list-style: none; display: flex; flex-direction: column; gap: 3px; }
    .sa-sources li { font-size: 12px; }
    .sa-cite { color: var(--sa-accent); text-decoration: none; }
    .sa-cite:hover { text-decoration: underline; }
    .sa-cite-n { font-family: var(--sa-font-mono); color: var(--sa-text-muted); margin-right: 6px; }
    .sa-faq-q { font-weight: 600; }

    .sa-results { flex: 1; overflow-y: auto; list-style: none; }
    .sa-empty { padding: 32px 16px; text-align: center; color: var(--sa-text-muted); font-size: 14px; display: none; }
    .sa-empty.sa-visible { display: block; }
    .sa-option { list-style: none; }
    .sa-result {
      display: block;
      padding: 12px 16px;
      border-bottom: 1px solid var(--sa-border);
      cursor: pointer;
      text-decoration: none;
      color: inherit;
      transition: background 0.08s;
    }
    .sa-option:last-child .sa-result { border-bottom: none; }
    .sa-result:hover, .sa-option[aria-selected="true"] .sa-result { background: var(--sa-accent-soft); }
    .sa-result-title { font-size: 14px; font-weight: 600; color: var(--sa-text); margin-bottom: 2px; }
    .sa-result-url { font-family: var(--sa-font-mono); font-size: 11px; color: var(--sa-accent); margin-bottom: 4px; word-break: break-all; }
    .sa-result-section { font-size: 11px; color: var(--sa-text-muted); margin-bottom: 4px; }
    .sa-result-snippet { font-size: 13px; color: var(--sa-text-muted); line-height: 1.45; }

    .sa-footer {
      padding: 8px 16px;
      border-top: 1px solid var(--sa-border);
      display: flex;
      align-items: center;
      justify-content: space-between;
      font-size: 11px;
      color: var(--sa-text-muted);
      gap: 8px;
      flex-wrap: wrap;
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
    .sa-brand { display: inline-flex; align-items: center; gap: 6px; letter-spacing: 0.08em; font-weight: 600; }
    .sa-brand-link {
      color: var(--sa-text-muted);
      text-decoration: none;
      border: 1px solid var(--sa-border);
      border-radius: 999px;
      padding: 2px 8px;
      transition: border-color 0.12s ease, color 0.12s ease, background 0.12s ease;
    }
    .sa-brand-link:hover { border-color: var(--sa-text-muted); color: var(--sa-text); background: var(--sa-bg-elevated); }
    .sa-brand-link:focus-visible { outline: 1px solid var(--sa-accent); outline-offset: 2px; }

    @media (prefers-reduced-motion: reduce) {
      .sa-modal { animation: none; }
      .sa-trigger, .sa-progress-fill, .sa-result, .sa-btn, .sa-close, .sa-brand-link { transition: none; }
      .sa-progress-indeterminate .sa-progress-fill { animation: none; width: 100%; opacity: 0.5; }
      .sa-caret { animation: none; }
    }

    @media (max-width: 640px) {
      .sa-backdrop { padding-top: 0; align-items: flex-end; }
      .sa-modal { max-width: 100%; max-height: 85vh; border-radius: var(--sa-radius) var(--sa-radius) 0 0; animation-name: sa-slide-up; }
      @keyframes sa-slide-up {
        from { opacity: 0; transform: translateY(40px); }
        to   { opacity: 1; transform: translateY(0); }
      }
    }
  `;
  shadow.appendChild(style);

  // -- Element helpers --
  function el(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text != null) node.textContent = text;
    return node;
  }

  function button(className, text, onClick, attrs) {
    const b = el("button", className, text);
    b.type = "button";
    if (onClick) b.addEventListener("click", onClick);
    if (attrs) Object.keys(attrs).forEach((k) => b.setAttribute(k, attrs[k]));
    return b;
  }

  function createSearchSvg(className) {
    const ns = "http://www.w3.org/2000/svg";
    const svg = document.createElementNS(ns, "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("aria-hidden", "true");
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
  const trigger = button(`sa-trigger sa-pos-${config.position}`, null, openModal, { "aria-label": "Search" });
  trigger.appendChild(createSearchSvg());
  applyTriggerOffsets();
  shadow.appendChild(trigger);

  // -- Backdrop + modal --
  const backdrop = el("div", "sa-backdrop");
  backdrop.addEventListener("click", (e) => {
    if (e.target === backdrop) closeModal();
  });
  shadow.appendChild(backdrop);

  const modal = el("div", "sa-modal");
  modal.setAttribute("role", "dialog");
  modal.setAttribute("aria-modal", "true");
  modal.setAttribute("aria-label", "Search");
  backdrop.appendChild(modal);

  // Header
  const header = el("div", "sa-header");
  header.appendChild(createSearchSvg("sa-search-icon"));
  modal.appendChild(header);

  const input = el("input", "sa-input");
  input.type = "text";
  input.setAttribute("role", "combobox");
  input.setAttribute("aria-label", "Search query");
  input.setAttribute("aria-autocomplete", "list");
  input.setAttribute("aria-haspopup", "listbox");
  input.setAttribute("aria-expanded", "false");
  input.setAttribute("aria-controls", "sa-results");
  input.setAttribute("autocomplete", "off");
  input.setAttribute("spellcheck", "false");
  input.placeholder = "Search…";
  header.appendChild(input);

  const askBtn = button("sa-btn sa-ask", "Ask", () => askQuestion(input.value.trim()), {
    "aria-label": "Ask a question about this site (Shift+Enter)",
    title: "Ask (Shift+Enter)",
  });
  askBtn.hidden = true;
  header.appendChild(askBtn);

  const closeBtn = button("sa-close", "esc", closeModal, { "aria-label": "Close (Esc)" });
  header.appendChild(closeBtn);

  const heart = document.createElement("canvas");
  heart.className = "sa-heart";
  heart.width = 7;
  heart.height = 4;
  heart.setAttribute("aria-hidden", "true");
  drawHeartSprite(0);

  // Live region for screen readers (loading, progress, counts, answers)
  const liveRegion = el("div", "sa-visually-hidden");
  liveRegion.setAttribute("role", "status");
  liveRegion.setAttribute("aria-live", "polite");
  liveRegion.setAttribute("aria-atomic", "true");
  modal.appendChild(liveRegion);

  // Status bar (visible progress)
  const status = el("div", "sa-status");
  const statusText = el("span", "sa-status-text");
  status.appendChild(statusText);
  const progressBar = el("div", "sa-progress-bar");
  progressBar.setAttribute("role", "progressbar");
  progressBar.setAttribute("aria-label", "Loading progress");
  progressBar.setAttribute("aria-valuemin", "0");
  progressBar.setAttribute("aria-valuemax", "100");
  const progressFill = el("div", "sa-progress-fill");
  progressBar.appendChild(progressFill);
  status.appendChild(progressBar);
  modal.appendChild(status);

  // Download consent card
  const consentCard = el("div", "sa-card");
  const consentLabel = el("div", "sa-card-label", "Download the search model?");
  const consentText = el("div");
  const consentActions = el("div", "sa-card-actions");
  const consentAccept = button("sa-btn sa-btn-primary", "Download", acceptConsent);
  const consentDecline = button("sa-btn", "Keyword search only", declineConsent);
  consentActions.appendChild(consentAccept);
  consentActions.appendChild(consentDecline);
  consentCard.appendChild(consentLabel);
  consentCard.appendChild(consentText);
  consentCard.appendChild(consentActions);
  modal.appendChild(consentCard);

  // Error area
  const errorEl = el("div", "sa-error");
  errorEl.setAttribute("role", "alert");
  const errorText = el("span");
  const retryBtn = button("sa-btn", "Retry", retryInit);
  errorEl.appendChild(errorText);
  errorEl.appendChild(retryBtn);
  modal.appendChild(errorEl);

  // Degraded notice
  const noticeEl = el("div", "sa-notice");
  modal.appendChild(noticeEl);

  // Answer card (agent)
  const answerCard = el("div", "sa-card sa-answer");
  const answerLabel = el("div", "sa-card-label");
  const answerLabelText = el("span", null, "Answer");
  const answerModel = el("span", "sa-model");
  answerLabel.appendChild(answerLabelText);
  answerLabel.appendChild(answerModel);
  const answerProgress = el("div", "sa-answer-progress sa-muted");
  const answerText = el("div", "sa-answer-text");
  const answerSources = el("ol", "sa-sources");
  const answerActions = el("div", "sa-card-actions");
  const stopBtn = button("sa-btn", "Stop", () => abortAgent("stopped"));
  // The three agent buttons get a fresh `onclick` each time they are shown.
  const answerRetryBtn = button("sa-btn", "Retry", null);
  const agentConsentAccept = button("sa-btn sa-btn-primary", "Download and answer", null);
  const agentConsentCancel = button("sa-btn", "Cancel", null);
  agentConsentCancel.onclick = hideAnswerCard;
  answerCard.appendChild(answerLabel);
  answerCard.appendChild(answerProgress);
  answerCard.appendChild(answerText);
  answerCard.appendChild(answerSources);
  answerCard.appendChild(answerActions);
  modal.appendChild(answerCard);

  // FAQ card (qa_lookup hits)
  const faqCard = el("div", "sa-card");
  faqCard.appendChild(el("div", "sa-card-label", "From the FAQ"));
  const faqQ = el("div", "sa-faq-q");
  const faqA = el("div");
  const faqSrc = el("div");
  faqCard.appendChild(faqQ);
  faqCard.appendChild(faqA);
  faqCard.appendChild(faqSrc);
  modal.appendChild(faqCard);

  // Empty state (outside the listbox)
  const emptyEl = el("div", "sa-empty", "No results found.");
  modal.appendChild(emptyEl);

  // Results
  const resultsList = el("ul", "sa-results");
  resultsList.id = "sa-results";
  resultsList.setAttribute("role", "listbox");
  resultsList.setAttribute("aria-label", "Search results");
  modal.appendChild(resultsList);

  // Footer
  const footer = el("div", "sa-footer");
  const footerNav = el("span");
  const keys = [["↑", ""], ["↓", " navigate "], ["enter", " open"]];
  keys.forEach(([key, after]) => {
    const kbd = el("kbd", null, key);
    footerNav.appendChild(kbd);
    if (after) footerNav.appendChild(document.createTextNode(after));
  });
  const askHint = el("span");
  askHint.hidden = true;
  askHint.appendChild(document.createTextNode(" "));
  askHint.appendChild(el("kbd", null, "shift+enter"));
  askHint.appendChild(document.createTextNode(" ask"));
  footerNav.appendChild(askHint);
  footer.appendChild(footerNav);

  const footerBrandLink = el("a", "sa-brand-link");
  footerBrandLink.href = "https://github.com/jt55401/eddie";
  footerBrandLink.target = "_blank";
  footerBrandLink.rel = "noopener noreferrer";
  footerBrandLink.setAttribute("aria-label", "Eddie on GitHub (opens in a new tab)");
  const footerBrand = el("span", "sa-brand");
  footerBrand.appendChild(document.createTextNode("EDDIE"));
  footerBrand.appendChild(heart);
  footerBrandLink.appendChild(footerBrand);
  footer.appendChild(footerBrandLink);
  modal.appendChild(footer);

  // -- Keyboard handling --
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      const q = input.value.trim();
      if (e.shiftKey) {
        if (q && !askBtn.hidden) {
          // Flush the pending search-as-you-type run so it neither races the
          // answer nor fires 200 ms later and cancels it.
          clearTimeout(searchTimer);
          if (lastSearchedQuery !== q) doSearch(q, true);
          askQuestion(q);
        }
        return;
      }
      if (selectedIndex >= 0 && selectedIndex < currentResults.length) {
        navigateTo(currentResults[selectedIndex].url);
      } else if (q) {
        clearTimeout(searchTimer);
        doSearch(q, true);
      }
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      moveSelection(1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      moveSelection(-1);
    }
  });

  // Search-as-you-type: retrieval only, never the agent.
  input.addEventListener("input", () => {
    clearTimeout(searchTimer);
    const q = input.value.trim();
    setSelected(-1);
    if (q.length >= 2) {
      searchTimer = setTimeout(() => doSearch(q, false), 200);
    } else if (q.length === 0) {
      clearResults();
    }
  });

  // Esc closes; Tab cycles through everything focusable in the modal.
  modal.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      closeModal();
      return;
    }
    if (e.key !== "Tab") return;
    const focusable = Array.from(modal.querySelectorAll("input, button, a[href]")).filter(isVisible);
    if (focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    const current = shadow.activeElement;
    if (e.shiftKey && (current === first || !focusable.includes(current))) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && current === last) {
      e.preventDefault();
      first.focus();
    }
  });

  function isVisible(node) {
    if (node.hidden || node.disabled) return false;
    let n = node;
    while (n && n !== modal) {
      if (n.hidden) return false;
      const cs = getComputedStyle(n);
      if (cs.display === "none" || cs.visibility === "hidden") return false;
      n = n.parentElement;
    }
    return true;
  }

  // -- Transports --
  // The decision (service worker or page workers) starts after `load`, from
  // an idle callback, so registration never delays the page. Modal opens
  // wait for it at most TRANSPORT_WAIT_MS; warm-up waits as long as it takes.
  function onIdle(fn) {
    if (typeof requestIdleCallback === "function") requestIdleCallback(() => fn(), { timeout: 2000 });
    else setTimeout(fn, 200);
  }

  function afterLoad(fn) {
    if (document.readyState === "complete") fn();
    else window.addEventListener("load", () => fn(), { once: true });
  }

  function ensureTransportPlan() {
    if (!transportPlan) transportPlan = setupTransport();
    return transportPlan;
  }

  async function setupTransport() {
    const kind = lib.chooseTransportKind({
      persist: config.persist,
      hasServiceWorker: !!navigator.serviceWorker,
      secureContext: window.isSecureContext,
    });
    if (kind !== "sw" || forcePageWorkers) return { kind: "worker" };
    let registration = null;
    let swSearch = null;
    try {
      registration = await lib.registerServiceWorker({
        container: navigator.serviceWorker,
        url: lib.assetUrl(baseUrl, "eddie-sw.js", version),
        scope: baseUrl,
      });
      swSearch = new lib.ServiceWorkerTransport(registration, { kind: "search", version });
      const hello = await swSearch.connect();
      // The service worker cannot run a webgpu-onnx lane (no WebGPU in its
      // scope) while this page could: search stays page-side for quality,
      // the agent still goes wherever the service worker can host it.
      if (lib.searchStaysOnPage({ swOnnx: hello.onnx, pageHasGpu: !!(await probePageGpu()), denseRuntime: config.denseRuntime })) {
        swSearch.terminate();
        swSearch = null;
      }
      return { kind: "sw", registration, hello, swSearch };
    } catch (err) {
      console.info("eddie: service worker unavailable, using page workers:", err && err.message ? err.message : err);
      if (swSearch) swSearch.terminate();
      return { kind: "worker" };
    }
  }

  function withDeadline(promise, ms) {
    return new Promise((resolve) => {
      const timer = setTimeout(() => resolve(null), ms);
      promise.then(
        (v) => {
          clearTimeout(timer);
          resolve(v);
        },
        () => {
          clearTimeout(timer);
          resolve(null);
        }
      );
    });
  }

  function probePageGpu() {
    if (pageGpu !== null) return Promise.resolve(pageGpu);
    if (pageGpuProbe) return pageGpuProbe;
    pageGpuProbe = (async () => {
      pageGpu = false;
      if (!navigator.gpu || typeof navigator.gpu.requestAdapter !== "function") return pageGpu;
      try {
        const adapter = await navigator.gpu.requestAdapter();
        if (adapter) {
          pageGpu = {
            maxBufferSize: adapter.limits ? adapter.limits.maxBufferSize : 0,
            hasF16: !!(adapter.features && adapter.features.has("shader-f16")),
          };
        }
      } catch (err) {
        console.warn("eddie: WebGPU adapter probe failed", err);
      }
      return pageGpu;
    })();
    return pageGpuProbe;
  }

  // -- Search transport --
  /** Create and wire the search transport (no init yet). `wait` bounds the transport decision. */
  function ensureSearchTransport(waitMs) {
    if (search) return Promise.resolve(search);
    if (searchCreating) return searchCreating;
    searchCreating = (async () => {
      const planPromise = ensureTransportPlan();
      const plan = waitMs == null ? await planPromise.catch(() => null) : await withDeadline(planPromise, waitMs);
      if (search) return search;
      let t = plan && plan.kind === "sw" && plan.swSearch && !forcePageWorkers ? plan.swSearch : null;
      if (!t) {
        try {
          t = new lib.DedicatedWorkerTransport(lib.assetUrl(baseUrl, "eddie-worker.js", version));
        } catch (err) {
          showInitError("Couldn't start the search worker: " + (err && err.message ? err.message : err), false);
          return null;
        }
      }
      attachSearch(t, plan);
      return t;
    })().finally(() => {
      searchCreating = null;
    });
    return searchCreating;
  }

  function attachSearch(t, plan) {
    search = t;
    initSent = false;
    host.dataset.transport = t.kind;
    t.on("status", handleStatus);
    t.on("ready", handleReady);
    t.on("error", (msg) => {
      if (msg.requestId != null) return; // request errors reject their promise
      if (msg.fatal) onFatal(msg.message);
      else showInitError(msg.message || "Search failed", false);
    });
    t.on("crash", (msg) => {
      // An uncaught error in the worker script (a 404, a syntax error, an
      // exception outside a handler) or a service worker that could not be
      // reconnected: start afresh on Retry.
      setWorkerState("dead");
      searchable = false;
      showStatus(false);
      showInitError(msg.message || "search worker failed to load", true);
      failInitWaiters(new Error(msg.message || "search worker failed"));
    });
    t.on("reset", () => {
      // The service worker was stopped while idle and reconnected: its engine
      // is empty again. Re-run init transparently when the modal is open;
      // otherwise the next open does it.
      console.info("eddie: search engine restarted");
      searchable = false;
      initSent = false;
      setWorkerState("idle");
      host.dataset.arms = "";
      host.dataset.lane = "";
      host.dataset.runtime = "";
      if (isOpen) postInit(false);
    });
    if (t.kind === "sw" && plan && plan.hello && lib.canReuseSearch(plan.hello.search, indexUrl)) {
      adoptReadyState(plan.hello.search);
    }
  }

  /** The service worker already holds a ready engine for this index: mirror it. */
  function adoptReadyState(st) {
    initSent = true;
    handleReady({
      type: "ready",
      lanes: st.lanes || [],
      lane: st.lane,
      runtime: st.runtime,
      arms: st.arms || { dense: false, sparse: false, bm25: true },
      degraded: st.degraded || [],
      manifest: st.manifest,
      reused: true,
    });
  }

  /** Modal-open path: make sure a transport exists and an init is under way. */
  async function ensureWorker() {
    if (search) {
      if (workerState === "error") {
        retryInit();
        return;
      }
      // With the modal closed nothing pinged the service worker, so Chrome
      // may have stopped it: a failed ping reconnects and emits reset,
      // which re-sends init while the modal is open. A worker that is
      // streaming load status is alive by definition (and may be too busy
      // to answer a ping in time).
      if (search.kind === "sw" && initSent && workerState !== "loading") await search.ensureAlive(lib.IDLE_PING_TIMEOUT_MS);
      if (search && !initSent) postInit(false);
      return;
    }
    const t = await ensureSearchTransport(TRANSPORT_WAIT_MS);
    if (!t) return;
    if (!initSent) postInit(false);
  }

  function postInit(consent) {
    if (!search) return;
    initSent = true;
    setWorkerState("loading");
    hide(errorEl);
    search.postRaw({
      type: "init",
      indexUrl,
      baseUrl,
      version,
      denseRuntime: config.denseRuntime,
      consent: !!consent,
      consentLane: consent ? pendingConsentLane : undefined,
    });
    updateKeepalive();
  }

  function retryInit() {
    hide(errorEl);
    if (!search || workerState === "dead") {
      if (search) {
        // A dead engine inside the service worker stays dead until Chrome
        // restarts it; this page continues with page-side workers.
        if (search.kind === "sw") forcePageWorkers = true;
        search.terminate();
      }
      search = null;
      searchable = false;
      setWorkerState("idle");
      ensureWorker();
      return;
    }
    postInit(false);
  }

  /** Resolves once the index is loaded in the current transport. */
  function whenIndexLoaded() {
    if (searchable) return Promise.resolve();
    return new Promise((resolve, reject) => {
      initWaiters.push({ resolve, reject });
    });
  }

  function resolveInitWaiters() {
    const waiters = initWaiters;
    initWaiters = [];
    waiters.forEach((w) => w.resolve());
  }

  function failInitWaiters(err) {
    const waiters = initWaiters;
    initWaiters = [];
    waiters.forEach((w) => w.reject(err));
  }

  /**
   * A request to the search engine. A "not initialised" reply (the service
   * worker restarted, or a page-side worker that never got init) re-runs the
   * init flow once and retries.
   */
  async function callWorker(type, payload) {
    if (!search) throw new Error("search worker not running");
    const requestId = ++requestSeq;
    // A service worker that died mid-request never answers: bound the wait
    // and let the retry below reconnect.
    const opts = { requestId, timeoutMs: search.kind === "sw" ? SW_CALL_TIMEOUT_MS : 0 };
    try {
      return await search.call(type, payload, opts);
    } catch (err) {
      if (!err || err.fatal || !search) throw err;
      if (err.timeout) {
        if (!(await search.ensureAlive())) throw err;
      } else if (!lib.isNotLoadedMessage(err.message) && !/service worker restarted/.test(err.message)) {
        throw err;
      }
      if (!search) throw err;
      if (!initSent || workerState === "idle") postInit(false);
      await whenIndexLoaded();
      return search.call(type, payload, { requestId: ++requestSeq, timeoutMs: opts.timeoutMs });
    }
  }

  function updateKeepalive() {
    const wanted = lib.keepaliveWanted({
      open: isOpen,
      streaming: !!(agentRun && !agentRun.done),
      pending: !!(search && search.pending && search.pending.size > 0),
    });
    if (search) search.setKeepalive(wanted);
    if (agent) agent.setKeepalive(wanted);
  }

  function onFatal(message) {
    setWorkerState("dead");
    searchable = false;
    showStatus(false);
    showInitError(message || "The search engine crashed.", true);
    failInitWaiters(new Error(message || "search engine crashed"));
  }

  function handleStatus(msg) {
    switch (msg.state) {
      case "loading_wasm":
        setWorkerState("loading");
        setStatus("Loading search engine…", null);
        announce("Loading search engine");
        break;
      case "loading_index":
        setStatus("Loading index…", msg.progress);
        break;
      case "index_ready":
        manifestInfo = msg.manifest || null;
        setWorkerState("index_ready");
        searchable = true;
        if (initSent) setStatus("Loading search model…", null);
        resolveInitWaiters();
        rerunQuery();
        break;
      case "consent_required":
        setWorkerState("awaiting_consent");
        showStatus(false);
        pendingConsentLane = msg.lane && msg.lane.id ? msg.lane.id : null;
        if (consentDeclined) break;
        showConsent(msg);
        break;
      case "downloading_model": {
        const pct = msg.progress == null ? null : Math.round(msg.progress * 100);
        const file = msg.file || "model";
        setStatus(`Downloading ${file}…` + (pct == null ? "" : ` ${pct}%`), msg.progress);
        if (pct != null) {
          const quarter = Math.floor(pct / 25);
          if (quarter !== lastAnnouncedQuarter && pct < 100) {
            lastAnnouncedQuarter = quarter;
            announce(`Downloading model, ${pct}%`);
          }
        }
        break;
      }
      case "loading_model":
        setStatus(msg.file === "transformers.js" ? "Loading WebGPU runtime…" : "Loading model…", null);
        break;
      case "error":
        setWorkerState(msg.fatal ? "dead" : "error");
        showStatus(false);
        showInitError(msg.message || "Failed to initialise search", !msg.unsupported);
        failInitWaiters(new Error(msg.message || "search failed to initialise"));
        break;
      default:
        break;
    }
  }

  function handleReady(msg) {
    setWorkerState("ready");
    searchable = true;
    manifestInfo = msg.manifest || manifestInfo;
    const arms = msg.arms || {};
    host.dataset.arms = Object.keys(arms).filter((k) => arms[k]).join(",");
    host.dataset.lane = msg.lane || "";
    host.dataset.runtime = msg.runtime || "";
    host.dataset.readyMs = String(Math.round(performance.now()));
    host.dataset.reused = msg.reused ? "true" : "false";
    if (msg.lane) rememberSearchConsent(msg.lane);
    showStatus(false);
    hide(consentCard);
    lastDegradedNotice = lib.degradedNotice(msg.arms, msg.degraded);
    announce(lastDegradedNotice ? "Search ready. " + lastDegradedNotice : "Search ready");
    resolveInitWaiters();
    rerunQuery();
    updateKeepalive();
  }

  // -- Warm at load (data-warm) --
  // Runs after `load` and an idle callback, once the transport is decided.
  // Never downloads a model without a prior consent on this browser (auto);
  // `always` is the site owner's choice to warm uncached lanes too.
  async function warmUp() {
    if (config.warm === "off") return;
    const plan = await ensureTransportPlan().catch(() => null);
    if (!plan || search) return; // the modal opened first: the normal flow owns init
    const engineReady = plan.kind === "sw" && !!plan.swSearch && lib.canReuseSearch(plan.hello.search, indexUrl);
    let decision = lib.decideWarm({ mode: config.warm, saveData, engineReady, checked: false });
    console.debug("eddie warm:", decision.action, decision.reason);
    if (decision.action === "none") return;
    const t = await ensureSearchTransport(null);
    if (!t) return;
    if (decision.action === "adopt") return; // attachSearch adopted the ready state
    if (initSent) return;
    let cr;
    try {
      cr = await search.call("cache_check", { indexUrl, baseUrl, version, denseRuntime: config.denseRuntime }, { requestId: ++requestSeq, timeoutMs: 240000 });
    } catch (err) {
      console.info("eddie warm: cache check failed:", err && err.message ? err.message : err);
      return;
    }
    if (initSent || !search) return;
    decision = lib.decideWarm({
      mode: config.warm,
      saveData,
      engineReady: cr.phase === "ready",
      checked: true,
      lane: cr.lane,
      cached: cr.cached,
      consentedLane: readSearchConsent(),
    });
    console.debug("eddie warm:", decision.action, decision.reason);
    if (decision.action === "init") {
      if (decision.consent && cr.lane) pendingConsentLane = cr.lane.id;
      postInit(decision.consent);
    } else if (decision.action === "adopt") {
      postInit(false); // the engine answers with ready at once
    }
  }

  function readSearchConsent() {
    try {
      return localStorage.getItem(SEARCH_CONSENT_KEY) || null;
    } catch (_) {
      return null;
    }
  }

  function rememberSearchConsent(laneId) {
    try {
      localStorage.setItem(SEARCH_CONSENT_KEY, String(laneId));
    } catch (_) {
      // storage unavailable: no warm-up next visit
    }
  }

  function rerunQuery() {
    const q = input.value.trim();
    if (isOpen && q.length >= 2) doSearch(q, false);
  }

  // -- Consent --
  function showConsent(msg) {
    const lane = msg.lane || {};
    const model = lane.model || lane.repo || "the search model";
    const short = String(model).split("/").pop();
    consentText.textContent = lib.consentCopy({
      sizeBytes: msg.sizeBytes,
      model: short,
      saveData: saveData || !!msg.saveData,
      consentText: config.consentText,
    });
    consentAccept.textContent = msg.sizeBytes == null ? "Download" : `Download ${lib.formatBytes(msg.sizeBytes)}`;
    show(consentCard);
    announce("Semantic search needs a model download. " + consentText.textContent);
  }

  function acceptConsent() {
    hide(consentCard);
    if (!search) return;
    if (pendingConsentLane) rememberSearchConsent(pendingConsentLane);
    postInit(true);
    input.focus();
  }

  function declineConsent() {
    consentDeclined = true;
    hide(consentCard);
    lastDegradedNotice = "Keyword-only results: the semantic model wasn't downloaded.";
    renderNotice();
    input.focus();
  }

  // -- Searching --
  function looksFactualQuery(query) {
    const q = query.toLowerCase().trim();
    if (!q) return false;
    if (q.includes("?")) return true;
    if (/^(who|what|when|where|why|how|does|do|is|are|can|could|should)\b/.test(q)) return true;
    return q.split(/\s+/).length >= 5;
  }

  function wantQa(query) {
    if (config.qaMode === "off") return false;
    if (!manifestInfo || !Array.isArray(manifestInfo.sections) || !manifestInfo.sections.includes("qa")) return false;
    if (config.qaMode === "always") return true;
    return looksFactualQuery(query);
  }

  async function doSearch(query, explicit) {
    if (!searchable || !search) return;
    // A different query cancels a running answer; re-searching the question
    // being answered (a late debounce, a re-run after init) does not.
    if (agentRun && !agentRun.done && query !== agentRun.question) abortAgent("new search");
    lastSearchedQuery = query;
    const token = ++requestSeq;
    activeSearchId = token;
    try {
      const msg = await callWorker("search", {
        query,
        topK: config.topK,
        mode: "hybrid",
        qa: wantQa(query) ? Math.max(1, Math.min(config.answerTopK, 3)) : 0,
        explicit: !!explicit,
      });
      if (token !== activeSearchId) return;
      renderSearchResult(msg, query);
    } catch (err) {
      if (token !== activeSearchId) return;
      showSearchError((err && err.message) || "Search failed");
    }
  }

  function renderSearchResult(msg, query) {
    hide(errorEl);
    currentResults = msg.results || [];
    setSelected(-1);
    // A missing arm is only worth a notice once loading is over (or the
    // visitor declined the download); while the model loads the status bar
    // already says so.
    if (workerState === "ready") {
      lastDegradedNotice = lib.degradedNotice(msg.arms, msg.degraded);
    } else if (!consentDeclined) {
      lastDegradedNotice = null;
    }
    renderNotice();
    renderFaq(msg.qa);
    renderResults();
    const n = currentResults.length;
    announce(n === 0 ? `No results for ${query}` : `${n} result${n === 1 ? "" : "s"} for ${query}. Use the arrow keys to browse.`);
  }

  function showSearchError(message) {
    resultsList.textContent = "";
    hide(emptyEl);
    input.setAttribute("aria-expanded", "false");
    errorText.textContent = message;
    retryBtn.hidden = workerState !== "error" && workerState !== "dead";
    show(errorEl);
  }

  function showInitError(message, canRetry) {
    errorText.textContent = message;
    retryBtn.hidden = !canRetry;
    show(errorEl);
    showStatus(false);
  }

  function renderNotice() {
    if (lastDegradedNotice) {
      noticeEl.textContent = lastDegradedNotice;
      show(noticeEl);
    } else {
      hide(noticeEl);
    }
  }

  function renderFaq(hits) {
    faqQ.textContent = "";
    faqA.textContent = "";
    faqSrc.textContent = "";
    if (!Array.isArray(hits) || hits.length === 0) {
      hide(faqCard);
      return;
    }
    const best = hits[0];
    if (!lib.faqPasses(best, config.qaMode)) {
      hide(faqCard);
      return;
    }
    faqQ.textContent = best.question;
    faqA.textContent = best.answer;
    if (best.source_url) {
      const a = el("a", "sa-cite", best.source_title || best.source_url);
      a.href = best.source_url;
      a.addEventListener("click", (e) => {
        e.preventDefault();
        navigateTo(best.source_url);
      });
      faqSrc.appendChild(document.createTextNode("Source: "));
      faqSrc.appendChild(a);
    }
    show(faqCard);
  }

  function renderResults() {
    resultsList.textContent = "";
    if (currentResults.length === 0) {
      input.setAttribute("aria-expanded", "false");
      if (input.value.trim()) show(emptyEl);
      else hide(emptyEl);
      return;
    }
    hide(emptyEl);
    input.setAttribute("aria-expanded", "true");
    currentResults.forEach((r, i) => {
      const li = el("li", "sa-option");
      li.id = `sa-opt-${i}`;
      li.setAttribute("role", "option");
      li.setAttribute("aria-selected", i === selectedIndex ? "true" : "false");

      const a = el("a", "sa-result");
      a.href = r.url;
      a.tabIndex = -1;
      a.appendChild(el("div", "sa-result-title", r.title || r.url));
      a.appendChild(el("div", "sa-result-url", r.url));
      if (r.section && !HIDDEN_SECTIONS.test(String(r.section).trim())) {
        a.appendChild(el("div", "sa-result-section", r.section));
      }
      if (r.snippet) {
        a.appendChild(el("div", "sa-result-snippet", r.snippet));
      }
      a.addEventListener("click", (e) => {
        e.preventDefault();
        navigateTo(r.url);
      });
      a.addEventListener("focus", () => setSelected(i));
      li.appendChild(a);
      resultsList.appendChild(li);
    });
    // Result links are Tab-reachable once rendered.
    resultsList.querySelectorAll(".sa-result").forEach((a) => {
      a.tabIndex = 0;
    });
  }

  function setSelected(index) {
    selectedIndex = index;
    const items = resultsList.querySelectorAll(".sa-option");
    items.forEach((li, i) => li.setAttribute("aria-selected", i === selectedIndex ? "true" : "false"));
    if (selectedIndex >= 0 && items[selectedIndex]) {
      input.setAttribute("aria-activedescendant", items[selectedIndex].id);
    } else {
      input.removeAttribute("aria-activedescendant");
    }
  }

  function moveSelection(delta) {
    if (currentResults.length === 0) return;
    let next = selectedIndex + delta;
    if (next < 0) next = currentResults.length - 1;
    if (next >= currentResults.length) next = 0;
    setSelected(next);
    const items = resultsList.querySelectorAll(".sa-option");
    const selected = items[selectedIndex];
    if (selected) selected.scrollIntoView({ block: "nearest" });
  }

  function navigateTo(url) {
    closeModal();
    window.location.href = url;
  }

  function clearResults() {
    lastSearchedQuery = "";
    currentResults = [];
    activeSearchId = 0;
    setSelected(-1);
    resultsList.textContent = "";
    hide(emptyEl);
    hide(faqCard);
    input.setAttribute("aria-expanded", "false");
  }

  // -- Agent --
  async function detectAgent() {
    if (agentInfo !== null) return agentInfo;
    if (config.agentMode === "off") {
      agentInfo = false;
      return agentInfo;
    }
    const gpu = await probePageGpu();
    if (agentInfo !== null) return agentInfo;
    agentInfo = gpu ? { maxBufferSize: gpu.maxBufferSize, hasF16: gpu.hasF16 } : false;
    if (agentInfo) {
      agentModel = lib.selectAgentModel({
        mode: config.agentModel,
        maxBufferSize: agentInfo.maxBufferSize,
        isMobile: lib.isMobileDevice(navigator),
        hasF16: agentInfo.hasF16,
      });
      askBtn.hidden = false;
      askHint.hidden = false;
    }
    return agentInfo;
  }

  function agentConsentGiven() {
    try {
      return localStorage.getItem(AGENT_CONSENT_KEY) === agentModel.id;
    } catch (_) {
      return false;
    }
  }

  function rememberAgentConsent() {
    try {
      localStorage.setItem(AGENT_CONSENT_KEY, agentModel.id);
    } catch (_) {
      // storage unavailable: ask again next time
    }
  }

  function resetAnswerCard() {
    answerProgress.textContent = "";
    answerText.textContent = "";
    answerText.classList.remove("sa-nohit");
    answerSources.textContent = "";
    clearAnswerActions();
    answerModel.textContent = agentModel ? `${agentModel.base} · runs in your browser` : "";
    hide(answerText);
    hide(answerSources);
    hide(answerProgress);
  }

  /** Remove the answer card's buttons; if one of them had focus, return it to the input. */
  function clearAnswerActions() {
    const active = shadow.activeElement;
    answerActions.textContent = "";
    if (active && !active.isConnected) input.focus();
  }

  function hideAnswerCard() {
    hide(answerCard);
    input.focus();
  }

  function showAnswerError(message, question) {
    resetAnswerCard();
    answerProgress.textContent = "Couldn't generate an answer" + (message ? ` (${message})` : "") + ".";
    answerProgress.style.color = "var(--sa-error)";
    show(answerProgress);
    answerActions.appendChild(answerRetryBtn);
    answerActions.appendChild(agentConsentCancel);
    answerRetryBtn.onclick = () => askQuestion(question);
    agentConsentCancel.onclick = hideAnswerCard;
    show(answerCard);
    announce("Couldn't generate an answer.");
  }

  async function askQuestion(question) {
    if (!question) {
      input.focus();
      return;
    }
    if (!(await detectAgent())) return;
    if (agentRun && !agentRun.done) abortAgent("new question");
    resetAnswerCard();
    answerProgress.style.color = "";
    show(answerCard);

    if (saveData) {
      answerProgress.textContent = `Answers are off while Data Saver is on: the answer model is a ${lib.formatBytes(agentModel.sizeBytes)} download.`;
      show(answerProgress);
      agentConsentCancel.onclick = hideAnswerCard;
      answerActions.appendChild(agentConsentCancel);
      return;
    }

    if (!agentLoaded && !agentConsentGiven()) {
      const consented = await requestAgentConsent();
      if (!consented) return;
    }

    const run = { requestId: ++requestSeq, question, text: "", aborted: false, done: false, evidence: [], started: performance.now() };
    agentRun = run;
    answerProgress.textContent = "";
    show(answerProgress);
    answerActions.appendChild(stopBtn);
    try {
      await loadAgent(run);
      if (run.aborted) return;
      answerProgress.textContent = "Planning…";
      const plan = await agentCall("plan", { requestId: run.requestId, question, site: siteName });
      if (run.aborted) return;
      const queries = Array.from(new Set((plan.queries || []).concat([question])));
      answerProgress.textContent = "Searching: " + queries.join(" · ");
      const lists = await Promise.all(
        queries.map((q) => callWorker("search", { query: q, topK: 6, mode: "hybrid", evidence: true }).then((m) => m.results || []).catch(() => []))
      );
      if (run.aborted) return;
      const evidence = lib.mergeEvidence(lists, 6);
      await expandShortEvidence(evidence);
      if (run.aborted) return;
      // Confident FAQ entries (build-time QA lane) go first: they are short,
      // direct, and already carry a source page.
      const faqHits = await callWorker("qa", { query: question, k: 3 }).then((m) => m.hits || []).catch(() => []);
      if (run.aborted) return;
      const faqItems = lib.qaEvidence(faqHits, 2);
      run.evidence = faqItems.concat(evidence.map((r) => ({ title: r.title, url: r.url, text: r.text || r.snippet || "" })));
      answerProgress.textContent = "Answering…";
      await streamAnswer(run);
    } catch (err) {
      if (run.aborted) return;
      console.warn("eddie agent:", err);
      showAnswerError(err && err.message ? err.message : String(err), question);
    }
  }

  function requestAgentConsent() {
    return new Promise((resolve) => {
      const size = lib.formatBytes(agentModel.sizeBytes);
      answerProgress.textContent = config.consentText
        ? config.consentText.replace(/\{size\}/g, size).replace(/\{model\}/g, agentModel.base)
        : `Answers come from ${agentModel.base}, a language model that runs in your browser. Download it once (about ${size})? It stays in your browser's cache.`;
      show(answerProgress);
      clearAnswerActions();
      agentConsentAccept.textContent = `Download ${size} and answer`;
      agentConsentAccept.onclick = () => {
        rememberAgentConsent();
        clearAnswerActions();
        resolve(true);
      };
      agentConsentCancel.onclick = () => {
        hideAnswerCard();
        resolve(false);
      };
      answerActions.appendChild(agentConsentAccept);
      answerActions.appendChild(agentConsentCancel);
      announce(answerProgress.textContent);
      agentConsentAccept.focus();
    });
  }

  /** The agent transport: the service worker when it has WebGPU, else a page-side module worker. */
  function ensureAgentTransport() {
    if (agent) return Promise.resolve(agent);
    if (agentCreating) return agentCreating;
    agentCreating = (async () => {
      const plan = await withDeadline(ensureTransportPlan(), TRANSPORT_WAIT_MS);
      if (agent) return agent;
      let t = null;
      if (plan && plan.kind === "sw" && plan.hello && plan.hello.gpu && !forcePageWorkers) {
        const sw = new lib.ServiceWorkerTransport(plan.registration, { kind: "agent", version });
        try {
          await sw.connect();
          t = sw;
        } catch (err) {
          console.info("eddie: agent falls back to a page worker:", err && err.message ? err.message : err);
          sw.terminate();
        }
      }
      if (!t) t = new lib.DedicatedWorkerTransport(lib.assetUrl(baseUrl, "eddie-agent-worker.js", version), { type: "module" });
      attachAgent(t);
      return t;
    })().finally(() => {
      agentCreating = null;
    });
    return agentCreating;
  }

  function attachAgent(t) {
    agent = t;
    host.dataset.agentTransport = t.kind;
    for (const type of ["progress", "loaded", "plan_result", "token", "done", "aborted", "error"]) {
      t.on(type, onAgentMessage);
    }
    t.on("crash", (msg) => {
      const message = msg.message || "agent worker failed to load";
      if (agentPendingLoad) {
        agentPendingLoad.reject(new Error(message));
        agentPendingLoad = null;
      }
      if (agentRun && !agentRun.done) {
        agentRun.done = true;
        showAnswerError(message, agentRun.question);
      }
    });
    t.on("reset", () => {
      // The service worker restarted: its model is gone; the next Ask loads again.
      console.info("eddie: agent engine restarted");
      agentLoaded = false;
      if (agentPendingLoad) {
        agentPendingLoad.reject(new Error("the answer engine restarted"));
        agentPendingLoad = null;
      }
      if (agentRun && !agentRun.done) {
        agentRun.done = true;
        if (agentRun.rejectStream) agentRun.rejectStream(new Error("the answer engine restarted"));
      }
    });
  }

  function loadAgent(run) {
    if (agentLoaded) return Promise.resolve();
    if (agentLoading) return agentLoading;
    agentLoading = (async () => {
      const t = await ensureAgentTransport();
      if (run.aborted) return;
      if (t.kind === "sw") {
        // The service worker may still hold the model from an earlier page
        // (or may have been stopped meanwhile: reconnect first).
        try {
          if (!(await t.ensureAlive(lib.IDLE_PING_TIMEOUT_MS))) throw new Error("the answer engine is unreachable");
          const st = await t.state();
          if (lib.canReuseAgent(st.agent, agentModel.id)) {
            agentLoaded = true;
            host.dataset.agentReused = "true";
            console.info("eddie agent: reusing " + agentModel.id + " from the service worker");
            return;
          }
        } catch (err) {
          console.warn("eddie agent: state query failed", err);
        }
      }
      host.dataset.agentReused = "false";
      await new Promise((resolve, reject) => {
        agentPendingLoad = { resolve, reject };
        answerProgress.textContent = `Loading ${agentModel.base}…`;
        announce(`Loading the answer model, ${agentModel.base}`);
        t.postRaw({ type: "load", model: agentModel.id });
      });
    })().finally(() => {
      agentLoading = null;
    });
    return agentLoading;
  }

  /** A plan request; a "model not loaded" reply (service worker restarted) reloads once and retries. */
  async function agentCall(type, payload) {
    try {
      return await agent.call(type, payload, { requestId: payload.requestId });
    } catch (err) {
      if (!err || !lib.isModelNotLoadedMessage(err.message)) throw err;
      agentLoaded = false;
      await loadAgent({ aborted: false });
      return agent.call(type, payload, { requestId: ++requestSeq });
    }
  }

  function streamAnswer(run) {
    return new Promise((resolve, reject) => {
      run.resolveStream = resolve;
      run.rejectStream = reject;
      run.text = "";
      run.lastPaint = 0;
      answerText.textContent = "";
      show(answerText);
      const caret = el("span", "sa-caret");
      caret.setAttribute("aria-hidden", "true");
      run.caret = caret;
      answerText.appendChild(caret);
      updateKeepalive();
      agent.postRaw({ type: "ask", requestId: run.requestId, question: run.question, site: siteName, evidence: run.evidence });
    });
  }

  function paintStream(run, force) {
    const now = performance.now();
    if (!force && reducedMotion && now - run.lastPaint < 600) return;
    run.lastPaint = now;
    answerText.textContent = lib.visibleStreamText(run.text);
    if (!run.done && run.caret) answerText.appendChild(run.caret);
  }

  function onAgentMessage(msg) {
    if (msg.type !== "token" && msg.type !== "progress") {
      console.debug("eddie agent event " + msg.type + " " + (msg.requestId == null ? "" : msg.requestId) + (msg.message ? " " + msg.message : ""));
    }
    switch (msg.type) {
      case "progress":
        if (agentRun && !agentRun.done) {
          const pct = typeof msg.progress === "number" ? Math.round(msg.progress * 100) : null;
          answerProgress.textContent = `Loading ${agentModel.base}…` + (pct != null ? ` ${pct}%` : "");
          if (pct != null && pct % 25 === 0) announce(`Loading the answer model, ${pct}%`);
        }
        break;
      case "loaded":
        agentLoaded = true;
        if (agentPendingLoad) {
          agentPendingLoad.resolve(msg);
          agentPendingLoad = null;
        }
        break;
      case "plan_result":
        break; // settled by the transport's call()
      case "token":
        if (agentRun && msg.requestId === agentRun.requestId && !agentRun.done) {
          agentRun.text += msg.text;
          paintStream(agentRun, false);
        }
        break;
      case "done":
        if (agentRun && msg.requestId === agentRun.requestId) finishAnswer(agentRun, msg);
        break;
      case "aborted":
        if (agentRun && msg.requestId === agentRun.requestId) {
          agentRun.done = true;
          if (agentRun.resolveStream) agentRun.resolveStream();
        }
        break;
      case "error":
        if (agentPendingLoad && msg.requestId == null) {
          agentPendingLoad.reject(new Error(msg.message || "model load failed"));
          agentPendingLoad = null;
        } else if (agentRun && !agentRun.done && (msg.requestId == null || msg.requestId === agentRun.requestId)) {
          agentRun.done = true;
          if (agentRun.rejectStream) agentRun.rejectStream(new Error(msg.message || "answer failed"));
        }
        break;
      default:
        break;
    }
  }

  function finishAnswer(run, msg) {
    run.done = true;
    clearAnswerActions();
    hide(answerProgress);
    answerText.textContent = "";
    if (msg.nohit) {
      answerText.classList.add("sa-nohit");
      answerText.textContent = msg.answer || lib.NOHIT;
      hide(answerSources);
    } else {
      renderAnswerText(msg.answer, msg.citations);
      answerSources.textContent = "";
      (msg.citations || []).forEach((c) => {
        const li = el("li");
        li.appendChild(el("span", "sa-cite-n", `[${c.n}]`));
        const a = el("a", "sa-cite", c.title || c.url);
        a.href = c.url;
        a.addEventListener("click", (e) => {
          e.preventDefault();
          navigateTo(c.url);
        });
        li.appendChild(a);
        answerSources.appendChild(li);
      });
      if (msg.citations && msg.citations.length) show(answerSources);
      else hide(answerSources);
    }
    show(answerText);
    const usage = msg.usage || {};
    run.usage = usage;
    announce(msg.nohit ? "No answer: the site doesn't cover that." : "Answer ready. " + msg.answer);
    if (run.resolveStream) run.resolveStream();
    updateKeepalive();
    console.info("eddie agent " + JSON.stringify({
      question: run.question,
      model: agentModel ? agentModel.id : null,
      evidence: run.evidence.length,
      citations: (msg.citations || []).length,
      nohit: !!msg.nohit,
      ttftMs: usage.ttftMs,
      totalMs: usage.totalMs,
      tps: usage.tps,
      sinceAskMs: Math.round(performance.now() - run.started),
      agentReused: host.dataset.agentReused === "true",
      transport: agent ? agent.kind : null,
    }));
    host.dataset.agentDoneMs = String(Math.round(performance.now()));
  }

  function renderAnswerText(text, citations) {
    const byN = new Map((citations || []).map((c) => [c.n, c]));
    const parts = String(text || "").split(/(\[\d+\])/g);
    parts.forEach((part) => {
      const m = /^\[(\d+)\]$/.exec(part);
      const c = m ? byN.get(Number(m[1])) : null;
      if (c) {
        const a = el("a", "sa-answer-cite", part);
        a.href = c.url;
        a.setAttribute("aria-label", `Source ${c.n}: ${c.title || c.url}`);
        a.addEventListener("click", (e) => {
          e.preventDefault();
          navigateTo(c.url);
        });
        answerText.appendChild(a);
      } else if (part) {
        answerText.appendChild(document.createTextNode(part));
      }
    });
  }

  function abortAgent(reason) {
    const run = agentRun;
    if (!run || run.done) return;
    console.debug("eddie agent abort " + reason + " " + run.requestId);
    run.aborted = true;
    run.done = true;
    if (agent) agent.postRaw({ type: "abort", requestId: run.requestId });
    if (run.resolveStream) run.resolveStream();
    updateKeepalive();
    if (reason === "stopped") {
      paintStream(run, true);
      clearAnswerActions();
      answerProgress.textContent = "Stopped.";
      show(answerProgress);
    } else {
      hide(answerCard);
    }
  }

  async function expandShortEvidence(items) {
    for (const item of items) {
      if ((item.text || "").length >= 200 || !item.url) continue;
      try {
        const res = await callWorker("page", { url: item.url });
        const chunks = (res.page && res.page.chunks) || [];
        const i = chunks.findIndex((c) => c.id === item.chunk);
        if (i < 0) continue;
        const parts = [];
        for (let j = Math.max(0, i - 1); j <= Math.min(chunks.length - 1, i + 1); j++) {
          if (chunks[j].text) parts.push(chunks[j].text);
        }
        item.text = parts.join(" ");
      } catch (_) {
        // keep the snippet
      }
    }
  }

  // -- Modal open/close --
  function openModal() {
    isOpen = true;
    rotateHeartSprite();
    backdrop.classList.add("sa-open");
    trigger.style.display = "none";
    input.value = "";
    liveRegion.textContent = "";
    clearResults();
    hide(answerCard);
    ensureWorker();
    detectAgent();
    updateKeepalive();
    requestAnimationFrame(() => input.focus());
  }

  function closeModal() {
    if (!isOpen) return;
    isOpen = false;
    clearTimeout(searchTimer);
    abortAgent("closed");
    backdrop.classList.remove("sa-open");
    trigger.style.display = "";
    trigger.focus();
    updateKeepalive();
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

  function rotateHeartSprite() {
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
        const color = HEART_PALETTE[row[x]];
        if (!color) continue;
        ctx.fillStyle = color;
        ctx.fillRect(x, y, 1, 1);
      }
    }
  }

  // -- Status helpers --
  function show(node) {
    node.classList.add("sa-visible");
  }

  function hide(node) {
    node.classList.remove("sa-visible");
  }

  function showStatus(visible) {
    status.classList.toggle("sa-visible", visible);
  }

  function setStatus(text, progress) {
    statusText.textContent = text;
    showStatus(true);
    if (progress == null) {
      progressBar.classList.add("sa-progress-indeterminate");
      progressBar.removeAttribute("aria-valuenow");
      progressBar.setAttribute("aria-busy", "true");
      progressFill.style.width = "";
    } else {
      const pct = Math.max(0, Math.min(100, Math.round(progress * 100)));
      progressBar.classList.remove("sa-progress-indeterminate");
      progressBar.setAttribute("aria-valuenow", String(pct));
      progressBar.removeAttribute("aria-busy");
      progressFill.style.width = pct + "%";
    }
  }

  let announceToggle = false;
  function announce(text) {
    announceToggle = !announceToggle;
    liveRegion.textContent = text + (announceToggle ? "" : "​");
  }

  // -- Global keyboard shortcut: Ctrl+K or Cmd+K --
  function isEditable(node) {
    if (!node || node.nodeType !== 1) return false;
    if (node.isContentEditable) return true;
    const tag = node.tagName;
    return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";
  }

  document.addEventListener("keydown", (e) => {
    if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
    if (typeof e.key !== "string" || e.key.toLowerCase() !== "k") return;
    if (isOpen) {
      e.preventDefault();
      closeModal();
      return;
    }
    const target = e.composedPath ? e.composedPath()[0] : e.target;
    if (isEditable(target)) return;
    e.preventDefault();
    openModal();
  });

  // A page going away tells the service worker to drop its ports and stop
  // any answer it is still generating for us.
  window.addEventListener("pagehide", () => {
    if (agentRun && !agentRun.done && agent) agent.postRaw({ type: "abort", requestId: agentRun.requestId });
    for (const t of [search, agent]) {
      if (!t || t.kind !== "sw") continue;
      try {
        t.postRaw({ type: "disconnect" });
      } catch (_) {
        // port already gone
      }
    }
  });

  // -- Mount --
  document.body.appendChild(host);
  host.dataset.persist = config.persist;
  host.dataset.warm = config.warm;
  afterLoad(() =>
    onIdle(() => {
      ensureTransportPlan().then(() => warmUp()).catch((err) => console.warn("eddie warm-up failed", err));
    })
  );
})();
