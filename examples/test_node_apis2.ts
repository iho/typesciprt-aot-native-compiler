// Smoke test for the second batch of node:* APIs
import { performance } from 'node:perf_hooks';
import { format, promisify, inspect, TextEncoder, TextDecoder } from 'node:util';
import { parse as pathParse, format as pathFormat } from 'node:path';
import { isMainThread, threadId } from 'node:worker_threads';
import { isatty } from 'node:tty';

// perf_hooks
const t0 = performance.now();
performance.mark('start');
const t1 = performance.now();
console.log('performance.now returns number:', typeof t0 === 'number');
console.log('performance.now increases:', t1 >= t0);
performance.measure('elapsed', 'start');
console.log('perf_hooks: OK');

// util.format
const s = format('hello %s, count=%d, obj=%j', 'world', 42, { x: 1 });
console.log('util.format:', s);
console.log('util.inspect:', inspect({ a: 1, b: [2, 3] }));

// util.promisify
function addCallback(a: number, b: number, cb: (err: any, result: number) => void): void {
  cb(null, a + b);
}
const addAsync = promisify(addCallback);
addAsync(3, 4).then((result: any) => {
  console.log('util.promisify result:', result);
});

// TextEncoder / TextDecoder
const enc = new TextEncoder();
const encoded = enc.encode('hello');
console.log('TextEncoder length:', encoded.length);
const dec = new TextDecoder();
const decoded = dec.decode('hello');
console.log('TextDecoder result:', decoded);
console.log('util: OK');

// path.parse / path.format
const parsed = pathParse('/usr/local/bin/node');
console.log('path.parse root:', parsed.root);
console.log('path.parse dir:', parsed.dir);
console.log('path.parse base:', parsed.base);
console.log('path.parse ext:', parsed.ext);
console.log('path.parse name:', parsed.name);
const formatted = pathFormat({ dir: '/usr/local/bin', base: 'node' });
console.log('path.format:', formatted);
console.log('path: OK');

// worker_threads
console.log('isMainThread:', isMainThread);
console.log('threadId:', threadId);
console.log('worker_threads: OK');

// tty
console.log('isatty(0):', isatty(0));
console.log('tty: OK');

console.log('All node:* API2 tests passed!');
