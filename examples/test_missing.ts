// Test exponentiation
const x = 2 ** 10;
console.log('2**10:', x);

// Bitwise assignments
let a = 0xFF;
a &= 0x0F;
console.log('&=:', a);
a |= 0xF0;
console.log('|=:', a);
a ^= 0xAA;
console.log('^=:', a);
a <<= 2;
console.log('<<=:', a);
a >>= 1;
console.log('>>=:', a);

// Array.at()
const arr = [1, 2, 3, 4, 5];
console.log('at(-1):', arr.at(-1));
console.log('at(0):', arr.at(0));

// Object.getOwnPropertyNames
const obj = { a: 1, b: 2 };
const keys = Object.getOwnPropertyNames(obj);
console.log('getOwnPropertyNames:', keys.length);

// String.prototype.at()
const str = 'hello';
console.log('str.at(-1):', str.at(-1));
