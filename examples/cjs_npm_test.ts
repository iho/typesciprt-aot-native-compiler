// Test npm package resolution: CJS package from node_modules.
// The CJS package exports add(), multiply(), and VERSION.
import { add, multiply, VERSION } from 'test-cjs-pkg';

const sum = add(3, 4);
console.log("add(3, 4) =", sum);          // 7

const product = multiply(5, 6);
console.log("multiply(5, 6) =", product); // 30

console.log("version:", VERSION);         // 1.0.0
