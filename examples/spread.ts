// ── Spread operator in array literals ────────────────────────────────────────
const a = [1, 2, 3];
const b = [4, 5];
const c = [...a, ...b];   // [1, 2, 3, 4, 5]

let sum = 0;
for (const x of c) {
    sum = sum + x;
}

// Exit: 1+2+3+4+5 = 15
sum
