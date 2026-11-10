package strings
import "unicode/utf8"
func Decode(s string) (rune, int) { return utf8.DecodeRuneInString(s) }
