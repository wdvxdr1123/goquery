package typeoracle

import clock "time"

var PackageInteger = 1
var PackageTyped []*Item
var PackageForward = PackageFuture
var PackageFuture = PackageTyped
var PackageMultiA, PackageMultiB = 7, "value"
var PackageTupleItem, PackageTupleCount = PackagePair()
var PackageFunction = PackageCreate
var PackageFunctionCopy = PackageFunction
var PackageFunctionTyped func(int) *Item = PackageCreate
var PackageFunctionZero func(int) *Item
var PackageFunctionGeneric = Identity[*Item]
var PackageFunctionNamed = PackageCallable(PackageCreate)
var PackageRecord = PackageContainer{Item: &Item{}, Values: []*Item{}, Call: PackageCreate}
var PackagePointer = &PackageContainer{}
var PackageAnonymous = struct{ Item *Item }{}
var PackageConstructed = PackageFactory()
var PackageGeneric = PackageGenericContainer[*Item]{}
var PackageDuration = clock.Second
const PackageDurationConstant = clock.Second

type PackageContainer struct { Item *Item; Values []*Item; Call func(int) *Item }
func (value PackageContainer) Get() *Item { return value.Item }
type PackageGenericContainer[T any] struct { Item T }
func (value PackageGenericContainer[T]) Get() T { return value.Item }
type PackageCallable func(int) *Item
func PackageCreate(int) *Item { return nil }
func PackagePair() (*Item,int) { return nil,2 }
func PackageFactory() PackageContainer { return PackageContainer{} }
func PackageIntegerPair() (int,int) { return 2,3 }

type PackageTag int8
const (
 PackageZero PackageTag = iota
 _
 PackageTwo
 PackageThree
 PackageReset = iota + 7
 PackageNext
 PackageLeft,PackageRight = iota,1<<iota
 PackageMoreLeft,PackageMoreRight
 PackageText = "ab"+"c"
 PackageFlag = true
 PackageMinimum = min(7,2,5)
 PackageMaximum = max(1,4,3)
 编号 = iota
)
const PackageSingle = iota
const PackageCopy = PackageTwo

func PackageValuesProbe() {
 integer := PackageInteger
 typed := PackageTyped
 forward := PackageForward
 multiA := PackageMultiA
 multiB := PackageMultiB
 tupleItem := PackageTupleItem
 tupleCount := PackageTupleCount
 unicodeOrdinal := 编号
 _ = unicodeOrdinal
 constant := PackageCopy
 text := PackageText
 flag := PackageFlag
 single := PackageSingle
 shifted := PackageRight
 moreShifted := PackageMoreRight
 reset := PackageReset
 next := PackageNext
 computed := integer + len(typed) + PackageSingle
 item := PackageRecord.Item
 pointerItem := PackagePointer.Item
 anonymousItem := PackageAnonymous.Item
 constructedItem := PackageConstructed.Item
 genericItem := PackageGeneric.Item
 duration := PackageDuration
 durationConstant := PackageDurationConstant
 _,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_ = integer,typed,forward,multiA,multiB,tupleItem,tupleCount,constant,text,flag,single,shifted,moreShifted,reset,next,computed,item,pointerItem,anonymousItem,constructedItem,genericItem,duration,durationConstant
 rows := [5]*Item{}
 indexed := rows[PackageCopy]
 indexedMinimum := rows[PackageMinimum]
 indexedMaximum := rows[PackageMaximum]
 minimum := min(PackageInteger, 1)
 maximum := max(PackageCopy, 3)
 for _, ranged := range PackageRecord.Values { _ = ranged }
 _,_,_,_,_ = indexed,indexedMinimum,indexedMaximum,minimum,maximum
}
func PackageFunctionProbe() {
 called := PackageFunction(1)
 copied := PackageFunctionCopy(2)
 typed := PackageFunctionTyped(3)
 zero := PackageFunctionZero(4)
 generic := PackageFunctionGeneric(&Item{})
 named := PackageFunctionNamed(5)
 field := PackageRecord.Call(6)
 method := PackageRecord.Get()
 genericMethod := PackageGeneric.Get()
 methodValue := PackageRecord.Get
 fromMethodValue := methodValue()
 functionValue := PackageFunction
 fromFunctionValue := functionValue(7)
 _,_,_,_,_,_,_,_,_,_,_,_,_ = called,copied,typed,zero,generic,named,field,method,genericMethod,methodValue,fromMethodValue,functionValue,fromFunctionValue
}
func PackageShadowProbe() {
 PackageInteger := "local"
 local := PackageInteger
 PackageFunction := func(string) string { return "" }
 localCall := PackageFunction("local")
 _,_ = local,localCall
}
func OrderedProbe(number int, fraction float32, word string) {
 integer := min(7,2,5)
 maximum := max(2,8,3)
 runeValue := min('a', 66)
 floating := min(1.5,2.5)
 mixed := max(1,2.5)
 narrow := max(fraction,1.5)
 typed := min(PackageTag(2),3)
 variable := max(number,2)
 text := min("abc","xy")
 equalWidth := len(min("ab","cd"))
 variableText := max(word,"x")
 one := min(number)
 tuple := min(PackageIntegerPair())
 typedString := max(string("a"), "xyz")
 rounded := int(min(float32(16777217),float32(16777218)))
 const lowest = min(2,1)
 const highest = max(1,2)
 rows := [3]*Item{}
 first := rows[lowest]
 last := rows[highest]
 _,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_ = integer,maximum,runeValue,floating,mixed,narrow,typed,variable,text,equalWidth,variableText,one,tuple,typedString,rounded,first,last
}
func OrderedFormal[T interface{~int|~int64},S ~[]*Item](left,right T, values S) {
 indexed := values[min(left,right)]
 made := len(make(S,max(left,right)))
 _,_ = indexed,made
}
