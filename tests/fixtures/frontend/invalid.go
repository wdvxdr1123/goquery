// Intentionally malformed Go expressions for parser rejection checks.
package corpus

var _ = call(a,
var _ = pkg.
var _ = f(
var _ = a[]
var _ = f(a..., b)
var _ = a[b...]
var _ = a[low::max]
var _ = a[low:high:]
var _ = value.()
var _ = value.(1)
var _ = value.(pkg.Type + Other)
var _ = value.([])
var _ = value.(map[string])
var _ = value.(Box[])
var _ = value.(func)
var _ = Point{X:}
var _ = Point{X: 1 Y: 2}
var _ = []int{1, 2
var _ = {}
var _ = interface{}{}
var _ = (Point){}

var _ = ''
var _ = 'ab'
var _ = '\q'
var _ = "\q"
var _ = '\"'
var _ = "\'"
var _ = '\12'
var _ = '\400'
var _ = '\x0'
var _ = '\xGG'
var _ = '\uD800'
var _ = '\U00110000'
var _ = "\UFFFFFFFF"
var _ = "\u123"
var _ = '\nA'
