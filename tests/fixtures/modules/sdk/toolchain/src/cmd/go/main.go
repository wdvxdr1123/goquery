package main
import ("golang.org/x/mod/modfile"; "golang.org/x/net/idna"; "net/http")
func Main() { modfile.Parse("v1"); idna.ToASCII("x"); http.Check("h") }
