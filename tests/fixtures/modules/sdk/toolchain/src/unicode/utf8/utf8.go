package utf8
func DecodeRuneInString(s string) (rune, int) { return 0, 1 }
func Own(s string) (rune, int) { return DecodeRuneInString(s) }
