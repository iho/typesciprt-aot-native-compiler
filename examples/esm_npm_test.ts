// Test npm package resolution: ESM package resolved via "exports" field in package.json.
import { greet, PI } from 'test-esm-pkg';

const msg = greet("world");
console.log(msg);        // Hello, world!
console.log("PI:", PI);  // PI: 3.14159
