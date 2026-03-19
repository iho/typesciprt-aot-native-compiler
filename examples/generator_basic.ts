function* counter(start: number, end: number) {
  let n = start
  while (n <= end) {
    yield n
    n++
  }
}

let sum = 0
for (const x of counter(10, 12)) {
  sum += x
}
// 10 + 11 + 12 = 33
process.exit(sum)
