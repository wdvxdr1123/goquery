package clean

import "fmt"

// Add returns the sum of two integers. This fixture is the positive control:
// gopls publishes no compiler or syntax diagnostics for it, so the Rust server
// must also publish an empty list rather than guessing about valid Go code.
func Add(left, right int) int {
	return left + right
}

// Use exercises the imported package so that the import is used exactly once.
func Use() string {
	return fmt.Sprintf("%d", Add(1, 2))
}
