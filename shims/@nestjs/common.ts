// @nestjs/common shim — NestJS decorator metadata API.
// Decorators store metadata on class constructors and methods via Reflect.
// The DI container in @nestjs/core reads this metadata to wire dependencies.

// ── Injectable ───────────────────────────────────────────────────────────────
export function Injectable(): (target: any) => void {
  return function(target: any): void {
    Reflect.defineMetadata("injectable", true, target, undefined);
  };
}

// ── Controller ───────────────────────────────────────────────────────────────
export function Controller(prefix?: string): (target: any) => void {
  return function(target: any): void {
    Reflect.defineMetadata("controller:prefix", prefix || "", target, undefined);
    Reflect.defineMetadata("injectable", true, target, undefined);
  };
}

// ── Module ────────────────────────────────────────────────────────────────────
export interface ModuleMetadata {
  imports?: any[];
  controllers?: any[];
  providers?: any[];
  exports?: any[];
}

export function Module(metadata: ModuleMetadata): (target: any) => void {
  return function(target: any): void {
    Reflect.defineMetadata("module:controllers", metadata.controllers || [], target, undefined);
    Reflect.defineMetadata("module:providers",   metadata.providers   || [], target, undefined);
    Reflect.defineMetadata("module:imports",     metadata.imports     || [], target, undefined);
    Reflect.defineMetadata("module:exports",     metadata.exports     || [], target, undefined);
  };
}

// ── HTTP method decorators ────────────────────────────────────────────────────
function makeRouteDecorator(method: string) {
  return function(path?: string): (target: any, key: string, _desc: any) => void {
    return function(target: any, key: string, _desc: any): void {
      const routes = Reflect.getMetadata("routes", target, undefined) || [];
      routes.push({ method, path: path || "", handler: key });
      Reflect.defineMetadata("routes", routes, target, undefined);
    };
  };
}

export const Get    = makeRouteDecorator("GET");
export const Post   = makeRouteDecorator("POST");
export const Put    = makeRouteDecorator("PUT");
export const Delete = makeRouteDecorator("DELETE");
export const Patch  = makeRouteDecorator("PATCH");
export const All    = makeRouteDecorator("ALL");
export const Head   = makeRouteDecorator("HEAD");
export const Options = makeRouteDecorator("OPTIONS");

// ── Param / Body / Query / Headers decorators (mark for extraction) ───────────
function makeParamDecorator(type: string) {
  return function(param?: string): (target: any, key: string, index: number) => void {
    return function(target: any, key: string, index: number): void {
      const params = Reflect.getMetadata("route:params", target, undefined) || [];
      params.push({ type, param: param || "", index, method: key });
      Reflect.defineMetadata("route:params", params, target, undefined);
    };
  };
}

export const Param   = makeParamDecorator("param");
export const Body    = makeParamDecorator("body");
export const Query   = makeParamDecorator("query");
export const Headers = makeParamDecorator("headers");
export const Req     = makeParamDecorator("req");
export const Res     = makeParamDecorator("res");
export const Request  = makeParamDecorator("req");
export const Response = makeParamDecorator("res");

// ── HttpCode ──────────────────────────────────────────────────────────────────
export function HttpCode(code: number): (target: any, key: string, _desc: any) => void {
  return function(target: any, key: string, _desc: any): void {
    Reflect.defineMetadata("route:httpcode:" + key, code, target, undefined);
  };
}

// ── UseGuards / UseInterceptors / UsePipes (no-op stubs) ─────────────────────
export function UseGuards(..._guards: any[]): any { return function(_t: any, _k?: string, _d?: any): void {}; }
export function UseInterceptors(..._i: any[]): any { return function(_t: any, _k?: string, _d?: any): void {}; }
export function UsePipes(..._p: any[]): any { return function(_t: any, _k?: string, _d?: any): void {}; }
export function UseFilters(..._f: any[]): any { return function(_t: any, _k?: string, _d?: any): void {}; }

// ── SetMetadata ───────────────────────────────────────────────────────────────
export function SetMetadata(key: string, value: any): any {
  return function(target: any, prop?: string, _desc?: any): void {
    Reflect.defineMetadata(key, value, target, undefined);
  };
}

// ── Inject ────────────────────────────────────────────────────────────────────
export function Inject(token?: any): (target: any, _key: string | undefined, index: number) => void {
  return function(target: any, _key: string | undefined, index: number): void {
    const tokens = Reflect.getMetadata("inject:tokens", target, undefined) || [];
    tokens[index] = token;
    Reflect.defineMetadata("inject:tokens", tokens, target, undefined);
  };
}

// ── Optional ──────────────────────────────────────────────────────────────────
export function Optional(): any { return function(): void {}; }

// ── HttpException ──────────────────────────────────────────────────────────────
export class HttpException {
  private _message: string;
  private _status: number;
  constructor(message: string, status: number) {
    this._message = message;
    this._status = status;
  }
  getMessage(): string { return this._message; }
  getStatus(): number { return this._status; }
}

export class BadRequestException extends HttpException {
  constructor(msg?: string) { super(msg || "Bad Request", 400); }
}
export class UnauthorizedException extends HttpException {
  constructor(msg?: string) { super(msg || "Unauthorized", 401); }
}
export class ForbiddenException extends HttpException {
  constructor(msg?: string) { super(msg || "Forbidden", 403); }
}
export class NotFoundException extends HttpException {
  constructor(msg?: string) { super(msg || "Not Found", 404); }
}
export class ConflictException extends HttpException {
  constructor(msg?: string) { super(msg || "Conflict", 409); }
}
export class InternalServerErrorException extends HttpException {
  constructor(msg?: string) { super(msg || "Internal Server Error", 500); }
}

// ── Logger ────────────────────────────────────────────────────────────────────
export class Logger {
  private _ctx: string;
  constructor(ctx?: string) { this._ctx = ctx || ""; }
  log(msg: string): void { console.log("[" + this._ctx + "] " + msg); }
  warn(msg: string): void { console.log("[WARN][" + this._ctx + "] " + msg); }
  error(msg: string): void { console.log("[ERROR][" + this._ctx + "] " + msg); }
  debug(msg: string): void { console.log("[DEBUG][" + this._ctx + "] " + msg); }
  verbose(msg: string): void { console.log("[VERBOSE][" + this._ctx + "] " + msg); }
}

// ── Type alias helpers ────────────────────────────────────────────────────────
export type Type<T = any> = new (...args: any[]) => T;
export interface INestApplication {
  listen(port: number): Promise<void>;
  get<T>(token: any): T;
  use(...args: any[]): this;
  enableCors(options?: any): void;
  setGlobalPrefix(prefix: string): void;
}
