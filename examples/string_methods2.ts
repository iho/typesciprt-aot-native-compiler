// Additional string methods
const s = "Hello, World!";

// replace / replaceAll
const r1 = s.replace("World", "TypeScript");  // "Hello, TypeScript!"
const r2 = "aababc".replaceAll("ab", "X");    // "aXXc"

// startsWith / endsWith
const sw = s.startsWith("Hello") ? 1 : 0;     // 1
const ew = s.endsWith("World!") ? 1 : 0;      // 1

// repeat
const r3 = "ab".repeat(3);  // "ababab"
const rl = r3.length;       // 6

// charAt / charCodeAt
const ch = s.charAt(0);         // "H"
const cc = s.charCodeAt(0);     // 72

// padStart / padEnd
const ps = "5".padStart(3, "0");  // "005" length=3
const pe = "5".padEnd(3, "0");    // "500" length=3

// String.fromCharCode
const fc = String.fromCharCode(65);  // "A"
const fcl = fc.length;               // 1

// sum: sw(1) + ew(1) + rl(6) + cc(72) + fcl(1) = 81
sw + ew + rl + cc + fcl
