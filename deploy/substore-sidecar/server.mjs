// server.mjs — Phlogo Gnosis <-> sub-store-convert sidecar.
//
// WHY THIS FILE EXISTS
//   Upstream tbxark/sub-store-convert ships:
//     - a bundled core (packages/core/build/index.js) exposing convert(url, target, opts)
//       where the input is always fetched by URL; and
//     - a Hono app with only `GET /sub?target=&url=`.
//   A real 52-node Clash config becomes a ~62 KB `data:` URL, far over the request-line
//   and header limits, so the GET-only API cannot carry real subscriptions.
//
//   This wrapper keeps upstream's conversion logic untouched and adds the minimum we
//   need: POST the Clash YAML in the body, get the target format's proxy lines back.
//   It also keeps the 503 KB bundle + peggy grammars loaded ONCE (a fresh `node`
//   invocation per request re-parses them on every call).
//
// INPUT CONTRACT
//   We feed it the Clash/mihomo YAML that Phlogo Gnosis already produces
//   (crates/distribution::mihomo_yaml). Verified on the real 52-node subscription:
//   Clash YAML -> 52/52 nodes for surge/stash/shadowrocket/egern/mihomo/sing-box,
//   and fewer for clients that cannot express某些协议 (qx drops hysteria2, etc.) —
//   that filtering is upstream's per-client capability logic and is desirable.
//   NOTE: a plain/base64 URI list does NOT work with this build (0 nodes parsed).
//
// API
//   GET  /health                     -> { ok, targets: [...] }
//   POST /convert?target=<target>    body = Clash YAML  -> text/plain proxy lines
//   POST /convert/shape?target=<t>   -> { target, ok, nodes, bytes }  (diagnostic)
//
// ENV
//   SUBSTORE_PORT      default 3000
//   SUBSTORE_HOST      default 127.0.0.1
//   SUBSTORE_MAX_BODY  default 8388608 (8 MiB)
//   SUBSTORE_BUNDLE    default ./index.mjs
//
// LICENSE NOTE
//   The vendored bundle inlines AGPL-3.0 code from Sub-Store (upstream's MIT wrapper
//   vendors it), so this sidecar is distributed under AGPL-3.0 — which is what
//   Phlogo Gnosis already uses. Keep the attribution in docs/THIRD_PARTY.md.

import { createServer } from 'node:http';
import { pathToFileURL } from 'node:url';
import { resolve } from 'node:path';

const PORT = Number(process.env.SUBSTORE_PORT || 3000);
const HOST = process.env.SUBSTORE_HOST || '127.0.0.1';
const MAX_BODY = Number(process.env.SUBSTORE_MAX_BODY || 8 * 1024 * 1024);
const BUNDLE = resolve(process.env.SUBSTORE_BUNDLE || './index.mjs');

let convert;
let TARGETS = [];

async function loadCore() {
  const mod = await import(pathToFileURL(BUNDLE).href);
  if (typeof mod.convert !== 'function') {
    throw new Error('bundle does not export convert()');
  }
  convert = mod.convert;
  // canonical target list, lowercased + de-duplicated (upstream has aliases:
  // surge/Surge/SurgeMac, mihomo/meta/clashmeta/... )
  const keys = Object.keys(mod.produce || {});
  TARGETS = [...new Set(keys.map((k) => k.toLowerCase()))].sort();
  return { targets: TARGETS.length };
}

function readBody(req) {
  return new Promise((res, rej) => {
    const chunks = [];
    let size = 0;
    req.on('data', (c) => {
      size += c.length;
      if (size > MAX_BODY) {
        rej(Object.assign(new Error(`body too large (> ${MAX_BODY} bytes)`), { status: 413 }));
        req.destroy();
        return;
      }
      chunks.push(c);
    });
    req.on('end', () => res(Buffer.concat(chunks).toString('utf8')));
    req.on('error', (e) => rej(Object.assign(e, { status: 400 })));
  });
}

function send(res, status, body, type = 'text/plain; charset=utf-8') {
  if (typeof res.setHeader === 'function') {
    res.statusCode = status;
    res.setHeader('content-type', type);
    res.setHeader('cache-control', 'no-store');
  }
  res.end(body);
}

function json(res, status, obj) {
  send(res, status, JSON.stringify(obj), 'application/json; charset=utf-8');
}

function shapeOf(text) {
  // Honest diagnostic: output formats differ (surge = one line per node,
  // sing-box/json = pretty JSON), so a single "node count" would be a lie.
  // Report non-empty lines and bytes instead.
  const lines = text.split('\n').filter((l) => l.trim()).length;
  return { lines, bytes: Buffer.byteLength(text, 'utf8') };
}

async function main() {
  const info = await loadCore();
  console.log(`[substore-sidecar] core loaded from ${BUNDLE}; ${info.targets} targets`);

  const server = createServer(async (req, res) => {
    try {
      const url = new URL(req.url, `http://${req.headers.host || 'localhost'}`);
      const path = url.pathname.replace(/\/+$/, '') || '/';

      if (req.method === 'GET' && path === '/health') {
        return json(res, 200, { ok: true, target_count: TARGETS.length, targets: TARGETS });
      }

      if (req.method === 'POST' && (path === '/convert' || path === '/convert/shape')) {
        const target = (url.searchParams.get('target') || '').toLowerCase().trim();
        if (!target) return json(res, 400, { error: 'missing ?target=' });
        if (!TARGETS.includes(target)) {
          return json(res, 400, { error: `unknown target '${target}'`, targets: TARGETS });
        }
        const body = await readBody(req);
        if (!body.trim()) return json(res, 400, { error: 'empty body (expected Clash YAML)' });

        // Upstream's convert() takes a URL; a data: URL keeps us out of any HTTP
        // fetch and avoids buffering the config to disk.
        const dataUrl = 'data:text/plain;base64,' + Buffer.from(body, 'utf8').toString('base64');

        const t0 = Date.now();
        let out;
        try {
          out = await convert(dataUrl, target, {});
        } catch (e) {
          return json(res, 502, { error: 'convert failed', detail: String(e && e.message) });
        }
        const text = typeof out === 'string' ? out : JSON.stringify(out, null, 2);
        const ms = Date.now() - t0;

        if (path === '/convert/shape') {
          const shape = shapeOf(text);
          return json(res, 200, { target, ok: true, ...shape, ms });
        }
        // empty output is a real failure mode (0 nodes parsed) — surface it loudly
        if (!text.trim()) {
          return json(res, 502, { error: 'empty conversion output (no nodes parsed)', target });
        }
        return send(res, 200, text);
      }

      if (path === '/') {
        return send(res, 200, 'phlogo-gnosis substore sidecar\nGET /health\nPOST /convert?target=surge\n');
      }
      return json(res, 404, { error: 'not found' });
    } catch (e) {
      const status = e && e.status ? e.status : 500;
      return json(res, status, { error: String((e && e.message) || e) });
    }
  });

  server.listen(PORT, HOST, () => {
    console.log(`[substore-sidecar] listening on http://${HOST}:${PORT}`);
  });

  const shutdown = (sig) => {
    console.log(`[substore-sidecar] ${sig} -> shutting down`);
    server.close(() => process.exit(0));
    setTimeout(() => process.exit(1), 5000).unref();
  };
  process.on('SIGINT', () => shutdown('SIGINT'));
  process.on('SIGTERM', () => shutdown('SIGTERM'));
}

main().catch((e) => {
  console.error('[substore-sidecar] fatal:', e);
  process.exit(1);
});
