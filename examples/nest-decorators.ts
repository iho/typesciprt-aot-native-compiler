/**
 * Minimal NestJS-compatible decorator shim.
 *
 * Uses the native Reflect API (provided by ts-runtime) instead of reflect-metadata.
 * Decorator semantics match @nestjs/common closely enough for bootstrapNative to work.
 */

// ── Metadata keys (same as @nestjs/common constants) ─────────────────────────

const MODULE_METADATA       = 'modules'
const CONTROLLER_WATERMARK  = '__controller__'
const INJECTABLE_WATERMARK  = '__injectable__'
const PATH_METADATA         = 'path'
const METHOD_METADATA       = 'method'

// ── HTTP method enum ──────────────────────────────────────────────────────────

export const RequestMethod = {
  GET:     0,
  POST:    1,
  PUT:     2,
  DELETE:  3,
  PATCH:   4,
  OPTIONS: 5,
  HEAD:    6,
  ALL:     7,
}

// ── Class decorators ──────────────────────────────────────────────────────────

export function Module(metadata: any): any {
  return function(target: any): any {
    Reflect.defineMetadata(MODULE_METADATA, metadata, target, undefined)
    return target
  }
}

export function Injectable(): any {
  return function(target: any): any {
    Reflect.defineMetadata(INJECTABLE_WATERMARK, true, target, undefined)
    return target
  }
}

export function Controller(prefix?: string): any {
  return function(target: any): any {
    Reflect.defineMetadata(CONTROLLER_WATERMARK, true, target, undefined)
    Reflect.defineMetadata(PATH_METADATA, prefix || '', target, undefined)
    return target
  }
}

// ── Method decorators ─────────────────────────────────────────────────────────
// The third argument to a method decorator is the PropertyDescriptor.
// We call Reflect.defineMetadata on descriptor.value (the method function itself).

function createMethodDecorator(httpMethod: number): any {
  return function(path?: string): any {
    return function(target: any, key: string, descriptor: any): any {
      const fn = descriptor.value
      Reflect.defineMetadata(PATH_METADATA,   path || '', fn, undefined)
      Reflect.defineMetadata(METHOD_METADATA, httpMethod,  fn, undefined)
      return descriptor
    }
  }
}

export const Get     = createMethodDecorator(RequestMethod.GET)
export const Post    = createMethodDecorator(RequestMethod.POST)
export const Put     = createMethodDecorator(RequestMethod.PUT)
export const Delete  = createMethodDecorator(RequestMethod.DELETE)
export const Patch   = createMethodDecorator(RequestMethod.PATCH)
export const Options = createMethodDecorator(RequestMethod.OPTIONS)
export const Head    = createMethodDecorator(RequestMethod.HEAD)
export const All     = createMethodDecorator(RequestMethod.ALL)
