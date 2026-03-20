// Smoke test for all new node:* APIs
import assert from 'node:assert';
import { stringify, parse } from 'node:querystring';
import { setTimeout as myTimeout } from 'node:timers';
import { StringDecoder } from 'node:string_decoder';
import { URL } from 'node:url';
import { execSync } from 'node:child_process';
import { gzipSync, gunzipSync } from 'node:zlib';

// node:assert
assert.ok(true, 'ok works');
assert.strictEqual(1 + 1, 2, 'strictEqual works');
assert.deepEqual({ a: 1 }, { a: 1 }, 'deepEqual works');
console.log('assert: OK');

// node:querystring
const qs = stringify({ name: 'Alice', age: '30' });
console.log('qs stringify:', qs);
const parsed = parse('name=Alice&age=30');
console.log('qs parse name:', parsed.name);
assert.strictEqual(parsed.name, 'Alice');
console.log('querystring: OK');

// node:string_decoder
const decoder = new StringDecoder('utf8');
const decoded = decoder.write('hello');
assert.strictEqual(decoded, 'hello');
console.log('string_decoder: OK');

// node:url
const u = new URL('https://example.com:8080/path?foo=bar#hash');
assert.strictEqual(u.protocol, 'https:');
assert.strictEqual(u.hostname, 'example.com');
assert.strictEqual(u.port, '8080');
assert.strictEqual(u.pathname, '/path');
assert.strictEqual(u.search, '?foo=bar');
assert.strictEqual(u.hash, '#hash');
console.log('url hostname:', u.hostname);
console.log('url: OK');

// node:child_process
const echo = execSync('echo hello');
console.log('child_process execSync:', echo.trim());
assert.strictEqual(echo.trim(), 'hello');
console.log('child_process: OK');

// node:zlib
const original = 'Hello, world! This is a test of gzip compression.';
// Use Buffer.from equivalent — pass string directly
const compressed = gzipSync(original);
console.log('zlib compressed length:', compressed.length);
const decompressed = gunzipSync(compressed);
console.log('zlib decompressed type:', typeof decompressed);
console.log('zlib: OK');

console.log('All node:* API tests passed!');
