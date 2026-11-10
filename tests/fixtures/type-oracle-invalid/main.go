package typeoracleinvalid

import (
	"io"
	"time"
)

type WrongReader interface{ Read(string) (int, error) }

func AnyArgument(value any) Count     { return 0 }
func ErrorArgument(value error) Count { return 0 }
func InvalidInterfaceValues(integer int) {
	badAnyDefault := AnyArgument(1 << 100)
	badAnyConversionDefault := any(1 << 100)
	badNonEmptyLiteral := ErrorArgument(1)
	badNonEmptyConversion := error(integer)
	badInterfaceNumeric := int(any(1))
	badInterfaceNarrowing := io.Reader(any(nil))
	badAnonymousSignature := (interface{ Read(string) (int, error) })(io.Reader(nil))
	badDuplicateMethod := (interface {
		M()
		M()
	})(nil)
	badConflictingMethod := (interface {
		error
		Error() int
	})(nil)
	badFunctionUniverse := (func(interface{ Error() string }))(func(error) {})
	badMapFunction := map[func()]any{}
	_ = []any{badAnyDefault, badAnyConversionDefault, badNonEmptyLiteral,
		badNonEmptyConversion, badInterfaceNumeric, badInterfaceNarrowing, badAnonymousSignature,
		badDuplicateMethod, badConflictingMethod, badFunctionUniverse, badMapFunction}
}

type ValueReader struct{}

func (ValueReader) Read([]byte) (int, error) { return 0, nil }

type PointerReader struct{}

func (*PointerReader) Read([]byte) (int, error) { return 0, nil }

type PointerWrapper struct{ PointerReader }
type ShadowReader struct {
	ValueReader
	Read int
}
type LeftReader struct{ ValueReader }
type RightReader struct{ ValueReader }
type DiamondReader struct {
	LeftReader
	RightReader
}
type PointerShadow struct{ ValueReader }

func (*PointerShadow) Read([]byte) (int, error) { return 0, nil }

type DefinedReader ValueReader
type DefinedPointer *PointerReader
type AliasReceiver struct{}
type AliasReceiverPointer = *AliasReceiver

func (AliasReceiverPointer) Read([]byte) (int, error) { return 0, nil }
func InvalidReceiverAlias(value AliasReceiver) {
	badReceiverAlias := ReaderArgument(value)
	_ = badReceiverAlias
}

type WrongConcrete struct{}

func (WrongConcrete) Read(string) (int, error) { return 0, nil }

func InvalidConcreteArguments(value PointerReader, wrapper PointerWrapper, shadow ShadowReader,
	diamond DiamondReader, pointerShadow PointerShadow, defined DefinedReader, pointer DefinedPointer,
	wrong WrongConcrete, face *io.Reader, double **ValueReader) {
	badPointerValue := ReaderArgument(value)
	badPointerPromotion := ReaderArgument(wrapper)
	badFieldShadow := ReaderArgument(shadow)
	badDiamond := ReaderArgument(diamond)
	badPointerShadow := ReaderArgument(pointerShadow)
	badDefinedReader := ReaderArgument(defined)
	badDefinedPointer := ReaderArgument(pointer)
	badConcreteSignature := ReaderArgument(wrong)
	badPointerInterface := ReaderArgument(face)
	badDoublePointer := ReaderArgument(double)
	_ = []any{badPointerValue, badPointerPromotion, badFieldShadow, badDiamond, badPointerShadow,
		badDefinedReader, badDefinedPointer, badConcreteSignature, badPointerInterface, badDoublePointer}
}

func ReaderArgument(value io.Reader) Count { return 0 }
func InvalidInterfaceArguments(closer io.Closer, wrong WrongReader, integer Count) {
	badInterfaceMissing := ReaderArgument(closer)
	badInterfaceSignature := ReaderArgument(wrong)
	badInterfaceConcrete := ReaderArgument(integer)
	_ = []any{badInterfaceMissing, badInterfaceSignature, badInterfaceConcrete}
}

type Count int
type OtherCount int
type Small int8
type Send[T any] chan<- T
type Number[T any] int

type Callback func(Count, ...int) *int
type OtherCallback func(Count, ...int) *int

func InvalidSignatures(callback Callback, other OtherCallback, builtin int, count Count,
	items []int, fixed func(Count, []int) *int) {
	badFunctionArity := callback()
	badFunctionArgument := callback(builtin)
	badFunctionOverflow := callback(1, 9223372036854775808)
	badFunctionSpread := callback(count, 1, items...)
	badFixedSpread := fixed(count, items...)
	badSignatureConversion := Callback(fixed)
	badNamedFunctionAssignment := func(fn Callback) *int { return nil }(other)
	badFunctionResultType := (func(int) int)(callback)
	badFunctionComparable := Comparable(callback)
	badLiteralArgument := func(Count) *int { return nil }(builtin)
	badGenericFunctionConstraint := Approx[string]
	badFunctionTwoResults, badFunctionExtra := callback(count)
	_ = []any{badFunctionArity, badFunctionArgument, badFunctionOverflow,
		badFunctionSpread, badFixedSpread, badSignatureConversion, badNamedFunctionAssignment,
		badFunctionResultType, badFunctionComparable, badLiteralArgument,
		badGenericFunctionConstraint, badFunctionTwoResults, badFunctionExtra}
}

func Pointer() *int { return nil }

func CountArgument(value Count) Count                  { return value }
func PairArguments() (Count, int)                      { return 1, 2 }
func CombinedArguments(value Count, n int) Count       { return value }
func NarrowArgument(value Small) Count                 { return 0 }
func ChannelArgument(value <-chan int) Count           { return 0 }
func CallbackArgument(value func(Count) *int) Count    { return 0 }
func VariadicArgument(value Count, items ...int) Count { return value }
func (value Count) Lookup(n Small) *int                { return nil }

func InvalidOrdinaryArguments(integer int, value Count, send chan<- int,
	callback Callback, other OtherCallback, fixed func(Count, []int) *int, items []int) {
	badOrdinaryArity := CountArgument()
	badOrdinaryExcess := CountArgument(value, value)
	badOrdinaryNamed := CountArgument(integer)
	badOrdinaryOverflow := NarrowArgument(128)
	badOrdinaryNil := CountArgument(nil)
	badOrdinaryChannel := ChannelArgument(send)
	badOrdinaryCallback := CallbackArgument(callback)
	badOrdinaryFixed := CallbackArgument(fixed)
	badOrdinaryVariadic := VariadicArgument(value, true)
	badOrdinarySpread := VariadicArgument(value, 1, items...)
	badOrdinaryNonVariadic := CountArgument(items...)
	badOrdinaryMethod := value.Lookup(128)
	badOrdinaryMethodArity := value.Lookup()
	badOrdinaryTupleArity := CountArgument(PairArguments())
	badOrdinaryTupleMix := CombinedArguments(value, PairArguments())
	badOrdinaryTupleSpread := VariadicArgument(PairArguments()...)
	badGenericTuple := Choose(PairArguments())
	badGenericTupleMixed := Choose(value, PairArguments())
	_ = []any{badOrdinaryArity, badOrdinaryExcess, badOrdinaryNamed, badOrdinaryOverflow,
		badOrdinaryNil, badOrdinaryChannel, badOrdinaryCallback, badOrdinaryFixed,
		badOrdinaryVariadic, badOrdinarySpread, badOrdinaryNonVariadic, badOrdinaryMethod,
		badOrdinaryMethodArity, other, badOrdinaryTupleArity, badOrdinaryTupleMix, badOrdinaryTupleSpread,
		badGenericTuple, badGenericTupleMixed}
}

func Choose[T any](a, b T) T     { return a }
func Identity[T any](value T) T  { return value }
func Element[T any](value []T) T { return value[0] }
func First[T any](value ...T) T  { return value[0] }

func Comparable[T comparable](value T) T      { return value }
func Approx[T ~int8](value T) T               { return value }
func Exact[T int](value T) T                  { return value }
func ExactSlice[S []E, E any](value S) S      { return value }
func Direction[C ~chan<- E, E any](value C) E { panic("not executed") }

type DefinedSlice []int
type BadStruct struct{ Values []int }
type BadBlank struct {
	_     []int
	Count int
}
type BadGeneric[T any] struct{ Value T }
type PointerType *int
type ConversionSource struct{ Value int }
type ConversionTarget struct{ Other int }
type ConversionChan chan *int
type ConversionReceive <-chan *int

func InvalidConversions(real float64, complexValue complex128, source ConversionSource,
	channel ConversionChan, slice []int, values []int) {
	badConvertOverflow := Identity(Small(128))
	badConvertFraction := Small(1.5)
	badConvertBoolean := Count(true)
	badConvertStruct := ConversionTarget(source)
	badConvertArity := Count()
	badConvertExtra := Count(1, 2)
	badConvertSpread := Count(values...)
	badConvertRealComplex := complex128(real)
	badConvertComplexReal := float64(complexValue)
	badConvertChannel := ConversionReceive(channel)
	badConvertSlice := [2]string(slice)
	badConvertNilArray := [0]int(nil)
	badConvertNilScalar := int(nil)
	badConvertRounded := Small(float64(127.9999999999999999999))
	badConvertDoubleRound := int32(float32(1.0000000596046447753906250000000001))
	_ = []any{badConvertOverflow, badConvertFraction, badConvertBoolean, badConvertStruct,
		badConvertArity, badConvertExtra, badConvertSpread, badConvertRealComplex, badConvertComplexReal,
		badConvertChannel, badConvertSlice, badConvertNilArray, badConvertNilScalar, badConvertRounded,
		badConvertDoubleRound}
}

type Interface interface{ Read() }

func StructTag[T any](value struct {
	X T `tag`
}) T {
	return value.X
}
func StructName[T any](value struct{ X T }) T { return value.X }
func InvalidStructs() {
	badStruct := Comparable(BadStruct{})
	badBlankStruct := Comparable(BadBlank{})
	badGenericStruct := Comparable(BadGeneric[[]int]{})
	badZeroArray := Comparable(struct{ Value [0][]int }{})
	badStructTag := StructTag(struct {
		X int `other`
	}{})
	badStructName := StructName(struct{ Y int }{})
	badStructArray := Identity(struct{ Value [18446744073709551615]int }{})
	badStructDuplicate := Identity(struct{ X, X int }{})
	badEmbeddedPointer := Comparable(struct{ PointerType }{})
	badEmbeddedInterface := Comparable(struct{ *Interface }{})
	_ = []any{badStruct, badBlankStruct, badGenericStruct, badZeroArray, badStructTag, badStructName, badStructArray, badStructDuplicate, badEmbeddedPointer, badEmbeddedInterface}
}

type LimitedStream[T comparable] chan T

func InvalidConstraints(named DefinedSlice, channel chan int) {
	badConstraintSlice := Comparable([]int{})
	badConstraintMap := Comparable(map[string]int{})
	badConstraintDefault := Approx(1)
	badConstraintNominal := Exact(Count(1))
	badConstraintExactSlice := ExactSlice(named)
	badConstraintDirection := Direction(channel)
	_ = []any{badConstraintSlice, badConstraintMap, badConstraintDefault, badConstraintNominal,
		badConstraintExactSlice, badConstraintDirection}
}

type T = int
type Holder[T any] struct{ Value T }

func Fixed[R any](value Holder[T], r R) *int { return nil }
func Shadowed[T any]() {
	badGenericShadow := Fixed(Holder[T]{}, 1)
	_ = badGenericShadow
}

func InvalidGeneric(small Small, other OtherCount, builtin int) {
	badGenericDistinct := Choose(small, other)
	badGenericBuiltin := Choose[int](small, builtin)
	badGenericOverflow := Identity[int8](128)
	badGenericFraction := Identity[int](1.5)
	badGenericKinds := Choose(true, 1)
	badGenericArity := Choose(1)
	badGenericExtra := Identity[int, string](1)
	badGenericContainer := Element(1)
	badGenericSpread := First((1)...)
	badGenericMixed := First(small, other)
	_ = []any{badGenericDistinct, badGenericBuiltin, badGenericOverflow, badGenericFraction,
		badGenericKinds, badGenericArity, badGenericExtra, badGenericContainer,
		badGenericSpread, badGenericMixed}
}

func Query(real float64, sendOnly chan<- int, integer int) {
	const narrow int8 = 127
	badMixed := real + 1i
	badRoundedFloat := integer + 0.999999999999999999999999999999
	badOverflow := 1 << 100
	badDivision := 1 / 0
	badUnary := !1
	badBoolean := true + false
	badTypedOverflow := narrow + 1
	badReceive := <-sendOnly
	badAddress := &Pointer()
	badTilde := ~1
	_ = []any{badMixed, badOverflow, badDivision, badUnary, badBoolean,
		badTypedOverflow, badReceive, badAddress, badTilde, badRoundedFloat}
}

func Named(a Count, b OtherCount, builtin int, channel Send[int], duration time.Duration,
	raw int64, first Number[int], different Number[string]) {
	const limit Small = 127
	badDistinct := a + b
	badBuiltin := a + builtin
	badNamedOverflow := limit + 1
	badNamedReceive := <-channel
	badImported := duration + raw
	badInstantiation := first + different
	_ = []any{badDistinct, badBuiltin, badNamedOverflow, badNamedReceive,
		badImported, badInstantiation}
}

type IntegralSet interface{ ~int | ~int64 }
type NarrowSet interface {
	IntegralSet
	~int
}
type EmptySet interface {
	~int
	~string
}
type StrictSet interface {
	comparable
	~int | ~[]int
}

func SetIntegral[T IntegralSet](value T) T      { return value }
func SetNarrow[T NarrowSet](value T) T          { return value }
func SetEmpty[T EmptySet](value T) T            { return value }
func SetStrict[T StrictSet](value T) T          { return value }
func SetNoCore[T interface{ int | string }]() T { var zero T; return zero }
func SetMethod[T interface {
	~int
	Read() string
}](value T) T {
	return value
}
func InvalidSets(text string, wide int64, count Count) {
	badSetIntegral := SetIntegral(text)
	badSetNarrow := SetNarrow(wide)
	badSetEmpty := SetEmpty(count)
	badSetStrict := SetStrict([]int{})
	badSetNoCore := SetNoCore()
	badSetMethod := SetMethod(count)
	badInferNil := Identity(nil)
	_ = []any{badSetIntegral, badSetNarrow, badSetEmpty, badSetStrict, badSetNoCore, badSetMethod, badInferNil}
}
func BadOverlap[T interface{ ~int | int }](value T) T          { return value }
func BadNamedTilde[T interface{ ~Count }](value T) T           { return value }
func BadInterfaceTilde[T interface{ ~interface{} }](value T) T { return value }
func BadMethodUnion[T interface {
	interface{ Read() string } | int
}](value T) T {
	return value
}
func BadComparableUnion[T interface{ comparable | int }](value T) T { return value }
func InvalidSetDeclarations(count Count) {
	badOverlap := BadOverlap(count)
	badNamedTilde := BadNamedTilde(count)
	badInterfaceTilde := BadInterfaceTilde(count)
	badMethodUnion := BadMethodUnion(count)
	badComparableUnion := BadComparableUnion(count)
	_ = []any{badOverlap, badNamedTilde, badInterfaceTilde, badMethodUnion, badComparableUnion}
}
func SetStrictStruct[T interface {
	comparable
	~struct{ Value int } | ~struct{ Value any }
}](value T) T {
	return value
}
func InvalidStrictStruct() {
	badSetStruct := SetStrictStruct(struct{ Value any }{})
	_ = badSetStruct
}

type CheckedSet[E comparable] interface{ ~[]E }

func SetChecked[S CheckedSet[E], E comparable](value S) E { var zero E; return zero }
func InvalidSetBound(slices [][]int) {
	badSetBound := SetChecked(slices)
	_ = badSetBound
}

type NativeExtractor[E any] interface{ Extract() E }

func NativeExtract[M NativeExtractor[E], E any](value M) E   { var zero E; return zero }
func NativeArgument[E any](value interface{ Extract() E }) E { var zero E; return zero }

type NativeValue struct{}

func (NativeValue) Extract() Count { return 0 }

type NativePointer struct{}

func (*NativePointer) Extract() Count { return 0 }

type NativeShadow struct {
	NativeValue
	Extract int
}
type NativeLeft struct{ NativeValue }
type NativeRight struct{ NativeValue }
type NativeDiamond struct {
	NativeLeft
	NativeRight
}
type NativeField struct{ Extract int }
type NativeSameLevel struct {
	NativeValue
	NativeField
}
type NativeDefined NativeValue
type NativeDefinedPointer *NativeValue
type NativeTwo struct{}

func (NativeTwo) First() Count  { return 0 }
func (NativeTwo) Second(string) {}
func NativeTwoExtract[M interface {
	First() E
	Second(E)
}, E any](value M) E {
	var zero E
	return zero
}
func InvalidNativeMethods(pointer NativePointer, shadow NativeShadow, diamond NativeDiamond, sameLevel NativeSameLevel,
	defined NativeDefined, definedPointer NativeDefinedPointer, double **NativeValue, iface *NativeExtractor[Count], value NativeValue, two NativeTwo) {
	badNativePointer := NativeExtract(pointer)
	badNativeArgument := NativeArgument(pointer)
	badNativeShadow := NativeExtract(shadow)
	badNativeDiamond := NativeExtract(diamond)
	badNativeSameLevel := NativeExtract(sameLevel)
	badNativeDefined := NativeExtract(defined)
	badNativeDefinedPointer := NativeExtract(definedPointer)
	badNativeDouble := NativeExtract(double)
	badNativeInterfacePointer := NativeExtract(iface)
	badNativeExplicit := NativeExtract[NativeValue, int](value)
	badNativeTwo := NativeTwoExtract(two)
	_ = []any{badNativePointer, badNativeArgument, badNativeShadow, badNativeDiamond, badNativeSameLevel, badNativeDefined, badNativeDefinedPointer, badNativeDouble, badNativeInterfacePointer, badNativeExplicit, badNativeTwo}
}

type IndexItem struct{}

func InvalidIndex(slice []*IndexItem, array [2]*IndexItem, mapping map[Count]*IndexItem, text string, float float64, pointer *[]*IndexItem) {
	badIndexNegative := slice[-1]
	badIndexFloat := slice[float]
	badIndexFraction := slice[1.5]
	badIndexTypedFloat := slice[float64(1)]
	badIndexOverflow := slice[1<<100]
	badIndexArrayBound := array[2]
	badIndexArrayNegative := array[-1]
	badIndexPointerSlice := pointer[0]
	badIndexArrayLiteral := ([2]*IndexItem{})[2]
	badIndexString := "a"[1]
	badIndexStringUTF8 := "界"[3]
	badIndexStringConcat := ("a" + "b")[2]
	badIndexStringRune := string('界')[3]
	badIndexText := text[true]
	badIndexMap := mapping["x"]
	badIndexMapNamed := mapping[int(1)]
	badIndexMapNil := mapping[nil]
	badIndexComma, badIndexCommaOK := slice[0]
	_ = []any{badIndexNegative, badIndexFloat, badIndexFraction, badIndexTypedFloat, badIndexOverflow, badIndexArrayBound, badIndexArrayNegative, badIndexPointerSlice, badIndexArrayLiteral, badIndexString, badIndexStringUTF8, badIndexStringConcat, badIndexStringRune, badIndexText, badIndexMap, badIndexMapNamed, badIndexMapNil, badIndexComma, badIndexCommaOK}
}

type IndexArray[T comparable] [2]int

func InvalidIndexFormal[T any]() {
	badIndexFormal := IndexArray[T]{}[0]
	_ = badIndexFormal
}

type SliceBadHolder struct{ Array [3]*IndexItem }

func SliceBadArray() [3]*IndexItem   { return [3]*IndexItem{} }
func SliceBadStruct() SliceBadHolder { return SliceBadHolder{} }
func InvalidSlices(items []*IndexItem, array [3]*IndexItem, pointer *[3]*IndexItem, mapping map[string]*IndexItem, text string, floating float64, mapStruct map[string]SliceBadHolder) {
	badSliceNegative := items[-1:]
	badSliceFraction := items[1.5:]
	badSliceTypedFloat := items[float64(1):]
	badSliceFloatValue := items[floating:]
	badSliceOverflow := items[:1<<100]
	badSliceOrder := items[2:1]
	badSliceFullOrder := items[0:2:1]
	badSliceFullLow := items[2:2:1]
	badSliceArrayHigh := array[:4]
	badSliceArrayLow := array[4:]
	badSliceArrayMax := array[:2:4]
	badSlicePointer := pointer[:4]
	badSliceString := "a"[:2]
	badSliceUTF8 := "界"[:4]
	badSliceConcat := ("a" + "b")[:3]
	badSliceRune := string('界')[:4]
	badSliceStringFull := text[:1:2]
	badSliceMap := mapping[:]
	badSliceLiteral := ([3]*IndexItem{})[:]
	badSliceCall := SliceBadArray()[:]
	badSliceStruct := SliceBadStruct().Array[:]
	badSliceMapField := mapStruct["k"].Array[:]
	badSliceComma, badSliceCommaOK := items[:]
	_ = []any{badSliceNegative, badSliceFraction, badSliceTypedFloat, badSliceFloatValue, badSliceOverflow, badSliceOrder, badSliceFullOrder, badSliceFullLow, badSliceArrayHigh, badSliceArrayLow, badSliceArrayMax, badSlicePointer, badSliceString, badSliceUTF8, badSliceConcat, badSliceRune, badSliceStringFull, badSliceMap, badSliceLiteral, badSliceCall, badSliceStruct, badSliceMapField, badSliceComma, badSliceCommaOK}
}
func InvalidSliceFormal[T any]() {
	badSliceFormal := (&IndexArray[T]{})[:]
	_ = badSliceFormal
}

type RangeBadBool bool
type RangeBadSequence func(func(*IndexItem) RangeBadBool)

func RangeNoYield()                                 {}
func RangeWrongYield(value int)                     {}
func RangeWrongReturn(yield func(int) bool) int     { return 0 }
func RangeYieldNoReturn(yield func(int))            {}
func RangeYieldMany(yield func(int, int, int) bool) {}
func RangeYieldVariadic(yield func(...int) bool)    {}
func RangeOuterVariadic(yield ...func(int) bool)    {}
func RangeZero(yield func() bool)                   {}
func InvalidRanges(send chan<- *IndexItem, pointer *[]*IndexItem, double **[2]*IndexItem, sequence RangeBadSequence, floating float64) {
	for badRangeSend := range send {
		_ = badRangeSend
	}
	for badRangePointer := range pointer {
		_ = badRangePointer
	}
	for badRangeDouble := range double {
		_ = badRangeDouble
	}
	for badRangeFloat := range floating {
		_ = badRangeFloat
	}
	for badRangeFloatLiteral := range 3.0 {
		_ = badRangeFloatLiteral
	}
	for badRangeOverflow := range 1 << 100 {
		_ = badRangeOverflow
	}
	for badRangeIntKey, badRangeIntValue := range 3 {
		_ = []any{badRangeIntKey, badRangeIntValue}
	}
	for badRangeNoYield := range RangeNoYield {
		_ = badRangeNoYield
	}
	for badRangeWrongYield := range RangeWrongYield {
		_ = badRangeWrongYield
	}
	for badRangeWrongReturn := range RangeWrongReturn {
		_ = badRangeWrongReturn
	}
	for badRangeYieldNoReturn := range RangeYieldNoReturn {
		_ = badRangeYieldNoReturn
	}
	for badRangeYieldMany := range RangeYieldMany {
		_ = badRangeYieldMany
	}
	for badRangeYieldVariadic := range RangeYieldVariadic {
		_ = badRangeYieldVariadic
	}
	for badRangeOuterVariadic := range RangeOuterVariadic {
		_ = badRangeOuterVariadic
	}
	for badRangeZero := range RangeZero {
		_ = badRangeZero
	}
	for badRangeBool := range sequence {
		_ = badRangeBool
	}
}
func InvalidRangeFormal[T any]() {
	for badRangeFormal := range [1]IndexArray[T]{} {
		_ = badRangeFormal
	}
}

func BoundAny[K any](values map[K]*ValueReader) {
	for _, boundAny := range values {
		_ = boundAny
	}
}
func BoundMethods[K interface{ Method() }](values map[K]*ValueReader) {
	for _, boundMethods := range values {
		_ = boundMethods
	}
}
func BoundUnion[K interface{ ~int | ~[]int }](values map[K]*ValueReader) {
	for _, boundUnion := range values {
		_ = boundUnion
	}
}
func BoundInterfaceField[K interface{ ~struct{ Field any } }](values map[K]*ValueReader) {
	for _, boundInterfaceField := range values {
		_ = boundInterfaceField
	}
}

type CoreBound[K comparable] interface{ ~map[K]*ValueReader }

func CoreBadElements[S interface{ ~[]*ValueReader | ~[]int }](values S) {
	for badCoreElements := range values {
		_ = badCoreElements
	}
}
func CoreBadLengths[S interface {
	~[1]*ValueReader | ~[2]*ValueReader
}](values S) {
	for badCoreLengths := range values {
		_ = badCoreLengths
	}
}
func CoreBadSend[C interface {
	~chan *ValueReader | ~chan<- *ValueReader
}](values C) {
	for badCoreSend := range values {
		_ = badCoreSend
	}
}
func CoreBadDirections[C interface {
	~<-chan *ValueReader | ~chan<- *ValueReader
}](values C) {
	for badCoreDirections := range values {
		_ = badCoreDirections
	}
}
func CoreBadChannelElements[C interface{ ~chan *ValueReader | ~chan int }](values C) {
	for badCoreChannelElements := range values {
		_ = badCoreChannelElements
	}
}
func CoreBadConstructors[S interface {
	~[]*ValueReader | ~[2]*ValueReader
}](values S) {
	for badCoreConstructors := range values {
		_ = badCoreConstructors
	}
}
func CoreBadAny[S any](values S) {
	for badCoreAny := range values {
		_ = badCoreAny
	}
}
func CoreBadIntegers[N interface{ ~int | ~int64 }](values N) {
	for badCoreIntegers := range values {
		_ = badCoreIntegers
	}
}
func CoreBadCallback[Y interface {
	~func(*ValueReader) bool | ~func(int) bool
}, F ~func(Y)](values F) {
	for badCoreCallback := range values {
		_ = badCoreCallback
	}
}
func CoreBadEmpty[S interface {
	~[]*ValueReader
	~[2]*ValueReader
}](values S) {
	for badCoreEmpty := range values {
		_ = badCoreEmpty
	}
}
func CoreBadPointer[E ~[2]*ValueReader, P ~*E](values P) {
	for badCorePointer := range values {
		_ = badCorePointer
	}
}
func CoreBadHeader[K any, S CoreBound[K]](values S) {
	for _, coreBadHeader := range values {
		_ = coreBadHeader
	}
}

type CoreDependent[E any, S ~[]E] interface{ ~[]*ValueReader }

func CoreBadCheck[E any, S CoreDependent[E, []int]](values S) {
	for _, coreBadCheck := range values {
		_ = coreBadCheck
	}
}

type ImpInt int
type ImpWide[N interface{ ~int | ~int64 }] interface{ ~[]*ValueReader }
type ImpExact[N interface{ int }] interface{ ~[]*ValueReader }
type ImpNamedExact[N interface{ ImpInt }] interface{ ~[]*ValueReader }
type ImpRead[N interface{ Read(string) string }] interface{ ~[]*ValueReader }
type ImpComparable[N comparable] interface{ ~[]*ValueReader }
type ImpReceive[N interface{ ~<-chan int }] interface{ ~[]*ValueReader }
type ImpEmptyTarget[N interface {
	~int
	~string
}] interface{ ~[]*ValueReader }

func ImpBadAny[N any, S ImpWide[N]](values S) {
	for _, impBadAny := range values {
		_ = impBadAny
	}
}
func ImpBadUnion[N interface{ ~int | ~string }, S ImpWide[N]](values S) {
	for _, impBadUnion := range values {
		_ = impBadUnion
	}
}
func ImpBadApprox[N ~int, S ImpExact[N]](values S) {
	for _, impBadApprox := range values {
		_ = impBadApprox
	}
}
func ImpBadNamed[N interface{ int }, S ImpNamedExact[N]](values S) {
	for _, impBadNamed := range values {
		_ = impBadNamed
	}
}
func ImpBadMethod[N interface{ Read(int) string }, S ImpRead[N]](values S) {
	for _, impBadMethod := range values {
		_ = impBadMethod
	}
}
func ImpBadMissing[N interface{ Extra() }, S ImpRead[N]](values S) {
	for _, impBadMissing := range values {
		_ = impBadMissing
	}
}
func ImpBadDynamic[N interface{ ~struct{ X any } }, S ImpComparable[N]](values S) {
	for _, impBadDynamic := range values {
		_ = impBadDynamic
	}
}
func ImpBadChannel[N interface{ ~chan int }, S ImpReceive[N]](values S) {
	for _, impBadChannel := range values {
		_ = impBadChannel
	}
}
func ImpBadEmpty[N ~int, S ImpEmptyTarget[N]](values S) {
	for _, impBadEmpty := range values {
		_ = impBadEmpty
	}
}

type RecMap[K comparable] map[K]*ValueReader
type RecPointer[K comparable] *struct{}
type RecInterface[K comparable] interface{ Method() K }
type RecAlias[K comparable] = RecMap[K]
type RecAny[T any] struct{}

func (_ RecPointer[K]) InvalidReceiverPointer(values map[K]*ValueReader) {
	for _, badReceiverPointer := range values {
		_ = badReceiverPointer
	}
}
func (_ RecInterface[K]) InvalidReceiverInterface(values map[K]*ValueReader) {
	for _, badReceiverInterface := range values {
		_ = badReceiverInterface
	}
}
func (_ RecAlias[K]) InvalidReceiverAlias(values map[K]*ValueReader) {
	for _, badGenericReceiverAlias := range values {
		_ = badGenericReceiverAlias
	}
}
func (_ RecMap[K, K]) InvalidReceiverDuplicate(values map[K]*ValueReader) {
	for _, badReceiverDuplicate := range values {
		_ = badReceiverDuplicate
	}
}
func (_ RecMap[K, Extra]) InvalidReceiverArity(values map[K]*ValueReader) {
	for _, badReceiverArity := range values {
		_ = badReceiverArity
	}
}
func (_ RecAny[T]) InvalidReceiverAny(values T) {
	for badReceiverAny := range values {
		_ = badReceiverAny
	}
}

func (_ (RecAny[Count])) InvalidParenthesizedReceiver(values map[Count]*ValueReader) {
 for _, badParenthesizedReceiver := range values { _ = badParenthesizedReceiver }
}

func (_ **RecMap[K]) InvalidDoublePointerReceiver(values map[K]*ValueReader) {
 for _, badDoublePointerReceiver := range values { _ = badDoublePointerReceiver }
}


type FieldLeft[S any] struct{ Values S }
type FieldRight[S any] struct{ Values S }
type FieldAmbiguous[S ~[]*ValueReader] struct{ FieldLeft[S]; FieldRight[S] }
type FieldMethod[S ~[]*ValueReader] struct{ FieldLeft[S] }
func (*FieldMethod[S]) Values() S { var zero S; return zero }
func (box FieldAmbiguous[S]) InvalidFieldAmbiguity() { for _, badFieldAmbiguity := range box.Values { _ = badFieldAmbiguity } }
func (box FieldMethod[S]) InvalidFieldMethod() { for _, badFieldMethod := range box.Values { _ = badFieldMethod } }
func (box *FieldMethod[S]) InvalidFieldPointerMethod() { for _, badFieldPointerMethod := range box.Values { _ = badFieldPointerMethod } }
type FieldBox[K comparable,S ~map[K]*ValueReader] struct { Values S; Slots [2]S }
func (box FieldBox[K,S]) InvalidFieldBounds() {
 for _, badFieldHigh := range box.Slots[0:3][0] { _ = badFieldHigh }
 for _, badFieldIndex := range box.Slots[-1] { _ = badFieldIndex }
 for _, badFieldString := range box.Slots["x"] { _ = badFieldString }
}
type FieldArrayBox[S any] struct{ Values [2]*ValueReader; Marker S }
type FieldModes[S any] struct{ Values map[string]FieldArrayBox[S] }
func (box FieldModes[S]) InvalidFieldAddressability() { for _, badFieldAddressability := range box.Values["k"].Values[:] { _ = badFieldAddressability } }
func InvalidFormalField[T ~struct{Values []*ValueReader}](box T) { for _, badFormalField := range box.Values { _ = badFormalField } }

func InvalidFormalPointerField[K comparable,S ~map[K]*ValueReader,P ~*FieldBox[K,S]](box P) {
 for _, badFormalPointerField := range box.Values { _ = badFormalPointerField }
}


func ParamBadKey[K comparable,L comparable](values map[K][]*ValueReader,key L) { for _, badParamKey := range values[key] { _ = badParamKey } }
func ParamBadSimilarKey[K ~int,L ~int](values map[K][]*ValueReader,key L) { for _, badParamSimilarKey := range values[key] { _ = badParamSimilarKey } }
func ParamBadElements[P interface{ ~[2][]*ValueReader | ~[][]int }](values P) { for _, badParamElements := range values[0] { _ = badParamElements } }
func ParamBadMapMix[P interface{ ~map[int][]*ValueReader | ~[][]*ValueReader }](values P) { for _, badParamMapMix := range values[0] { _ = badParamMapMix } }
func ParamBadMapKeys[P interface{ ~map[int][]*ValueReader | ~map[int64][]*ValueReader }](values P) { for _, badParamMapKeys := range values[0] { _ = badParamMapKeys } }
func ParamBadBounds[P interface{ ~[2][]*ValueReader | ~[3][]*ValueReader }](values P) { for _, badParamBounds := range values[2] { _ = badParamBounds } }
func ParamBadIndex[P ~[][]*ValueReader,I interface{ ~int | ~float64 }](values P,index I) { for _, badParamIndex := range values[index] { _ = badParamIndex } }
func ParamBadFullString[P interface{ ~string | ~[]byte }](values P) { for badParamFullString := range values[0:1:1][0] { _ = badParamFullString } }
func ParamBadSliceKinds[P interface{ ~[2]*ValueReader | ~[]*ValueReader }](values P) { for _, badParamSliceKinds := range values[:] { _ = badParamSliceKinds } }
func ParamBadHigh[P ~[2]*ValueReader](values P) { for _, badParamHigh := range values[0:3] { _ = badParamHigh } }
func ParamBadConst[Count ~int](values map[Count][]*ValueReader) {
 const key Count = 0
 for _, badParamConst := range values[key] { _ = badParamConst }
}

func BuiltinBadLen() { badBuiltinLen := len(3); _ = badBuiltinLen }
func BuiltinBadCap() { badBuiltinCap := cap("abc"); _ = badBuiltinCap }
func BuiltinBadLenNil() { badBuiltinLenNil := len(nil); _ = badBuiltinLenNil }
func BuiltinBadLenArity() { badBuiltinLenArity := len(); _ = badBuiltinLenArity }
func BuiltinBadSpread(values []int) { badBuiltinLenSpread := len(values...); _ = badBuiltinLenSpread }
func BuiltinBadAppendNil() { badBuiltinAppendNil := append(nil, 1); _ = badBuiltinAppendNil }
func BuiltinBadAppendType(values []int) { badBuiltinAppendType := append(values, "abc"); _ = badBuiltinAppendType }
func BuiltinBadAppendOverflow(values []uint8) { badBuiltinAppendOverflow := append(values, 256); _ = badBuiltinAppendOverflow }
func BuiltinBadAppendSpread(values []int, other []int64) { badBuiltinAppendSpread := append(values, other...); _ = badBuiltinAppendSpread }
func BuiltinBadAppendString(values []rune) { badBuiltinAppendString := append(values, "abc"...); _ = badBuiltinAppendString }
func BuiltinBadMakeType() { badBuiltinMakeType := make(int, 1); _ = badBuiltinMakeType }
func BuiltinBadMakeSize() { badBuiltinMakeSize := make([]int); _ = badBuiltinMakeSize }
func BuiltinBadMakeNegative() { badBuiltinMakeNegative := make([]int, -1); _ = badBuiltinMakeNegative }
func BuiltinBadMakeFraction() { badBuiltinMakeFraction := make([]int, 1.5); _ = badBuiltinMakeFraction }
func BuiltinBadMakeTypedFloat(size float64) { badBuiltinMakeTypedFloat := make([]int, size); _ = badBuiltinMakeTypedFloat }
func BuiltinBadMakeCapacity() { badBuiltinMakeCapacity := make([]int, 3, 2); _ = badBuiltinMakeCapacity }
func BuiltinBadMakeMap() { badBuiltinMakeMap := make(map[int]int, 1, 2); _ = badBuiltinMakeMap }
func BuiltinBadMakeChannel() { badBuiltinMakeChannel := make(chan int, -1); _ = badBuiltinMakeChannel }
func BuiltinBadMakeKey() { badBuiltinMakeKey := make(map[[]int]int); _ = badBuiltinMakeKey }
func BuiltinBadCopyNil() { badBuiltinCopyNil := copy(nil, "abc"); _ = badBuiltinCopyNil }
func BuiltinBadCopyType(values []int, other []int64) { badBuiltinCopyType := copy(values, other); _ = badBuiltinCopyType }
func BuiltinBadCopyString(values []uint16) { badBuiltinCopyString := copy(values, "abc"); _ = badBuiltinCopyString }
func BuiltinBadNewNil() { badBuiltinNewNil := new(nil); _ = badBuiltinNewNil }
func BuiltinBadNewSpread(values []int) { badBuiltinNewSpread := new(values...); _ = badBuiltinNewSpread }
func BuiltinBadRecover() { badBuiltinRecover := recover(1); _ = badBuiltinRecover }
func BuiltinBadClear(values []int) { badBuiltinClear := clear(values); _ = badBuiltinClear }
func BuiltinBadDelete(values map[int]int) { badBuiltinDelete := delete(values, 1); _ = badBuiltinDelete }
func BuiltinBadFormal[T interface{~string|~int}](value T) { badBuiltinFormal := len(value); _ = badBuiltinFormal }
func BuiltinBadFormalCap[T interface{~string|~[]int}](value T) { badBuiltinFormalCap := cap(value); _ = badBuiltinFormalCap }
func BuiltinBadConstantBound(values [3]int) { const length = len("abc"); badBuiltinConstantBound := values[length]; _ = badBuiltinConstantBound }
func BuiltinBadArrayBound(values [3]int) { const length = cap([3]int{}); badBuiltinArrayBound := values[length]; _ = badBuiltinArrayBound }

func BuiltinBadNewMapKey() { badBuiltinNewMapKey := new(map[[]int]int); _ = badBuiltinNewMapKey }
func BuiltinBadMakeNestedKey() { badBuiltinMakeNestedKey := make([]map[[]int]int, 1); _ = badBuiltinMakeNestedKey }

type NumericFloat float32
type NumericOtherFloat float32
func badNumericIntegerCase(value int, c complex64, values []float32) { badNumericInteger := complex(value, value); _ = badNumericInteger }
func badNumericDifferentCase(value int, c complex64, values []float32) { badNumericDifferent := complex(float32(1), float64(1)); _ = badNumericDifferent }
func badNumericNamesCase(value int, c complex64, values []float32) { badNumericNames := complex(NumericFloat(1), NumericOtherFloat(1)); _ = badNumericNames }
func badNumericNamedBasicCase(value int, c complex64, values []float32) { badNumericNamedBasic := complex(NumericFloat(1), float32(1)); _ = badNumericNamedBasic }
func badNumericImagArgumentCase(value int, c complex64, values []float32) { badNumericImagArgument := complex(1i, 2); _ = badNumericImagArgument }
func badNumericBooleanCase(value int, c complex64, values []float32) { badNumericBoolean := complex(true, 2); _ = badNumericBoolean }
func badNumericOverflowCase(value int, c complex64, values []float32) { badNumericOverflow := complex(float32(1), 1e40); _ = badNumericOverflow }
func badNumericArityCase(value int, c complex64, values []float32) { badNumericArity := complex(1); _ = badNumericArity }
func badNumericExcessCase(value int, c complex64, values []float32) { badNumericExcess := complex(1, 2, 3); _ = badNumericExcess }
func badNumericSpreadCase(value int, c complex64, values []float32) { badNumericSpread := complex(values...); _ = badNumericSpread }
func badNumericRealFloatCase(value int, c complex64, values []float32) { badNumericRealFloat := real(float32(1)); _ = badNumericRealFloat }
func badNumericImagIntegerCase(value int, c complex64, values []float32) { badNumericImagInteger := imag(value); _ = badNumericImagInteger }
func badNumericRealBooleanCase(value int, c complex64, values []float32) { badNumericRealBoolean := real(true); _ = badNumericRealBoolean }
func badNumericRealArityCase(value int, c complex64, values []float32) { badNumericRealArity := real(1, 2); _ = badNumericRealArity }
func badNumericComplexRealCase(value int, c complex64, values []float32) { badNumericComplexReal := float32(2+1i); _ = badNumericComplexReal }
func badNumericIntegerImagCase(value int, c complex64, values []float32) { badNumericIntegerImag := int(1i); _ = badNumericIntegerImag }
func badNumericImagOverflowCase(value int, c complex64, values []float32) { badNumericImagOverflow := complex64(1+1e40i); _ = badNumericImagOverflow }
func badNumericRealOverflowCase(value int, c complex64, values []float32) { badNumericRealOverflow := complex64(1e40+1i); _ = badNumericRealOverflow }
func badNumericDivideZeroCase(value int, c complex64, values []float32) { badNumericDivideZero := (1+2i) / (1i-1i); _ = badNumericDivideZero }
func badNumericOrderedCase(value int, c complex64, values []float32) { badNumericOrdered := c < c; _ = badNumericOrdered }
func badNumericBoundCase(value int, c complex64, values []float32) { badNumericBound := [3]int{}[real(complex(3, 1))]; _ = badNumericBound }
func badNumericComplexIndexCase(value int, c complex64, values []float32) { badNumericComplexIndex := [3]int{}[complex(1, 1)]; _ = badNumericComplexIndex }
func badNumericTypedIndexCase(value int, c complex64, values []float32) { badNumericTypedIndex := [3]int{}[real(complex64(1))]; _ = badNumericTypedIndex }
func badNumericFractionIndexCase(value int, c complex64, values []float32) { badNumericFractionIndex := [3]int{}[real(complex(1.5, 1))]; _ = badNumericFractionIndex }
func badNumericNegativeIndexCase(value int, c complex64, values []float32) { badNumericNegativeIndex := [3]int{}[imag(complex(1, -1))]; _ = badNumericNegativeIndex }
func badNumericRoundedBoundCase(value int, c complex64, values []float32) { badNumericRoundedBound := [1]int{}[int(real(complex64(16777216)+1))-16777215]; _ = badNumericRoundedBound }
func badNumericTinyImagCase(value int, c complex64, values []float32) { badNumericTinyImag := float64(1+1e-400i); _ = badNumericTinyImag }
func BadNumericFormal[T ~float32](value T) { badNumericFormal := complex(value,value); _ = badNumericFormal }


type Item struct{}

type MethodBox[T any] struct{ Value T }
func (b MethodBox[T]) Get() T { return b.Value }
func (b *MethodBox[T]) Put(value T) *Item { return nil }
func (b MethodBox[T]) Spread(values ...T) T { return b.Value }
func (b MethodBox[T]) Pair() (T, bool) { return b.Value, true }
type MethodOuter[T any] struct{ MethodBox[T] }
type MethodPointerOuter[T any] struct{ *MethodBox[T] }
type MethodView[T any] interface{ Get() T; Put(T) *Item }
type MethodAlias = MethodBox[*Item]
type MethodGenericAlias[T any] = MethodBox[T]
type MethodDefinedPointer *MethodBox[*Item]
type MethodDerived MethodBox[*Item]
type MethodLeft struct{ MethodBox[*Item] }
type MethodRight struct{ MethodBox[*Item] }
type MethodDiamond struct{ MethodLeft; MethodRight }
type MethodBlocked struct{ Get int; MethodBox[*Item] }
type MethodLimited[T comparable] struct{ Value T }
func (b MethodLimited[T]) Get() T { return b.Value }
func MethodApply[T any](fn func() T) T { return fn() }
func badMethodExprPointerCase(box MethodBox[*Item], pointer *MethodBox[*Item], entries map[int]MethodBox[*Item], named MethodDefinedPointer, iface *MethodView[*Item]) { badMethodExprPointer := MethodBox[*Item].Put; _ = badMethodExprPointer }
func badMethodTemporaryCase(box MethodBox[*Item], pointer *MethodBox[*Item], entries map[int]MethodBox[*Item], named MethodDefinedPointer, iface *MethodView[*Item]) { badMethodTemporary := MethodBox[*Item]{}.Put; _ = badMethodTemporary }
func badMethodMapPointerCase(box MethodBox[*Item], pointer *MethodBox[*Item], entries map[int]MethodBox[*Item], named MethodDefinedPointer, iface *MethodView[*Item]) { badMethodMapPointer := entries[0].Put; _ = badMethodMapPointer }
func badMethodNoReceiverCase(box MethodBox[*Item], pointer *MethodBox[*Item], entries map[int]MethodBox[*Item], named MethodDefinedPointer, iface *MethodView[*Item]) { badMethodNoReceiver := MethodBox[*Item].Get(); _ = badMethodNoReceiver }
func badMethodWrongReceiverCase(box MethodBox[*Item], pointer *MethodBox[*Item], entries map[int]MethodBox[*Item], named MethodDefinedPointer, iface *MethodView[*Item]) { badMethodWrongReceiver := MethodBox[*Item].Get(MethodBox[int]{}); _ = badMethodWrongReceiver }
func badMethodUnboundArgumentCase(box MethodBox[*Item], pointer *MethodBox[*Item], entries map[int]MethodBox[*Item], named MethodDefinedPointer, iface *MethodView[*Item]) { badMethodUnboundArgument := (*MethodBox[*Item]).Put(pointer, 2); _ = badMethodUnboundArgument }
func badMethodBoundArityCase(box MethodBox[*Item], pointer *MethodBox[*Item], entries map[int]MethodBox[*Item], named MethodDefinedPointer, iface *MethodView[*Item]) { badMethodBoundArity := box.Get(1); _ = badMethodBoundArity }
func badMethodBoundTypeCase(box MethodBox[*Item], pointer *MethodBox[*Item], entries map[int]MethodBox[*Item], named MethodDefinedPointer, iface *MethodView[*Item]) { badMethodBoundType := box.Put(2); _ = badMethodBoundType }
func badMethodParenTypeCase(box MethodBox[*Item], pointer *MethodBox[*Item], entries map[int]MethodBox[*Item], named MethodDefinedPointer, iface *MethodView[*Item]) { badMethodParenType := (box.Put)(2); _ = badMethodParenType }
func badMethodSpreadCase(box MethodBox[*Item], pointer *MethodBox[*Item], entries map[int]MethodBox[*Item], named MethodDefinedPointer, iface *MethodView[*Item]) { badMethodSpread := box.Spread([]int{}...); _ = badMethodSpread }
func badMethodExprExtraCase(box MethodBox[*Item], pointer *MethodBox[*Item], entries map[int]MethodBox[*Item], named MethodDefinedPointer, iface *MethodView[*Item]) { badMethodExprExtra := MethodBox[*Item].Get(box,nil); _ = badMethodExprExtra }
func badMethodAmbiguityCase(box MethodBox[*Item], pointer *MethodBox[*Item], entries map[int]MethodBox[*Item], named MethodDefinedPointer, iface *MethodView[*Item]) { badMethodAmbiguity := MethodDiamond{}.Get; _ = badMethodAmbiguity }
func badMethodFieldCallCase(box MethodBox[*Item], pointer *MethodBox[*Item], entries map[int]MethodBox[*Item], named MethodDefinedPointer, iface *MethodView[*Item]) { badMethodFieldCall := MethodBlocked{}.Get(); _ = badMethodFieldCall }
func badMethodDefinedPointerCase(box MethodBox[*Item], pointer *MethodBox[*Item], entries map[int]MethodBox[*Item], named MethodDefinedPointer, iface *MethodView[*Item]) { badMethodDefinedPointer := named.Get; _ = badMethodDefinedPointer }
func badMethodPointerInterfaceCase(box MethodBox[*Item], pointer *MethodBox[*Item], entries map[int]MethodBox[*Item], named MethodDefinedPointer, iface *MethodView[*Item]) { badMethodPointerInterface := iface.Get; _ = badMethodPointerInterface }
func badMethodConstraintCase(box MethodBox[*Item], pointer *MethodBox[*Item], entries map[int]MethodBox[*Item], named MethodDefinedPointer, iface *MethodView[*Item]) { badMethodConstraint := MethodLimited[[]int]{}.Get; _ = badMethodConstraint }
func badMethodExprConstraintCase(box MethodBox[*Item], pointer *MethodBox[*Item], entries map[int]MethodBox[*Item], named MethodDefinedPointer, iface *MethodView[*Item]) { badMethodExprConstraint := MethodLimited[[]int].Get; _ = badMethodExprConstraint }
func badMethodDerivedCase(box MethodBox[*Item], pointer *MethodBox[*Item], entries map[int]MethodBox[*Item], named MethodDefinedPointer, iface *MethodView[*Item]) { badMethodDerived := MethodDerived.Get; _ = badMethodDerived }

type MethodInvalidAlias[T any] = T
func MethodAliasRHS() { badMethodAliasRHS := MethodInvalidAlias[MethodBox[*Item]]{}.Get; _ = badMethodAliasRHS }
func MethodBadAddress(box MethodBox[*Item], entries map[int]MethodBox[*Item]) { badMethodAddressMap := &entries[0]; _ = badMethodAddressMap }
func MethodBadDereference(box MethodBox[*Item]) { badMethodDerefValue := *box; _ = badMethodDerefValue }


type AssertReader interface { Get() *Item }
type AssertWriter interface { Put(*Item) *Item }
func badAssertConcreteCase(value int) { badAssertConcrete := value.(int); _ = badAssertConcrete }
func badAssertMissingCase(value AssertReader) { badAssertMissing := value.(int); _ = badAssertMissing }
func badAssertSignatureCase(value AssertReader) { badAssertSignature := value.(MethodBox[int]); _ = badAssertSignature }
func badAssertPointerMethodCase(value AssertWriter) { badAssertPointerMethod := value.(MethodBox[*Item]); _ = badAssertPointerMethod }
func badAssertDefinedPointerCase(value AssertReader) { badAssertDefinedPointer := value.(MethodDefinedPointer); _ = badAssertDefinedPointer }
func badAssertAmbiguousCase(value AssertReader) { badAssertAmbiguous := value.(MethodDiamond); _ = badAssertAmbiguous }
func badAssertBlockedCase(value AssertReader) { badAssertBlocked := value.(MethodBlocked); _ = badAssertBlocked }
func badAssertAddressCase(value any) { badAssertAddress := &value.(Item); _ = badAssertAddress }
func badAssertTupleConcreteCase(value int) { badAssertTupleConcrete := value.(Item); _ = badAssertTupleConcrete }
func badAssertShadowCase(value any, Item int) { badAssertShadow := value.(Item); _ = badAssertShadow }
func badAssertShadowNestedCase(value any, Item int) { badAssertShadowNested := value.([]Item); _ = badAssertShadowNested }
func badAssertNilCase() { badAssertNil := nil.(Item); _ = badAssertNil }
func badAssertTupleCase(value int) { badAssertTupleValue, badAssertTupleOK := value.(int); _, _ = badAssertTupleValue, badAssertTupleOK }

func BadTypeSwitchConcrete(value int) { switch narrowed := value.(type) { default: badSwitchConcrete := narrowed; _ = badSwitchConcrete } }
func BadTypeSwitchMissing(value AssertReader) { switch narrowed := value.(type) { case int: badSwitchMissing := narrowed; _ = badSwitchMissing } }
func BadTypeSwitchPointer(value AssertWriter) { switch narrowed := value.(type) { case MethodBox[*Item]: badSwitchPointer := narrowed; _ = badSwitchPointer } }
func BadTypeSwitchSignature(value AssertReader) { switch narrowed := value.(type) { case MethodBox[int]: badSwitchSignature := narrowed; _ = badSwitchSignature } }
func BadTypeSwitchDuplicate(value any) { switch narrowed := value.(type) { case MethodBox[*Item], MethodAlias: badSwitchDuplicate := narrowed; _ = badSwitchDuplicate } }
func BadTypeSwitchNilDuplicate(value any) { switch narrowed := value.(type) { case nil, nil: badSwitchNilDuplicate := narrowed; _ = badSwitchNilDuplicate } }
func BadTypeSwitchShadow(value any, MethodAlias int) { switch narrowed := value.(type) { case MethodAlias: badSwitchShadow := narrowed; _ = badSwitchShadow } }
func BadTypeSwitchNestedShadow(value any, MethodAlias int) { switch narrowed := value.(type) { case []*MethodAlias: badSwitchNestedShadow := narrowed; _ = badSwitchNestedShadow } }
func BadTypeSwitchNilShadow(value any, nil int) { switch narrowed := value.(type) { case nil: badSwitchNilShadow := narrowed; _ = badSwitchNilShadow } }

func BadClauseAlias(value any) { switch narrowed := value.(type) { case MethodBox[*Item]: badClauseAlias := narrowed; _ = badClauseAlias; case MethodAlias: _ = narrowed } }
func BadClauseNil(value any) { switch narrowed := value.(type) { case nil: badClauseNil := narrowed; _ = badClauseNil; case ((nil)): _ = narrowed } }
func BadClauseStructural(value any) { switch narrowed := value.(type) { case struct{ F int }: badClauseStructural := narrowed; _ = badClauseStructural; case struct{ F int }: _ = narrowed } }
func BadClauseInterface(value any) { switch narrowed := value.(type) { case interface{ Get() *Item }: badClauseInterface := narrowed; _ = badClauseInterface; case interface{ Get() (result *Item) }: _ = narrowed } }
func BadClauseDefault(value any) { switch narrowed := value.(type) { default: badClauseDefault := narrowed; _ = badClauseDefault; default: _ = narrowed } }
func BadClauseLaterMissing(value AssertReader) { switch narrowed := value.(type) { case MethodAlias: badClauseLaterMissing := narrowed; _ = badClauseLaterMissing; case int: _ = narrowed } }
func BadClauseLaterSignature(value AssertReader) { switch narrowed := value.(type) { case MethodAlias: badClauseLaterSignature := narrowed; _ = badClauseLaterSignature; case MethodBox[int]: _ = narrowed } }
func BadClauseParenNilShadow(value any, nil int) { switch narrowed := value.(type) { case ((nil)): badClauseParenNilShadow := narrowed; _ = badClauseParenNilShadow } }
func BadClauseBasicAlias(value any) { switch narrowed := value.(type) { case byte: badClauseBasicAlias := narrowed; _ = badClauseBasicAlias; case uint8: _ = narrowed } }
func BadClauseNilIndex(value any) { switch narrowed := value.(type) { case nil[int]: badClauseNilIndex := narrowed; _ = badClauseNilIndex } }
func BadClauseNilCall(value any) { switch narrowed := value.(type) { case nil(): badClauseNilCall := narrowed; _ = badClauseNilCall } }
func BadClauseNilBinary(value any) { switch narrowed := value.(type) { case nil+1: badClauseNilBinary := narrowed; _ = badClauseNilBinary } }

func BadReceiveSendOnly(channel chan<- *Item) { badReceiveSendOnly := <-channel; _ = badReceiveSendOnly }
func BadReceiveTuple(channel chan<- *Item) { badReceiveTupleValue, badReceiveTupleOK := <-channel; _, _ = badReceiveTupleValue, badReceiveTupleOK }
func BadReceiveNonChannel(value *Item) { badReceiveNonChannel := <-value; _ = badReceiveNonChannel }
func BadReceiveNil() { badReceiveNil := <-nil; _ = badReceiveNil }
func BadReceiveAddress(channel chan Item) { badReceiveAddress := &(<-channel); _ = badReceiveAddress }
func BadReceiveMethod(channel chan MethodBox[*Item]) { badReceiveMethod := (<-channel).Put(nil); _ = badReceiveMethod }
func BadReceiveArraySlice(channel chan [2]int) { badReceiveArraySlice := (<-channel)[:1]; _ = badReceiveArraySlice }
func BadReceiveCallType(channel chan func(int) *Item) { badReceiveCallType := (<-channel)("bad"); _ = badReceiveCallType }
func BadReceiveCallArity(channel chan func(int) *Item) { badReceiveCallArity := (<-channel)(); _ = badReceiveCallArity }
func BadReceiveTypeName() { badReceiveTypeName := <-MethodBox[*Item]; _ = badReceiveTypeName }
func BadReceiveFormalSend[C ~chan *Item | ~chan<- *Item](channel C) { badReceiveFormalSend := <-channel; _ = badReceiveFormalSend }
func BadReceiveFormalElement[C ~chan int | ~chan string](channel C) { badReceiveFormalElement := <-channel; _ = badReceiveFormalElement }
func ReceiveFactory() *Item { return nil }
func BadReceiveSelect() { select { case selected := ReceiveFactory(): badReceiveSelect := selected; _ = badReceiveSelect } }

func BadBindingBasic() { var badBindingBasic int = "wrong"; _ = badBindingBasic }
func BadBindingOverflow() { var badBindingOverflow int8 = 128; _ = badBindingOverflow }
func BadBindingDirection(channel chan<- *Item) { var badBindingDirection <-chan *Item = channel; _ = badBindingDirection }
func BadBindingFormal[T any](value T) { var badBindingFormal string = value; _ = badBindingFormal }
func BadBindingFormalDistinct[T, U any](value U) { var badBindingFormalDistinct T = value; _ = badBindingFormalDistinct }
func BadBindingFormalConstant[T ~int8]() { var badBindingFormalConstant T = 128; _ = badBindingFormalConstant }
func BadBindingFormalNil[T ~int]() { var badBindingFormalNil T = nil; _ = badBindingFormalNil }
func BadBindingCopy() { var invalidBindingSource int = "wrong"; badBindingCopy := invalidBindingSource; _ = badBindingCopy }
