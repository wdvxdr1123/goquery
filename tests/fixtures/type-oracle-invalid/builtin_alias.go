package typeoracleinvalid

import len "strings"
var _ = len.TrimSpace
func BuiltinImportedAliasWrong() { badBuiltinImportLen := len("value"); _ = badBuiltinImportLen }
