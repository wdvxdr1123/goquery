package helper

// Known is the only exported declaration. The qualified-selector fixture uses
// it as a positive control so its import is genuinely used.
func Known() int {
	return 1
}

// unexported is intentionally not exported; qualified selectors outside this
// package must never reach it.
func unexported() int {
	return 2
}

// Fill keeps this fixture above the feature sampling size floor.
var Fill = "8223456789012345678901234567890123456789012345678901234567890123456789"
