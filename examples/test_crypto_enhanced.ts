import { createHash, createHmac, randomBytes, pbkdf2Sync, scryptSync, timingSafeEqual, randomFillSync } from 'node:crypto';

// Basic hash (pre-existing)
const h = createHash('sha256');
h.update('hello');
const hex = h.digest('hex');
console.log('sha256:', hex);

// PBKDF2
const key1 = pbkdf2Sync('password', 'salt', 1000, 32, 'sha256');
console.log('pbkdf2 key length:', key1.length);

// scrypt
const key2 = scryptSync('password', 'salt', 32);
console.log('scrypt key length:', key2.length);

// timingSafeEqual
const buf1 = randomBytes(16);
const buf2 = randomBytes(16);
const eq = timingSafeEqual(buf1, buf1);
const neq = timingSafeEqual(buf1, buf2);
console.log('timing safe equal (same):', eq);

// randomFillSync
const buf3 = randomBytes(8);
const filled = randomFillSync(buf3);
console.log('randomFillSync length:', filled.length);

console.log('crypto enhanced: OK');
