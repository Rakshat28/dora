function greet(name) {
    return "Hello, " + name;
}

function add(a, b) {
    return a + b;
}

const multiply = (x, y) => x * y;

class Calculator {
    constructor(value) {
        this.value = value;
    }

    increment() {
        return this.value + 1;
    }
}
