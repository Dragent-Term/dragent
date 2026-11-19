package config

import "testing"

func TestValidateConfigValue(t *testing.T) {
	cases := []struct {
		name      string
		valueType string
		raw       any
		want      string
		wantErr   bool
	}{
		{"int ok", "int", float64(5000), "5000", false},
		{"int rejects float", "int", float64(3.5), "", true},
		{"int rejects string", "int", "abc", "", true},
		{"int rejects negative", "int", float64(-1), "", true},
		{"duration ok", "duration_sec", float64(18000), "18000", false},
		{"float ok", "float", float64(2.5), "2.5", false},
		{"bool ok", "bool", true, "true", false},
		{"bool rejects number", "bool", float64(1), "", true},
		{"string ok", "string", "gpt-5.4-mini", "gpt-5.4-mini", false},
		{"string rejects empty", "string", "  ", "", true},
		{"string rejects number", "string", float64(1), "", true},
		{"unknown type", "weird", "x", "", true},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			got, err := validateConfigValue(c.valueType, c.raw)
			if c.wantErr && err == nil {
				t.Fatalf("expected error, got value %q", got)
			}
			if !c.wantErr && err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if !c.wantErr && got != c.want {
				t.Fatalf("got %q, want %q", got, c.want)
			}
		})
	}
}
