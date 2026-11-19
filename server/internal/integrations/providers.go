package integrations

import (
	"net/http"
	"net/url"
	"strings"
)

// provider is what the server knows about one service: which of the
// values the app sends are secret, where its API lives, and how a request
// carries the key. The desktop app builds the requests themselves; the
// server only adds the credentials and sends them on.
type provider struct {
	key string
	// Its keys can belong to a workspace rather than a person (a Sentry
	// internal integration, a Notion integration), so an organization may
	// share one connection.
	shareable bool
	// Values kept encrypted; every one is required.
	secrets []string
	// Values the app needs back (an address, a workspace); the ones in
	// required must be there.
	config   []string
	required []string
	// The fixed address its requests go to. Empty: the connection's own
	// "url" (a self-hosted server), else defaultURL.
	api        string
	defaultURL string
	authorize  func(req *http.Request, secrets, config map[string]string)
}

func header(name, prefix, secret string) func(*http.Request, map[string]string, map[string]string) {
	return func(req *http.Request, secrets, _ map[string]string) {
		req.Header.Set(name, prefix+secrets[secret])
	}
}

func bearer(secret string) func(*http.Request, map[string]string, map[string]string) {
	return header("Authorization", "Bearer ", secret)
}

var providers = map[string]provider{
	"gitlab": {
		key: "gitlab", shareable: true,
		secrets: []string{"token"}, config: []string{"url"},
		defaultURL: "https://gitlab.com",
		authorize:  header("PRIVATE-TOKEN", "", "token"),
	},
	"forgejo": {
		key:     "forgejo",
		secrets: []string{"token"}, config: []string{"url"}, required: []string{"url"},
		authorize: header("Authorization", "token ", "token"),
	},
	"linear": {
		key:     "linear",
		secrets: []string{"api_key"},
		api:     "https://api.linear.app",
		// Linear takes a personal key bare, without "Bearer".
		authorize: header("Authorization", "", "api_key"),
	},
	"jira": {
		key:     "jira",
		secrets: []string{"token"}, config: []string{"url", "email"}, required: []string{"url", "email"},
		authorize: func(req *http.Request, secrets, config map[string]string) {
			req.SetBasicAuth(config["email"], secrets["token"])
		},
	},
	"plane": {
		key:     "plane",
		secrets: []string{"api_key"}, config: []string{"workspace", "url"}, required: []string{"workspace"},
		defaultURL: "https://api.plane.so",
		authorize:  header("X-API-Key", "", "api_key"),
	},
	"youtrack": {
		key:     "youtrack",
		secrets: []string{"token"}, config: []string{"url"}, required: []string{"url"},
		authorize: bearer("token"),
	},
	"asana": {
		key:     "asana",
		secrets: []string{"token"}, config: []string{"workspace"},
		api:       "https://app.asana.com",
		authorize: bearer("token"),
	},
	"trello": {
		key:     "trello",
		secrets: []string{"api_key", "token"},
		api:     "https://api.trello.com",
		// Trello takes the key and its token as query parameters.
		authorize: func(req *http.Request, secrets, _ map[string]string) {
			query := req.URL.Query()
			query.Set("key", secrets["api_key"])
			query.Set("token", secrets["token"])
			req.URL.RawQuery = query.Encode()
		},
	},
	"monday": {
		key:       "monday",
		secrets:   []string{"token"},
		api:       "https://api.monday.com",
		authorize: header("Authorization", "", "token"),
	},
	"notion": {
		key: "notion", shareable: true,
		secrets:   []string{"token"},
		api:       "https://api.notion.com",
		authorize: bearer("token"),
	},
	"featurebase": {
		key: "featurebase", shareable: true,
		secrets:   []string{"api_key"},
		api:       "https://do.featurebase.app",
		authorize: header("X-API-Key", "", "api_key"),
	},
	"plain": {
		key: "plain", shareable: true,
		secrets: []string{"api_key"}, config: []string{"workspace"},
		api:       "https://core-api.uk.plain.com",
		authorize: bearer("api_key"),
	},
	"sentry": {
		key: "sentry", shareable: true,
		secrets: []string{"token"}, config: []string{"organization", "url"}, required: []string{"organization"},
		defaultURL: "https://sentry.io",
		authorize:  bearer("token"),
	},
	"slack": {
		key:       "slack",
		secrets:   []string{"token"},
		api:       "https://slack.com",
		authorize: bearer("token"),
	},
}

// base is the address a connection's requests must go to.
func (p provider) base(config map[string]string) (*url.URL, error) {
	raw := p.api
	if raw == "" {
		raw = strings.TrimSpace(config["url"])
	}
	if raw == "" {
		raw = p.defaultURL
	}
	return url.Parse(raw)
}

func contains(list []string, value string) bool {
	for _, item := range list {
		if item == value {
			return true
		}
	}
	return false
}
