package unusedvar

// Use declares a short variable that is never read. gopls reports UnusedVar at
// the identifier and offers the Remove variable quickfix for this shape.
func Use() {
	value := 1
}
