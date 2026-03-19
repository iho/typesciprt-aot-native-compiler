let internalVal = 10

const obj = {
  get value() {
    return internalVal
  },
  set value(v: number) {
    internalVal = v
  }
}

obj.value = 42
const result = obj.value
result  // exits with 42
