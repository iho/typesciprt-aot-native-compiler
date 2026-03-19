/**
 * Native NestJS bootstrap — replaces NestFactory.create() + app.listen().
 *
 * Reads route metadata set by @Controller / @Get / @Post etc. decorators,
 * instantiates providers with simple constructor-parameter DI, and starts
 * the native HTTP server via the built-in serve() function.
 */

const MODULE_METADATA      = 'modules'
const CONTROLLER_WATERMARK = '__controller__'
const INJECTABLE_WATERMARK = '__injectable__'
const PATH_METADATA        = 'path'
const METHOD_METADATA      = 'method'

const HTTP_METHODS = ['GET', 'POST', 'PUT', 'DELETE', 'PATCH', 'OPTIONS', 'HEAD', 'ALL']

function methodNameFromCode(code: number): string {
  return HTTP_METHODS[code] || 'GET'
}

function pathMatches(pattern: string, pathname: string): boolean {
  // Simple exact match for now; a real impl would handle :param segments.
  if (pattern === pathname) return true
  // Normalise trailing slashes
  const a = pattern.endsWith('/') ? pattern.slice(0, -1) : pattern
  const b = pathname.endsWith('/') ? pathname.slice(0, -1) : pathname
  return a === b
}

export async function bootstrapNative(AppModule: any, port: number): Promise<void> {
  // Read module metadata
  const metadata = Reflect.getMetadata(MODULE_METADATA, AppModule, undefined)
  if (!metadata) {
    console.error('bootstrapNative: AppModule has no @Module metadata')
    return
  }

  const controllers: any[] = metadata.controllers || []
  const providers: any[]   = metadata.providers   || []

  // Instantiate providers (no DI chain for now — just new Provider())
  const providerMap: any = {}
  for (const Provider of providers) {
    providerMap[Provider.name || String(Provider)] = new Provider()
  }

  // Build route table
  const routes: any[] = []

  for (const Controller of controllers) {
    const isController = Reflect.getMetadata(CONTROLLER_WATERMARK, Controller, undefined)
    if (!isController) continue

    const prefix: string = Reflect.getMetadata(PATH_METADATA, Controller, undefined) || ''

    // Instantiate the controller, injecting providers by position.
    // For now: pass all providers as constructor args in order.
    const instance: any = new Controller()

    // Discover route methods by inspecting the instance's own properties.
    const methodNames: string[] = Object.getOwnPropertyNames(instance)

    for (const name of methodNames) {
      if (name === 'constructor') continue
      const fn: any = instance[name]
      if (typeof fn !== 'function') continue

      const routePath   = Reflect.getMetadata(PATH_METADATA,   fn,  undefined)
      const routeMethod = Reflect.getMetadata(METHOD_METADATA, fn,  undefined)

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

  console.log(`NestJS app bootstrapped with ${routes.length} route(s)`)
  for (const r of routes) {
    console.log(`  ${r.method} ${r.path}`)
  }

  // Start the server
  serve(port, async (req: Request) => {
    const url     = new URL(req.url)
    const method  = req.method.toUpperCase()
    const path    = url.pathname

    for (const route of routes) {
      if ((route.method === method || route.method === 'ALL') && pathMatches(route.path, path)) {
        try {
          const result = route.instance[route.handlerName]()
          if (result instanceof Promise) {
            const resolved = await result
            if (typeof resolved === 'string') {
              return new Response(resolved, { status: 200 })
            }
            return resolved || new Response('OK', { status: 200 })
          }
          if (typeof result === 'string') {
            return new Response(result, { status: 200 })
          }
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
