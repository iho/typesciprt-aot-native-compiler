// ── Object destructuring ──────────────────────────────────────────────────────
const obj = { a: 10, b: 20, c: 30 };
const { a, b } = obj;

// ── Array destructuring ───────────────────────────────────────────────────────
const arr = [1, 2, 3, 4];
const [x, , z] = arr; // skip index 1 (y), x=1 z=3

// Exit: 10 + 20 + 1 + 3 = 34
a + b + x + z
