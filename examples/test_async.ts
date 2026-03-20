// Test async/await patterns

// 1. Basic async/await
async function fetchValue(n: number): Promise<number> {
  return n * 2;
}

async function main() {
  const v = await fetchValue(21);
  console.log('basic await:', v);

  // 2. Async in loop
  let sum = 0;
  for (let i = 1; i <= 5; i++) {
    sum += await fetchValue(i);
  }
  console.log('async loop sum:', sum);

  // 3. Promise chaining
  const result = await Promise.resolve(10)
    .then((x: number) => x + 5)
    .then((x: number) => x * 2);
  console.log('promise chain:', result);

  // 4. Error handling
  async function failingFn(): Promise<string> {
    throw new Error('expected error');
  }
  let caught = false;
  try {
    await failingFn();
  } catch (e: any) {
    caught = true;
    console.log('async error caught:', e.message);
  }
  console.log('error handled:', caught);

  // 5. Promise.all with dynamic values
  const inputs = [1, 2, 3, 4, 5];
  const doubled = await Promise.all(inputs.map((x: number) => fetchValue(x)));
  console.log('parallel map sum:', doubled.reduce((a: number, b: number) => a + b, 0));
}

main();
