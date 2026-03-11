// Default parameter values

// Regular function with defaults
function greet(name: string, greeting: string = "Hello") {
  return greeting.length + name.length;
}

// Called with both args → no defaults used
const r1 = greet("World", "Hi");   // "Hi"(2) + "World"(5) = 7

// Called with one arg → greeting defaults to "Hello"
const r2 = greet("Bob");           // "Hello"(5) + "Bob"(3) = 8

// Arrow function with default (handled in lower_arrow_like)
const add = (a: number, b: number = 10) => a + b;
const r3 = add(5, 3);   // 5 + 3 = 8
const r4 = add(5);      // 5 + 10 = 15

// exit: r1(7) + r2(8) + r3(8) + r4(15) - 38 = 0
// Let's use r1+r4 = 22
r1 + r4;
