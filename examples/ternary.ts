function isEven(n: number) {
    let is_even = n % 2 === 0 ? 1 : 0;
    return is_even;
}

let sum = 0;
sum = sum + isEven(10); // + 1
sum = sum + isEven(11); // + 0
sum = sum + isEven(24); // + 1
sum; // 2
