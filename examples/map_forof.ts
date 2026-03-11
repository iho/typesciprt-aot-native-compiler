const m = new Map();
m.set("a", 1);
m.set("b", 2);
let total = 0;
for (const [k, v] of m.entries()) {
  total = total + v;
}
total  // 3
