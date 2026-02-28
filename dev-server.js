// dev-server.js — lightweight dev proxy for testing the Internex frontend.
// Requires only Node.js (no npm install needed — uses built-in modules).
//
// Serves:
//   /              → assets/index.html
//   /app.js etc.   → assets/*
//   /proxy?url=X   → fetches X upstream and streams it back

const http  = require("http");
const https = require("https");
const fs    = require("fs");
const path  = require("path");
const url   = require("url");

const PORT = parseInt(process.env.PORT, 10) || 3000;
const ASSETS = path.join(__dirname, "assets");

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".css":  "text/css; charset=utf-8",
  ".js":   "application/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".png":  "image/png",
  ".svg":  "image/svg+xml",
  ".ico":  "image/x-icon",
};

function decodeProxyUrl(raw) {
  if (!raw) return "";
  const idx = raw.indexOf("/proxy?url=");
  if (idx === -1) return "";
  let encoded = raw.slice(idx + 11);
  const amp = encoded.indexOf("&");
  if (amp !== -1) encoded = encoded.slice(0, amp);
  const hash = encoded.indexOf("#");
  if (hash !== -1) encoded = encoded.slice(0, hash);
  try { return decodeURIComponent(encoded); } catch { return ""; }
}

function serveStatic(req, res) {
  let filePath = req.url.split("?")[0];
  if (filePath === "/") filePath = "/index.html";
  const full = path.join(ASSETS, filePath);

  // Prevent directory traversal.
  if (!full.startsWith(ASSETS)) {
    res.writeHead(403);
    res.end("Forbidden");
    return;
  }

  fs.readFile(full, (err, data) => {
    if (err) {
      if (filePath === "/favicon.ico") {
        res.writeHead(204);
        res.end();
        return;
      }
      res.writeHead(404);
      res.end("Not found");
      return;
    }
    const ext = path.extname(full);
    res.writeHead(200, { "Content-Type": MIME[ext] || "application/octet-stream" });
    res.end(data);
  });
}

// ── Runtime script tag to inject ──
const RUNTIME_TAG = `<script>window.__internex_base = %BASE_JSON%;</script>\n<script src="/internex.runtime.js"></script>\n`;

const PROXY_HOST = `localhost:${PORT}`;

function resolveProxyUrl(rawUrl, baseUrl) {
  if (!rawUrl) return rawUrl;
  const s = String(rawUrl).trim();
  if (!s || s.startsWith("data:") || s.startsWith("blob:") || s.startsWith("javascript:") || s.startsWith("mailto:") || s.startsWith("tel:") || s.startsWith("#")) {
    return s;
  }
  // Do not rewrite our injected runtime or proxy-local assets.
  if (s === "/internex.runtime.js" || s.startsWith("/internex.runtime.js?")) return s;
  if (s.includes("/proxy?url=")) return s;
  if (s.startsWith("//")) {
    const scheme = baseUrl ? new URL(baseUrl).protocol : "https:";
    return "/proxy?url=" + encodeURIComponent(scheme + s);
  }
  if (/^(https?|wss?):\/\//i.test(s)) {
    // Remap proxy-origin URLs (any protocol) to upstream origin.
    try {
      const abs = new URL(s);
      if (abs.host.toLowerCase() === PROXY_HOST && baseUrl) {
        const base = new URL(baseUrl);
        const remapped = new URL(abs.pathname + abs.search + abs.hash, base.origin);
        return "/proxy?url=" + encodeURIComponent(remapped.href);
      }
    } catch { /* ignore */ }
    return "/proxy?url=" + encodeURIComponent(s);
  }
  if (s.startsWith("/")) {
    const origin = baseUrl ? new URL(baseUrl).origin : "";
    return origin ? "/proxy?url=" + encodeURIComponent(origin + s) : s;
  }
  if (baseUrl) {
    try { return "/proxy?url=" + encodeURIComponent(new URL(s, baseUrl).href); }
    catch { /* ignore */ }
  }
  return s;
}

// Lightweight HTML URL rewriter (rewrites src/href/action/poster/etc.).
function rewriteHtmlUrls(html, baseUrl) {
  return html.replace(
    /((?:src|href|action|poster|formaction|background|data|cite)\s*=\s*["'])([^"']*)(["'])/gi,
    (match, pre, rawUrl, post) => {
      const rewritten = resolveProxyUrl(rawUrl, baseUrl);
      return pre + rewritten + post;
    }
  );
}

// Lightweight CSS URL rewriter.
function rewriteCssUrls(css, baseUrl) {
  return css.replace(
    /url\(\s*["']?([^"')]+)["']?\s*\)/gi,
    (match, rawUrl) => {
      const rewritten = resolveProxyUrl(rawUrl, baseUrl);
      if (!rewritten || rewritten === rawUrl) return match;
      return "url(\"" + rewritten + "\")";
    }
  );
}

// Detect content type category from headers.
function contentCategory(headers) {
  const ct = (headers["content-type"] || "").toLowerCase();
  if (ct.includes("text/html") || ct.includes("application/xhtml")) return "html";
  if (ct.includes("text/css")) return "css";
  if (ct.includes("javascript") || ct.includes("ecmascript")) return "js";
  return "other";
}

function proxyRequest(req, res) {
  let target = decodeProxyUrl(req.url || "");

  if (!target) {
    res.writeHead(400);
    res.end("Missing ?url= parameter");
    return;
  }

  // Safety: if the target points back at our own proxy (any protocol),
  // remap to the Referer-derived upstream origin so we don't loop.
  try {
    const t = new URL(target);
    if (t.host.toLowerCase() === PROXY_HOST) {
      // Try to recover the upstream origin from the Referer header.
      const ref = req.headers["referer"] || "";
      const upBase = decodeProxyUrl(ref);
      if (upBase) {
        const upOrigin = new URL(upBase).origin;
        target = upOrigin + t.pathname + t.search + t.hash;
      }
    }
  } catch { /* ignore */ }

  const normalizedTarget = encodeURI(target);

  let targetUrl;
  try {
    targetUrl = new URL(normalizedTarget);
  } catch {
    try {
      // Attempt to salvage URLs containing unescaped characters.
      targetUrl = new URL(encodeURI(target));
    } catch {
      res.writeHead(400);
      res.end("Invalid URL");
      return;
    }
  }

  if (targetUrl.protocol !== "http:" && targetUrl.protocol !== "https:") {
    res.writeHead(400);
    res.end("Only http/https URLs are supported");
    return;
  }

  const lib = targetUrl.protocol === "https:" ? https : http;

  const headers = { ...req.headers };
  headers["user-agent"] = headers["user-agent"] || "Internex/1.0";
  headers["accept"] = headers["accept"] || "*/*";
  headers["accept-language"] = headers["accept-language"] || "en-US,en;q=0.9";
  headers["accept-encoding"] = "identity"; // avoid compressed responses for rewriting
  headers["host"] = targetUrl.host;
  headers["origin"] = targetUrl.origin;

  // Prefer decoding the proxy referer into the upstream referer.
  const ref = headers["referer"] || "";
  const decodedRef = decodeProxyUrl(ref);
  headers["referer"] = decodedRef || target;

  const proxyReq = lib.request(normalizedTarget, {
    method: req.method,
    headers,
  }, (upRes) => {
    // Follow redirects manually (up to 5 hops).
    if ([301, 302, 303, 307, 308].includes(upRes.statusCode) && upRes.headers["location"]) {
      const abs = new URL(upRes.headers["location"], target).href;
      const headers = { ...stripSecHeaders(upRes.headers) };
      headers["location"] = "/proxy?url=" + encodeURIComponent(abs);
      res.writeHead(upRes.statusCode, headers);
      res.end();
      return;
    }

    const cat = contentCategory(upRes.headers);
    const headers = { ...stripSecHeaders(upRes.headers) };

    if (cat === "html") {
      // Buffer the HTML so we can inject the runtime and rewrite URLs.
      delete headers["content-length"];  // we'll change it
      delete headers["content-encoding"];
      headers["content-type"] = "text/html; charset=utf-8";

      const chunks = [];
      upRes.on("data", (c) => chunks.push(c));
      upRes.on("end", () => {
        let html = Buffer.concat(chunks).toString("utf-8");

        // Inject runtime script as early as possible in <head>.
        const runtimeSnippet = RUNTIME_TAG.replace("%BASE_JSON%", JSON.stringify(target));
        const headIdx = html.indexOf("</head>");
        const htmlIdx = html.indexOf("<head");
        if (htmlIdx !== -1 && headIdx !== -1) {
          // Insert right after <head…>
          const afterHead = html.indexOf(">", htmlIdx) + 1;
          html = html.slice(0, afterHead) + "\n" + runtimeSnippet + html.slice(afterHead);
        } else if (html.indexOf("<html") !== -1) {
          const afterHtml = html.indexOf(">", html.indexOf("<html")) + 1;
          html = html.slice(0, afterHtml) + "\n<head>" + runtimeSnippet + "</head>\n" + html.slice(afterHtml);
        } else {
          html = runtimeSnippet + html;
        }

        // Rewrite absolute URLs in HTML attributes.
        html = rewriteHtmlUrls(html, target);

        res.writeHead(upRes.statusCode, headers);
        res.end(html);
      });
    } else if (cat === "css") {
      delete headers["content-length"];
      delete headers["content-encoding"];
      const chunks = [];
      upRes.on("data", (c) => chunks.push(c));
      upRes.on("end", () => {
        let css = Buffer.concat(chunks).toString("utf-8");
        css = rewriteCssUrls(css, target);
        res.writeHead(upRes.statusCode, headers);
        res.end(css);
      });
    } else {
      // Non-rewritable: stream through directly.
      res.writeHead(upRes.statusCode, headers);
      upRes.pipe(res);
    }
  });

  proxyReq.on("error", (err) => {
    console.error("Upstream error:", err.message);
    res.writeHead(502);
    res.end("Upstream fetch failed: " + err.message);
  });

  req.pipe(proxyReq);
}

// Strip headers that block iframe embedding or proxy operation.
function stripSecHeaders(h) {
  const out = { ...h };
  delete out["content-security-policy"];
  delete out["content-security-policy-report-only"];
  delete out["x-frame-options"];
  delete out["cross-origin-opener-policy"];
  delete out["cross-origin-embedder-policy"];
  delete out["cross-origin-resource-policy"];
  delete out["strict-transport-security"];
  delete out["permissions-policy"];
  delete out["referrer-policy"];
  return out;
}

const server = http.createServer((req, res) => {
  // CORS — allow the frontend to talk to us from any origin.
  res.setHeader("Access-Control-Allow-Origin", "*");
  res.setHeader("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
  res.setHeader("Access-Control-Allow-Headers", "*");
  if (req.method === "OPTIONS") {
    res.writeHead(204);
    res.end();
    return;
  }

  if (req.url.startsWith("/proxy")) {
    proxyRequest(req, res);
  } else {
    serveStatic(req, res);
  }
});

// ── WebSocket upgrade handler ──
server.on("upgrade", (req, socket, head) => {
  const target = decodeProxyUrl(req.url || "");
  if (!target) {
    socket.destroy();
    return;
  }

  let targetUrl;
  try {
    // Normalise ws(s):// into the URL object.
    targetUrl = new URL(target);
  } catch {
    socket.destroy();
    return;
  }

  // ws/wss → use the appropriate net module.
  const useTls = targetUrl.protocol === "wss:" || targetUrl.protocol === "https:";
  const net = require("net");
  const tls = require("tls");
  const defaultPort = useTls ? 443 : 80;
  const port = parseInt(targetUrl.port, 10) || defaultPort;

  // Build the raw HTTP upgrade request to send upstream.
  let path = targetUrl.pathname + targetUrl.search;
  let rawRequest = `GET ${path} HTTP/1.1\r\n`;
  rawRequest += `Host: ${targetUrl.host}\r\n`;

  // Forward WebSocket headers.
  const wsHeaders = [
    "upgrade", "connection", "sec-websocket-key",
    "sec-websocket-version", "sec-websocket-extensions",
    "sec-websocket-protocol", "origin", "user-agent",
  ];
  for (const h of wsHeaders) {
    if (req.headers[h]) {
      const name = h.replace(/(^|-)(\w)/g, (_, p, c) => (p ? "-" : "") + c.toUpperCase());
      let val = req.headers[h];
      // Rewrite Origin to match upstream.
      if (h === "origin") val = targetUrl.origin;
      rawRequest += `${name}: ${val}\r\n`;
    }
  }
  rawRequest += "\r\n";

  const connectOpts = { host: targetUrl.hostname, port, servername: targetUrl.hostname };

  const upSocket = useTls ? tls.connect(connectOpts) : net.connect(connectOpts);

  upSocket.on("connect", () => {
    upSocket.write(rawRequest);
    if (head && head.length) upSocket.write(head);

    // Pipe the 101 response and all subsequent frames back to the browser.
    upSocket.pipe(socket);
    socket.pipe(upSocket);
  });

  upSocket.on("error", (err) => {
    console.error("WebSocket upstream error:", err.message);
    socket.destroy();
  });

  socket.on("error", () => upSocket.destroy());
  socket.on("close", () => upSocket.destroy());
  upSocket.on("close", () => socket.destroy());
});

server.listen(PORT, () => {
  console.log(`\n  Internex dev server running at:\n`);
  console.log(`    → http://localhost:${PORT}\n`);
});
