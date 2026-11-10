// Package syntaxfeatures exercises syntax-only LSP features.
//
// It intentionally contains multi-line comments, grouped imports, nested
// blocks, composite literals and function literals so folding, selection and
// code lens behaviour can be compared with gopls on a small fixture.
package syntaxfeatures

import (
	"errors"
	"fmt"
	"strings"
)

/* A multi-line block comment
   spanning several lines
   for the comment folding range. */

//go:generate echo syntax-features

// Conversion describes one unit conversion.
type Conversion struct {
	Name  string            `json:"name"`
	Scale float64           `json:"scale"`
	Tags  map[string]string `json:"tags"`
	Apply func(float64) float64
}

// Formula evaluates the conversion pipeline step by step.
type Formula interface {
	// Evaluate applies the configured conversions.
	Evaluate(value float64) (float64, error)
	// Label returns the display label.
	Label() string
}

// Pipeline is the default formula implementation.
type Pipeline struct {
	Steps []Conversion
	table map[string]int
}

var (
	defaultScale = 1.5
	descriptions = []string{
		"alpha",
		"beta",
	}
	lookup = map[string]int{
		"alpha": 1,
		"beta":  2,
	}
)

const (
	baseUnit = 10
	double   = baseUnit * 2
)

var builder = func(prefix string) func(float64) float64 {
	return func(value float64) float64 {
		return value * defaultScale
	}
}

// Evaluate runs every conversion in order.
func (p *Pipeline) Evaluate(value float64) (float64, error) {
	if len(p.Steps) == 0 {
		return value, errors.New("empty pipeline")
	}
	total := value
	for index, step := range p.Steps {
		switch {
		case step.Apply != nil:
			total = step.Apply(total)
		case step.Scale != 0:
			total *= step.Scale
		default:
			if index == 0 {
				continue
			}
			return total, fmt.Errorf("step %d has no effect", index)
		}
		if strings.HasPrefix(step.Name, "skip") {
			break
		}
	}
	select {
	case <-make(chan struct{}):
		return total, nil
	default:
	}
	return total, nil
}

// Label renders the pipeline label.
func (p *Pipeline) Label() string {
	names := make([]string, 0, len(p.Steps))
	for _, step := range p.Steps {
		names = append(names, step.Name)
	}
	return strings.Join(names, "+")
}

// Identity returns its generic input.
func Identity[T any](value T) T {
	return value
}

// Pair groups two values.
type Pair[K comparable, V any] struct {
	Key   K
	Value V
}
