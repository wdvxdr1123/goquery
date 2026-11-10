package syntaxif

// Broken contains an if statement without a condition. go/parser reports a
// zero-width "missing condition in if statement" error at the opening brace.
func Broken() {
	if {
	}
}

// Fill keeps this fixture above the feature sampling size floor without
// changing the parse error position above.
var Fill = "32234567890123456789012345678901234567890123456789012345678901"
