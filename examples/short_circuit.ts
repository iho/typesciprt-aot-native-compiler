function test_short_circuit(): number {
    let side_effect = 0;

    // AND short circuit
    let result_and = false && (side_effect = 1);
    if (side_effect == 1) {
        return 1; // Failed: side effect executed
    }

    // OR short circuit
    let result_or = true || (side_effect = 2);
    if (side_effect == 2) {
        return 2; // Failed: side effect executed
    }

    // AND no short circuit
    let result_and_ok = true && (side_effect = 3);
    if (side_effect != 3) {
        return 3; // Failed: side effect didn't execute
    }

    // OR no short circuit
    let result_or_ok = false || (side_effect = 4);
    if (side_effect != 4) {
        return 4; // Failed: side effect didn't execute
    }

    return 0; // Success
}

test_short_circuit();
