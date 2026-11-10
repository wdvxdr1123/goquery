package other

var Value = 10

type Shape struct {
	Width int
}

func (s Shape) Area() int {
	return s.Width
}
