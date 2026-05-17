function greet(name: string): string {
    return "Hello, " + name;
}

function add(a: number, b: number): number {
    return a + b;
}

interface Shape {
    area(): number;
    perimeter(): number;
}

class Circle implements Shape {
    constructor(private radius: number) {}

    area(): number {
        return Math.PI * this.radius ** 2;
    }

    perimeter(): number {
        return 2 * Math.PI * this.radius;
    }
}

type Point = {
    x: number;
    y: number;
};

export {};
