// Tests Promise.race via select(): fast (10ms) beats slow (500ms).
async function fast(): Promise<number> {
  await sleep(10);
  return 1;
}

async function slow(): Promise<number> {
  await sleep(500);
  return 2;
}

const winner = await select(fast(), slow());
winner; // exit code 1
