package typeoracle

import (
	"bytes"
	"io"
	"maps"
	"slices"
	"time"
)

type Item struct{ Field int }
type Alias = Item
type Box[T any] struct{ Value T }

func Pointer() *Item            { return nil }
func Identity[T any](value T) T { return value }

func Choose[T any](a, b T) T             { return a }
func Swap[A, B any](a A, b B) (B, A)     { return b, a }
func Element[T any](a []T) T             { return a[0] }
func MapElement[V any](a map[string]V) V { panic("not executed") }
func Pointed[T any](a *T) T              { return *a }
func ArrayElement[T any](a [3]T) T       { return a[0] }
func Read[T any](a <-chan T) T           { return <-a }
func First[T any](a ...T) T              { return a[0] }
func Zero[T any]() T                     { var result T; return result }

type Items []*Item
type ConversionSource struct {
	Value *Item `source`
}
type ConversionTarget struct {
	Value *Item `target`
}
type ConversionPointer *Item
type ConversionPointerOther *Item
type ConversionByte byte
type ConversionBytes []ConversionByte
type ConversionFloat float32
type ConversionReceive <-chan *Item

func Conversions(source ConversionSource, pointer *Item, integer int64, real float64,
	complexValue complex128, data []byte, text string, slice []*Item, channel chan *Item) {
	constant := Small(127)
	builtin := int8(127)
	integral := int8(12.0)
	narrowed := int8(integer)
	realValue := float32(real)
	complexTyped := complex64(complexValue)
	complexConstant := complex128(1.25)
	toText := string(integer)
	literalText := string(65)
	textFromBytes := string(data)
	bytesFromText := []byte(text)
	runesFromText := []rune(text)
	namedBytes := ConversionBytes(text)
	structValue := ConversionTarget(source)
	structPointer := (*ConversionTarget)(&source)
	namedPointer := ConversionPointer(pointer)
	otherPointer := ConversionPointerOther(namedPointer)
	nominal := Number[int](integer)
	array := [2]*Item(slice)
	arrayPointer := (*[2]*Item)(slice)
	receive := (<-chan *Item)(channel)
	namedReceive := ConversionReceive(channel)
	nilPointer := (*Item)(nil)
	nilSlice := []*Item(nil)
	nilMap := map[string]*Item(nil)
	nilChannel := (chan *Item)(nil)
	nilNamed := Stream[Item](nil)
	inferredNil := Identity[*Item](nil)
	const assigned ConversionFloat = 16777217
	rounded32 := Small(int32(assigned) - 16777089)
	rounded64 := Small(int64(float64(9007199254740993)) - 9007199254740865)
	typedRound := Small(float64(127.000000000000000000001))
	explicitDouble := int32(float32(float64(1.0000000596046447753906250000000001)))
	nested := Identity(Small(127))
	_ = []any{constant, builtin, integral, narrowed, realValue, complexTyped, complexConstant,
		toText, literalText, textFromBytes, bytesFromText, runesFromText, namedBytes, structValue,
		structPointer, namedPointer, otherPointer, nominal, array, arrayPointer, receive, namedReceive,
		nilPointer, nilSlice, nilMap, nilChannel, nilNamed, inferredNil, rounded32, rounded64,
		typedRound, explicitDouble, nested}
}

type Key struct {
	ID   int
	Name string
}
type KeyAlias = Key
type EmbeddedPointer = *Item
type EmbeddedChain = EmbeddedPointer
type RecursiveKey struct {
	Next *RecursiveKey
	_    int
}
type GenericKey[T comparable] struct {
	Value   T
	Pair    [2]T
	Pointer *T
}
type Tagged struct {
	Value *Item `json:"value"`
}
type KeyLookup map[Key]*Item

func StructValue[T any](value struct {
	Value T `json:"value"`
}) T {
	return value.Value
}

func Structs(key Key, alias KeyAlias, recursive RecursiveKey, generic GenericKey[string], lookup KeyLookup, now time.Time) {
	comparableKey := Comparable(key)
	comparableAlias := Comparable(alias)
	comparableRecursive := Comparable(recursive)
	comparableGeneric := Comparable(generic)
	comparableTime := Comparable(now)
	cloned := maps.Clone(lookup)
	compacted := slices.Compact([]Key{key})
	inferredField := StructValue(Tagged{})
	anonymous := struct {
		A, B int
		_    [2]int
		*Item
	}{}
	comparableAnonymous := Comparable(anonymous)
	tagIdentity := Choose(struct {
		X int `tag`
	}{}, struct {
		X int "tag"
	}{})
	rawBytes := Identity(struct {
		X int "\xff\x00"
	}{})
	private := Identity(struct{ private int }{})
	nested := Comparable(struct {
		Value struct{ Count int }
		Array [2]Key
	}{})
	unicodeFields := Comparable(struct{ ᴬ, Ĥ, ǅ int }{})
	embeddedAlias := Comparable(struct{ EmbeddedChain }{})
	_ = []any{comparableKey, comparableAlias, comparableRecursive, comparableGeneric,
		comparableTime, cloned, compacted, inferredField, anonymous, comparableAnonymous,
		tagIdentity, rawBytes, private, nested, unicodeFields, embeddedAlias}
}

type Lookup map[string]*Item
type Tiny[T ~int8] int8
type LimitedStream[T comparable] chan T
type BlankType[_ ~int] int

func Blank[_ int]() int { return 0 }

func Exact[T Count](value T) T            { return value }
func Approx[T ~int8](value T) T           { return value }
func Comparable[T comparable](value T) T  { return value }
func Make[T []E, E any](value E) T        { return nil }
func Chain[A []B, B *C, C any](value A) C { var result C; return result }

func Constraints(values Items, plain []*Item, lookup Lookup, pointer *Item, small Small, array [3]*Item,
	channel chan *Item, tiny Tiny[Small], stream LimitedStream[*Item], blankValue BlankType[int]) {
	cloned := slices.Clone(values)
	deleted := slices.Delete(values, 0, 1)
	compacted := slices.Compact(values)
	index := slices.Index(values, pointer)
	mapClone := maps.Clone(lookup)
	exact := Exact(1)
	exactTyped := Exact(Count(1))
	approximate := Approx(small)
	explicitApprox := Approx[int8](1)
	comparablePointer := Comparable(pointer)
	comparableArray := Comparable(array)
	comparableChannel := Comparable(channel)
	made := Make(pointer)
	defaultMade := Make(1)
	chained := Chain(plain)
	namedArithmetic := tiny + 1
	received := <-stream
	blank := Blank()
	blankExplicit := Blank[int]()
	blankNamed := blankValue + 1
	_ = []any{cloned, deleted, compacted, index, mapClone, exact, exactTyped, approximate,
		explicitApprox, comparablePointer, comparableArray, comparableChannel, made,
		defaultMade, chained, namedArithmetic, received, blank, blankExplicit, blankNamed}
}

func GenericCalls(pointer *Item, small Small, values []*Item, named Items,
	mapping map[string]*Item, array [3]*Item, channel chan *Item, instantiated Number[byte]) {
	explicit := Identity[*Item](pointer)
	inferred := Identity(pointer)
	nested := Identity(Identity(pointer))
	parenthesized := (Identity[*Item])(pointer)
	fromLiteral := Identity(1)
	fromRune := Identity('x')
	fromFloat := Identity(1.25)
	fromString := Identity("text")
	fromBool := Identity(true)
	nominal := Identity(instantiated)
	typedFirst := Choose(small, 2)
	typedLast := Choose(2, small)
	promoted := Choose(1, 1.25)
	promotedReverse := Choose(1.25, 1)
	promotedRune := Choose(1, 'x')
	converted := Choose[float64](1, 2.5)
	swapped, swappedPointer := Swap[*Item](pointer, 2)
	element := Element(values)
	namedElement := Element(named)
	mapElement := MapElement(mapping)
	arrayElement := ArrayElement(array)
	dereferenced := Pointed(pointer)
	received := Read(channel)
	variadic := First(pointer, pointer)
	spread := First(values...)
	explicitSpread := First[*Item](values...)
	zero := Zero[*Item]()
	_ = []any{explicit, inferred, nested, parenthesized, fromLiteral, fromRune, fromFloat,
		fromString, fromBool, nominal, typedFirst, typedLast, promoted, promotedReverse,
		promotedRune, converted, swapped, swappedPointer, element, namedElement, mapElement,
		arrayElement, dereferenced, received, variadic, spread, explicitSpread, zero}
}

func Query(pointer *Item, nested **Item, imported *bytes.Buffer) {
	var declared *Item
	var double **Item
	var alias *Alias
	var slice []*Item
	var array [3]*Item
	var mapping map[string]*Item
	var channel chan<- *Item
	var receive <-chan *Item
	var generic Box[*Item]
	allocated := new(Item)
	allocatedDouble := new(*Item)
	allocatedSlice := new([]*Item)
	allocatedBuiltin := new(int)
	addressed := &Item{}
	addressedLocal := &declared
	value := *pointer
	indirect := *nested
	returned := Pointer()
	field := generic.Value
	literalSlice := []*Item{}
	literalMap := map[string]*Item{}
	fromMap, ok := mapping[""]
	fromSlice := slice[0]
	fromArray := array[0]
	fromChannel := <-receive
	integer := 1
	binary := integer + 1
	inferredGeneric := Identity(pointer)
	_ = []any{declared, double, alias, slice, array, mapping, channel, receive, generic,
		allocated, allocatedDouble, allocatedSlice, allocatedBuiltin, addressed, addressedLocal,
		value, indirect, returned, field, literalSlice, literalMap, fromMap, ok, fromSlice,
		fromArray, fromChannel, integer, binary, inferredGeneric, imported}
}

func Basic(i int, small int8, u uint8, real float64, flag bool, text string, ch chan *Item) {
	const narrow int8 = 126
	const raw = 7
	arithmetic := i + 2
	reverse := 2 + i
	converted := i + 1.0
	narrowSum := narrow + 1
	negative := -narrow
	complement := ^u
	combined := raw*3 + 1
	shift := 1 << 4
	runtimeShift := i << 3
	compare := i > 2
	logical := flag && true
	not := !flag
	joined := text + "!"
	literalString := "hello" + ` world`
	runeValue := '世'
	escapedRune := '\xFF'
	floatValue := 1.5
	imagValue := 2i
	realSum := real + 1
	equal := text == ""
	received, receivedOK := <-ch
	_ = []any{small, arithmetic, reverse, converted, narrowSum, negative, complement,
		combined, shift, runtimeShift, compare, logical, not, joined, literalString,
		runeValue, escapedRune, floatValue, imagValue, realSum, equal, received, receivedOK}
}

type Count int

// Declaration parameters below must shadow this package type.
type T int
type CountAlias = Count
type IntegerAlias = int
type OtherCount Count
type Small int8
type Flag bool
type Text string
type Stream[T any] chan *T
type StreamAlias[T any] = Stream[T]
type Receive[T any] <-chan T
type Recursive *Recursive
type Number[T any] int

type Callback func(Count, ...*Item) (*Item, bool)
type OtherCallback func(value Count, items ...*Item) (result *Item, ok bool)
type ReaderView interface{ Read([]byte) (int, error) }
type EmptyView interface{}
type KeyBox[K comparable] struct{ Value K }

func KeyBoxValue[K comparable](value KeyBox[K]) K { return value.Value }

func InterfaceTags(value interface {
	M(struct {
		Value int `a`
	})
}) {
	converted := (interface {
		M(struct {
			Value int `b`
		})
	})(value)
	_ = converted
}

type ErrorView interface{ Error() string }
type GetView[T any] interface{ Get() T }
type EmbeddedGet interface {
	GetView[*Item]
	Extra()
}
type DuplicateGet interface {
	GetView[*Item]
	Get() *Item
}

func AnyArgument(value any) Count                                          { return 0 }
func EmptyArgument(value EmptyView) Count                                  { return 0 }
func ErrorArgument(value error) Count                                      { return 0 }
func AnonymousArgument(value interface{ Read([]byte) (int, error) }) Count { return 0 }
func GetArgument[T any](value interface{ Get() T }) T                      { panic("not executed") }
func GenericAnyArgument[T any](value T, rest any) T                        { return value }
func InterfaceValues(reader io.ReadCloser, err error, get EmbeddedGet) {
	nilAny := any(nil)
	integerAny := any(1)
	booleanAny := any(true)
	stringAny := any("text")
	defaultNil := AnyArgument(nil)
	defaultInt := AnyArgument(1)
	defaultRune := AnyArgument('x')
	defaultFloat := AnyArgument(1.5)
	namedEmpty := EmptyArgument(1)
	convertedEmpty := EmptyView("text")
	errorNil := error(nil)
	errorValue := ErrorView(err)
	argumentError := ErrorArgument(err)
	anonymous := AnonymousArgument(reader)
	getValue := GetArgument(get)
	duplicated := DuplicateGet(get)
	fromEmbedded := GetArgument(duplicated)
	genericAny := GenericAnyArgument(Count(1), "text")
	typedAny := Identity(any(1))
	shape := struct {
		Value any
		Fn    func(error) any
	}{}
	comparableShape := Comparable(struct{ Value any }{})
	comparableAny := Comparable[any](nil)
	keyBox := KeyBox[int]{}
	keyValue := KeyBoxValue(keyBox)
	_ = []any{nilAny, integerAny, booleanAny, stringAny, defaultNil, defaultInt, defaultRune,
		defaultFloat, namedEmpty, convertedEmpty, errorNil, errorValue, argumentError,
		anonymous, getValue, duplicated, fromEmbedded, genericAny, typedAny, shape, comparableShape,
		comparableAny, keyBox, keyValue}
}

type ValueReader struct{}

func (ValueReader) Read([]byte) (int, error) { return 0, nil }

type PointerReader struct{}

func (*PointerReader) Read([]byte) (int, error) { return 0, nil }

type ValueWrapper struct{ ValueReader }
type PointerWrapper struct{ PointerReader }
type EmbeddedPointerReader struct{ *PointerReader }
type InterfaceWrapper struct{ io.Reader }
type ReaderPointerAlias = *PointerReader
type AliasReceiver struct{}
type AliasReceiverPointer = *AliasReceiver

func (AliasReceiverPointer) Read([]byte) (int, error) { return 0, nil }
func AliasReceiverArgument(pointer *AliasReceiver) {
	fromReceiverAlias := ReadArgument(pointer)
	_ = fromReceiverAlias
}

type ReaderValueAlias = ValueReader
type GenericReader[T any] struct{}

func (GenericReader[R]) Read([]R) (int, error) { return 0, nil }

type GenericReaderWrapper[T any] struct{ GenericReader[T] }

func ConcreteArguments(value ValueReader, pointer *PointerReader, wrapper ValueWrapper,
	ptrWrapper *PointerWrapper, embedded EmbeddedPointerReader, face InterfaceWrapper,
	alias ReaderValueAlias, ptrAlias ReaderPointerAlias, generic GenericReader[byte], wrapped GenericReaderWrapper[byte]) {
	fromValue := ReadArgument(value)
	fromPointer := ReadArgument(pointer)
	fromWrapper := ReadArgument(wrapper)
	fromPointerWrapper := ReadArgument(ptrWrapper)
	fromEmbeddedPointer := ReadArgument(embedded)
	fromInterface := ReadArgument(face)
	fromAlias := ReadArgument(alias)
	fromPointerAlias := ReadArgument(ptrAlias)
	fromGeneric := ReadArgument(generic)
	fromGenericWrapper := ReadArgument(wrapped)
	fromAnonymous := ReadArgument(struct{ ValueReader }{})
	fromBuffer := ReadArgument(&bytes.Buffer{})
	function := ReadArgument
	fromFunction := function(pointer)
	_ = []any{fromValue, fromPointer, fromWrapper, fromPointerWrapper, fromEmbeddedPointer,
		fromInterface, fromAlias, fromPointerAlias, fromGeneric, fromGenericWrapper,
		fromAnonymous, fromBuffer, function, fromFunction}
}

type ReaderAlias = ReaderView
type DerivedReader ReaderView

func ReadArgument(value io.Reader) Count  { return 0 }
func ViewArgument(value ReaderView) Count { return 0 }
func InterfaceArguments(value io.ReadCloser, local ReaderView, alias ReaderAlias, derived DerivedReader) {
	fromReader := ReadArgument(value)
	fromLocal := ReadArgument(local)
	fromAlias := ReadArgument(alias)
	fromDerived := ReadArgument(derived)
	fromLarger := ViewArgument(value)
	function := ReadArgument
	fromFunction := function(value)
	_ = []any{fromReader, fromLocal, fromAlias, fromDerived, fromLarger, function, fromFunction}
}

type CallbackHolder struct{ Fn func(Count) *Item }

func (value Count) Lookup(n int) *Item { return nil }

func Callable(value Count, items ...*Item) (*Item, bool) { return nil, true }
func Apply[A, B any](fn func(A) B, value A) B            { return fn(value) }

func CountArgument(value Count) Count                          { return value }
func PairArguments() (Count, int)                              { return 1, 2 }
func CombinedArguments(value Count, n int) Count               { return value }
func VariadicArguments(value Count, items ...int) Count        { return value }
func ChannelArgument(value <-chan *Item) *Item                 { return nil }
func CallbackArgument(fn func(Count) *Item, value Count) *Item { return fn(value) }
func PairCounts() (Count, Count)                               { return 1, 2 }
func OrdinaryArguments(value Count, channel chan *Item, values []int, callback CallbackHolder) {
	constant := CountArgument(1)
	typed := CountArgument(value)
	method := value.Lookup(1)
	fromChannel := ChannelArgument(channel)
	fromNil := ChannelArgument(nil)
	fromCallback := CallbackArgument(callback.Fn, value)
	fromLiteral := CallbackArgument(func(Count) *Item { return nil }, 1)
	fromVariadic, variadicOK := Callable(value)
	combined := CombinedArguments(PairArguments())
	variadicTuple := VariadicArguments(PairArguments())
	functionTuple := func(value Count, n int) Count { return value }(PairArguments())
	genericTuple := CombinedArguments(Swap(2, value))
	localPair := PairArguments
	localTuple := CombinedArguments(localPair())
	swappedA, swappedB := Swap(PairArguments())
	variadicInferred := First(PairCounts())
	_ = []any{constant, typed, method, fromChannel, fromNil, fromCallback, fromLiteral,
		fromVariadic, variadicOK, values, combined, variadicTuple, functionTuple, genericTuple,
		localPair, localTuple, swappedA, swappedB, variadicInferred}
}

func Signatures(value Count, items []*Item, callback Callback) {
	function := Callable
	literal := func(value Count) *Item { return nil }
	converted := Callback(Callable)
	other := OtherCallback(callback)
	nilFunction := Callback(nil)
	item, ok := converted(value, items...)
	empty, emptyOK := converted(1)
	plain, plainOK := function(value, items...)
	literalResult := literal(1)
	instantiated := Identity[*Item]
	identityResult := instantiated(nil)
	callbackIdentity := Identity(converted)
	inferred := Apply(literal, value)
	structFunction := struct{ Fn func(Count) *Item }{literal}
	fieldResult := structFunction.Fn(value)
	nested := func(fn func(Count) *Item) *Item { return fn(value) }
	nestedResult := nested(literal)
	bound := value.Lookup
	boundResult := bound(1)
	holder := CallbackHolder{literal}
	holderResult := holder.Fn(value)
	_ = []any{function, literal, converted, other, nilFunction, item, ok, empty, emptyOK,
		plain, plainOK, literalResult, instantiated, identityResult, callbackIdentity,
		inferred, structFunction, fieldResult, nested, nestedResult, bound, boundResult, holder, holderResult}
}

func Named(a Count, b CountAlias, other OtherCount, small Small, flag Flag, text Text,
	stream Stream[Item], alias StreamAlias[Item], receive Receive[Count], p Recursive,
	duration time.Duration, first Number[int], second Number[int], different Number[string], plain IntegerAlias, integer int) {
	const limit Small = 126
	sum := a + b
	aliasSum := b + a
	builtinAlias := plain + integer
	fromLiteral := a + 1
	reversed := 1 + a
	negative := -a
	complement := ^small
	limited := limit + 1
	compare := a == b
	logical := flag && true
	inverse := !flag
	joined := text + "!"
	shifted := a << 2
	item, ok := <-stream
	aliasedItem := <-alias
	count := <-receive
	recursive := *p
	seconds := duration + 1
	numbers := first + second
	_ = []any{other, different, sum, fromLiteral, reversed, negative, complement,
		limited, compare, logical, inverse, joined, shifted, item, ok, aliasedItem,
		count, recursive, seconds, numbers, aliasSum, builtinAlias}
}

func Structured(a Number[map[string][]int], b Number[map[string][]int],
	arrayA Number[[3]*Item], arrayB Number[[3]*Item],
	channelA Number[chan<- *Item], channelB Number[chan<- *Item],
	byteA Number[byte], byteB Number[uint8]) {
	maps := a + b
	arrays := arrayA + arrayB
	channels := channelA + channelB
	bytes := byteA + byteB
	_ = []any{maps, arrays, channels, bytes}
}

func LocalShadow() {
	type Count int
	var value Count
	result := value + 1
	_ = result
}

// Type-set normalization, embedding and core-term inference.
type IntegralSet interface{ ~int | ~int64 }
type CountSet interface {
	IntegralSet
	~int
}
type SharedSet interface{ IntegralSet | ~int }
type SliceSet[E any] interface{ ~[]E }
type CheckedSliceSet[E comparable] interface{ ~[]E }
type StrictSet interface {
	comparable
	~int | ~[]int
}
type MethodSet interface {
	~int
	Read() string
}
type SetReader int

func (SetReader) Read() string                                 { return "" }
func SetIntegral[T IntegralSet](value T) T                     { return value }
func SetCount[T CountSet](value T) T                           { return value }
func SetShared[T SharedSet](value T) T                         { return value }
func SetStrict[T StrictSet](value T) T                         { return value }
func SetSlice[S SliceSet[E], E any](value S) E                 { var zero E; return zero }
func SetChecked[S CheckedSliceSet[E], E comparable](value S) E { var zero E; return zero }
func SetMethod[T MethodSet](value T) T                         { return value }
func SetExact[T interface {
	int
	~int | ~string
}]() T {
	var zero T
	return zero
}
func SetChannels[C interface{ chan E | <-chan E }, E any](value C) E { var zero E; return zero }
func SetComparable[T comparable](value T) T                          { return value }
func SetCalls(count Count, wide int64, reader SetReader, items []*Item, ch chan Count, recv <-chan Count, boxed any) {
	setInt := SetIntegral(1)
	setNamed := SetIntegral(count)
	setWide := SetIntegral(wide)
	setIntersection := SetCount(count)
	setOverlappingInterface := SetShared(count)
	setStrict := SetStrict(count)
	setSlice := SetSlice(items)
	setChecked := SetChecked(items)
	setMethod := SetMethod(reader)
	setExact := SetExact()
	setChannel := SetChannels(ch)
	setReceive := SetChannels(recv)
	setComparableInterface := SetComparable(boxed)
	setExplicitNil := Identity[any](nil)
	_ = []any{setInt, setNamed, setWide, setIntersection, setOverlappingInterface, setStrict, setSlice, setChecked, setMethod, setExact, setChannel, setReceive, setComparableInterface, setExplicitNil}
}
func SetBasic[T interface{ Read() string }](value T) T { return value }
func SetAnyUnion[T interface{ any | int }](value T) T  { return value }
func SetStrictStruct[T interface {
	comparable
	~struct{ Value int } | ~struct{ Value any }
}](value T) T {
	return value
}
func SetDuplicate[T interface {
	MethodSet
	interface{ Read() string }
}](value T) T {
	return value
}
func MoreSetCalls(reader SetReader, text string) {
	setBasic := SetBasic(reader)
	setUniversal := SetAnyUnion(text)
	setStruct := SetStrictStruct(struct{ Value int }{})
	setDuplicate := SetDuplicate(reader)
	_ = []any{setBasic, setUniversal, setStruct, setDuplicate}
}

type Extractor[E any] interface{ Extract() E }
type NativeReader struct{}

func (NativeReader) Extract() *Item { return nil }

type NativePointerReader struct{}

func (*NativePointerReader) Extract() *Item { return nil }

type PromotedReader struct{ NativeReader }
type PromotedPointerReader struct{ *NativePointerReader }
type NativeGenericReader[E any] struct{}

func (NativeGenericReader[Payload]) Extract() Payload { var zero Payload; return zero }

type NativeReaderAlias = NativeReader
type PointerReaderAlias = *NativePointerReader
type RecursiveReader struct {
	*RecursiveReader
	NativeReader
}
type InterfaceReader struct{ Extractor[*Item] }

func Extract[M Extractor[E], E any](value M) E                { return value.Extract() }
func ExtractArgument[E any](value interface{ Extract() E }) E { return value.Extract() }
func ExtractTwo[M interface {
	First() E
	Second(E)
}, E any](value M) E {
	return value.First()
}

type TwoReader struct{}

func (TwoReader) First() *Item { return nil }
func (TwoReader) Second(*Item) {}

type NextReader struct{}

func (NextReader) Next() NativeReader { return NativeReader{} }
func ExtractNext[A interface{ Next() B }, B Extractor[C], C any](value A) C {
	return value.Next().Extract()
}
func NativeMethodCalls(value NativeReader, pointer *NativePointerReader, promoted PromotedReader, embedded PromotedPointerReader,
	generic NativeGenericReader[*Item], alias NativeReaderAlias, pointerAlias PointerReaderAlias, recursive RecursiveReader, iface InterfaceReader, two TwoReader, next NextReader) {
	methodConcrete := Extract(value)
	methodPointer := Extract(pointer)
	methodPromoted := Extract(promoted)
	methodEmbeddedPointer := Extract(embedded)
	methodGenericReceiver := Extract(generic)
	methodAlias := Extract(alias)
	methodPointerAlias := Extract(pointerAlias)
	methodRecursive := Extract(recursive)
	methodEmbeddedInterface := Extract(iface)
	methodArgument := ExtractArgument(value)
	methodArgumentPointer := ExtractArgument(pointer)
	methodTwo := ExtractTwo(two)
	methodNext := ExtractNext(next)
	_ = []any{methodConcrete, methodPointer, methodPromoted, methodEmbeddedPointer, methodGenericReceiver, methodAlias, methodPointerAlias, methodRecursive, methodEmbeddedInterface, methodArgument, methodArgumentPointer, methodTwo, methodNext}
}

type NativeAliasReceiver struct{}
type NativeAliasPointer = *NativeAliasReceiver

func (NativeAliasPointer) Extract() *Item { return nil }
func NativeAliasCall(value *NativeAliasReceiver) {
	methodAliasReceiver := Extract(value)
	_ = methodAliasReceiver
}
func IndexValues(slice []*Item, array [2]*Item, pointer *[2]*Item, mapping map[Count]*Item, boxes map[any]*Item, text Text, index Count, unsigned uint64) {
	indexSlice := slice[index]
	indexArray := array[1]
	indexPointer := pointer[1]
	indexUnsigned := array[unsigned]
	indexMap := mapping[Count(1)]
	indexMapConstant := mapping[1.0]
	indexMapValue, indexMapOK := mapping[index]
	indexBox := boxes[nil]
	indexText := text[0]
	indexLiteral := "界"[2]
	indexConcat := ("a" + "b")[1]
	indexRuneString := string('界')[2]
	indexRaw := `ab`[1]
	indexFloat := array[1.0]
	indexArrayLiteral := ([2]*Item{})[0]
	indexPointerLiteral := (&[2]*Item{})[0]
	indexSliceLiteral := ([]*Item{})[100]
	const textConstant = "ab"
	indexConstant := textConstant[1]
	_ = []any{indexSlice, indexArray, indexPointer, indexUnsigned, indexMap, indexMapConstant, indexMapValue, indexMapOK, indexBox, indexText, indexLiteral, indexConcat, indexRuneString, indexRaw, indexFloat, indexArrayLiteral, indexPointerLiteral, indexSliceLiteral, indexConstant}
}

type SliceItems []*Item
type SliceArrayHolder struct{ Array [3]*Item }
type SliceNestedHolder struct{ Nested SliceArrayHolder }

func SliceArray() [3]*Item                  { return [3]*Item{} }
func SliceHolder() SliceArrayHolder         { return SliceArrayHolder{} }
func SlicePointerHolder() *SliceArrayHolder { return nil }
func SliceValues(items SliceItems, array [3]*Item, pointer *[3]*Item, holder SliceArrayHolder, nested SliceNestedHolder, text Text, low int, high uint64) {
	sliceNamed := items[:]
	sliceFull := items[low:high:high]
	sliceArray := array[0:3]
	sliceArrayFull := array[:2:3]
	slicePointer := pointer[:]
	sliceDeref := (*pointer)[:]
	sliceField := holder.Array[:]
	sliceNested := nested.Nested.Array[:]
	slicePointerField := SlicePointerHolder().Array[:]
	sliceNamedText := text[0:1]
	sliceLiteral := "界"[1:3]
	sliceConcat := ("a" + "b")[0:2]
	sliceRune := string('界')[:3]
	sliceFloat := array[1.0:2.0]
	sliceEmpty := array[3:3]
	sliceNil := ([]*Item)(nil)[:]
	slicePointerLiteral := (&[3]*Item{})[:]
	sliceRuntime := ([]*Item{})[:100]
	sliceNestedIndex := items[:][0]
	sliceInferred := Identity(items[:])
	_ = []any{sliceNamed, sliceFull, sliceArray, sliceArrayFull, slicePointer, sliceDeref, sliceField, sliceNested, slicePointerField, sliceNamedText, sliceLiteral, sliceConcat, sliceRune, sliceFloat, sliceEmpty, sliceNil, slicePointerLiteral, sliceRuntime, sliceNestedIndex, sliceInferred}
}

type RangeYield[E any] func(E) bool
type RangeSequence[E any] func(RangeYield[E])
type RangeBoolAlias = bool
type RangeAliasSequence func(func(*Item) RangeBoolAlias)

func RangeIterator(yield func(*Item) bool)            {}
func RangePairIterator(yield func(Count, *Item) bool) {}
func RangeValues(slice []*Item, array [2]*Item, pointer *[2]*Item, mapping map[Count]*Item, receive <-chan *Item, integer Count, sequence RangeSequence[*Item], alias RangeAliasSequence) {
	for rangeSliceKey, rangeSliceValue := range slice {
		_ = []any{rangeSliceKey, rangeSliceValue}
	}
	for rangeArrayKey, rangeArrayValue := range array {
		_ = []any{rangeArrayKey, rangeArrayValue}
	}
	for rangePointerKey, rangePointerValue := range pointer {
		_ = []any{rangePointerKey, rangePointerValue}
	}
	for rangeMapKey, rangeMapValue := range mapping {
		_ = []any{rangeMapKey, rangeMapValue}
	}
	for rangeReceive := range receive {
		_ = rangeReceive
	}
	for rangeInteger := range integer {
		_ = rangeInteger
	}
	for rangeLiteral := range 3 {
		_ = rangeLiteral
	}
	for rangeRune := range 'a' {
		_ = rangeRune
	}
	for rangeNegative := range -3 {
		_ = rangeNegative
	}
	for rangeTextKey, rangeTextValue := range "界" {
		_ = []any{rangeTextKey, rangeTextValue}
	}
	for rangeFunction := range RangeIterator {
		_ = rangeFunction
	}
	for rangePairKey, rangePairValue := range RangePairIterator {
		_ = []any{rangePairKey, rangePairValue}
	}
	for rangeNamed := range sequence {
		_ = rangeNamed
	}
	for rangeAliasBool := range alias {
		_ = rangeAliasBool
	}
	for rangeLiteralKey, rangeLiteralValue := range []*Item{} {
		_ = []any{rangeLiteralKey, rangeLiteralValue}
	}
	for rangeSlicedKey, rangeSlicedValue := range slice[:] {
		_ = []any{rangeSlicedKey, rangeSlicedValue}
	}
	for _, rangeNested := range slice {
		copied := Identity(rangeNested)
		_ = copied
	}
}

type BoundsKeys interface{ ~int | ~int64 }
type BoundsComparable interface{ comparable }
type BoundsMap[K comparable] map[K]*Item

func BoundDirect[K comparable](values map[K]*Item) {
	for _, boundDirect := range values {
		_ = boundDirect
	}
}
func BoundNamed[K comparable](values BoundsMap[K]) {
	for _, boundNamed := range values {
		_ = boundNamed
	}
}
func BoundFinite[K BoundsKeys](values map[K]*Item) {
	for _, boundFinite := range values {
		_ = boundFinite
	}
}
func BoundAlias[K BoundsComparable](values map[K]*Item) {
	for _, boundAlias := range values {
		_ = boundAlias
	}
}
func BoundComposite[K comparable](values map[[2]K]*Item) {
	for _, boundComposite := range values {
		_ = boundComposite
	}
}
func BoundDependent[E ~[2]K, K comparable](values map[E]*Item) {
	for _, boundDependent := range values {
		_ = boundDependent
	}
}

func BoundNested[K comparable](values map[K]*Item) {
	closure := func() {
		for _, boundNested := range values {
			_ = boundNested
		}
	}
	_ = closure
}
func BoundBodyShadow[K comparable](values map[K]*Item) {
	comparable := 1
	_ = comparable
	for _, boundBodyShadow := range values {
		_ = boundBodyShadow
	}
}

type CoreSliceA []*Item
type CoreSliceB []*Item
type CoreEither interface{ CoreSliceA | CoreSliceB }
type CoreBound[K comparable] interface{ ~map[K]*Item }

func CoreSlice[S ~[]*Item](values S) {
	for coreSliceKey, coreSliceValue := range values {
		_, _ = coreSliceKey, coreSliceValue
	}
}
func CoreUnion[S CoreEither](values S) {
	for coreUnionKey, coreUnionValue := range values {
		_, _ = coreUnionKey, coreUnionValue
	}
}
func CoreArray[S ~[2]*Item](values S) {
	for coreArrayKey, coreArrayValue := range values {
		_, _ = coreArrayKey, coreArrayValue
	}
}
func CorePointer[S ~*[2]*Item](values S) {
	for corePointerKey, corePointerValue := range values {
		_, _ = corePointerKey, corePointerValue
	}
}
func CoreMap[S ~map[string]*Item](values S) {
	for coreMapKey, coreMapValue := range values {
		_, _ = coreMapKey, coreMapValue
	}
}
func CoreNamedMap[K comparable, S CoreBound[K]](values S) {
	for _, coreNamedMapValue := range values {
		_ = coreNamedMapValue
	}
}
func CoreChannel[C interface{ ~chan *Item | ~<-chan *Item }](values C) {
	for coreChannelValue := range values {
		_ = coreChannelValue
	}
}
func CoreString[S ~string](values S) {
	for coreStringKey, coreStringValue := range values {
		_, _ = coreStringKey, coreStringValue
	}
}
func CoreFunction[F ~func(func(*Item) bool)](values F) {
	for coreFunctionValue := range values {
		_ = coreFunctionValue
	}
}
func CoreCallback[Y ~func(*Item) bool, F ~func(Y)](values F) {
	for coreCallbackValue := range values {
		_ = coreCallbackValue
	}
}
func CoreTuple[F ~func(func(int, *Item) bool)](values F) {
	for coreTupleKey, coreTupleValue := range values {
		_, _ = coreTupleKey, coreTupleValue
	}
}
func CoreFormal[E any, S ~[]E](values S) {
	for coreFormalKey, coreFormalValue := range values {
		_, _ = coreFormalKey, coreFormalValue
	}
}
func CoreInteger[N ~int](values N) {
	for coreIntegerValue := range values {
		_ = coreIntegerValue
	}
}

type CoreDependent[E any, S ~[]E] interface{ ~[]*Item }

func CoreDependentRange[E any, S CoreDependent[E, []E]](values S) {
	for coreDependentKey, coreDependentValue := range values {
		_, _ = coreDependentKey, coreDependentValue
	}
}

type ImpInt int
type ImpWide[N interface{ ~int | ~int64 }] interface{ ~[]*Item }
type ImpExact[N interface{ int }] interface{ ~[]*Item }
type ImpRead[N interface{ Read(string) string }] interface{ ~[]*Item }
type ImpComparable[N comparable] interface{ ~[]*Item }

func ImpApprox[N ~int, S ImpWide[N]](values S) {
	for impApproxKey, impApproxValue := range values {
		_, _ = impApproxKey, impApproxValue
	}
}
func ImpNamed[N interface{ ImpInt }, S ImpWide[N]](values S) {
	for _, impNamedValue := range values {
		_ = impNamedValue
	}
}
func ImpUnion[N interface{ ~int | ~int64 }, S ImpWide[N]](values S) {
	for _, impUnionValue := range values {
		_ = impUnionValue
	}
}
func ImpExactFormal[N interface{ int }, S ImpExact[N]](values S) {
	for _, impExactValue := range values {
		_ = impExactValue
	}
}
func ImpMethods[N interface {
	Read(string) string
	Extra()
}, S ImpRead[N]](values S) {
	for _, impMethodValue := range values {
		_ = impMethodValue
	}
}
func ImpEmpty[N interface {
	~int
	~string
}, S ImpRead[N]](values S) {
	for _, impEmptyValue := range values {
		_ = impEmptyValue
	}
}
func ImpStrict[N interface{ ~struct{ X int } | ~[2]int }, S ImpComparable[N]](values S) {
	for _, impStrictValue := range values {
		_ = impStrictValue
	}
}

type RecMap[K comparable] map[K]*Item
type RecPair[K comparable, S ~map[K]*Item] struct{}
type RecList[S CoreEither] struct{}
type RecIterator[Y ~func(*Item) bool, F ~func(Y)] struct{}

func (values RecMap[Key]) ReceiverDirect() {
	for _, receiverDirect := range values {
		_ = receiverDirect
	}
}
func (_ *RecMap[Key]) ReceiverParam(values map[Key]*Item) {
	for _, receiverParam := range values {
		_ = receiverParam
	}
}
func (_ RecPair[X, Y]) ReceiverRename(values Y) {
	for _, receiverRename := range values {
		_ = receiverRename
	}
}
func (_ RecPair[Y, X]) ReceiverSwap(values X) {
	for _, receiverSwap := range values {
		_ = receiverSwap
	}
}
func (_ RecList[T]) ReceiverList(values T) {
	for receiverListKey, receiverListValue := range values {
		_, _ = receiverListKey, receiverListValue
	}
}
func (_ RecIterator[Callback, Sequence]) ReceiverFunction(values Sequence) {
	for receiverFunction := range values {
		_ = receiverFunction
	}
}
func (_ RecMap[K]) ReceiverNested(values map[K]*Item) {
	closure := func() {
		for _, receiverNested := range values {
			_ = receiverNested
		}
	}
	_ = closure
}
func (_ RecMap[int]) ReceiverBuiltinShadow(values map[int]*Item) {
	for _, receiverBuiltin := range values {
		_ = receiverBuiltin
	}
}

func (_ (RecPair[X,Y])) ReceiverParenthesized(values Y) {
 for _, receiverParenthesized := range values { _ = receiverParenthesized }
}
func (_ *(RecPair[X,Y])) ReceiverParenthesizedPointer(values Y) {
 for _, receiverParenthesizedPointer := range values { _ = receiverParenthesizedPointer }
}
func (_ RecPair[
 X,
 Y,
]) ReceiverTrailingComma(values Y) {
 for _, receiverTrailing := range values { _ = receiverTrailing }
}


type FieldBox[K comparable,S ~map[K]*Item] struct {
 Values S
 Nested struct{ Values S }
 Pointer *struct{ Values S }
 Slots [2]S
 Rows []S
}
type FieldEmbedded[K comparable,S ~map[K]*Item] struct{ FieldBox[K,S] }
type FieldEmbeddedPointer[K comparable,S ~map[K]*Item] struct{ *FieldBox[K,S] }
type FieldDefinedPointer[K comparable,S ~map[K]*Item] *FieldBox[K,S]
func (box *FieldBox[K,S]) FieldRanges() {
 for _, fieldDirect := range box.Values { _ = fieldDirect }
 for _, fieldNested := range box.Nested.Values { _ = fieldNested }
 for _, fieldPointer := range box.Pointer.Values { _ = fieldPointer }
 for _, fieldIndex := range box.Slots[0] { _ = fieldIndex }
 for _, fieldRow := range box.Rows[0] { _ = fieldRow }
 for _, fieldSlice := range box.Slots[0:1:2][0] { _ = fieldSlice }
 for _, fieldRowSlice := range box.Rows[0:1][0] { _ = fieldRowSlice }
 for _, fieldIndirect := range (*box).Values { _ = fieldIndirect }
}
func (box FieldEmbedded[K,S]) FieldPromotion() { for _, fieldPromoted := range box.Values { _ = fieldPromoted } }
func (box FieldEmbeddedPointer[K,S]) FieldPointerPromotion() { for _, fieldPromotedPointer := range box.Values { _ = fieldPromotedPointer } }
func FieldDefinedPointerRange[K comparable,S ~map[K]*Item](box FieldDefinedPointer[K,S]) { for _, fieldDefinedPointer := range box.Values { _ = fieldDefinedPointer } }
type FieldArrayBox[S any] struct { Values [2]*Item; Marker S }
type FieldModes[S any] struct { Values map[string]FieldArrayBox[S]; Pointers map[string]*FieldArrayBox[S] }
func (box FieldModes[S]) FieldAddressability() { for _, fieldAddressable := range box.Pointers["k"].Values[:] { _ = fieldAddressable } }

func FieldFormalPointer[K comparable,S ~map[K]*Item,P ~*FieldBox[K,S]](box P) {
 for _, fieldFormalPointer := range (*box).Values { _ = fieldFormalPointer }
}

func (box FieldBox[K,S]) FieldCopy() { copied := box; for _, fieldCopy := range copied.Values { _ = fieldCopy } }
type FieldNestedRanges[K comparable,S ~map[K]*Item] struct{ Rows []FieldBox[K,S] }
func (box FieldNestedRanges[K,S]) FieldNestedLoops() { for _, row := range box.Rows { for _, fieldNestedLoop := range row.Values { _ = fieldNestedLoop } } }
func (box FieldModes[S]) FieldCopiedArray() { copied := box.Values["k"]; for _, fieldCopiedArray := range copied.Values[:] { _ = fieldCopiedArray } }


type ParamMapA[K comparable] map[K][]*Item
type ParamMapB[K comparable] map[K][]*Item
type ParamBox[K comparable,S ~map[K][]*Item] struct{ Values S }
func ParamDirect[K comparable](values map[K][]*Item,key K) { for _, paramDirect := range values[key] { _ = paramDirect } }
func (box ParamBox[K,S]) ParamReceiver(key K) { for _, paramReceiver := range box.Values[key] { _ = paramReceiver } }
func ParamFromSlice[K comparable](values map[K][]*Item,keys []K) { for _, paramFromSlice := range values[keys[0]] { _ = paramFromSlice } }
func ParamFromRange[K comparable](values map[K][]*Item) { for key := range values { for _, paramFromRange := range values[key] { _ = paramFromRange } } }
func ParamMapUnion[K comparable,M interface{ParamMapA[K] | ParamMapB[K]}](values M,key K) { for _, paramMapUnion := range values[key] { _ = paramMapUnion } }
func ParamSequence[P interface{ ~[2][]*Item | ~[][]*Item }](values P,index int) { for _, paramSequence := range values[index] { _ = paramSequence } }
func ParamPointerSequence[P interface{ ~*[2][]*Item | ~[][]*Item }](values P,index int) { for _, paramPointerSequence := range values[index] { _ = paramPointerSequence } }
func ParamArrayLengths[P interface{ ~[2][]*Item | ~[3][]*Item }](values P) { for _, paramArrayLengths := range values[0] { _ = paramArrayLengths } }
func ParamIntegerIndex[P ~[][]*Item,I interface{ ~int | ~int64 }](values P,index I) { for _, paramIntegerIndex := range values[index] { _ = paramIntegerIndex } }
func ParamSlice[P ~[]*Item](values P) { for _, paramSlice := range values[1:] { _ = paramSlice } }
func ParamArraySlice[P ~[2]*Item](values P) { for _, paramArraySlice := range values[0:1:2] { _ = paramArraySlice } }
func ParamPointerSlice[P ~*[2]*Item](values P) { for _, paramPointerSlice := range values[:] { _ = paramPointerSlice } }
func ParamStringSlice[P interface{ ~string | ~[]byte }](values P) { for paramStringSlice := range values[:][0] { _ = paramStringSlice } }
func ParamSliceInteger[P ~[]*Item,I interface{ ~int | ~uint }](values P,index I) { for _, paramSliceInteger := range values[index:] { _ = paramSliceInteger } }
type ParamReverse[S interface{ParamMapA[K] | ParamMapB[K]},K comparable] struct{ Values S }
func (box ParamReverse[Values,Key]) ParamDependency(key Key) { for _, paramDependency := range box.Values[key] { _ = paramDependency } }

type ParamInteger int
func ParamConstantScope[P ~[][]*Item](values P) {
 const index ParamInteger = 0
 ParamInteger := 0
 _ = ParamInteger
 for _, paramConstantScope := range values[index] { _ = paramConstantScope }
}

func ParamShadow[K comparable,M ~map[K][]*Item](values M,key K) {
 { K := 1; _ = K; var copied M = values; for _, paramShadow := range copied[key] { _ = paramShadow } }
}

// Builtin results retain native operand modes and declaration identities.
type BuiltinSlice []*Item
type BuiltinBytes []byte
type BuiltinCount int
func BuiltinPair() ([]*Item, *Item) { return nil, nil }
func BuiltinArray() [3]int { return [3]int{} }
func BuiltinValues(values BuiltinSlice, bytes BuiltinBytes, rows [3]*Item, pointer *[3]int, mapping map[string]*Item, channel <-chan *Item, value *Item) {
 appended := append(values, value)
 unchanged := append(values)
 spread := append(values, values...)
 tuple := append(BuiltinPair())
 fromNil := append([]*Item(nil), value)
 stringBytes := append(bytes, "abc"...)
 byteSlice := append(bytes, []byte("abc")...)
 made := make(BuiltinSlice, 1, 3)
 madeMap := make(map[string]*Item)
 sizedMap := make(map[string]*Item, 2)
 madeChannel := make(chan *Item, 2)
 copied := copy(values, values)
 copiedBytes := copy(bytes, "abc")
 copiedSlices := copy(bytes, []byte("abc"))
 count := len(values)
 capacity := cap(values)
 countMap := len(mapping)
 countChannel := len(channel)
 capacityChannel := cap(channel)
 arrayCount := len(rows)
 pointerCapacity := cap(pointer)
 stringCount := len("你a")
 const builtinBound = len("abc")
 last := rows[builtinBound-1]
 const extent = cap([3]int{})
 lastArray := rows[extent-1]
 runtimeCount := len(BuiltinArray())
 recovered := recover()
 allocatedValue := new(value)
 allocatedNumber := new(7)
 allocatedRune := new('x')
 allocatedSlice := new(values)
 allocatedFunction := new(BuiltinArray)
 _,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_,_ = appended,unchanged,spread,tuple,fromNil,stringBytes,byteSlice,made,madeMap,sizedMap,madeChannel,copied,copiedBytes,copiedSlices,count,capacity,countMap,countChannel,capacityChannel,arrayCount,pointerCapacity,stringCount,last,lastArray,runtimeCount,recovered,allocatedValue,allocatedNumber,allocatedRune,allocatedSlice,allocatedFunction,0
}
func BuiltinFormals[S ~[]*Item, N ~int|~int64, Text interface{~string|~[]byte}](values S, size N, text Text) {
 count := len(values)
 capacity := cap(values)
 madeCount := len(make(S, size))
 appendedCount := len(append(values, (*Item)(nil)))
 copied := copy(values, values)
 copiedText := copy([]byte{}, text)
 appendedText := len(append([]byte{}, text...))
 textCount := len(text)
 _,_,_,_,_,_,_,_ = count,capacity,madeCount,appendedCount,copied,copiedText,appendedText,textCount
}
func BuiltinHeterogeneous[T interface{~string|~map[int]int|~[]int}](value T) { count := len(value); _ = count }
func BuiltinArrayTerms[T interface{~[2]int|~[3]string}](value T) { count := len(value); capacity := cap(value); _,_ = count,capacity }
func BuiltinShadowed() {
 len := func(value string) *Item { return nil }
 customLength := len("abc")
 append := func(value int) *Item { return nil }
 customAppend := append(1)
 make := func() *Item { return nil }
 customMake := make()
 new := func(value int) *Item { return nil }
 customNew := new(1)
 _,_,_,_ = customLength,customAppend,customMake,customNew
}

func BuiltinRange[S ~[]*Item](values S) {
 for _, fromAppend := range append(values, values...) { _ = fromAppend }
 for _, fromMake := range make(S, 2) { _ = fromMake }
}

type NumericFloat float32
type NumericOtherFloat float32
type NumericComplex complex64
type NumericFloatAlias = float32
func NumericPair() (float32, float32) { return 1, 2 }
func NativeNumeric(f float32, d float64, c complex64, z complex128, named NumericFloat, nc NumericComplex) {
 madeComplex := complex(1, 2)
 _ = madeComplex
 madeFraction := complex(1.25, 2.5)
 _ = madeFraction
 madeNarrow := complex(f, 2)
 _ = madeNarrow
 madeWide := complex(1, d)
 _ = madeWide
 madeNamed := complex(named, named)
 _ = madeNamed
 madeNamedLiteral := complex(named, 2)
 _ = madeNamedLiteral
 madeAlias := complex(NumericFloatAlias(1), f)
 _ = madeAlias
 madeTuple := complex(NumericPair())
 _ = madeTuple
 realNarrow := real(c)
 _ = realNarrow
 imagWide := imag(z)
 _ = imagWide
 realNamed := real(nc)
 _ = realNamed
 realNumber := real(7)
 _ = realNumber
 imagNumber := imag(7)
 _ = imagNumber
 realFraction := real(complex(1.25, 2.5))
 _ = realFraction
 imagFraction := imag(complex(1.25, 2.5))
 _ = imagFraction
 realPureImag := real(2i)
 _ = realPureImag
 imagPureImag := imag(2i)
 _ = imagPureImag
 imagDecimal := imag(0123i)
 _ = imagDecimal
 imagHex := imag(0xffi)
 _ = imagHex
 imagBinary := imag(0b101i)
 _ = imagBinary
 complexZeroArgument := complex(1+0i, 2+0i)
 _ = complexZeroArgument
 sumComplex := (2+3i)+(4+5i)
 _ = sumComplex
 differenceComplex := (2+3i)-(4+5i)
 _ = differenceComplex
 productComplex := (2+3i)*(4+5i)
 _ = productComplex
 quotientComplex := (4+2i)/(2+1i)
 _ = quotientComplex
 negativeComplex := -(2+3i)
 _ = negativeComplex
 runtimeComplex := c + 2i
 _ = runtimeComplex
 runtimeNamedComplex := nc + 2i
 _ = runtimeNamedComplex
 complexConversion := complex64(1+2i)
 _ = complexConversion
 complexWiden := complex128(complex64(1+2i))
 _ = complexWiden
 zeroImagInt := int(2+0i)
 _ = zeroImagInt
 zeroImagFloat := float32(complex128(2))
 _ = zeroImagFloat
 realUint := uint8(real(complex(7, 3)))
 _ = realUint
 imagInt := int(imag(complex(7, 3)))
 _ = imagInt
 indexReal := [4]int{}[real(complex(3, 1))]
 _ = indexReal
 indexComplex := [4]int{}[3+0i]
 _ = indexComplex
 sliceComponent := []int{}[:imag(complex(3, 0))]
 _ = sliceComponent
 makeComponent := make([]int, real(complex(3, 1)))
 _ = makeComponent
 minComponent := min(real(complex(7, 3)), 2)
 _ = minComponent
 roundedCancel := [1]int{}[int(real((complex64(16777216)+1)-complex64(16777216)))]
 _ = roundedCancel
 roundedArgument := [1]int{}[int(real((complex64(16777216)+16777217)-complex64(33554432)))]
 _ = roundedArgument
 largeExact := [1]int{}[int(real(complex((1<<100)+1,0))-(1<<100)-1)]
 _ = largeExact
 allocatedComponent := new(real(2i))
 _ = allocatedComponent
 runtimeComplexZero := c / (1i-1i)
 _ = runtimeComplexZero
 runtimeFloatZero := f / 0
 _ = runtimeFloatZero
 fractionArithmetic := complex(0.25,0.5)+complex(0.25,0.5)
 _ = fractionArithmetic
 fractionQuotient := (1+1i)/(2+0i)
 _ = fractionQuotient
 hexFloatImag := 0x1.8p1i
 _ = hexFloatImag
 largeComplex := complex(1<<150,2)
 _ = largeComplex
}

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

func NativeMethodValues(box MethodBox[*Item], pointer *MethodBox[*Item], outer MethodOuter[*Item],
 nested MethodPointerOuter[*Item], view MethodView[*Item], values []MethodBox[*Item], entries map[int]MethodBox[*Item]) {
 boundGet := box.Get
 _ = boundGet
 boundPut := box.Put
 _ = boundPut
 boundPointer := pointer.Get
 _ = boundPointer
 boundPromoted := outer.Get
 _ = boundPromoted
 boundPromotedPointer := nested.Put
 _ = boundPromotedPointer
 boundInterface := view.Get
 _ = boundInterface
 boundInterfacePut := view.Put
 _ = boundInterfacePut
 boundAlias := MethodAlias{}.Get
 _ = boundAlias
 boundGenericAlias := MethodGenericAlias[*Item]{}.Get
 _ = boundGenericAlias
 boundSlicePointer := values[0].Put
 _ = boundSlicePointer
 boundMapValue := entries[0].Get
 _ = boundMapValue
 expressionValue := MethodBox[*Item].Get
 _ = expressionValue
 expressionPointer := (*MethodBox[*Item]).Put
 _ = expressionPointer
 expressionPromoted := MethodOuter[*Item].Get
 _ = expressionPromoted
 expressionPromotedPointer := MethodPointerOuter[*Item].Put
 _ = expressionPromotedPointer
 expressionInterface := MethodView[*Item].Get
 _ = expressionInterface
 expressionAlias := MethodAlias.Get
 _ = expressionAlias
 boundSpread := box.Spread
 _ = boundSpread
 expressionSpread := MethodBox[*Item].Spread
 _ = expressionSpread
 boundPair := box.Pair
 _ = boundPair
 expressionPair := MethodBox[*Item].Pair
 _ = expressionPair
 calledBound := boundGet()
 _ = calledBound
 calledPut := boundPut(nil)
 _ = calledPut
 calledExpression := expressionValue(box)
 _ = calledExpression
 calledPointerExpression := expressionPointer(pointer,nil)
 _ = calledPointerExpression
 calledPromotedExpression := expressionPromoted(outer)
 _ = calledPromotedExpression
 calledInterface := boundInterface()
 _ = calledInterface
 calledInterfaceExpression := expressionInterface(view)
 _ = calledInterfaceExpression
 calledGenericArgument := MethodApply(box.Get)
 _ = calledGenericArgument
 calledExplicitGenericArgument := MethodApply[*Item](pointer.Get)
 _ = calledExplicitGenericArgument
 calledVariadic := boundSpread(nil,nil)
 _ = calledVariadic
 calledSpread := boundSpread([]*Item{}...)
 _ = calledSpread
 calledExpressionSpread := expressionSpread(box, []*Item{}...)
 _ = calledExpressionSpread
 calledExpressionVariadic := expressionSpread(box,nil,nil)
 _ = calledExpressionVariadic
 directGeneric := box.Get()
 _ = directGeneric
 directPointer := box.Put(nil)
 _ = directPointer
 paired, ok := boundPair()
 _, _ = paired, ok
 expressionPaired, expressionOK := expressionPair(box)
 _, _ = expressionPaired, expressionOK
 addressedMethodBox := &box
 _ = addressedMethodBox
 boundDereference := (*pointer).Put
 _ = boundDereference
 calledDereference := boundDereference(nil)
 _ = calledDereference
}


// Assertions share native interface relations and comma-ok projections.
type AssertReader interface { Get() *Item }
type AssertOther interface { Get() int }
type AssertAlias = MethodBox[*Item]
func NativeAssertions(value any, reader AssertReader) {
 assertConcrete := value.(MethodBox[*Item]); _ = assertConcrete
 assertPointer := reader.(*MethodBox[*Item]); _ = assertPointer
 assertValue := reader.(MethodBox[*Item]); _ = assertValue
 assertPromoted := reader.(MethodOuter[*Item]); _ = assertPromoted
 assertEmbeddedPointer := reader.(MethodPointerOuter[*Item]); _ = assertEmbeddedPointer
 assertAlias := reader.(AssertAlias); _ = assertAlias
 assertGenericAlias := reader.(MethodGenericAlias[*Item]); _ = assertGenericAlias
 assertInterface := value.(AssertReader); _ = assertInterface
 assertConflictingInterface := reader.(AssertOther); _ = assertConflictingInterface
 assertAnonymousInterface := reader.(interface{ Get() *Item }); _ = assertAnonymousInterface
 assertAnonymousStruct := value.(struct{ Field *Item }); _ = assertAnonymousStruct
 assertSlice := value.([]*Item); _ = assertSlice
 assertArray := value.([2]*Item); _ = assertArray
 assertMap := value.(map[string]*Item); _ = assertMap
 assertChan := value.(<-chan *Item); _ = assertChan
 assertFunction := value.(func() *Item); _ = assertFunction
 assertCalled := value.(func() *Item)(); _ = assertCalled
 assertMethod := reader.(*MethodBox[*Item]).Get(); _ = assertMethod
 assertField := reader.(MethodBox[*Item]).Value; _ = assertField
 assertDereference := *value.(**Item); _ = assertDereference
 assertIndexed := value.([]*Item)[0]; _ = assertIndexed
 assertArithmetic := value.(int) + 1; _ = assertArithmetic
 assertTuple, assertOK := reader.(*MethodBox[*Item]); _, _ = assertTuple, assertOK
 assertBasicTuple, assertBasicOK := value.(int); _, _ = assertBasicTuple, assertBasicOK
 assertInterfaceTuple, assertInterfaceOK := reader.(AssertOther); _, _ = assertInterfaceTuple, assertInterfaceOK
}
var AssertGlobal any
var AssertGlobalValue, AssertGlobalOK = AssertGlobal.(*Item)

func NativeAssertionGlobals() {
 assertGlobalValue := AssertGlobalValue; _ = assertGlobalValue
 assertGlobalOK := AssertGlobalOK; _ = assertGlobalOK
}


func NativeTypeSwitch(value any, reader AssertReader) {
 switch narrowed := value.(type) {
 case *MethodBox[*Item]:
  switchPointer := narrowed; _ = switchPointer
  switchPointerCall := narrowed.Get(); _ = switchPointerCall
  switchAddress := &narrowed; _ = switchAddress
  switchCopiedCall := switchPointer.Get(); _ = switchCopiedCall
 case MethodBox[*Item]:
  switchValue := narrowed; _ = switchValue
  switchValueCall := narrowed.Get(); _ = switchValueCall
  switchPointerMethod := narrowed.Put(nil); _ = switchPointerMethod
 case [](*Item):
  switchSlice := narrowed; _ = switchSlice
  switchIndex := narrowed[0]; _ = switchIndex
 case func(value *Item) *Item:
  switchFunction := narrowed; _ = switchFunction
  switchFunctionCall := narrowed(nil); _ = switchFunctionCall
 case interface{ Get() *Item }:
  switchInterface := narrowed; _ = switchInterface
  switchInterfaceCall := narrowed.Get(); _ = switchInterfaceCall
 case nil:
  switchNil := narrowed; _ = switchNil
 case int, string:
  switchMulti := narrowed; _ = switchMulti
 default:
  switchDefault := narrowed; _ = switchDefault
  switch inner := narrowed.(type) {
  case *Item:
   switchNested := inner; _ = switchNested
  }
 }
 switch narrowed := reader.(type) {
 case MethodAlias:
  switchAlias := narrowed; _ = switchAlias
 case *MethodBox[*Item]:
  switchStrictPointer := narrowed; _ = switchStrictPointer
 case nil:
  switchInterfaceNil := narrowed; _ = switchInterfaceNil
 default:
  switchInterfaceDefault := narrowed; _ = switchInterfaceDefault
 }
 switch value := value.(type) {
 case *Item:
  switchSameName := value; _ = switchSameName
  _ = func() { switchCaptured := value; _ = switchCaptured }
 }
 switch initValue := value; narrowed := initValue.(type) {
 case MethodGenericAlias[*Item]:
  switchInit := narrowed; _ = switchInit
 }
}
func NativeSwitchCaseTypeShadow(value any) {
 switch MethodAlias := value.(type) {
 case MethodAlias:
  switchHeaderType := MethodAlias; _ = switchHeaderType
 }
}


func NativeClauseNil(value any, reader AssertReader) {
 switch narrowed := value.(type) {
 case (((nil))):
  clauseParenNil := narrowed; _ = clauseParenNil
 default:
  clauseParenDefault := narrowed; _ = clauseParenDefault
 }
 switch narrowed := reader.(type) {
 case (nil):
  clauseInterfaceNil := narrowed; _ = clauseInterfaceNil
 case (MethodAlias):
  clauseParenAlias := narrowed; _ = clauseParenAlias
  clauseParenCall := narrowed.Get(); _ = clauseParenCall
 }
 switch narrowed := value.(type) {
 case int, ((nil)):
  clauseMixedNil := narrowed; _ = clauseMixedNil
 case string:
  clauseString := narrowed; _ = clauseString
 }
 switch narrowed := value.(type) {
 case struct { Field *Item }:
  clauseStruct := narrowed.Field; _ = clauseStruct
 case struct { Other *Item }:
  clauseOtherStruct := narrowed.Other; _ = clauseOtherStruct
 }
}

type ClauseReader interface { Get() *Item }
func NativeClauseNamedInterfaces(value any) {
 switch narrowed := value.(type) {
 case AssertReader: clauseNamedA := narrowed; _ = clauseNamedA
 case ClauseReader: clauseNamedB := narrowed; _ = clauseNamedB
 }
}


type ReceiveItems chan *Item
type ReceiveOnly <-chan *Item
type ReceiveCallback func(value *Item) *Item
func NativeReceives(both chan *Item, reader <-chan *Item, named ReceiveItems, only ReceiveOnly, numbers chan int, structs chan MethodBox[*Item], functions chan ReceiveCallback, slices chan []*Item, arrays chan [2]*Item, nested <-chan <-chan *Item) {
 received := <-both; _ = received
 recvOnly := <-reader; _ = recvOnly
 recvNamed := <-named; _ = recvNamed
 recvNamedOnly := <-only; _ = recvNamedOnly
 recvTuple, recvOK := <-reader; _, _ = recvTuple, recvOK
 recvCopy := recvTuple; _ = recvCopy
 recvBoolCopy := recvOK; _ = recvBoolCopy
 recvArithmetic := <-numbers + 1; _ = recvArithmetic
 recvField := (<-structs).Value; _ = recvField
 recvMethod := (<-structs).Get(); _ = recvMethod
 recvStruct := <-structs; _ = recvStruct
 recvAddressed := &recvStruct; _ = recvAddressed
 recvPointerCall := recvStruct.Put(nil); _ = recvPointerCall
 recvIndex := (<-slices)[0]; _ = recvIndex
 recvArray := <-arrays; _ = recvArray
 recvArraySlice := recvArray[:1]; _ = recvArraySlice
 recvNested := <- (<-nested); _ = recvNested
 recvFunction := <-functions; _ = recvFunction
 recvFunctionCall := recvFunction(nil); _ = recvFunctionCall
 recvDirectCall := (<-functions)(nil); _ = recvDirectCall
 recvFuncTuple, recvFuncOK := <-functions; _, _ = recvFuncTuple, recvFuncOK
 recvTupleCall := recvFuncTuple(nil); _ = recvTupleCall
 select {
 case selected, selectedOK := <-reader:
  recvSelect := selected; _ = recvSelect
  recvSelectOK := selectedOK; _ = recvSelectOK
  _ = func() { recvCaptured := selected; _ = recvCaptured }
 case named := <-named:
  recvShadow := named; _ = recvShadow
 }
}
func NativeReceiveFormal[C ~chan *Item | ~<-chan *Item](channel C) {
 recvFormal := <-channel; _ = recvFormal
 recvFormalTuple, recvFormalOK := <-channel; _, _ = recvFormalTuple, recvFormalOK
 recvFormalCopy := recvFormalTuple; _ = recvFormalCopy
 select { case chosen := <-channel: recvFormalSelect := chosen; _ = recvFormalSelect }
}
func ReceiveGlobalSource() chan *Item { channel := make(chan *Item,1); channel <- nil; return channel }
var ReceiveGlobal = ReceiveGlobalSource()
var ReceiveGlobalValue, ReceiveGlobalOK = <-ReceiveGlobal
func NativeReceiveGlobals() { recvGlobal := ReceiveGlobalValue; _ = recvGlobal; recvGlobalOK := ReceiveGlobalOK; _ = recvGlobalOK }

// Ordinary bindings retain formal declaration identity through initialization.
func BindingFormal[T any, S ~[]T, M ~map[string]T](value T, values S, table M) {
 var bindingZero T
 var bindingAssigned T = value
 bindingCopy := bindingAssigned
 var bindingSlice S = values
 bindingSliceCopy := bindingSlice
 var bindingMap M = table
 bindingElement, bindingPresent := bindingMap["key"]
 for _, bindingRange := range bindingSlice { bindingRangeCopy := bindingRange; _, _ = bindingRangeCopy, bindingZero }
 _, _, _, _ = bindingCopy, bindingElement, bindingPresent, bindingSliceCopy
}
func BindingShadow[T any](value T) {
 bindingOuter := value
 { T := 1; bindingShadowCopy := bindingOuter; _, _ = T, bindingShadowCopy }
 { type T string; bindingTypeShadowCopy := bindingOuter; _ = bindingTypeShadowCopy }
}
func BindingSpelledInt[int any](value int) { bindingIntCopy := value; _ = bindingIntCopy }
func BindingSecond[T any](value T) { bindingSecondCopy := value; _ = bindingSecondCopy }
func BindingTyped(value *Item, channel <-chan *Item, table map[string]*Item) {
 var bindingTyped *Item = value
 var bindingReceived *Item = <-channel
 var bindingIndexed *Item = table["key"]
 bindingTypedCopy := bindingTyped
 _, _, _ = bindingTypedCopy, bindingReceived, bindingIndexed
}

type BindingBox[T any] struct{ Value T }
func BindingComposite[T any]() { bindingBox := BindingBox[T]{}; bindingBoxCopy := bindingBox; bindingBoxField := bindingBoxCopy.Value; _ = bindingBoxField }
