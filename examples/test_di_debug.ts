// Debug DI

function Injectable(): (target: any) => void {
  return function(target: any) {
    Reflect.defineMetadata("injectable", true, target);
  };
}

@Injectable()
class Engine {
  start(): string { return "vroom"; }
}

@Injectable()
class Car {
  constructor(private engine: Engine) {}
  drive(): string { return this.engine.start(); }
}

// Check basic construction with parameter properties
const eng = new Engine();
console.log("engine start:", eng.start());  // vroom

const car1 = new Car(eng);
console.log("car drive:", car1.drive());  // vroom

// Check paramtypes
const types = Reflect.getMetadata("design:paramtypes", Car);
console.log("types defined:", types !== undefined);  // true
console.log("types length:", types ? types.length : -1);  // 1
console.log("types[0] === Engine:", types ? (types[0] === Engine) : false);  // true

// DI resolution
const resolvedEngine = new Engine();
const resolvedCar = new Car(resolvedEngine);
console.log("resolved car:", resolvedCar.drive());  // vroom

console.log("debug done");
