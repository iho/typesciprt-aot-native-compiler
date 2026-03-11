// ── String methods ────────────────────────────────────────────────────────────
const s = "Hello, World!";

// length = 13
const len = s.length;

// indexOf
const idx = s.indexOf("World");  // 7

// includes → true (1)
const hasHello = s.includes("Hello");

// toUpperCase / toLowerCase
const upper = s.toUpperCase();
console.log(upper);   // HELLO, WORLD!

const lower = s.toLowerCase();
console.log(lower);   // hello, world!

// slice
const sliced = s.slice(7, 12);
console.log(sliced);  // World

// trim
const padded = "  hello  ";
const trimmed = padded.trim();
console.log(trimmed);  // hello

// split
const parts = s.split(", ");
const firstPart = parts[0];  // "Hello"
console.log(firstPart);

// Exit: len(13) - idx(7) = 6
len - idx
