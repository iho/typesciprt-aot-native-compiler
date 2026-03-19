// Array destructuring assignment (swap)
let a = 10, b = 20
;[a, b] = [b, a]
// a=20, b=10

// Object destructuring assignment
let x = 0, y = 0
;({ x, y } = { x: 3, y: 9 })
// x=3, y=9

process.exit(a + b + x + y) // 20+10+3+9 = 42
