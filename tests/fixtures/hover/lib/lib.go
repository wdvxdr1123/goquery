// Package lib supports hover fixtures.
package lib

// Version is the fixture version.
var Version = "1.0"

// Base is embedded across packages.
type Base struct {
	// ID identifies the base.
	ID int
}

// Double doubles its input.
func Double(value int) int { return value * 2 }

// Unlisted is not exported.
func Unlisted() {}
