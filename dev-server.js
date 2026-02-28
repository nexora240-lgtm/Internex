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
      res.writeHead(404);
      res.end("Not found");
      return;
    }
    const ext = path.extname(full);
    res.writeHead(200, { "Content-Type": MIME[ext] || "application/octet-stream" });
    res.end(data);
  });
}

function proxyRequest(req, res) {
  const parsed  = url.parse(req.url, true);
  const target  = parsed.query.url;

  if (!target) {
    res.writeHead(400);
    res.end("Missing ?url= parameter");
    return;
  }

  let targetUrl;
  try {
    targetUrl = new URL(target);
  } catch {
    res.writeHead(400);
    res.end("Invalid URL");
    return;
  }

  if (targetUrl.protocol !== "http:" && targetUrl.protocol !== "https:") {
    res.writeHead(400);
    res.end("Only http/https URLs are supported");
    return;
  }

  const lib = targetUrl.protocol === "https:" ? https : http;

  const proxyReq = lib.request(target, {
    method: "GET",
    headers: {
      "User-Agent": req.headers["user-agent"] || "Internex/1.0",
      "Accept":     req.headers["accept"] || "*/*",
      "Accept-Language": req.headers["accept-language"] || "en-US,en;q=0.9",
    },
  }, (upRes) => {
    // Strip security headers that block iframe embedding.
    const headers = { ...upRes.headers };
    delete headers["content-security-policy"];
    delete headers["content-security-policy-report-only"];
    delete headers["x-frame-options"];
    delete headers["cross-origin-opener-policy"];
    delete headers["cross-origin-embedder-policy"];
    delete headers["cross-origin-resource-policy"];

    // Rewrite redirect Location through the proxy.
    if (headers["location"]) {
      const abs = new URL(headers["location"], target).href;
      headers["location"] = "/proxy?url=" + encodeURIComponent(abs);
    }

    res.writeHead(upRes.statusCode, headers);
    upRes.pipe(res);
  });

  proxyReq.on("error", (err) => {
    console.error("Upstream error:", err.message);
    res.writeHead(502);
    res.end("Upstream fetch failed: " + err.message);
  });

  proxyReq.end();
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

server.listen(PORT, () => {
  console.log(`\n  Internex dev server running at:\n`);
  console.log(`    → http://localhost:${PORT}\n`);
});
