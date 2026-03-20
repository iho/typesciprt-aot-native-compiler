// Test class features

// 1. Basic inheritance + super
class Animal {
  name: string;
  constructor(name: string) {
    this.name = name;
  }
  speak(): string {
    return `${this.name} makes a sound`;
  }
}

class Dog extends Animal {
  constructor(name: string) {
    super(name);
  }
  speak(): string {
    return `${this.name} barks`;
  }
}

const dog = new Dog('Rex');
console.log('inheritance:', dog.speak());
console.log('instanceof:', dog instanceof Dog);

// 2. Static methods + fields
class Counter {
  static count: number = 0;
  constructor() {
    Counter.count++;
  }
  static getCount(): number {
    return Counter.count;
  }
}
new Counter();
new Counter();
new Counter();
console.log('static count:', Counter.getCount());

// 3. Getters/setters
class Circle {
  #radius: number;
  constructor(r: number) {
    this.#radius = r;
  }
  get radius(): number { return this.#radius; }
  set radius(r: number) { this.#radius = r; }
  get area(): number { return Math.PI * this.#radius * this.#radius; }
}
const c = new Circle(5);
console.log('getter:', c.radius);
c.radius = 10;
console.log('setter:', c.radius);
console.log('area check:', c.area > 300);

// 4. try/catch/finally
let result = '';
try {
  throw new Error('test error');
} catch (e: any) {
  result = e.message;
} finally {
  result += ' (caught)';
}
console.log('try/catch:', result);

// 5. Generator
function* range(n: number) {
  for (let i = 0; i < n; i++) yield i;
}
const nums = [...range(5)];
console.log('generator:', nums.join(','));

console.log('All class tests OK');
