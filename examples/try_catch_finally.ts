// try/catch/finally all together.
// throw → catch sets result=7, finally adds 0, exit = 7.
let result = 0;
try {
    throw 7;
} catch (e) {
    result = e;
} finally {
    result = result + 0;
}
result
