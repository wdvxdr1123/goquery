package typeoracleshadow

import min "strings"

func len(string) *int { return nil }
func cap(string) *int { return nil }
func append(string) *int { return nil }
func copy(string) *int { return nil }
func make(string) *int { return nil }
func new(string) *int { return nil }
func recover(string) *int { return nil }
func QueryBuiltins() {
 length := len("value")
 capacity := cap("value")
 appended := append("value")
 copied := copy("value")
 made := make("value")
 allocated := new("value")
 recovered := recover("value")
 imported := min.TrimSpace(" value ")
 _,_,_,_,_,_,_,_ = length,capacity,appended,copied,made,allocated,recovered,imported
}
func LocalImportShadow() {
 min := func(string) *int { return nil }
 local := min("value")
 _ = local
}
