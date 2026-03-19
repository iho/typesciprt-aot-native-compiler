function makeRange(start: number, end: number) {
  return {
    [Symbol.iterator]() {
      let current = start
      return {
        next() {
          if (current < end) {
            return { value: current++, done: false }
          }
          return { value: undefined, done: true }
        }
      }
    }
  }
}

let sum = 0
for (const n of makeRange(1, 6)) {
  sum += n
}
process.exit(sum) // 1+2+3+4+5 = 15
