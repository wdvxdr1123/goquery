package strings
import "internal/trim"
func TrimSpace(s string) string { return trim.Clean(s) }
