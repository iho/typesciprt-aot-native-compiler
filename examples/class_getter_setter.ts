// Class with getter/setter and private constructor parameter shorthand
class Circle {
  private _radius: number;
  constructor(radius: number) { this._radius = radius; }
  get area() { return Math.PI * this._radius * this._radius; }
  set radius(r: number) { this._radius = r; }
}

const c = new Circle(5);
const area1 = Math.round(c.area);  // ~79
c.radius = 10;
const area2 = Math.round(c.area);  // ~314

// Virtual method dispatch: parent's describe() calls overridden child's area()
class Shape {
  name: string;
  constructor(name: string) { this.name = name; }
  area(): number { return 0; }
  describe(): string { return `${this.name} with area ${Math.round(this.area())}`; }
}

class Rectangle extends Shape {
  constructor(private w: number, private h: number) {
    super("Rectangle");
  }
  area(): number { return this.w * this.h; }
}

const rect = new Rectangle(4, 6);
const desc = rect.describe();
const correctDesc = desc === "Rectangle with area 24";

process.exit(area1 + area2 + (correctDesc ? 1 : 0));
// 79 + 314 + 1 = 394 → exit code 138 (= 394 mod 256)
