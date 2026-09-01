// SPDX-License-Identifier: GPL-3.0-only

// Eddie boot loader: the script a page carries by default. It draws the
// trigger button (same data-* attributes, placement and theme as the full
// widget) and installs the Ctrl/Cmd+K shortcut; the full widget
// (eddie-widget.js, next to this file, same ?v=) is fetched on the first
// interaction with the trigger, on the shortcut, on `window.eddie.open()`,
// or after load when lib/boot.js decides the visitor is returning (the full
// widget then runs its own warm-up). A first-time visitor pays for this
// file only.
//
// widget/build.sh concatenates widget/src/lib/boot.js ahead of this file
// inside one IIFE (`EddieLib`).

"use strict";

(function () {
  const scriptEl = document.currentScript;
  if (!scriptEl || document.getElementById("eddie-host") || document.getElementById("eddie-boot")) return;
  const lib = EddieLib;
  const get = (name) => scriptEl.getAttribute(name);
  const layout = lib.bootLayout(get);
  const scriptHref = new URL(scriptEl.src, location.href).href;
  const widgetUrl = lib.widgetScriptUrl(scriptHref, get("data-index-url") || "");

  const host = document.createElement("div");
  host.id = "eddie-boot";
  host.dataset.theme = layout.theme;
  const shadow = host.attachShadow({ mode: "closed" });
  const style = document.createElement("style");
  style.textContent =
    ":host{all:initial;position:fixed;z-index:999999;--b:#fff;--t:#1a1a1a;--l:#e0e0e0;--a:#2563eb}" +
    ":host([data-theme=dark]){--b:#1a1a1a;--t:#e8e8e8;--l:#333;--a:#60a5fa}" +
    "@media(prefers-color-scheme:dark){:host([data-theme=auto]){--b:#1a1a1a;--t:#e8e8e8;--l:#333;--a:#60a5fa}}" +
    "button{position:fixed;width:48px;height:48px;border-radius:50%;border:1px solid var(--l);background:var(--b);color:var(--t);cursor:pointer;display:flex;align-items:center;justify-content:center;padding:0;box-shadow:0 2px 12px rgba(0,0,0,.1);transition:transform .15s ease,box-shadow .15s ease}" +
    "button:hover{transform:scale(1.06);box-shadow:0 4px 16px rgba(0,0,0,.15)}button:active{transform:scale(.96)}" +
    "button:focus-visible{outline:2px solid var(--a);outline-offset:2px}button[aria-busy=true]{opacity:.6}" +
    "svg{width:20px;height:20px;stroke:currentColor;fill:none;stroke-width:2;stroke-linecap:round;stroke-linejoin:round}" +
    "@media(prefers-reduced-motion:reduce){button{transition:none}}";
  shadow.appendChild(style);

  const trigger = document.createElement("button");
  trigger.type = "button";
  trigger.setAttribute("aria-label", "Search");
  const v = `${24 + layout.offsetY}px`;
  const h = `${24 + layout.offsetX}px`;
  trigger.style[layout.position.startsWith("top") ? "top" : "bottom"] = v;
  trigger.style[layout.position.endsWith("left") ? "left" : "right"] = h;
  trigger.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>';
  shadow.appendChild(trigger);

  let loading = null; // the injected <script>, once requested
  const handoff = {
    open: false, // the visitor asked to open before the widget arrived
    load,
    dispose() {
      if (host.parentNode) host.parentNode.removeChild(host);
      document.removeEventListener("keydown", onKey);
      if (window.__eddieBoot === handoff) delete window.__eddieBoot;
    },
  };
  window.__eddieBoot = handoff;

  /** Fetch the full widget once; it reads this script's data-* attributes from its own tag. */
  function load(reason) {
    if (loading) return;
    const s = document.createElement("script");
    for (const attr of Array.from(scriptEl.attributes)) {
      if (attr.name.startsWith("data-")) s.setAttribute(attr.name, attr.value);
    }
    s.setAttribute("data-boot", reason || "");
    s.src = widgetUrl;
    s.async = true;
    s.onerror = () => {
      loading = null; // the next interaction retries
      trigger.removeAttribute("aria-busy");
    };
    loading = s;
    document.head.appendChild(s);
  }

  function open() {
    handoff.open = true;
    trigger.setAttribute("aria-busy", "true");
    load("open");
  }

  function isEditable(node) {
    if (!node || node.nodeType !== 1) return false;
    if (node.isContentEditable) return true;
    const tag = node.tagName;
    return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";
  }

  function onKey(e) {
    if (!(e.ctrlKey || e.metaKey) || e.altKey || typeof e.key !== "string" || e.key.toLowerCase() !== "k") return;
    if (isEditable(e.composedPath ? e.composedPath()[0] : e.target)) return;
    e.preventDefault();
    open();
  }

  trigger.addEventListener("pointerover", () => load("hover"), { once: true });
  trigger.addEventListener("focusin", () => load("focus"), { once: true });
  trigger.addEventListener("click", open);
  document.addEventListener("keydown", onKey);
  // `window.eddie.open()` works before and after the full widget arrives.
  window.eddie = Object.assign(window.eddie || {}, { open });
  document.body.appendChild(host);

  let saveData = false;
  let reducedData = false;
  try {
    saveData = !!(navigator.connection && navigator.connection.saveData);
    reducedData = !!(window.matchMedia && window.matchMedia("(prefers-reduced-data: reduce)").matches);
  } catch (_) {
    // no connection info: treat as a normal connection
  }
  let used = false;
  let consented = false;
  try {
    used = !!localStorage.getItem(lib.SEARCH_USED_KEY);
    consented = !!localStorage.getItem(lib.SEARCH_CONSENT_KEY);
  } catch (_) {
    // storage unavailable: first-visit rules
  }
  const decision = lib.decideBoot({ warm: layout.warm, saveData, reducedData, used, consented });
  host.dataset.boot = decision.action;
  if (decision.action === "idle") {
    const go = () => {
      if (typeof requestIdleCallback === "function") requestIdleCallback(() => load("idle"), { timeout: 3000 });
      else setTimeout(() => load("idle"), 500);
    };
    if (document.readyState === "complete") go();
    else window.addEventListener("load", go, { once: true });
  }
})();
