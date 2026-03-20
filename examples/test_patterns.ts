// Test various TypeScript patterns

// 1. Labeled break
outer: for (let i = 0; i < 3; i++) {
  for (let j = 0; j < 3; j++) {
    if (i === 1 && j === 1) break outer;
  }
}
console.log('labeled break: OK');

// 2. Optional chaining on method calls
const obj: any = { nested: { value: 42 } };
const v = obj?.nested?.value;
console.log('optional chain:', v);

// 3. Nullish coalescing
const x = null ?? 'default';
console.log('nullish:', x);

// 4. Array destructuring in assignment
let a: number, b: number;
[a, b] = [10, 20];
console.log('array destruct assign:', a, b);

// 5. Object destructuring in assignment
let c: number, d: number;
({ c, d } = { c: 30, d: 40 });
console.log('object destruct assign:', c, d);

// 6. Spread in array literal
const arr1 = [1, 2, 3];
const arr2 = [...arr1, 4, 5];
console.log('spread array:', arr2.length);

// 7. Computed property key
const key = 'hello';
const computed = { [key]: 'world' };
console.log('computed key:', computed.hello);

// 8. String template with expression
const name = 'world';
const tmpl = `Hello ${name}!`;
console.log('template:', tmpl);

// 9. typeof check
console.log('typeof:', typeof 42 === 'number');

// 10. Comma operator
let e = (1, 2, 3);
console.log('comma op:', e);

console.log('All patterns OK');
