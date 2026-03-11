function sum(first: number, ...rest: number[]) {
  let total = first;
  for (const n of rest) {
    total = total + n;
  }
  return total;
}
sum(1, 2, 3, 4)  // 10
