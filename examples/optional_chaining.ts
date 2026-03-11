// ── Optional chaining (?.) + nullish coalescing (??) ─────────────────────────
const obj = { x: 7 };
const nullObj: any = null;

const v1 = obj?.x;        // 7  (obj is not null)
const v2 = nullObj?.x;    // undefined  (nullObj is null → short-circuit)
const v3 = v2 ?? 5;       // 5  (undefined is nullish)
const v4 = v1 ?? 99;      // 7  (not nullish)

// Exit: 5 + 7 = 12
v3 + v4
