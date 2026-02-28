/*
 * internex.runtime.js  –  Client-side runtime for the Internex proxy.
 * Injected into every proxied HTML page AFTER window.__internex_base is set.
 *
 * Design principles
 * ─────────────────
 *  1. Every native function / descriptor is saved BEFORE any patching.
 *  2. Overrides NEVER call themselves; they always call the saved original.
 *  3. A single URL-rewriting function (__internex_rewrite_url) is the only
 *     place that decides whether a URL needs rewriting.
 *  4. MutationObserver acts as a safety-net for anything that slips through.
 */
(function () {
  "use strict";

  /* ═══════════════════════════════════════════════════════════════════════
   * §0  CONFIGURATION
   * ═══════════════════════════════════════════════════════════════════════ */

  var PROXY_ORIGIN = location.origin;
  var BASE_URL     = window.__internex_base || "";
  var BASE_ORIGIN  = "";
  try { BASE_ORIGIN = new URL(BASE_URL).origin; } catch (_) { /* */ }

  function decodeBaseFromLocation() {
    try {
      var href = String(location.href || "");
      var i = href.indexOf("/proxy?url=");
      if (i === -1) return "";
      var encoded = href.slice(i + 11);
      // Trim off any extra params after url=…
      var amp = encoded.indexOf("&");
      if (amp !== -1) encoded = encoded.slice(0, amp);
      return decodeURIComponent(encoded);
    } catch (_) {
      return "";
    }
  }

  function setBase(url) {
    BASE_URL = url || "";
    try { BASE_ORIGIN = new URL(BASE_URL).origin; }
    catch (_) { BASE_ORIGIN = ""; }
    if (window.__internex) window.__internex.base = BASE_URL;
  }

  function getBaseURL() {
    if (BASE_URL) return BASE_URL;
    var decoded = decodeBaseFromLocation();
    if (decoded) setBase(decoded);
    return BASE_URL;
  }

  function getBaseOrigin() {
    if (BASE_ORIGIN) return BASE_ORIGIN;
    getBaseURL();
    return BASE_ORIGIN;
  }

  // Allow late-initialization (defensive – the injector SHOULD set it first)
  if (!BASE_URL) {
    Object.defineProperty(window, "__internex_base", {
      get: function () { return BASE_URL; },
      set: function (v) { setBase(v || ""); },
      configurable: true,
      enumerable: true,
    });
  }

  /* ═══════════════════════════════════════════════════════════════════════
   * §1  SAVE ALL ORIGINALS  (before any mutation)
   * ═══════════════════════════════════════════════════════════════════════ */

  // --- Functions -----------------------------------------------------------
  var _fetch            = window.fetch;
  var _xhrOpen          = XMLHttpRequest.prototype.open;
  var _WebSocket        = window.WebSocket;
  var _EventSource      = window.EventSource;
  var _sendBeacon       = navigator.sendBeacon
                            ? navigator.sendBeacon.bind(navigator) : null;
  var _Worker           = window.Worker;
  var _SharedWorker     = window.SharedWorker;
  var _Audio            = window.Audio;
  var _windowOpen       = window.open;
  var _pushState        = History.prototype.pushState;
  var _replaceState     = History.prototype.replaceState;
  var _locationAssign   = Location.prototype.assign;
  var _locationReplace  = Location.prototype.replace;
  var _postMessage      = window.postMessage;
  var _setAttribute     = Element.prototype.setAttribute;
  var _setAttributeNS   = Element.prototype.setAttributeNS;
  var _getAttribute     = Element.prototype.getAttribute;
  var _getAttributeNS   = Element.prototype.getAttributeNS;
  var _insertAdjacentHTML = Element.prototype.insertAdjacentHTML;
  var _docWrite         = document.write;
  var _docWriteln       = document.writeln;
  var _createObjectURL  = URL.createObjectURL;
  var _revokeObjectURL  = URL.revokeObjectURL;
  var _CSSSetProperty   = CSSStyleDeclaration.prototype.setProperty;
  var _CSSInsertRule    = CSSStyleSheet && CSSStyleSheet.prototype
                            ? CSSStyleSheet.prototype.insertRule : null;
  var _CSSReplace       = CSSStyleSheet && CSSStyleSheet.prototype
                            ? CSSStyleSheet.prototype.replace : null;
  var _CSSReplaceSync   = CSSStyleSheet && CSSStyleSheet.prototype
                            ? CSSStyleSheet.prototype.replaceSync : null;
  var _DOMParse         = DOMParser.prototype.parseFromString;
  var _createFrag       = Range.prototype.createContextualFragment;
  var _swRegister       = (navigator.serviceWorker &&
                           navigator.serviceWorker.register)
                            ? navigator.serviceWorker.register
                                .bind(navigator.serviceWorker) : null;

  // --- Property descriptors ------------------------------------------------
  function saveDesc(proto, prop) {
    try { return Object.getOwnPropertyDescriptor(proto, prop) || null; }
    catch (_) { return null; }
  }

  var _d_innerHTML  = saveDesc(Element.prototype,            "innerHTML");
  var _d_outerHTML  = saveDesc(Element.prototype,            "outerHTML");
  var _d_cssText    = saveDesc(CSSStyleDeclaration.prototype,"cssText");

  var _d_locHref      = saveDesc(Location.prototype, "href");
  var _d_locOrigin    = saveDesc(Location.prototype, "origin");
  var _d_locProtocol  = saveDesc(Location.prototype, "protocol");
  var _d_locHost      = saveDesc(Location.prototype, "host");
  var _d_locHostname  = saveDesc(Location.prototype, "hostname");
  var _d_locPort      = saveDesc(Location.prototype, "port");
  var _d_locPathname  = saveDesc(Location.prototype, "pathname");
  var _d_locSearch    = saveDesc(Location.prototype, "search");
  var _d_locHash      = saveDesc(Location.prototype, "hash");

  // --- Storage (CRITICAL: grab the real objects BEFORE any defineProperty) --
  var _realLocalStorage   = window.localStorage;
  var _realSessionStorage = window.sessionStorage;

  // ShadowRoot.innerHTML (Web Components)
  var _d_shadowInnerHTML = null;
  try { _d_shadowInnerHTML = saveDesc(ShadowRoot.prototype, "innerHTML"); }
  catch (_) { /* ShadowRoot may not exist */ }

  /* ═══════════════════════════════════════════════════════════════════════
   * §2  URL REWRITING CORE
   * ═══════════════════════════════════════════════════════════════════════ */

  function isProxied(u) {
    return typeof u === "string" && u.indexOf("/proxy?url=") !== -1;
  }

  /**
   * Rewrite a single URL so it passes through the proxy.
   *   - data:, blob:, mailto:, tel:, #fragment, about:blank  →  unchanged
   *   - javascript:  →  sanitised
   *   - already proxied  →  unchanged
   *   - absolute / protocol-relative / root-relative / relative  →  /proxy?url=…
   */
  function rewriteUrl(raw) {
    if (raw == null || typeof raw !== "string") return raw;
    var s = raw.trim();
    if (!s) return s;

    // Pass-through
    var c0 = s.charAt(0);
    if (c0 === "#" || s === "about:blank" ||
        s.lastIndexOf("data:",  0) === 0 ||
        s.lastIndexOf("blob:",  0) === 0 ||
        s.lastIndexOf("mailto:",0) === 0 ||
        s.lastIndexOf("tel:",   0) === 0) return s;

    // Sanitise javascript:
    if (/^\s*javascript\s*:/i.test(s)) return "javascript:void(0)";

    // Already proxied
    if (isProxied(s)) return s;

    // Protocol-relative  //host/path
    if (s.charAt(0) === "/" && s.charAt(1) === "/") {
      var baseScheme = "https:";
      var b = getBaseURL();
      if (b) {
        try { baseScheme = new URL(b).protocol; } catch (_) { /* */ }
      }
      return "/proxy?url=" + encodeURIComponent(baseScheme + s);
    }

    // Absolute  http(s)://… or ws(s)://…
    if (/^(https?|wss?):\/\//i.test(s)) {
      return "/proxy?url=" + encodeURIComponent(s);
    }

    // Root-relative  /path
    if (c0 === "/") {
      var origin = getBaseOrigin();
      if (origin) return "/proxy?url=" + encodeURIComponent(origin + s);
      return s;
    }

    // Relative  path/file
    var baseURL = getBaseURL();
    if (baseURL) {
      try { return "/proxy?url=" + encodeURIComponent(new URL(s, baseURL).href); }
      catch (_) { /* fall through */ }
    }

    return s;
  }

  function resolveAbsolute(raw) {
    if (raw == null || typeof raw !== "string") return "";
    var s = raw.trim();
    if (!s) return "";
    if (/^(https?|wss?):\/\//i.test(s)) return s;
    if (s.charAt(0) === "/" && s.charAt(1) === "/") {
      var baseScheme = "https:";
      var b = getBaseURL();
      if (b) {
        try { baseScheme = new URL(b).protocol; } catch (_) { /* */ }
      }
      return baseScheme + s;
    }
    if (s.charAt(0) === "/") {
      var origin = getBaseOrigin();
      return origin ? (origin + s) : s;
    }
    var baseURL = getBaseURL();
    if (baseURL) {
      try { return new URL(s, baseURL).href; } catch (_) { /* */ }
    }
    return s;
  }

  /** Decode a proxied URL back to the original. */
  function decodeUrl(proxied) {
    if (!proxied || typeof proxied !== "string") return proxied;
    var i = proxied.indexOf("/proxy?url=");
    if (i === -1) return proxied;
    try { return decodeURIComponent(proxied.slice(i + 11)); }
    catch (_) { return proxied; }
  }

  /** Rewrite a srcset attribute value. */
  function rewriteSrcset(val) {
    if (val == null) return val;
    if (typeof val !== "string") return val;
    if (!val) return val;
    return val.split(",").map(function (entry) {
      var parts = entry.trim().split(/\s+/);
      if (parts.length > 0) parts[0] = rewriteUrl(parts[0]);
      return parts.join(" ");
    }).join(", ");
  }

  // ---- Inline HTML URL rewriter (lightweight – for innerHTML etc.) --------
  var _htmlAttrRe  = /((?:src|href|action|poster|formaction|background|data|cite)\s*=\s*)(["'])((?:(?!\2).)*?)\2/gi;
  var _srcsetAttrRe = /(srcset\s*=\s*)(["'])((?:(?!\2).)*?)\2/gi;

  function rewriteHtml(html) {
    if (typeof html !== "string" || !html) return html;
    html = html.replace(_htmlAttrRe, function (_m, pre, q, val) {
      return pre + q + rewriteUrl(val) + q;
    });
    html = html.replace(_srcsetAttrRe, function (_m, pre, q, val) {
      return pre + q + rewriteSrcset(val) + q;
    });
    return html;
  }

  // ---- CSS url(…) rewriter ------------------------------------------------
  var _cssUrlRe = /url\(\s*["']?([^"')\s]+?)["']?\s*\)/gi;

  function rewriteCssValue(val) {
    if (typeof val !== "string" || val.indexOf("url(") === -1) return val;
    return val.replace(_cssUrlRe, function (match, u) {
      var t = u.trim();
      var r = rewriteUrl(t);
      return (r === t) ? match : 'url("' + r + '")';
    });
  }

  function rewriteCssText(css) {
    if (typeof css !== "string" || !css) return css;
    // url()
    css = css.replace(_cssUrlRe, function (match, u) {
      var t = u.trim();
      var r = rewriteUrl(t);
      return (r === t) ? match : 'url("' + r + '")';
    });
    // @import "…"
    css = css.replace(/@import\s+["']([^"']+)["']/gi, function (match, u) {
      if (isProxied(u)) return match;
      return '@import "' + rewriteUrl(u) + '"';
    });
    return css;
  }

  // ---- Expose globally ----------------------------------------------------
  window.__internex_rewrite_url = rewriteUrl;
  window.__internex_decode_url  = decodeUrl;
  window.__internex = {
    origin:     PROXY_ORIGIN,
    base:       getBaseURL(),
    encode:     rewriteUrl,
    decode:     decodeUrl,
    rewriteUrl: rewriteUrl,
    rewriteHtml:rewriteHtml,
    scope:      function (_ctx, fn) { return fn; },
  };

  /* ═══════════════════════════════════════════════════════════════════════
   * §3  NETWORK API PATCHES
   * ═══════════════════════════════════════════════════════════════════════ */

  // fetch
  if (_fetch) {
    window.fetch = function (input, init) {
      if (typeof input === "string") {
        input = rewriteUrl(input);
      } else if (input instanceof URL) {
        input = rewriteUrl(input.toString());
      } else if (input instanceof Request) {
        input = new Request(rewriteUrl(input.url), input);
      }
      return _fetch.call(window, input, init);
    };
  }

  // XMLHttpRequest
  XMLHttpRequest.prototype.open = function (method, url) {
    if (typeof url === "string") {
      arguments[1] = rewriteUrl(url);
    } else if (url && typeof url.toString === "function") {
      arguments[1] = rewriteUrl(url.toString());
    }
    return _xhrOpen.apply(this, arguments);
  };

  // WebSocket
  if (_WebSocket) {
    window.WebSocket = function (url, proto) {
      url = rewriteUrl(url);
      return proto !== undefined ? new _WebSocket(url, proto) : new _WebSocket(url);
    };
    window.WebSocket.prototype  = _WebSocket.prototype;
    window.WebSocket.CONNECTING = 0;
    window.WebSocket.OPEN       = 1;
    window.WebSocket.CLOSING    = 2;
    window.WebSocket.CLOSED     = 3;
  }

  // EventSource
  if (_EventSource) {
    window.EventSource = function (url, opts) {
      return new _EventSource(rewriteUrl(url), opts);
    };
    window.EventSource.prototype  = _EventSource.prototype;
    window.EventSource.CONNECTING = 0;
    window.EventSource.OPEN       = 1;
    window.EventSource.CLOSED     = 2;
  }

  // sendBeacon
  if (_sendBeacon) {
    navigator.sendBeacon = function (url, data) {
      return _sendBeacon(rewriteUrl(url), data);
    };
  }

  // Worker / SharedWorker
  if (_Worker) {
    window.Worker = function (url, opts) { return new _Worker(rewriteUrl(url), opts); };
    window.Worker.prototype = _Worker.prototype;
  }
  if (_SharedWorker) {
    window.SharedWorker = function (url, opts) { return new _SharedWorker(rewriteUrl(url), opts); };
    window.SharedWorker.prototype = _SharedWorker.prototype;
  }

  // Audio constructor (accepts optional src)
  if (_Audio) {
    window.Audio = function (src) {
      var a = new _Audio();          // create without src
      if (src !== undefined) a.src = src;  // setter will rewrite
      return a;
    };
    window.Audio.prototype = _Audio.prototype;
  }

  // window.open
  if (_windowOpen) {
    window.open = function (url, target, features) {
      return _windowOpen.call(window, rewriteUrl(url), target, features);
    };
  }

  /* ═══════════════════════════════════════════════════════════════════════
   * §4  DOM PROPERTY PATCHES  (src, href, action, poster, data, formAction)
   * ═══════════════════════════════════════════════════════════════════════ */

  /**
   * Safely override a URL property on an element prototype.
   * The original descriptor is captured in the closure — no recursion risk.
   */
  function patchProp(Ctor, prop) {
    try {
      if (!Ctor || !Ctor.prototype) return;
      var d = Object.getOwnPropertyDescriptor(Ctor.prototype, prop);
      if (!d || !d.set) return;
      Object.defineProperty(Ctor.prototype, prop, {
        get: d.get
          ? function () {
              var v = d.get.call(this);
              return typeof v === "string" ? decodeUrl(v) : v;
            }
          : undefined,
        set: function (v) { d.set.call(this, rewriteUrl(v)); },
        configurable: true,
        enumerable: true,
      });
    } catch (_) { /* immutable or non-configurable — skip */ }
  }

  function patchSrcsetProp(Ctor) {
    try {
      if (!Ctor || !Ctor.prototype) return;
      var d = Object.getOwnPropertyDescriptor(Ctor.prototype, "srcset");
      if (!d || !d.set) return;
      Object.defineProperty(Ctor.prototype, "srcset", {
        get: d.get ? function () { return d.get.call(this); } : undefined,
        set: function (v) { d.set.call(this, rewriteSrcset(v)); },
        configurable: true,
        enumerable: true,
      });
    } catch (_) { /* ignore */ }
  }

  // src
  var srcCtors = [HTMLImageElement, HTMLScriptElement, HTMLIFrameElement,
                  HTMLSourceElement, HTMLEmbedElement, HTMLInputElement];
  if (typeof HTMLMediaElement  !== "undefined") srcCtors.push(HTMLMediaElement);
  if (typeof HTMLTrackElement  !== "undefined") srcCtors.push(HTMLTrackElement);
  srcCtors.forEach(function (C) { patchProp(C, "src"); });

  // srcset
  [HTMLImageElement, HTMLSourceElement]
    .forEach(function (C) { patchSrcsetProp(C); });

  // href  (NOT HTMLBaseElement — that sets the document base URL)
  [HTMLAnchorElement, HTMLAreaElement, HTMLLinkElement]
    .forEach(function (C) { patchProp(C, "href"); });

  // action
  patchProp(HTMLFormElement, "action");

  // formAction
  [HTMLInputElement, HTMLButtonElement]
    .forEach(function (C) { patchProp(C, "formAction"); });

  // poster
  patchProp(HTMLVideoElement, "poster");

  // data
  patchProp(HTMLObjectElement, "data");

  /* ═══════════════════════════════════════════════════════════════════════
   * §5  DOM METHOD PATCHES
   * ═══════════════════════════════════════════════════════════════════════ */

  // --- setAttribute --------------------------------------------------------
  var URL_ATTR_SET = new Set([
    "src", "href", "action", "formaction", "poster", "data",
    "background", "cite", "manifest", "codebase", "classid",
    "longdesc", "usemap", "ping"
  ]);

  Element.prototype.setAttribute = function (name, value) {
    var lower = (name || "").toLowerCase();
    // Special: <base href> must NOT be proxied.
    if (lower === "href" && this.tagName === "BASE") {
      try {
        BASE_URL    = value;
        BASE_ORIGIN = new URL(value).origin;
      } catch (_) { /* */ }
      return _setAttribute.call(this, name, value);
    }
    if (URL_ATTR_SET.has(lower))     value = rewriteUrl(value);
    else if (lower === "srcset" || lower === "imagesrcset")     value = rewriteSrcset(value);
    else if (lower === "style")      value = rewriteCssValue(value);
    return _setAttribute.call(this, name, value);
  };

  if (_setAttributeNS) {
    Element.prototype.setAttributeNS = function (ns, name, value) {
      var lower = (name || "").toLowerCase();
      if (lower === "href" || lower === "xlink:href") {
        value = rewriteUrl(value);
      } else if (lower === "srcset" || lower === "imagesrcset") {
        value = rewriteSrcset(value);
      } else if (lower === "style") {
        value = rewriteCssValue(value);
      }
      return _setAttributeNS.call(this, ns, name, value);
    };
  }

  Element.prototype.getAttribute = function (name) {
    var lower = (name || "").toLowerCase();
    var v = _getAttribute.call(this, name);
    if (v == null) return v;
    if (URL_ATTR_SET.has(lower)) return decodeUrl(v);
    if (lower === "srcset" || lower === "imagesrcset") return rewriteSrcset(v);
    return v;
  };

  if (_getAttributeNS) {
    Element.prototype.getAttributeNS = function (ns, name) {
      var lower = (name || "").toLowerCase();
      var v = _getAttributeNS.call(this, ns, name);
      if (v == null) return v;
      if (lower === "href" || lower === "xlink:href") return decodeUrl(v);
      if (lower === "srcset" || lower === "imagesrcset") return rewriteSrcset(v);
      return v;
    };
  }

  // --- innerHTML -----------------------------------------------------------
  if (_d_innerHTML && _d_innerHTML.set) {
    Object.defineProperty(Element.prototype, "innerHTML", {
      get: _d_innerHTML.get,
      set: function (html) { _d_innerHTML.set.call(this, rewriteHtml(html)); },
      configurable: true, enumerable: true,
    });
  }

  // --- outerHTML -----------------------------------------------------------
  if (_d_outerHTML && _d_outerHTML.set) {
    Object.defineProperty(Element.prototype, "outerHTML", {
      get: _d_outerHTML.get,
      set: function (html) { _d_outerHTML.set.call(this, rewriteHtml(html)); },
      configurable: true, enumerable: true,
    });
  }

  // --- ShadowRoot.innerHTML (Web Components) --------------------------------
  if (_d_shadowInnerHTML && _d_shadowInnerHTML.set) {
    try {
      Object.defineProperty(ShadowRoot.prototype, "innerHTML", {
        get: _d_shadowInnerHTML.get,
        set: function (html) { _d_shadowInnerHTML.set.call(this, rewriteHtml(html)); },
        configurable: true, enumerable: true,
      });
    } catch (_) { /* */ }
  }

  // --- insertAdjacentHTML --------------------------------------------------
  Element.prototype.insertAdjacentHTML = function (pos, html) {
    return _insertAdjacentHTML.call(this, pos, rewriteHtml(html));
  };

  // --- document.write / writeln --------------------------------------------
  document.write = function () {
    var a = new Array(arguments.length);
    for (var i = 0; i < a.length; i++) a[i] = rewriteHtml(arguments[i]);
    return _docWrite.apply(document, a);
  };
  document.writeln = function () {
    var a = new Array(arguments.length);
    for (var i = 0; i < a.length; i++) a[i] = rewriteHtml(arguments[i]);
    return _docWriteln.apply(document, a);
  };

  /* ═══════════════════════════════════════════════════════════════════════
   * §6  CSS PATCHES
   * ═══════════════════════════════════════════════════════════════════════ */

  // setProperty
  CSSStyleDeclaration.prototype.setProperty = function (prop, value, priority) {
    if (typeof value === "string") value = rewriteCssValue(value);
    return _CSSSetProperty.call(this, prop, value, priority || "");
  };

  if (_CSSInsertRule) {
    CSSStyleSheet.prototype.insertRule = function (rule, index) {
      if (typeof rule === "string") rule = rewriteCssText(rule);
      return _CSSInsertRule.call(this, rule, index);
    };
  }

  if (_CSSReplace) {
    CSSStyleSheet.prototype.replace = function (text) {
      if (typeof text === "string") text = rewriteCssText(text);
      return _CSSReplace.call(this, text);
    };
  }

  if (_CSSReplaceSync) {
    CSSStyleSheet.prototype.replaceSync = function (text) {
      if (typeof text === "string") text = rewriteCssText(text);
      return _CSSReplaceSync.call(this, text);
    };
  }

  // cssText
  if (_d_cssText && _d_cssText.set) {
    Object.defineProperty(CSSStyleDeclaration.prototype, "cssText", {
      get: _d_cssText.get,
      set: function (v) { _d_cssText.set.call(this, rewriteCssText(v)); },
      configurable: true, enumerable: true,
    });
  }

  // Common CSS properties that may contain url()
  ["backgroundImage", "borderImage", "borderImageSource",
   "listStyleImage", "cursor", "content"].forEach(function (prop) {
    try {
      var d = Object.getOwnPropertyDescriptor(CSSStyleDeclaration.prototype, prop);
      if (!d || !d.set) return;
      Object.defineProperty(CSSStyleDeclaration.prototype, prop, {
        get: d.get ? function () { return d.get.call(this); } : undefined,
        set: function (v) { d.set.call(this, rewriteCssValue(v)); },
        configurable: true, enumerable: true,
      });
    } catch (_) { /* browser may not expose descriptor */ }
  });

  /* ═══════════════════════════════════════════════════════════════════════
   * §7  PARSER PATCHES
   * ═══════════════════════════════════════════════════════════════════════ */

  DOMParser.prototype.parseFromString = function (str, type) {
    if (type && type.indexOf("html") !== -1) str = rewriteHtml(str);
    return _DOMParse.call(this, str, type);
  };

  if (_createFrag) {
    Range.prototype.createContextualFragment = function (html) {
      return _createFrag.call(this, rewriteHtml(html));
    };
  }

  /* ═══════════════════════════════════════════════════════════════════════
   * §8  NAVIGATION PATCHES
   * ═══════════════════════════════════════════════════════════════════════ */

  // history
  History.prototype.pushState = function (state, title, url) {
    if (url) {
      var abs = resolveAbsolute(String(url));
      if (abs) setBase(abs);
      url = rewriteUrl(String(url));
    }
    return _pushState.call(this, state, title, url);
  };
  History.prototype.replaceState = function (state, title, url) {
    if (url) {
      var abs = resolveAbsolute(String(url));
      if (abs) setBase(abs);
      url = rewriteUrl(String(url));
    }
    return _replaceState.call(this, state, title, url);
  };

  // location methods
  Location.prototype.assign = function (url) {
    return _locationAssign.call(this, rewriteUrl(url));
  };
  Location.prototype.replace = function (url) {
    return _locationReplace.call(this, rewriteUrl(url));
  };

  function targetHref() {
    var base = getBaseURL();
    if (base) return base;
    var decoded = decodeBaseFromLocation();
    if (decoded) return decoded;
    var d = decodeUrl(String(location.href || ""));
    return d || String(location.href || "");
  }

  function targetURL() {
    try { return new URL(targetHref()); } catch (_) { return null; }
  }

  // location getters/setters (best-effort, some browsers restrict these)
  try {
    if (_d_locHref && (_d_locHref.get || _d_locHref.set)) {
      Object.defineProperty(Location.prototype, "href", {
        get: function () { return targetHref(); },
        set: function (v) {
          if (_d_locHref.set) _d_locHref.set.call(this, rewriteUrl(String(v)));
          else _locationAssign.call(this, rewriteUrl(String(v)));
        },
        configurable: true,
      });
    }
  } catch (_) { /* ignore */ }

  try {
    if (_d_locOrigin && _d_locOrigin.get) {
      Object.defineProperty(Location.prototype, "origin", {
        get: function () {
          var u = targetURL();
          return u ? u.origin : _d_locOrigin.get.call(this);
        },
        configurable: true,
      });
    }
  } catch (_) { /* ignore */ }

  function patchLocGetter(name, desc, getter) {
    if (!desc || !desc.get) return;
    try {
      Object.defineProperty(Location.prototype, name, {
        get: getter,
        configurable: true,
      });
    } catch (_) { /* ignore */ }
  }

  patchLocGetter("protocol", _d_locProtocol, function () {
    var u = targetURL();
    return u ? u.protocol : _d_locProtocol.get.call(this);
  });
  patchLocGetter("host", _d_locHost, function () {
    var u = targetURL();
    return u ? u.host : _d_locHost.get.call(this);
  });
  patchLocGetter("hostname", _d_locHostname, function () {
    var u = targetURL();
    return u ? u.hostname : _d_locHostname.get.call(this);
  });
  patchLocGetter("port", _d_locPort, function () {
    var u = targetURL();
    return u ? u.port : _d_locPort.get.call(this);
  });
  patchLocGetter("pathname", _d_locPathname, function () {
    var u = targetURL();
    return u ? u.pathname : _d_locPathname.get.call(this);
  });
  patchLocGetter("search", _d_locSearch, function () {
    var u = targetURL();
    return u ? u.search : _d_locSearch.get.call(this);
  });
  patchLocGetter("hash", _d_locHash, function () {
    var u = targetURL();
    return u ? u.hash : _d_locHash.get.call(this);
  });

  // Document URL/baseURI/referrer (best-effort)
  try {
    var _d_docURL = saveDesc(Document.prototype, "URL");
    if (_d_docURL && _d_docURL.get) {
      Object.defineProperty(Document.prototype, "URL", {
        get: function () { return targetHref(); },
        configurable: true,
      });
    }
  } catch (_) { /* ignore */ }

  try {
    var _d_baseURI = saveDesc(Document.prototype, "baseURI");
    if (_d_baseURI && _d_baseURI.get) {
      Object.defineProperty(Document.prototype, "baseURI", {
        get: function () { return targetHref(); },
        configurable: true,
      });
    }
  } catch (_) { /* ignore */ }

  try {
    var _d_referrer = saveDesc(Document.prototype, "referrer");
    if (_d_referrer && _d_referrer.get) {
      Object.defineProperty(Document.prototype, "referrer", {
        get: function () {
          var r = _d_referrer.get.call(this);
          return typeof r === "string" ? decodeUrl(r) : r;
        },
        configurable: true,
      });
    }
  } catch (_) { /* ignore */ }

  // Anchor clicks  (capture phase so we beat framework listeners)
  document.addEventListener("click", function (e) {
    var el = e.target;
    while (el && el.tagName !== "A") el = el.parentElement;
    if (!el) return;
    var href = _getAttribute.call(el, "href");
    if (!href || href.charAt(0) === "#" ||
        /^\s*javascript\s*:/i.test(href) || isProxied(href)) return;
    e.preventDefault();
    var dest = rewriteUrl(href);
    if (el.target === "_blank") { _windowOpen.call(window, dest); }
    else { location.href = dest; }
  }, true);

  // Form submissions
  document.addEventListener("submit", function (e) {
    var form = e.target;
    if (!form || form.tagName !== "FORM") return;
    var action = _getAttribute.call(form, "action");
    if (action && !isProxied(action)) {
      _setAttribute.call(form, "action", rewriteUrl(action));
    }
  }, true);

  // postMessage
  window.postMessage = function (msg, origin, transfer) {
    if (origin && origin !== "*" && origin !== PROXY_ORIGIN) origin = PROXY_ORIGIN;
    return _postMessage.call(window, msg, origin, transfer);
  };

  /* ═══════════════════════════════════════════════════════════════════════
   * §9  STORAGE PATCHES  (namespaced by target origin)
   *
   * KEY FIX: _realLocalStorage / _realSessionStorage were captured in §1
   * BEFORE any defineProperty.  The getter returns a pre-built wrapper —
   * it never accesses window.localStorage / window.sessionStorage again,
   * so there is zero risk of infinite recursion.
   * ═══════════════════════════════════════════════════════════════════════ */

  function storagePrefix() {
    return "__ix_" + getBaseOrigin() + "_";
  }

  function nsStorage(real) {
    return {
      getItem:    function (k) { return real.getItem(storagePrefix() + k); },
      setItem:    function (k, v) { real.setItem(storagePrefix() + k, String(v)); },
      removeItem: function (k) { real.removeItem(storagePrefix() + k); },
      clear: function () {
        var p = storagePrefix();
        for (var i = real.length - 1; i >= 0; i--) {
          var k = real.key(i);
          if (k && k.indexOf(p) === 0) real.removeItem(k);
        }
      },
      key: function (idx) {
        var p = storagePrefix();
        var n = 0;
        for (var i = 0; i < real.length; i++) {
          var k = real.key(i);
          if (k && k.indexOf(p) === 0) {
            if (n === idx) return k.slice(p.length);
            n++;
          }
        }
        return null;
      },
      get length() {
        var p = storagePrefix();
        var n = 0;
        for (var i = 0; i < real.length; i++) {
          if ((real.key(i) || "").indexOf(p) === 0) n++;
        }
        return n;
      },
    };
  }

  var _nsLocal   = nsStorage(_realLocalStorage);
  var _nsSession = nsStorage(_realSessionStorage);

  try {
    Object.defineProperty(window, "localStorage", {
      get: function () { return _nsLocal; },
      configurable: true,
    });
  } catch (_) { /* browser may block */ }

  try {
    Object.defineProperty(window, "sessionStorage", {
      get: function () { return _nsSession; },
      configurable: true,
    });
  } catch (_) { /* browser may block */ }

  /* ═══════════════════════════════════════════════════════════════════════
   * §10  SERVICE WORKER PATCH
   * ═══════════════════════════════════════════════════════════════════════ */

  if (_swRegister) {
    navigator.serviceWorker.register = function (url, opts) {
      return _swRegister(rewriteUrl(url), opts);
    };
  }

  /* ═══════════════════════════════════════════════════════════════════════
   * §11  BLOB URL TRACKING
   * ═══════════════════════════════════════════════════════════════════════ */

  var _blobMap = new Map();

  URL.createObjectURL = function (obj) {
    var u = _createObjectURL.call(URL, obj);
    _blobMap.set(u, obj);
    return u;
  };
  URL.revokeObjectURL = function (url) {
    _blobMap.delete(url);
    return _revokeObjectURL.call(URL, url);
  };

  /* ═══════════════════════════════════════════════════════════════════════
   * §12  MUTATION OBSERVER  (safety-net for dynamically-added elements)
   *
   * Only observes childList (not attributes) so that the _setAttribute
   * calls inside fixNode() do NOT re-trigger the observer.
   * ═══════════════════════════════════════════════════════════════════════ */

  function fixNode(el) {
    if (el.nodeType !== 1) return;

    // URL attributes
    URL_ATTR_SET.forEach(function (attr) {
      var v = _getAttribute.call(el, attr);
      if (!v) return;
      var r = rewriteUrl(v);
      if (r !== v) _setAttribute.call(el, attr, r);
    });

    // srcset
    var ss = _getAttribute.call(el, "srcset");
    if (ss) {
      var rs = rewriteSrcset(ss);
      if (rs !== ss) _setAttribute.call(el, "srcset", rs);
    }

    // Inline style  url(…)
    var st = _getAttribute.call(el, "style");
    if (st && st.indexOf("url(") !== -1) {
      var rs2 = rewriteCssValue(st);
      if (rs2 !== st) _setAttribute.call(el, "style", rs2);
    }

    // <style> elements — rewrite CSS text content
    if (el.tagName === "STYLE" && el.textContent) {
      var ct = el.textContent;
      var rc = rewriteCssText(ct);
      if (rc !== ct) el.textContent = rc;
    }
  }

  var _mo = new MutationObserver(function (records) {
    for (var i = 0; i < records.length; i++) {
      var nodes = records[i].addedNodes;
      for (var j = 0; j < nodes.length; j++) {
        var n = nodes[j];
        if (n.nodeType !== 1) continue;
        fixNode(n);
        if (n.querySelectorAll) {
          var kids = n.querySelectorAll("*");
          for (var k = 0; k < kids.length; k++) fixNode(kids[k]);
        }
      }
    }
  });

  var _moTarget = document.documentElement || document;
  _mo.observe(_moTarget, { childList: true, subtree: true });

  /* ═══════════════════════════════════════════════════════════════════════ */

  console.log("[Internex] Runtime v2 loaded — base:", BASE_URL);
})();
