package syntaxvar

// Broken assigns nothing to a new variable, so go/parser expects a semicolon
// when it reaches the following if statement.
func Broken() {
	var value =
	if {
	}
}

// Fill keeps this fixture above the feature sampling size floor without
// changing the parse error position above.
var Fill = "42234567890123456789012345678901234567890123456789012345678901"
