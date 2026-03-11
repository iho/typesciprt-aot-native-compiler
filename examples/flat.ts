// Array.prototype.flat
const nested = [1, [2, 3], [4, [5, 6]]];
const flat2 = nested.flat(2);  // [1,2,3,4,5,6]

let sum = 0;
for (const x of flat2) {
  sum = sum + x;
}
sum  // 1+2+3+4+5+6 = 21
