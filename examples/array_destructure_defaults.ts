const [a = 10, b = 20, c] = [1, undefined, 3]
// a = 1 (present), b = 20 (undefined -> default), c = 3
const result = a + b + c  // 1 + 20 + 3 = 24
result
