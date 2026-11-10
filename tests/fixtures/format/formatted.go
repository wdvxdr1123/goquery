// Package formatfixture exercises document formatting.
package formatfixture

import (
	"fmt"
	"strings"
)

// Pair keeps two aligned fields.
type Pair struct {
	Name  string
	Value int
}

const (
	Alpha = 1 // first
	Beta  = 22
)

// Format renders a pair.
func Format(pair Pair) string {
	return fmt.Sprintf("%s=%d", strings.TrimSpace(pair.Name), pair.Value)
}
