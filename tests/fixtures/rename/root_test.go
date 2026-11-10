package rename

import "testing"

func TestHelper(t *testing.T) {
	record := Record{Name: "test"}
	record.Method()
	if ExportedVar != 1 {
		t.Fatal("unexpected")
	}
	if helperValue() == 0 {
		t.Fatal("unexpected")
	}
}
