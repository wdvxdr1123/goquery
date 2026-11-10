package qualifiedselector

import "example.test/diagnostics/helper"

// MissingValue and MissingType are not declared by helper. gopls reports
// UndeclaredImportedName at the member identifier with the fully qualified
// spelling, both for value selectors and for qualified type positions.
func Use() int {
	_ = helper.MissingValue
	var typed helper.MissingType
	_ = typed
	return helper.Known()
}

// Fill keeps this fixture above the feature sampling size floor.
var Fill = "9223456789012345678901234567890123456789012345678901234567890123456789"
