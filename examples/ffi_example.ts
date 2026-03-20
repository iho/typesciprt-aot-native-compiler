// FFI example: calling a native C function from TypeScript.
//
// 1. Write a C file (or Rust #[no_mangle] fn):
//      int64_t my_add(int64_t a, int64_t b) {
//          // a and b are NaN-boxed TsVal integers.
//          // Extract i32 from TAG_INT encoding: lower 32 bits.
//          int32_t ia = (int32_t)(a & 0xFFFFFFFF);
//          int32_t ib = (int32_t)(b & 0xFFFFFFFF);
//          int32_t sum = ia + ib;
//          // Re-encode as TAG_INT: 0x7FFE_0000_0000_0000 | (uint32_t)sum
//          return 0x7FFE000000000000LL | (uint32_t)sum;
//      }
//
// 2. Compile to a static library:
//      gcc -c my_add.c -o my_add.o && ar rcs libmy_add.a my_add.o
//
// 3. Compile this TypeScript file linking the native library:
//      tscc ffi_example.ts --link-lib ./libmy_add.a
//
// The `declare function` declaration maps directly to an extern C symbol.
declare function my_add(a: number, b: number): number;

const result = my_add(10, 32);
console.log(result); // 42
process.exit(result as unknown as number);
