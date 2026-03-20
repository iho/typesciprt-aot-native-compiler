/**
 * Native bootstrap for real @nestjs/common decorators.
 *
 * Works with the actual NestJS metadata keys written by @Module, @Controller,
 * @Injectable, and @Get/@Post/etc. from @nestjs/common. Replaces @nestjs/core's
 * NestFactory.create() + app.listen() using the built-in serve() function.
 */

// Metadata keys — must match constants.ts in @nestjs/common exactly.
const CONTROLLER_WATERMARK = '__controller__'
const PATH_METADATA        = 'path'
const METHOD_METADATA      = 'method'

const HTTP_METHODS = ['GET', 'POST', 'PUT', 'DELETE', 'PATCH', 'ALL', 'OPTIONS', 'HEAD']

function methodNameFromCode(code: number): string {
  return HTTP_METHODS[code] || 'GET'
}

function pathMatches(pattern: string, pathname: string): boolean {
  if (pattern === pathname) return true
  const a = pattern.endsWith('/') ? pattern.slice(0, -1) : pattern
  const b = pathname.endsWith('/') ? pathname.slice(0, -1) : pathname
  return a === b
}

export async function bootstrapReal(AppModule: any, port: number): Promise<void> {
  // Real @nestjs/common @Module stores each key separately:
  // Reflect.defineMetadata('controllers', [...], AppModule)
  // Reflect.defineMetadata('providers',   [...], AppModule)
  const controllers: any[] = Reflect.getMetadata('controllers', AppModule, undefined) || []
  const providers: any[]   = Reflect.getMetadata('providers', AppModule, undefined) || []

  // Instantiate providers
  const providerMap: any = {}
  for (const Provider of providers) {
    const name = Provider.name || String(Provider)
    providerMap[name] = new Provider()
  }

  // Build route table by scanning controller instances
  const routes: any[] = []

  for (const ControllerClass of controllers) {
    const isController = Reflect.getMetadata(CONTROLLER_WATERMARK, ControllerClass, undefined)
    if (!isController) continue

    const prefix: string = Reflect.getMetadata(PATH_METADATA, ControllerClass, undefined) || ''
    const instance: any = new ControllerClass()

    // Scan own method names on the instance
    const methodNames: string[] = Object.getOwnPropertyNames(instance)
    for (const name of methodNames) {
      if (name === 'constructor') continue
      const fn: any = instance[name]
      if (typeof fn !== 'function') continue

      // Real NestJS method decorators set metadata on descriptor.value (the fn itself)
      const routePath   = Reflect.getMetadata(PATH_METADATA,   fn, undefined)
      const routeMethod = Reflect.getMetadata(METHOD_METADATA, fn, undefined)

      if (routePath === undefined || routeMethod === undefined) continue

      const fullPath = (prefix + routePath).replace('//', '/') || '/'
      routes.push({
        method: methodNameFromCode(routeMethod),
        path: fullPath,
        handlerName: name,
        instance,
      })
    }
  }

  console.log(`NestJS (real @nestjs/common) bootstrapped with ${routes.length} route(s)`)
  for (const r of routes) {
    console.log(`  ${r.method} ${r.path}`)
  }

  serve(port, async (req: Request) => {
    const url    = new URL(req.url)
    const method = req.method.toUpperCase()
    const path   = url.pathname

    for (const route of routes) {
      if ((route.method === method || route.method === 'ALL') && pathMatches(route.path, path)) {
        try {
          const result = route.instance[route.handlerName]()
          if (result instanceof Promise) {
            const resolved = await result
            if (typeof resolved === 'string') return new Response(resolved, { status: 200 })
            return resolved || new Response('OK', { status: 200 })
          }
          if (typeof result === 'string') return new Response(result, { status: 200 })
          return result || new Response('OK', { status: 200 })
        } catch (err) {
          console.error('Route handler threw:', err)
          return new Response('Internal Server Error', { status: 500 })
        }
      }
    }

    return new Response('Not Found', { status: 404 })
  })
}
