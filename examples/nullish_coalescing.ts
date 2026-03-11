// ── Nullish coalescing (??) ───────────────────────────────────────────────────
const n1 = null;
const n2 = null;

const v1 = n1 ?? 10;  // 10  (null is nullish)
const v2 = n2 ?? 20;  // 20  (null is nullish)
const v3 = 5 ?? 99;   // 5   (5 is not nullish, short-circuits)

// Exit: 10 + 20 + 5 = 35
v1 + v2 + v3
