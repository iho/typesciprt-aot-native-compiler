// Shim for the 'uid' npm package — generates short unique random strings.
export function uid(size: number): string {
  let result = ''
  while (result.length < size) {
    result += Math.random().toString(36).slice(2)
  }
  return result.slice(0, size)
}
