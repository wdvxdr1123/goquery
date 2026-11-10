// Package semtokfixture exercises range semantic tokens.
package semtokfixture

// Threshold is a documented constant.
const Threshold = 42

// Pair groups a name and a value.
type Pair struct {
	Name  string
	Value int
}

// Shape is implemented by Pair.
type Shape interface {
	Measure() int
}

// Measure reports the stored value.
func (p Pair) Measure() int {
	total := p.Value * Threshold
	if total > 0 {
		total += len(p.Name)
	}
	for index := 0; index < 2; index++ {
		total -= index
	}
	return total
}

var registry = map[string]Pair{
	"first":  {Name: "a", Value: 1},
	"second": {Name: "b", Value: 2},
}
