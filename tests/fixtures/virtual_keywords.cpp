#include <string>

class Shape {
public:
    virtual ~Shape() = default;
    virtual void draw() const = 0;
};

class Circle : public Shape {
public:
    void draw() const override {
        // draw circle
    }
};

class Rectangle : public Shape {
private:
    virtual std::string getName() const;
    
public:
    void draw() const override {
        // draw rectangle
    }
};
