// try/finally: finally always runs.  Exit code comes from result.
let result = 1;
try {
    result = 10;
} finally {
    result = result + 5;
}
result
