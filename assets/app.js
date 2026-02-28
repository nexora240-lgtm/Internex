(function () {
  "use strict";

  const form    = document.getElementById("url-form");
  const input   = document.getElementById("url-input");
  const frame   = document.getElementById("proxy-frame");

  let backend = "";

  // ── Load config ──
  async function loadConfig() {
    try {
      const res  = await fetch("config.json");
      const json = await res.json();
      backend = (json.backend || "").replace(/\/+$/, "");
    } catch {
      // Fallback: assume the backend is the same origin (self-hosted).
      backend = window.location.origin;
    }
  }

  // ── Normalise URL ──
  function normalise(raw) {
    let url = raw.trim();
    if (!url) return "";
    // Add scheme if missing.
    if (!/^https?:\/\//i.test(url)) {
      url = "https://" + url;
    }
    return url;
  }

  // ── Navigate ──
  function navigate(raw) {
    const url = normalise(raw);
    if (!url) return;
    input.value = url;
    frame.src = backend + "/proxy?url=" + encodeURIComponent(url);
  }

  // ── Events ──
  form.addEventListener("submit", function (e) {
    e.preventDefault();
    navigate(input.value);
  });

  // Update the URL bar when the user navigates inside the iframe
  // (best-effort — blocked by cross-origin restrictions on most sites).
  frame.addEventListener("load", function () {
    try {
      const inner = frame.contentWindow.location.href;
      if (inner && inner !== "about:blank") {
        const params = new URL(inner).searchParams;
        const target = params.get("url");
        if (target) input.value = target;
      }
    } catch {
      // Cross-origin — ignore.
    }
  });

  // ── Boot ──
  loadConfig().then(function () {
    // If the page was opened with ?url=… pre-fill and navigate.
    const params = new URLSearchParams(window.location.search);
    const initial = params.get("url");
    if (initial) navigate(initial);
  });
})();
