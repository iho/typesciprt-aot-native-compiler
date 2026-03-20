// @nestjs/core shim — NestFactory and minimal DI container + HTTP server.
// Reads Reflect metadata written by @nestjs/common decorators to wire
// controllers/providers and register HTTP routes.

// ── DI Container ──────────────────────────────────────────────────────────────

function resolveProvider(token: any, instanceCache: any): any {
  if (instanceCache[token]) { return instanceCache[token]; }
  // Get constructor parameter types (emitted by emitDecoratorMetadata).
  const paramTypes: any[] = Reflect.getMetadata("design:paramtypes", token, undefined) || [];
  const injectTokens: any[] = Reflect.getMetadata("inject:tokens", token, undefined) || [];
  const args: any[] = [];
  for (let i = 0; i < paramTypes.length; i++) {
    const actualToken = injectTokens[i] !== undefined ? injectTokens[i] : paramTypes[i];
    if (actualToken && actualToken !== token) {
      args.push(resolveProvider(actualToken, instanceCache));
    } else {
      args.push(undefined);
    }
  }
  // Invoke constructor with resolved args (up to 4 deps supported via ts_func_call4).
  let instance: any;
  const n = args.length;
  if (n === 0) { instance = new token(); }
  else if (n === 1) { instance = new token(args[0]); }
  else if (n === 2) { instance = new token(args[0], args[1]); }
  else if (n === 3) { instance = new token(args[0], args[1], args[2]); }
  else { instance = new token(args[0], args[1], args[2], args[3]); }
  instanceCache[token] = instance;
  return instance;
}

function collectProviders(moduleClass: any, instanceCache: any, all: any[]): void {
  const providers: any[] = Reflect.getMetadata("module:providers", moduleClass, undefined) || [];
  const imports: any[] = Reflect.getMetadata("module:imports", moduleClass, undefined) || [];
  for (const mod of imports) {
    collectProviders(mod, instanceCache, all);
  }
  for (const p of providers) {
    all.push(p);
  }
}

// ── Route matching ───────────────────────────────────────────────────────────
function matchRoute(pattern: string, pathname: string): any {
  if (pattern === "" || pattern === "/") {
    return pathname === "/" || pathname === "" ? {} : null;
  }
  const pParts = pattern.split("/").filter((s: string) => s !== "");
  const uParts = pathname.split("/").filter((s: string) => s !== "");
  if (pParts.length !== uParts.length) { return null; }
  const params: any = {};
  for (let i = 0; i < pParts.length; i++) {
    if (pParts[i].startsWith(":")) {
      params[pParts[i].slice(1)] = uParts[i];
    } else if (pParts[i] !== uParts[i]) {
      return null;
    }
  }
  return params;
}

function joinPaths(a: string, b: string): string {
  const sep = "/";
  const left = a.endsWith(sep) ? a.slice(0, a.length - 1) : a;
  const right = b.startsWith(sep) ? b : sep + b;
  return left + right || sep;
}

// ── NestApplication ──────────────────────────────────────────────────────────

class NestApplication {
  private _instanceCache: any;
  private _routes: any[];
  private _globalPrefix: string;

  constructor(moduleClass: any) {
    this._instanceCache = {};
    this._routes = [];
    this._globalPrefix = "";
    this._bootstrap(moduleClass);
  }

  private _bootstrap(moduleClass: any): void {
    const allProviders: any[] = [];
    collectProviders(moduleClass, this._instanceCache, allProviders);
    // Pre-resolve all providers.
    for (const p of allProviders) {
      resolveProvider(p, this._instanceCache);
    }
    // Wire controllers from this module and imported modules.
    this._wireModule(moduleClass);
  }

  private _wireModule(moduleClass: any): void {
    const imports: any[] = Reflect.getMetadata("module:imports", moduleClass, undefined) || [];
    for (const mod of imports) {
      this._wireModule(mod);
    }
    const controllers: any[] = Reflect.getMetadata("module:controllers", moduleClass, undefined) || [];
    for (const ctrl of controllers) {
      this._wireController(ctrl);
    }
  }

  private _wireController(ctrlClass: any): void {
    const prefix: string = Reflect.getMetadata("controller:prefix", ctrlClass, undefined) || "";
    const instance: any = resolveProvider(ctrlClass, this._instanceCache);
    // In our compiler, method decorators receive `this` (the instance), so route metadata
    // is stored on the instance object, not on prototype.
    const routes: any[] = Reflect.getMetadata("routes", instance, undefined) || [];
    for (const route of routes) {
      const fullPath = joinPaths(prefix, route.path);
      const paramMeta: any[] = Reflect.getMetadata("route:params", instance, undefined) || [];
      const handlerParamMeta = paramMeta.filter((p: any) => p.method === route.handler);
      this._routes.push({
        method: route.method,
        path: fullPath,
        handler: route.handler,
        instance,
        paramMeta: handlerParamMeta,
      });
    }
  }

  get<T>(token: any): T {
    return resolveProvider(token, this._instanceCache) as T;
  }

  use(_middleware: any): this { return this; }
  enableCors(_options?: any): void {}
  setGlobalPrefix(prefix: string): void { this._globalPrefix = prefix; }

  async listen(port: number): Promise<void> {
    const self = this;
    serve(port, async (req: any) => {
      return await self._handleRequest(req);
    });
  }

  private async _handleRequest(req: any): Promise<any> {
    const rawUrl: string = req.url || "/";
    // Strip scheme+host if present: "http://host:port/path?q" → "/path?q"
    let urlPath: string = rawUrl;
    const protoEnd = rawUrl.indexOf("://");
    if (protoEnd >= 0) {
      const slashAfterHost = rawUrl.indexOf("/", protoEnd + 3);
      urlPath = slashAfterHost >= 0 ? rawUrl.slice(slashAfterHost) : "/";
    }
    const qIdx = urlPath.indexOf("?");
    const pathname: string = qIdx >= 0 ? urlPath.slice(0, qIdx) : urlPath;
    const queryStr: string = qIdx >= 0 ? urlPath.slice(qIdx + 1) : "";
    const method: string = req.method || "GET";

    // Resolve global prefix.
    let effectivePath = pathname;
    if (this._globalPrefix && effectivePath.startsWith("/" + this._globalPrefix)) {
      effectivePath = effectivePath.slice(this._globalPrefix.length + 1) || "/";
    }
    for (const route of this._routes) {
      if (route.method !== method && route.method !== "ALL") { continue; }
      const params = matchRoute(route.path, effectivePath);
      if (params === null) { continue; }

      // Extract body if needed.
      let body: any = undefined;
      if (method === "POST" || method === "PUT" || method === "PATCH") {
        try {
          const text = await req.text();
          body = text ? JSON.parse(text) : undefined;
        } catch (_e: any) { body = undefined; }
      }

      // Parse query string.
      const query: any = {};
      if (queryStr) {
        for (const part of queryStr.split("&")) {
          const eqIdx = part.indexOf("=");
          if (eqIdx >= 0) {
            query[decodeURIComponent(part.slice(0, eqIdx))] = decodeURIComponent(part.slice(eqIdx + 1));
          }
        }
      }

      // Build args for handler based on @Param/@Body/@Query decorators.
      let args: any[];
      if (route.paramMeta.length > 0) {
        args = [];
        for (const pm of route.paramMeta) {
          if (pm.type === "param") { args[pm.index] = pm.param ? params[pm.param] : params; }
          else if (pm.type === "body") { args[pm.index] = body; }
          else if (pm.type === "query") { args[pm.index] = pm.param ? query[pm.param] : query; }
          else if (pm.type === "req") { args[pm.index] = req; }
          else { args[pm.index] = undefined; }
        }
      } else {
        args = [];
      }

      try {
        let result: any = route.instance[route.handler](...args);
        if (result && typeof result === "object" && typeof result.then === "function") {
          result = await result;
        }
        const statusCode: number = Reflect.getMetadata("route:httpcode:" + route.handler, route.instance, undefined) || 200;
        if (typeof result === "string") {
          return new Response(result, { status: statusCode, headers: { "Content-Type": "text/plain" } });
        }
        return new Response(JSON.stringify(result), { status: statusCode, headers: { "Content-Type": "application/json" } });
      } catch (e: any) {
        const status = e.getStatus ? e.getStatus() : 500;
        const msg = e.getMessage ? e.getMessage() : "Internal Server Error";
        return new Response(JSON.stringify({ message: msg, statusCode: status }), {
          status,
          headers: { "Content-Type": "application/json" }
        });
      }
    }

    return new Response(JSON.stringify({ message: "Not Found", statusCode: 404 }), {
      status: 404,
      headers: { "Content-Type": "application/json" }
    });
  }
}

// ── NestFactory ───────────────────────────────────────────────────────────────
export class NestFactory {
  static async create(moduleClass: any, _options?: any): Promise<NestApplication> {
    return new NestApplication(moduleClass);
  }
}

export default NestFactory;
