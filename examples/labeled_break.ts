// Test labeled break: break out of outer loop from inner loop
let total = 0
outer: for (let i = 0; i < 5; i++) {
  for (let j = 0; j < 5; j++) {
    if (j === 2) break outer
    total = total + 1
  }
}
// i=0: j=0 (+1), j=1 (+1), j=2 breaks outer → total=2

// Test labeled continue: skip to next outer iteration
let total2 = 0
outer2: for (let i = 0; i < 3; i++) {
  for (let j = 0; j < 3; j++) {
    if (j === 1) continue outer2
    total2 = total2 + 1
  }
}
// Each i: j=0 (+1), j=1 continue outer2 (skip rest of inner) → 3 * 1 = total2=3

total + total2
// 2 + 3 = 5
