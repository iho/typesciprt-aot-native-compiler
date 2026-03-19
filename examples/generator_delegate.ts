function* naturals(n: number) {
  let i = 1
  while (i <= n) {
    yield i++
  }
}

function* doubled(n: number) {
  yield* naturals(n)
}

let sum = 0
for (const x of doubled(5)) {
  sum += x
}
// 1+2+3+4+5 = 15
process.exit(sum)
