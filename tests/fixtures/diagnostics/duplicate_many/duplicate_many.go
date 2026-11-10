package duplicatemany

// D is declared three times. gopls reports one plain redeclaration per later
// declaration and one "(see details)" diagnostic on the first declaration for
// each later declaration.
func D() {}

// Fill keeps this fixture above the feature sampling size floor.
var Fill = "12234567890123456789012345678901234567890123456789012345678901"

func D() {}

func D() {}
