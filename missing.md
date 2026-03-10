 ---
  Missing language features

  Control flow

  - switch / case — no support
  - try / catch / finally — no error handling
  - throw — no exceptions
  - do...while — only while and for are supported
  - Labeled statements (outer: for ...) — ignored

  Functions & closures

  - Arrow functions (() => {}, x => x + 1) — fall into the catch-all, silently return nothing
  - Closures / upvalue capture — variables from outer scopes can't be captured
  - Default parameters (function f(x = 0)) — default values are ignored
  - Rest parameters (...args) — ignored
  - Spread arguments (f(...arr)) — explicitly errored
  - Function overloads — TypeScript overload signatures are ignored
  - Generator functions (function*, yield) — not supported

  Types & values

  - null / undefined literals — no runtime representation (undefined partially exists as a TsVal constant)
  - Template literals (`hello ${name}`) — fall into catch-all
  - Regular expressions — not supported
  - Symbol — not supported
  - BigInt — not supported
  - Floating-point numbers — all numbers are i32 (no f64)

  Classes

  - Class inheritance (extends) — not implemented
  - super calls — not supported
  - static methods/properties — ignored
  - Getters / setters (get foo(), set foo(v)) — ignored
  - Private fields (#field) — not supported
  - Decorators — parsed but not applied

  Modules

  - Circular imports — would cause infinite recursion
  - Re-exports (export { foo } from './bar') — source is ignored
  - import * as ns — namespace import not bound
  - import defaultExport from — default import bound by name but export side not tracked
  - External/npm modules — silently skipped (no stubs)

  Built-in APIs

  - Array methods (.map, .filter, .reduce, .push, .pop, etc.) — none
  - String methods (.slice, .indexOf, .split, etc.) — none
  - Math object — no math functions
  - JSON — no serialization
  - console — only console.log with one argument
  - process.exit — not implemented
  - setTimeout / Promise.resolve / etc. — no real async I/O

  Type system

  - Generic type parameters — erased but not validated
  - Union / intersection types — erased
  - Mapped types, conditional types — erased
  - typeof / instanceof operators — typeof is in the expression catch-all; instanceof likewise
  - Type narrowing — no runtime effect anyway

  Other

  - Destructuring (const { a, b } = obj, const [x, y] = arr) — variable binding patterns outside BindingIdentifier are silently skipped
  - Optional chaining (obj?.prop) — falls into catch-all
  - Nullish coalescing (a ?? b) — falls into catch-all
  - Computed property names ({ [key]: value }) — object literal key must be a static identifier
  - for...of / for...in — not supported
  - typeof expression — returns nothing (catch-all)

  The highest-impact items to implement next would be: arrow functions, null/undefined, destructuring, for...of, template literals, and Array/String built-in methods.
