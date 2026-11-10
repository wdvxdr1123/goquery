package main

import (
	"fmt"

	"example.test/features/lib"
)

// Greeting is returned by Greet. See https://go.dev/issue/1234 for context.
const Greeting = "hello"

// Counter counts things.
type Counter struct {
	value int
	label string
}

// Inc adds one to the counter.
func (c *Counter) Inc() {
	c.value++
}

// Value returns the current count.
func (c Counter) Value() int { return c.value }

// Greet returns a greeting for the given name.
func Greet(name string) string {
	return Greeting + ", " + name
}

func main() {
	counter := &Counter{label: "demo"}
	counter.Inc()
	fmt.Println(Greet(counter.label), counter.Value())
	var shape lib.Shape = lib.Square{Side: 2}
	fmt.Println(shape.Area())
}
