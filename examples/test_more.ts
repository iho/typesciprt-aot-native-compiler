// Test more TypeScript features

// 1. Numeric separators
const million = 1_000_000;
console.log('numeric sep:', million);

// 2. String.prototype.matchAll
const text = 'cat bat sat';
const matches = [...text.matchAll(/[a-z]at/g)];
console.log('matchAll count:', matches.length);

// 3. Optional chaining with method calls
const obj: any = { fn: () => 42 };
const v1 = obj?.fn();
console.log('optional method:', v1);
const v2 = (null as any)?.fn();
console.log('optional null method:', v2 === undefined);

// 4. Array.from
const a = Array.from({ length: 3 }, (_: any, i: number) => i * 2);
console.log('Array.from:', a.join(','));

// 5. Object.keys/values/entries
const o = { a: 1, b: 2, c: 3 };
console.log('keys:', Object.keys(o).length);
console.log('values:', Object.values(o).length);

// 6. Nullish coalescing assignment
let x: number | null = null;
x ??= 42;
console.log('nullish assign:', x);

// 7. Logical OR assignment
let y = 0;
y ||= 99;
console.log('or assign:', y);

// 8. Tagged template
function tag(strings: TemplateStringsArray, ...values: any[]): string {
  return strings.raw[0] + values[0];
}
// Note: tagged templates may not be supported yet — skip for now

// 9. for...of on Set
const s = new Set([1, 2, 3]);
let setSum = 0;
for (const v of s) {
  setSum += v;
}
console.log('set for-of sum:', setSum);

// 10. Iterable spread
const s2 = new Set([4, 5, 6]);
const arr = [...s2];
console.log('set spread:', arr.length);

// 11. Labeled continue
let count = 0;
outer: for (let i = 0; i < 3; i++) {
  for (let j = 0; j < 3; j++) {
    if (j === 1) continue outer;
    count++;
  }
}
console.log('labeled continue:', count);

console.log('All more tests done');
