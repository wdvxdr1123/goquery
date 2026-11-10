package typeoracleinvalid

import "unsafe"

func MethodUnsafePointer() {
 badMethodUnsafeDeref := *unsafe.Pointer(nil)
 _ = badMethodUnsafeDeref
}
