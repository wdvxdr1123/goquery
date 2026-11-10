// Deliberately malformed function bodies.
package corpus

func missingType() { var a }
func missingValue() { a := }
func missingCall() { go }
func badIncrement() { a, b++ }
func missingRight() { a += }
func unclosed() { work()
func badType() { type T = 1 }
func missingCondition() { if {} }
func assignmentCondition() { if x := work() {} }
func badFor() { for i := 0; i < n {} }
func missingColon() { switch x { case 1 work() } }
func missingCaseSeparator() { switch x { case 1: work() other() } }
func badRangeOperator() { for i += range values {} }
func missingRangeValue() { for range {} }
func tooManyRangeNames() { for a, b, c := range values {} }
func missingGotoLabel() { goto }
func badReceiveAssignment() { select { case x += <-ch: return } }
func missingCommunicationColon() { select { case <-ch return } }
func missingVarType() { var ( a ) }
func unclosedGroup() { type ( T int }
func invalidSwitchBinding() { switch a, b := input.(type) {} }
func assignedSwitchBinding() { switch value = input.(type) {} }
func ambiguousLiteralHeader() { if T{} == x {} }
func missingGenericConstraint() { type Broken[T any, U] struct{} }
