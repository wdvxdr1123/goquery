package duplicate

// Dup is declared twice in the same file block. gopls reports both the plain
// redeclaration and a "(see details)" diagnostic on the first declaration, each
// with relatedInformation pointing at the other declaration.
func Dup() {}

// Fill keeps this fixture above the feature sampling size floor.
var Fill = "02234567890123456789012345678901234567890123456789012345678901"

func Dup() {}
