// ── Base class ────────────────────────────────────────────────────────────────
class Animal {
    age: number;

    constructor(age: number) {
        this.age = age;
    }

    getAge(): number {
        return this.age;
    }

    static classify(n: number): number {
        return n * 2;
    }
}

// ── Derived class with super() ────────────────────────────────────────────────
class Dog extends Animal {
    #name: number;   // use number to keep it simple (no string type inference needed)

    constructor(nameCode: number, age: number) {
        super(age);
        this.#name = nameCode;
    }

    getCode(): number {
        return this.#name;
    }

    // Inherited getAge() comes from Animal

    // super.getAge() call
    describe(): number {
        return super.getAge();
    }

    // Getter / setter
    get code(): number {
        return this.#name;
    }

    set code(v: number) {
        this.#name = v;
    }
}

// ── Exercise ──────────────────────────────────────────────────────────────────
let d = new Dog(5, 3);

let age = d.getAge();       // inherited → 3
let code = d.getCode();     // private field → 5
let desc = d.describe();    // super.getAge() → 3
let gotten = d.code;        // getter → 5
d.code = 10;
let after = d.code;         // setter + getter → 10

// Static method
let doubled = Animal.classify(6);  // 6 * 2 = 12

// age(3) + code(5) + desc(3) + gotten(5) + after(10) + doubled(12) = 38
age + code + desc + gotten + after + doubled
