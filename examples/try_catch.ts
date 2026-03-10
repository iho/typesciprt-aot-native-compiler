// Basic try/catch: throw 42, catch it, exit with that value.
let result = 0;
try {
    throw 42;
    result = 99; // unreachable
} catch (e) {
    result = e;
}
result
