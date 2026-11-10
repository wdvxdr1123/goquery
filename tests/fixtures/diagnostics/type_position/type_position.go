package typeposition

// MissingElem and MissingKey are never declared. gopls reports UndeclaredName
// with the unqualified type identifier as the diagnostic range, while the rest
// of this file stays clean so only those two type-position errors are compared.
var value *MissingElem

var mapping map[MissingKey]int

// Use reads both variables so no UnusedVar diagnostic appears for them.
func Use() {
	_ = value
	_ = mapping
}

// Fill keeps this fixture above the feature sampling size floor.
var Fill = "7223456789012345678901234567890123456789012345678901234567890123456789"
