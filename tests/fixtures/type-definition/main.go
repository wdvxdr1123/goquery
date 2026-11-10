package main
import external "example.test/typedef/lib"
type hidden struct{}
var Global = external.New()
func Probe(p external.Public, input []external.Alias) {
 a := external.New()
 var alias external.Alias
 var pointer *external.Alias
 var slice []*external.Alias
 var array [2]external.Alias
 var channel chan external.Alias
 var mapping map[string]external.Alias
 f := external.New
 var callback external.Callback
 pairA, pairB := external.Pair()
 generic := external.Generic(1)
 var genericAlias external.GenericAlias[int]
 field := p.Item
 method := p.Get()
 type Local struct{}
 type LocalAlias = Local
 var local Local
 var localPointer *Local
 var localSlice []Local
 literal := Local{}
 localLiteralSlice := []Local{}
 localFunction := func() Local { return Local{} }
 var localAlias LocalAlias
 _, _ = external.Pair()
 _, _ = external.Duplicate()
 for _, item := range input { _ = item }
 _ = []any{a, alias, pointer, slice, array, channel, mapping, f, callback, pairA, pairB, generic, genericAlias, field, method, local, localPointer, localSlice, literal, localLiteralSlice, localFunction, localAlias}
}
