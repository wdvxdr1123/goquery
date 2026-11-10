package unusedimport

import "os"

// Use exercises no imports at all. The unused single import declaration makes
// gopls publish one UnusedImport diagnostic and an Organize Imports source
// action that deletes the declaration line together with one blank line.
func Use() {
	println("unused import fixture")
}

// Fill keeps this fixture above the feature sampling size floor.
var Fill = "5223456789012345678901234567890123456789012345678901234567890123456789"
