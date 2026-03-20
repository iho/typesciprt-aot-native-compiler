// Test resolve() function with dynamic dispatch

function Injectable(): (t: any) => void {
  return (t: any) => { Reflect.defineMetadata("injectable", true, t); };
}

@Injectable()
class Repo {
  find(): string { return "data"; }
}

@Injectable()
class Service {
  constructor(private repo: Repo) {}
  get(): string { return this.repo.find(); }
}

function resolve(cls: any): any {
  const paramTypes = Reflect.getMetadata("design:paramtypes", cls) || [];
  console.log("resolving, paramTypes.length:", paramTypes.length);
  if (paramTypes.length === 0) {
    console.log("new with 0 args");
    return new cls();
  }
  if (paramTypes.length === 1) {
    const dep = resolve(paramTypes[0]);
    console.log("dep resolved:", dep !== undefined);
    return new cls(dep);
  }
  return new cls();
}

const svc = resolve(Service);
console.log("svc:", svc !== undefined);  // true
console.log("svc.get():", svc.get());   // data

console.log("resolve done");
