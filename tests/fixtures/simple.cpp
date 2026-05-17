#include <string>

class Animal {
public:
    virtual std::string speak() = 0;
    virtual ~Animal() = default;
};

class Dog : public Animal {
public:
    std::string speak() override {
        return "Woof";
    }
};

int add(int a, int b) {
    return a + b;
}

template <typename T>
T multiply(T x, T y) {
    return x * y;
}

struct Point {
    double x;
    double y;
};
