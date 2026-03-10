// ── typeof ────────────────────────────────────────────────────────────────────
let n: number = 42;
let s: string = "hello";
let b: boolean = true;

let t_num  = typeof n === "number";    // true (1)
let t_str  = typeof s === "string";    // true (1)
let t_bool = typeof b === "boolean";   // true (1)
let t_wrong = typeof n === "string";   // false (0)

// ── instanceof ────────────────────────────────────────────────────────────────
class Shape {
    sides: number;
    constructor(sides: number) {
        this.sides = sides;
    }
}

class Triangle extends Shape {
    constructor() {
        super(3);
    }
}

let tri = new Triangle();
let shp = new Shape(4);

let tri_is_triangle  = tri instanceof Triangle;  // true  (1)
let tri_is_shape     = tri instanceof Shape;     // true  (1) via inheritance
let shp_is_shape     = shp instanceof Shape;     // true  (1)
let shp_is_triangle  = shp instanceof Triangle;  // false (0)

// Exit code: 1+1+1+0 + 1+1+1+0 = 6
t_num + t_str + t_bool + t_wrong + tri_is_triangle + tri_is_shape + shp_is_shape + shp_is_triangle
