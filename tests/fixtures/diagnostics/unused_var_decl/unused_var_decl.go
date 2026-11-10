package unusedvardecl

// Use declares a typed variable that is never read. gopls reports UnusedVar but
// its quickfix path only rewrites short declarations, so this shape has none.
func Use() {
	var value int
}
