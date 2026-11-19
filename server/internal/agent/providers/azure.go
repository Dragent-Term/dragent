package providers

import (
	"fmt"
	"strings"
)

// NewAzureAdapter creates a streaming adapter for Azure OpenAI Service.
// Azure uses per-deployment URLs and authenticates with an "api-key" header
// instead of the standard "Authorization: Bearer" pattern.
//
//   - endpoint:   Azure resource URL, e.g. https://myresource.openai.azure.com
//   - deployment: deployment name configured in Azure AI Studio
//   - apiKey:     Azure OpenAI API key
//   - apiVersion: e.g. "2024-02-01" (defaults to that if empty)
//
// Docs: https://learn.microsoft.com/azure/ai-services/openai/reference
func NewAzureAdapter(endpoint, deployment, apiKey, apiVersion string) *OpenAICompatAdapter {
	if apiVersion == "" {
		apiVersion = "2024-02-01"
	}
	baseURL := fmt.Sprintf("%s/openai/deployments/%s?api-version=%s",
		strings.TrimRight(endpoint, "/"), deployment, apiVersion)
	return NewOpenAICompatAdapter(OpenAICompatConfig{
		Name:       "azure",
		BaseURL:    baseURL,
		APIKey:     apiKey,
		AuthHeader: "api-key",
		AuthPrefix: "", // Azure does not use a "Bearer " prefix
	})
}
