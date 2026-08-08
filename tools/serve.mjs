// A static server for the web viewer, for local development.
//
// `node tools/serve.mjs [port]` then open http://localhost:8080.
//
// Not a build step and not shipped anywhere — GitHub Pages serves the same
// files in production. It exists because opening `web/index.html` as a `file://`
// URL does not work: ES modules and `fetch` both need an origin, and the wasm
// will not instantiate without the `application/wasm` content type, which this
// sets and the filesystem does not.

import { createReadStream, statSync } from 'node:fs';
import { createServer } from 'node:http';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';

// fileURLToPath, not URL.pathname: on Windows the latter yields `/C:/…` with
// forward slashes, which never matches the backslashes `join` produces, and the
// containment check below then rejects every request.
const root = fileURLToPath(new URL('../web/', import.meta.url));
const port = Number(process.argv[2] ?? 8080);

const types = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.wasm': 'application/wasm',
  '.gz': 'application/gzip',
  '.nkp': 'application/octet-stream',
  '.json': 'application/json',
  '.css': 'text/css; charset=utf-8',
};

createServer((request, response) => {
  const url = new URL(request.url, 'http://localhost');
  // Strip leading slashes and normalise, so `..` cannot climb out of web/.
  const relative = normalize(decodeURIComponent(url.pathname)).replace(/^([/\\])+/, '');
  const path = join(root, relative === '' ? 'index.html' : relative);
  if (!path.startsWith(root)) {
    response.writeHead(403).end('no');
    return;
  }

  let size;
  try {
    const info = statSync(path);
    if (info.isDirectory()) throw new Error('directory');
    size = info.size;
  } catch {
    response.writeHead(404, { 'content-type': 'text/plain' }).end(`404 ${relative}`);
    return;
  }

  response.writeHead(200, {
    'content-type': types[extname(path)] ?? 'application/octet-stream',
    'content-length': size,
    // Never cache during development: the whole point is to see a rebuild.
    'cache-control': 'no-store',
  });
  createReadStream(path).pipe(response);
}).listen(port, () => {
  console.log(`serving ${root} on http://localhost:${port}`);
});
