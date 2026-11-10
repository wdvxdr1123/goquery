package valueshadowinvalid
import int "strings"
var _ = int.TrimSpace
var uint8 = 1
var Invalid uint8
func Probe() {
 badRole := Invalid
 _ = badRole
}
func AliasCall() { badRoleAlias := int(1); _ = badRoleAlias }
func ValueCall() { badRoleValue := uint8(1); _ = badRoleValue }
