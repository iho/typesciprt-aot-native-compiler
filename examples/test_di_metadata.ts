// Test emitDecoratorMetadata: design:paramtypes for DI

function Injectable(): (target: any) => void {
  return function(target: any) {
    Reflect.defineMetadata("injectable", true, target);
  };
}

@Injectable()
class UserRepository {
  find(): string { return "users"; }
}

@Injectable()
class UserService {
  constructor(private repo: UserRepository) {}
  getUsers(): string { return this.repo.find(); }
}

@Injectable()
class AppController {
  constructor(
    private userService: UserService,
    private repo: UserRepository,
  ) {}
  run(): string {
    return this.userService.getUsers() + ":" + this.repo.find();
  }
}

// Check design:paramtypes metadata
const serviceTypes = Reflect.getMetadata("design:paramtypes", UserService);
const controllerTypes = Reflect.getMetadata("design:paramtypes", AppController);

// UserService has one param: UserRepository
if (serviceTypes && serviceTypes.length === 1) {
  const isRepo = serviceTypes[0] === UserRepository;
  console.log("UserService paramtypes[0] is UserRepository:", isRepo);  // true
} else {
  console.log("UserService paramtypes: missing or wrong length");
}

// AppController has two params: UserService, UserRepository
if (controllerTypes && controllerTypes.length === 2) {
  const isSvc = controllerTypes[0] === UserService;
  const isRepo = controllerTypes[1] === UserRepository;
  console.log("AppController paramtypes[0] is UserService:", isSvc);    // true
  console.log("AppController paramtypes[1] is UserRepository:", isRepo); // true
} else {
  console.log("AppController paramtypes: missing or wrong length");
}

// Simulate simple DI container
function resolve<T>(cls: any): T {
  const paramTypes = Reflect.getMetadata("design:paramtypes", cls) || [];
  const deps = paramTypes.map((DepCls: any) => resolve(DepCls));
  // Simple instantiation — spread deps as constructor args
  if (deps.length === 0) return new cls();
  if (deps.length === 1) return new cls(deps[0]);
  if (deps.length === 2) return new cls(deps[0], deps[1]);
  return new cls();
}

const controller = resolve<AppController>(AppController);
console.log("DI result:", controller.run());  // users:users

console.log("di metadata done");
