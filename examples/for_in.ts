// ── for...in loop over object keys ────────────────────────────────────────────
const obj = { a: 1, b: 2, c: 3 };
let count = 0;
for (const key in obj) {
    count = count + 1;
}

// Exit: 3 (three keys: a, b, c)
count
