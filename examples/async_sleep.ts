// Tests real Tokio-backed sleep: the result of the async function is 7.
async function delayedAdd(a: number, b: number): Promise<number> {
  await sleep(10);
  return a + b;
}

const result = await delayedAdd(3, 4);
result; // exit code 7
