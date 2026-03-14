// Regression test: user-class .add(method, path, handler) must NOT be routed
// through ts_container_add (which only accepts 1 arg).  Previously, any call
// to .add() was treated as a Set/WeakSet container operation.
class Router {
  private routes: string[] = [];

  add(method: string, path: string, handler: string): number {
    this.routes.push(method + ':' + path + ':' + handler);
    return this.routes.length;
  }

  count(): number {
    return this.routes.length;
  }
}

const router = new Router();
router.add("GET", "/", "home");
router.add("POST", "/users", "createUser");
router.add("GET", "/users/:id", "getUser");
router.count() // 3
