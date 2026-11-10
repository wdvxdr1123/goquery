package http
import "golang.org/x/net/http/httpguts"
func Check(s string) bool { return httpguts.ValidHeaderFieldName(s) }
