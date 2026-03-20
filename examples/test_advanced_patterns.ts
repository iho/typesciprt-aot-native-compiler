// Advanced patterns needed for real-world frameworks

// ── 1. Array.from with various inputs ─────────────────────────────────────────
const arr1 = Array.from([1, 2, 3]);
console.log("from array:", arr1.join(","));  // 1,2,3

const arr2 = Array.from({length: 3}, (_: any, i: number) => i * 2);
console.log("from length:", arr2.join(","));  // 0,2,4

const set = new Set([10, 20, 30]);
const arr3 = Array.from(set);
console.log("from set:", arr3.join(","));  // 10,20,30

// ── 2. Object.getOwnPropertyNames ─────────────────────────────────────────────
const obj = { a: 1, b: 2 };
const names = Object.getOwnPropertyNames(obj);
console.log("own names len:", names.length);  // 2

// ── 3. Computed property names ────────────────────────────────────────────────
const key = "dynamic";
const obj2 = { [key]: "value", static: "other" };
console.log("computed:", obj2.dynamic);  // value
console.log("static:", obj2.static);    // other

// ── 4. Property shorthand in objects ─────────────────────────────────────────
const x = 10, y = 20;
const point = { x, y };
console.log("shorthand:", point.x + "," + point.y);  // 10,20

// ── 5. Default parameters ────────────────────────────────────────────────────
function greet(name: string = "World"): string {
  return "Hello, " + name + "!";
}
console.log("default:", greet());          // Hello, World!
console.log("with arg:", greet("Alice"));  // Hello, Alice!

// ── 6. Rest and spread in various positions ───────────────────────────────────
function sum(...nums: number[]): number {
  return nums.reduce((acc: number, n: number) => acc + n, 0);
}
console.log("rest sum:", sum(1, 2, 3, 4, 5));  // 15

const nums = [1, 2, 3];
console.log("spread sum:", sum(...nums));  // 6

// ── 7. Nullish coalescing assignment ─────────────────────────────────────────
let val: number | null = null;
val ??= 42;
console.log("nullish assign:", val);  // 42

// ── 8. Optional chaining with method calls ───────────────────────────────────
const maybeObj: any = null;
const result = maybeObj?.toString() ?? "none";
console.log("optional call:", result);  // none

// ── 9. Array.isArray in conditions ───────────────────────────────────────────
function processValue(v: any): string {
  if (Array.isArray(v)) return "array:" + v.join(",");
  if (typeof v === "string") return "string:" + v;
  return "other";
}
console.log("array:", processValue([1, 2, 3]));  // array:1,2,3
console.log("string:", processValue("hello"));   // string:hello
console.log("other:", processValue(42));          // other

// ── 10. Class with static and instance interaction ───────────────────────────
class EventEmitter {
  private listeners: Map<string, Function[]> = new Map();

  on(event: string, fn: Function): void {
    const existing = this.listeners.get(event) || [];
    existing.push(fn);
    this.listeners.set(event, existing);
  }

  emit(event: string, ...args: any[]): void {
    const fns = this.listeners.get(event) || [];
    fns.forEach((fn: Function) => fn(...args));
  }
}

const ee = new EventEmitter();
const results: string[] = [];
ee.on("data", (v: string) => results.push(v));
ee.emit("data", "hello");
ee.emit("data", "world");
console.log("events:", results.join(","));  // hello,world

// ── 11. String template literals ─────────────────────────────────────────────
const name = "TypeScript";
const version = 5;
console.log("template:", `${name} v${version}`);  // TypeScript v5

// ── 12. Destructuring with defaults ──────────────────────────────────────────
const { a = 1, b: renamed = 2 } = {} as any;
console.log("destruct defaults:", a, renamed);  // 1 2

console.log("advanced done");
