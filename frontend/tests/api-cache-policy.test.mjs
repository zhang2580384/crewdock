import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const directory = path.dirname(fileURLToPath(import.meta.url));
const apiSource = fs.readFileSync(path.join(directory, '..', 'src', 'api.ts'), 'utf8');
const nginxConfig = fs.readFileSync(path.join(directory, '..', 'nginx.conf'), 'utf8');

test('API fetches bypass the browser HTTP cache', () => {
  assert.match(apiSource, /fetch\(`\$\{API_BASE\}\$\{path\}`, \{ \.\.\.init, headers, cache: 'no-store' \}\)/);
  assert.match(apiSource, /fetch\(`\$\{API_BASE\}\$\{path\}`, \{ headers, cache: 'no-store' \}\)/);
});

test('API and health responses explicitly forbid intermediary caching', () => {
  for (const location of ['/api/', '/health']) {
    const start = nginxConfig.indexOf(`location ${location} {`);
    assert.notEqual(start, -1, `${location} location should exist`);
    const block = nginxConfig.slice(start, nginxConfig.indexOf('\n  }', start));
    assert.match(block, /proxy_cache off;/);
    assert.match(block, /Cache-Control "no-store, no-cache, must-revalidate, private"/);
    assert.match(block, /X-Accel-Expires "0"/);
  }
});
