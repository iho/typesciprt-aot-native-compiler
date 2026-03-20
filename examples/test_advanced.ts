// Test advanced features

// 1. Symbol
const sym = Symbol('test');
console.log('symbol type:', typeof sym === 'symbol');

// 2. Map iteration
const m = new Map<string, number>();
m.set('a', 1);
m.set('b', 2);
m.set('c', 3);
let sum = 0;
for (const [k, v] of m.entries()) {
  sum += v;
}
console.log('map sum:', sum);

// 3. Set
const s = new Set<number>([1, 2, 3, 2, 1]);
console.log('set size:', s.size);
s.add(4);
console.log('set has 4:', s.has(4));
console.log('set has 5:', s.has(5));

// 4. WeakMap
const wm = new WeakMap();
const key = {};
wm.set(key, 'value');
console.log('weakmap has:', wm.has(key));

// 5. Promise.all
async function runAll() {
  const results = await Promise.all([
    Promise.resolve(1),
    Promise.resolve(2),
    Promise.resolve(3),
  ]);
  console.log('Promise.all:', results[0] + results[1] + results[2]);
}
runAll();

// 6. String methods
const str = '  hello world  ';
console.log('trim:', str.trim());
console.log('toUpperCase:', 'hello'.toUpperCase());
console.log('split:', 'a,b,c'.split(',').length);
console.log('includes:', 'hello world'.includes('world'));
console.log('slice:', 'hello world'.slice(6));

// 7. Array spread+destructure
const [first, ...rest] = [1, 2, 3, 4, 5];
console.log('first:', first);
console.log('rest len:', rest.length);

// 8. Object spread
const base = { a: 1, b: 2 };
const extended = { ...base, c: 3 };
console.log('spread obj:', extended.c);

// 9. Conditional type narrowing
function test(x: string | number): string {
  if (typeof x === 'string') return x.toUpperCase();
  return String(x * 2);
}
console.log('narrowing str:', test('hello'));
console.log('narrowing num:', test(21));

console.log('All advanced tests started');
