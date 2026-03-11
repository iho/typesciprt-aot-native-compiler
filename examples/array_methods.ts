// ── Array methods: push, pop, indexOf, join ───────────────────────────────────
const arr: number[] = [];
arr.push(10);
arr.push(20);
arr.push(30);

// arr = [10, 20, 30], length = 3
const len = arr.length;

// pop removes 30
const popped = arr.pop();

// arr = [10, 20]
const idx = arr.indexOf(20);  // 1

// join
const joined = arr.join(",");
console.log(joined);   // "10,20"

// Exit: len(3) + idx(1) = 4
len + idx
