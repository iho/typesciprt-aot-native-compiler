// Arrow functions and array higher-order methods
const nums = [1, 2, 3, 4, 5];

// map: double each element → [2, 4, 6, 8, 10]
const doubled = nums.map((x) => x * 2);

// filter: keep evens → [2, 4]
const evens = nums.filter((x) => x % 2 === 0);

// reduce: sum all → 15
const sum = nums.reduce((acc, x) => acc + x, 0);

// find: first > 3 → 4
const found = nums.find((x) => x > 3);

// some / every → true (1)
const hasBig = nums.some((x) => x > 4);
const allPos = nums.every((x) => x > 0);

// Arrow function called directly
const triple = (x: number) => x * 3;
const t = triple(4);  // 12

// findIndex: first element > 2 → index 2
const idx = nums.findIndex((x) => x > 2);  // 2

// Exit: sum + found + hasBig + allPos + t + idx - 34
// = 15 + 4 + 1 + 1 + 12 + 2 - 34 = 1
// Let's make it simpler: sum = 15, exit with 15
sum;
