const re = /^hello/i;
const a = re.test("Hello world");  // true
const b = re.test("world");        // false
const re2 = new RegExp("\\d+");
const c = re2.test("abc123");      // true
a + b + c  // 1+0+1 = 2
