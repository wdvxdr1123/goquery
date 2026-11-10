package undefined

// Use refers to a name that no scope declares. gopls reports one
// UndeclaredName diagnostic with the full identifier span. The long comment
// below keeps this fixture above the feature sampling size floor.
func Use() int {
	return missingValue
}

// Fill is unused documentation padding only; it keeps the sampled file size
// above the floor without introducing any additional diagnostic.
var Fill = "1223456789012345678901234567890123456789012345678901234567890123456789"
