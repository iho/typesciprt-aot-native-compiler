// Logical assignment operators: ??=, ||=, &&=

// ??= : assign only when null/undefined
let a: number | null = null;
a ??= 10;       // null  → assign 10  → a = 10

let b: number | null = 5;
b ??= 20;       // 5     → keep       → b = 5

// ||= : assign only when falsy
let c = 0;
c ||= 3;        // 0 (falsy) → assign 3 → c = 3

let d = 7;
d ||= 99;       // 7 (truthy) → keep  → d = 7

// &&= : assign only when truthy
let e = 4;
e &&= 2;        // 4 (truthy) → assign 2 → e = 2

let f = 0;
f &&= 99;       // 0 (falsy) → keep   → f = 0

// exit: a(10) + b(5) + c(3) + d(7) + e(2) + f(0) - 27 = 0
// Let's exit with: a + b + c + d + e + f = 27
a + b + c + d + e + f;
