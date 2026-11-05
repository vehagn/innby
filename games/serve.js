// Serves the example games, in plain Node with no packages: each game's files at
// /{game}/, the shared ones at /lib/, and for a game with rules (a logic.js), its API at
// /{game}/api/, which works its scores out and sends them to Innby. See docs/games.md.
//
//   PORT                 where to listen (8082)
//   INNBY_URL            the site's origin, which the browser-only games call
//   INNBY_INTERNAL_URL   Innby's internal API, for games with rules
//                        (http://localhost:8083; in a cluster, its internal Service)
//   INNBY_GAME_KEY_{ID}  each such game's key, from `innby game-key {id}`, e.g.
//                        INNBY_GAME_KEY_SNAKE; without it, the game plays but keeps
//                        no scores
//
// It's a small server for small games: put a proxy in front for HTTPS.
import { createReadStream } from "node:fs";
import { readdir, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, join, normalize, sep } from "node:path";
import { fileURLToPath } from "node:url";

import { Innby, gameApi } from "./lib/server.js";

const root = fileURLToPath(new URL(".", import.meta.url));
const port = Number(process.env.PORT || 8082);
const internal = process.env.INNBY_INTERNAL_URL || "http://localhost:8083";

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".wasm": "application/wasm",
};

// Scripts and styles from this origin only; Innby's site too, for the browser-only
// games, which call it from the page; and WebAssembly, for games written in Rust.
const policy = [
  "default-src 'self'",
  `connect-src 'self' ${process.env.INNBY_URL || ""}`.trim(),
  "script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'",
  "style-src 'self' 'unsafe-inline'",
  "frame-ancestors 'none'",
  "base-uri 'none'",
].join("; ");

/** Each game with rules, by id: its API. */
async function games() {
  const apis = new Map();
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const logicFile = join(root, entry.name, "logic.js");
    if (!entry.isDirectory() || !(await stat(logicFile).catch(() => null))) continue;
    // A game in Rust needs its .wasm built first: `just games`.
    const logic = await import(logicFile).catch((error) => {
      console.warn(`${entry.name}: its rules didn't load, so it's left out: ${error.message}`);
    });
    if (!logic) continue;
    const key = process.env[`INNBY_GAME_KEY_${entry.name.toUpperCase().replaceAll("-", "_")}`];
    if (!key) console.warn(`${entry.name}: no INNBY_GAME_KEY, so its scores aren't kept`);
    apis.set(entry.name, gameApi(logic, key ? new Innby(internal, entry.name, key) : null));
  }
  return apis;
}

const apis = await games();

createServer(async (request, response) => {
  response.setHeader("Content-Security-Policy", policy);
  response.setHeader("Referrer-Policy", "no-referrer");
  response.setHeader("X-Content-Type-Options", "nosniff");
  try {
    const path = new URL(request.url, "http://game").pathname;
    // For the kubelet's probes.
    if (path === "/healthz") {
      response.writeHead(200, { "Content-Type": "text/plain" }).end("ok");
      return;
    }
    const [, game, rest] = path.match(/^\/([a-z0-9-]+)\/(.*)$/) ?? [];
    if (game && rest.startsWith("api/")) {
      const api = apis.get(game);
      if (api && (await api(request, response, rest.slice(4)))) return;
      response.writeHead(404).end();
      return;
    }
    if (request.method !== "GET" && request.method !== "HEAD") {
      response.writeHead(405).end();
      return;
    }
    // A file under the games' folder, never outside it, and never this server's own.
    let file = normalize(join(root, decodeURIComponent(path)));
    if (!file.startsWith(root) || file === join(root, "serve.js") || file.includes(`${sep}test${sep}`)) {
      response.writeHead(404).end();
      return;
    }
    if ((await stat(file).catch(() => null))?.isDirectory()) {
      // Its page's links are relative to the folder.
      if (!path.endsWith("/")) {
        response.writeHead(301, { Location: `${path}/` }).end();
        return;
      }
      file = join(file, "index.html");
    }
    const type = TYPES[extname(file)];
    if (!type || !(await stat(file).catch(() => null))?.isFile()) {
      response.writeHead(404).end();
      return;
    }
    response.writeHead(200, { "Content-Type": type, "Cache-Control": "no-cache" });
    createReadStream(file).pipe(response);
  } catch (error) {
    console.error(error);
    if (!response.headersSent) response.writeHead(500);
    response.end();
  }
}).listen(port, () => {
  console.log(`games on :${port}: ${[...apis.keys()].join(", ") || "none"} with rules`);
});
