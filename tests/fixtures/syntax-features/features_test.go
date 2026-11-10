package syntaxfeatures

import "testing"

// TestPipeline exercises the fixture pipeline.
func TestPipeline(t *testing.T) {
	tests := []struct {
		name   string
		steps  []Conversion
		values []float64
	}{
		{
			name: "single",
			steps: []Conversion{
				{Name: "double", Scale: 2},
			},
			values: []float64{1, 2},
		},
		{
			name:  "empty",
			steps: nil,
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			pipeline := &Pipeline{Steps: test.steps}
			for _, value := range test.values {
				result, err := pipeline.Evaluate(value)
				if err != nil {
					t.Fatalf("Evaluate(%v): %v", value, err)
				}
				if result < 0 {
					t.Errorf("Evaluate(%v) = %v", value, result)
				}
			}
		})
	}
}

// BenchmarkPipeline measures the fixture pipeline.
func BenchmarkPipeline(b *testing.B) {
	pipeline := &Pipeline{Steps: []Conversion{{Name: "double", Scale: 2}}}
	for range b.N {
		if _, err := pipeline.Evaluate(1); err != nil {
			b.Fatal(err)
		}
	}
}
