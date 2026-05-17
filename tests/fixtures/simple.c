#include <stdio.h>

int add(int a, int b) {
    return a + b;
}

int multiply(int x, int y) {
    return x * y;
}

typedef struct {
    float x;
    float y;
} Point;

void greet(const char *name) {
    printf("Hello, %s\n", name);
}
