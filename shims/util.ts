// Shim for Node.js 'util' module — minimal implementation for NestJS logger.
export function inspect(value: any, options?: any): string {
  if (value === null) return 'null'
  if (value === undefined) return 'undefined'
  if (typeof value === 'string') return value
  return JSON.stringify(value)
}
