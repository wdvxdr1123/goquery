package httpguts
func ValidHeaderFieldName(s string) bool { return true }
func Own(s string) bool { return ValidHeaderFieldName(s) }
