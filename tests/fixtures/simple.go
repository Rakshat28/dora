package main

import "fmt"

func greet(name string) string {
	return "Hello, " + name
}

func add(a int, b int) int {
	return a + b
}

func multiply(x int, y int) int {
	return x * y
}

type Point struct {
	X float64
	Y float64
}

type Rectangle struct {
	Width  float64
	Height float64
}

func (r Rectangle) area() float64 {
	return r.Width * r.Height
}
