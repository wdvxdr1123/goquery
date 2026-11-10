package main

import alias "example.test/project/lib"

func Main() { alias.Public() }
func Hidden() { alias.private() }
func Shadow() {
 alias := struct{ Public func() }{}
 alias.Public()
}
var Item alias.Thing
