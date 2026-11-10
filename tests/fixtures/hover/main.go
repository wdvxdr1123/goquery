// Package main exercises hover rendering shapes.
package main

import (
	"fmt"
	"strings"
	str "strings"

	"example.test/hover/lib"
)

// Defined is a defined basic type.
type Defined int

// Alias is an alias of Defined.
type Alias = Defined

// Box is a generic container.
type Box[T any] struct {
	// Value is stored directly.
	Value  T
	hidden int
	lib.Base
}

// Inner is embedded.
type Inner struct{ N int }

// Shape is an interface.
type Shape interface {
	// Area documents the method.
	Area() float64
}

// Rect implements Shape.
type Rect struct{ W, H float64 }

// Area implements Shape.
func (r Rect) Area() float64 { return r.W * r.H }

// Scale scales a Rect.
func (r *Rect) Scale(factor float64) { r.W *= factor; r.H *= factor }

// SumFunc adds numbers.
func SumFunc(values ...int) (total int, err error) { return 0, nil }

// GenericFunc is generic.
func GenericFunc[T ~int](value T) T { return value }

// DocConst is documented.
const DocConst = 42

const (
	// Written keeps an expression.
	Written = 0x10
	Inherited
)

var (
	// Answer is documented.
	Answer = 42
	Ratio  = 1.5
	Named  string
)

func main() {
	local := Defined(1)
	text := strings.TrimSpace(" x ")
	alias := str.ToUpper("y")
	box := Box[int]{Value: 1}
	rect := Rect{W: 2, H: 3}
	fmt.Println(local, text, alias, box.Value, rect.Area(), lib.Double(2), lib.Version)
	_ = SumFunc(1, 2)
	_ = GenericFunc(3)
	_ = Alias(4)
	_ = DocConst
	_ = Written
	_ = Inherited
	_ = Answer
	_ = Ratio
	_ = Named
	_ = Shape(rect)
	_ = Inner{N: 1}
}
