package http
import "net/url"
func Other(u url.URL) string { return u.Path }
