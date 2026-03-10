class Counter {
    count: number;
    constructor(start: number) {
        this.count = start;
    }
    increment() {
        this.count = this.count + 1;
    }
    get(): number {
        return this.count;
    }
}

const c = new Counter(10);
console.log(c.get());
c.increment();
console.log(c.get());
