class Counter {
  static count = 0

  static increment() {
    Counter.count++
  }

  static getCount() {
    return Counter.count
  }

  static reset() {
    Counter.count = 0
  }
}

Counter.increment()
Counter.increment()
Counter.increment()
// exit code 3
Counter.getCount()
