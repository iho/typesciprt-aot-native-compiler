// Switch statement tests — exit code = sum of matched values
let total = 0

// Basic switch with break
const x = 2
switch (x) {
  case 1:
    total += 10
    break
  case 2:
    total += 20
    break
  case 3:
    total += 30
    break
}

// Default case
const y = 99
switch (y) {
  case 1:
    total += 1
    break
  default:
    total += 5
    break
}

// Fallthrough (no break between case 1 and case 2)
const z = 1
switch (z) {
  case 1:
    total += 1
  case 2:
    total += 2
    break
  case 3:
    total += 3
    break
}

// String switch
const s = 'hello'
switch (s) {
  case 'world':
    total += 100
    break
  case 'hello':
    total += 3
    break
}

// total = 20 + 5 + 1 + 2 + 3 = 31
total
