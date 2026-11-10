package typeoracleinvalid

var ValueCycleA = ValueCycleB
var ValueCycleB = ValueCycleA
var ValueTypedCycle int = ValueTypedCycle
const ValueConstCycleA = ValueConstCycleB
const ValueConstCycleB = ValueConstCycleA
var ValueInvalidConstant int = "wrong"
var ValueBadMap map[[]int]int
var ValueBadComposite = struct{ M map[[]int]int }{}
var ValueBadSlice = []map[[]int]int{}
var ValueFunction = ValueCreate
var ValueHolder = struct{ Call func(int)*ValueReader }{Call:ValueCreate}
func ValueCreate(int) *ValueReader { return nil }
func ValueCycles() {
 badValueCycle := ValueCycleA
 badValueTypedCycle := ValueTypedCycle
 badValueConstCycle := ValueConstCycleA
 badValueInvalidConstant := ValueInvalidConstant
 badValueMap := ValueBadMap
 badValueComposite := ValueBadComposite
 badValueSlice := ValueBadSlice
 _,_ = badValueComposite,badValueSlice
 _,_,_,_,_ = badValueCycle,badValueTypedCycle,badValueConstCycle,badValueInvalidConstant,badValueMap
}
func ValueBadCall() { badValueCall := ValueFunction("wrong"); _ = badValueCall }
func ValueBadFieldCall() { badValueFieldCall := ValueHolder.Call("wrong"); _ = badValueFieldCall }
func ValueBadCallArity() { badValueCallArity := ValueFunction(); _ = badValueCallArity }
func ValueNotCallable() { badValueNotCallable := ValueInvalidConstant(); _ = badValueNotCallable }
func ValueBadMinEmpty() { badOrderedEmpty := min(); _ = badOrderedEmpty }
func ValueBadMinBool() { badOrderedBool := min(true,false); _ = badOrderedBool }
func ValueBadMaxComplex() { badOrderedComplex := max(1i,2i); _ = badOrderedComplex }
func ValueBadMinTypes(left int,right int64) { badOrderedTypes := min(left,right); _ = badOrderedTypes }
func ValueBadMinOverflow(value uint8) { badOrderedOverflow := min(300,1,value); _ = badOrderedOverflow }
func ValueBadMaxString() { badOrderedString := max("x",1); _ = badOrderedString }
func ValueBadMinSpread(values []int) { badOrderedSpread := min(values...); _ = badOrderedSpread }
func ValueBadMaxNil() { badOrderedNil := max(nil); _ = badOrderedNil }
func ValueBadFloatOverflow(value float64) { badOrderedFloatOverflow := min(1e400,value); _ = badOrderedFloatOverflow }
func ValueBadUnordered[T interface{~int|~bool}](left,right T) { badOrderedFormal := min(left,right); _ = badOrderedFormal }
func ValueBadFormalIdentity[T ~int,U ~int](left T,right U) { badOrderedFormalIdentity := min(left,right); _ = badOrderedFormalIdentity }
func ValueBadBound(rows [3]*ValueReader) { const upper = max(1,3); badOrderedBound := rows[upper]; _ = badOrderedBound }
