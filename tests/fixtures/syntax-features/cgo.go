package syntaxfeatures

// This file exercises the regenerate-cgo code lens.
/*
#include <stdlib.h>
*/
import "C"

// Random returns a C random number.
func Random() int {
	return int(C.random())
}
