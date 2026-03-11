function makeAdder(x: number) {
  return (y: number) => x + y;
}
const add5 = makeAdder(5);
const add10 = makeAdder(10);
add5(3) + add10(1)  // 8 + 11 = 19
