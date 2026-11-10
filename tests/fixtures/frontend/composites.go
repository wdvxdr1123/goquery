package corpus

var _ = Point{X: 1, Y: 2}
var _ = []int{1, 2, 3}
var _ = [3]int{0: 1, 2: 3}
var _ = map[string]int{"a": 1}
var _ = []Point{{X: 1}, {X: 2}}
var _ = map[Point]Point{{X: 1}: {Y: 2}}
var _ = Box[int]{Value: 1}.Value
var _ = &Point{X: 1}
var _ = struct{ X int }{X: 1}.X
var _ = func(x int) int { return x }(1)
var _ = make([]int, 3)
var _ = [...]int{1, 2}
