// ── Computed property names ───────────────────────────────────────────────────
const key1 = "a";
const key2 = "b";

const obj = { [key1]: 10, [key2]: 20, c: 5 };

// obj.a = 10, obj.b = 20, obj.c = 5
const sum = obj.a + obj.b + obj.c;

// Exit: 10 + 20 + 5 = 35
sum
