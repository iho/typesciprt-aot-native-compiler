// Map built-in
const m = new Map();
m.set("a", 10);
m.set("b", 20);
m.set("c", 5);

const a = m.get("a");    // 10
const b = m.get("b");    // 20
const sz = m.size;       // 3

const has_a = m.has("a") ? 1 : 0;   // 1
const has_x = m.has("x") ? 1 : 0;   // 0

// a(10) + b(20) + sz(3) + has_a(1) + has_x(0) = 34
a + b + sz + has_a + has_x
