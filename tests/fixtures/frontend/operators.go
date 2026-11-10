package corpus

var _ = a + b * c
var _ = a - b - c
var _ = <-ch + -x
var _ = (a || b) && c
var _ = x &^ mask << shift

var _ = '\a' + '\b' + '\f' + '\n' + '\r' + '\t' + '\v'
var _ = '\\' + '\''
var _ = '\000' + '\377' + '\x00' + '\xff'
var _ = '\u0000' + '\uD7FF' + '\uE000' + '\U0010FFFF'
var _ = "\"\\\xFF\377\uD7FF\uE000\U0010FFFF"
var _ = `raw \q \UFFFFFFFF`
var _ = '世'
