package httpguts
import "golang.org/x/net/idna"
func ValidHeaderFieldName(s string) bool { return len(idna.ToASCII(s)) > 0 }
func Own(s string) bool { return ValidHeaderFieldName(s) }
