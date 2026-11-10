package corpus

var _ = pkg.Make[T](arg, more...).Field[0]
var _ = a[:]
var _ = a[low:high]
var _ = a[:high:max]
var _ = a[low:high:max].Field
var _ = value.(pkg.Type)
var _ = value.(*pkg.Type)
var _ = value.(interface{ Method() int })
var _ = value.(map[string][]*pkg.Type)
var _ = value.(<-chan int)
var _ = value.(chan<- *Thing)
var _ = value.(pkg.Box[int, *Thing])
var _ = value.([2+3]int)
var _ = value.(func(int, string) (bool, error))
var _ = value.((int))
// Syntax-only guard accepted by go/parser; context requires a type switch.
var _ = value.(type)
