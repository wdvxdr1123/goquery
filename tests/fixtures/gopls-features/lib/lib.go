package lib

// Shape is implemented by area-bearing shapes.
type Shape interface {
	Area() int
}

// Square is an axis-aligned square.
type Square struct {
	Side int
}

// Area returns the square area.
func (s Square) Area() int { return s.Side * s.Side }

// Circle is a circle.
type Circle struct {
	Radius int
}

// Area returns the circle area.
func (c Circle) Area() int { return 3 * c.Radius * c.Radius }
