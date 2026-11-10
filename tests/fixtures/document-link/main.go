// Package documentlink exercises gopls-compatible document links.
//
// Plain URLs: https://example.com/path?q=1#frag and http://example.org/
// Trailing punctuation: https://example.com/foo. https://example.com/bar,
// Parentheses: (https://example.com/a(b)c) brackets [https://example.com/a[b]c]
// Braces https://example.com/a{b}c and query text example.com?q=1
// Scheme-less hosts: example.com example.com/ example.com/x.y?x=1 go.dev/pkg
// Mail: contact@example.com and author@sub.example.org.
// Addresses: 127.0.0.1 192.168.0.1:8080 and 10.0.0.1/path
// IPv6: [::1]:8080 bare ::1 and 2001:db8::1.
// Issues: golang/go#1234 and two links golang/go#5 plus another/repo#77.
// Non-ASCII before a URL: 参数 https://example.com/unicode/路径 and 中文.中国
// Other schemes: mailto:user@example.com ftp://example.com/file file:///tmp/x
// Word-adjacent: xhttps://example.com and https://example.com)end
// Repeated: https://example.com https://example.com https://example.com
package documentlink

import (
	"fmt"
	. "strings"
	alias "example.com/documentlink/sub"
	_ "unsafe"
)

// A struct tag is a string literal and participates in scanning.
type Link struct {
	URL    string `json:"url" doc:"https://example.com/tag"`
	Issue  string `doc:"golang/go#9"`
	RawTag string `doc:"see example.com/tag"`
}

var (
	StringLink = "https://example.com/in-string"
	Plain      = "see example.com/plain and golang/go#42"
	Raw        = `raw string with https://example.com/raw and example.com/raw-host
second https://example.com/second line`
	Escaped = "escaped \"https://example.com/quoted\" and \t tab"
	Rune    = 'x'
	Multi   = "http://a.example.com:8443/x?y=1#z"
)

/* block comment https://example.com/block over
multiple lines with example.com/block-host and golang/go#7 */

var _ = fmt.Sprint
var _ = Contains
var _ = alias.Value
