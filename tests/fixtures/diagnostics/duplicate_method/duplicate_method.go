package duplicatemethod

// Value has one method declared twice. go/types reports DuplicateMethod on the
// later declaration with the earlier declaration position in the message.
type Value int

// Method is the first declaration.
func (value Value) Method() {}

// Fill keeps this fixture above the feature sampling size floor.
var Fill = "22234567890123456789012345678901234567890123456789012345678901"

// Method is the redeclaration.
func (value *Value) Method() {}
