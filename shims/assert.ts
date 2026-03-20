export class AssertionError extends Error {
  actual: any;
  expected: any;
  operator: string;
  constructor(opts: any) {
    super(opts.message || String(opts.actual) + ' ' + opts.operator + ' ' + String(opts.expected));
    this.actual = opts.actual;
    this.expected = opts.expected;
    this.operator = opts.operator || '==';
  }
}

export function ok(value: any, message?: string | Error): void {
  if (!value) {
    if (message instanceof Error) throw message;
    throw new AssertionError({ actual: value, expected: true, operator: '==', message: message || 'Assertion failed' });
  }
}

export function strictEqual(actual: any, expected: any, message?: string | Error): void {
  if (actual !== expected) {
    if (message instanceof Error) throw message;
    throw new AssertionError({ actual, expected, operator: '===', message: message || ('Expected ' + String(expected) + ' but got ' + String(actual)) });
  }
}

export function notStrictEqual(actual: any, expected: any, message?: string | Error): void {
  if (actual === expected) {
    if (message instanceof Error) throw message;
    throw new AssertionError({ actual, expected, operator: '!==', message: message || 'Expected values to not be strictly equal' });
  }
}

export function equal(actual: any, expected: any, message?: string | Error): void {
  if (actual != expected) {
    if (message instanceof Error) throw message;
    throw new AssertionError({ actual, expected, operator: '==', message: message || ('Expected ' + String(expected) + ' but got ' + String(actual)) });
  }
}

export function notEqual(actual: any, expected: any, message?: string | Error): void {
  if (actual == expected) {
    if (message instanceof Error) throw message;
    throw new AssertionError({ actual, expected, operator: '!=', message: message || 'Expected values to not be equal' });
  }
}

export function deepEqual(actual: any, expected: any, message?: string | Error): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    if (message instanceof Error) throw message;
    throw new AssertionError({ actual, expected, operator: 'deepEqual', message: message || 'Deep equal failed' });
  }
}

export function deepStrictEqual(actual: any, expected: any, message?: string | Error): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    if (message instanceof Error) throw message;
    throw new AssertionError({ actual, expected, operator: 'deepStrictEqual', message: message || 'Deep strict equal failed' });
  }
}

export function notDeepStrictEqual(actual: any, expected: any, message?: string | Error): void {
  if (JSON.stringify(actual) === JSON.stringify(expected)) {
    if (message instanceof Error) throw message;
    throw new AssertionError({ actual, expected, operator: 'notDeepStrictEqual', message: message || 'Expected values to not be deeply equal' });
  }
}

export function throws(fn: () => void, errorOrMessage?: any, message?: string): void {
  let threw = false;
  try { fn(); } catch (e) { threw = true; }
  if (!threw) throw new AssertionError({ actual: fn, expected: 'throws', operator: 'throws', message: message || 'Expected function to throw' });
}

export function doesNotThrow(fn: () => void, message?: string | Error): void {
  try {
    fn();
  } catch (e) {
    const msg = message ? String(message) : ('Expected function not to throw, got: ' + String(e));
    throw new AssertionError({ actual: e, expected: 'no throw', operator: 'doesNotThrow', message: msg });
  }
}

export function fail(message?: string | Error): never {
  if (message instanceof Error) throw message;
  throw new AssertionError({ actual: undefined, expected: undefined, operator: 'fail', message: message || 'Assertion failed' });
}

export function match(actual: string, regexp: RegExp, message?: string | Error): void {
  if (!regexp.test(actual)) {
    if (message instanceof Error) throw message;
    throw new AssertionError({ actual, expected: regexp, operator: 'match', message: message || (actual + ' does not match ' + String(regexp)) });
  }
}

export function doesNotMatch(actual: string, regexp: RegExp, message?: string | Error): void {
  if (regexp.test(actual)) {
    if (message instanceof Error) throw message;
    throw new AssertionError({ actual, expected: regexp, operator: 'doesNotMatch', message: message || (actual + ' matches ' + String(regexp)) });
  }
}

function assert(value: any, message?: string | Error): void { ok(value, message); }
assert.ok = ok;
assert.strictEqual = strictEqual;
assert.notStrictEqual = notStrictEqual;
assert.equal = equal;
assert.notEqual = notEqual;
assert.deepEqual = deepEqual;
assert.deepStrictEqual = deepStrictEqual;
assert.notDeepStrictEqual = notDeepStrictEqual;
assert.throws = throws;
assert.doesNotThrow = doesNotThrow;
assert.fail = fail;
assert.match = match;
assert.doesNotMatch = doesNotMatch;
assert.AssertionError = AssertionError;
export default assert;
