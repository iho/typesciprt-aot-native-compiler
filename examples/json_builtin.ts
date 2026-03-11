const obj: any = { a: 1, b: "hello" };
const s = JSON.stringify(obj);
const parsed: any = JSON.parse(s);
parsed.a  // 1
