package niltype
type nil struct{ Value int }
func Probe(value any) {
 switch narrowed := value.(type) {
 case nil: literalName := narrowed; _ = literalName
 case *nil: pointerName := narrowed; _ = pointerName
 }
 switch narrowed := value.(type) {
 case ((nil)): parenthesizedName := narrowed; _ = parenthesizedName
 }
}
