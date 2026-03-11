const opts: any = { strict: true, a: 1, b: 2 };
const { strict, ...rest } = opts;
rest.a + rest.b  // 3
