package providers

import "testing"

func TestResolveEndpointURL(t *testing.T) {
	tests := []struct {
		name    string
		baseURL string
		want    string
	}{
		{
			name:    "chat completions endpoint",
			baseURL: "https://openrouter.ai/api/v1/chat/completions",
			want:    "https://openrouter.ai/api/v1/chat/completions",
		},
		{
			name:    "completions endpoint",
			baseURL: "https://api.example.com/v1/completions",
			want:    "https://api.example.com/v1/completions",
		},
		{
			name:    "responses endpoint",
			baseURL: "https://api.example.com/v1/responses",
			want:    "https://api.example.com/v1/responses",
		},
		{
			name:    "trailing slash is trimmed",
			baseURL: "https://openrouter.ai/api/v1/chat/completions/",
			want:    "https://openrouter.ai/api/v1/chat/completions",
		},
		{
			name:    "surrounding whitespace and slash",
			baseURL: "  https://openrouter.ai/api/v1/chat/completions/  ",
			want:    "https://openrouter.ai/api/v1/chat/completions",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := resolveEndpointURL(tt.baseURL); got != tt.want {
				t.Fatalf("resolveEndpointURL(%q) = %q, want %q", tt.baseURL, got, tt.want)
			}
		})
	}
}
