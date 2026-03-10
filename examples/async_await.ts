// async/await example

async function fetchNumber(): Promise<number> {
    return 42;
}

async function add(a: number, b: number): Promise<number> {
    return a + b;
}

async function run() {
    const x = await fetchNumber();
    console.log(x);

    const sum = await add(10, 32);
    console.log(sum);
}

await run();
