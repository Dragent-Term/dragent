package model

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
)

type GitHubProvider struct {
	clientID     string
	clientSecret string
	redirectURL  string
}

func NewGitHubProvider(clientID, clientSecret, redirectURL string) *GitHubProvider {
	return &GitHubProvider{
		clientID:     clientID,
		clientSecret: clientSecret,
		redirectURL:  redirectURL,
	}
}

func (p *GitHubProvider) GetAuthURL(state string) string {
	return fmt.Sprintf("https://github.com/login/oauth/authorize?client_id=%s&redirect_uri=%s&scope=user:email&state=%s",
		p.clientID, url.QueryEscape(p.redirectURL), state)
}

func (p *GitHubProvider) ExchangeCode(ctx context.Context, code string) (string, error) {
	payload := map[string]string{
		"client_id":     p.clientID,
		"client_secret": p.clientSecret,
		"code":          code,
		"redirect_uri":  p.redirectURL,
	}
	body, err := json.Marshal(payload)
	if err != nil {
		return "", err
	}

	req, err := http.NewRequestWithContext(ctx, "POST", "https://github.com/login/oauth/access_token", bytes.NewBuffer(body))
	if err != nil {
		return "", err
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json")

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("github oauth exchange failed with status: %d", resp.StatusCode)
	}

	respBody, err := io.ReadAll(resp.Body)
	if err != nil {
		return "", err
	}

	var result struct {
		AccessToken string `json:"access_token"`
		Error       string `json:"error"`
	}
	if err := json.Unmarshal(respBody, &result); err != nil {
		return "", err
	}

	if result.Error != "" {
		return "", fmt.Errorf("github oauth exchange error: %s", result.Error)
	}

	return result.AccessToken, nil
}

func (p *GitHubProvider) FetchUserInfo(ctx context.Context, token string) (*OAuthUserInfo, error) {
	req, err := http.NewRequestWithContext(ctx, "GET", "https://api.github.com/user", nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Accept", "application/json")

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("failed to fetch user info: %d", resp.StatusCode)
	}

	var user struct {
		ID        int64  `json:"id"`
		Email     string `json:"email"`
		Name      string `json:"name"`
		Login     string `json:"login"`
		AvatarURL string `json:"avatar_url"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&user); err != nil {
		return nil, err
	}

	// Only an address GitHub has verified identifies the person: accounts
	// are matched by email, so an unverified one would let anyone claim an
	// existing account by adding its address to their own GitHub account.
	emails, err := p.fetchUserEmails(ctx, token)
	if err != nil {
		return nil, err
	}
	email, err := verifiedGitHubEmail(user.Email, emails)
	if err != nil {
		return nil, err
	}

	// GitHub exposes one display-name string (often unset) — fall back to the
	// login handle so OAuth users never end up completely nameless.
	fullName := user.Name
	if strings.TrimSpace(fullName) == "" {
		fullName = user.Login
	}
	firstName, lastName := splitFullName(fullName)

	return &OAuthUserInfo{
		Email:          email,
		ProviderUserID: fmt.Sprintf("%d", user.ID),
		FirstName:      firstName,
		LastName:       lastName,
		AvatarURL:      user.AvatarURL,
	}, nil
}

// gitHubEmail is one address from GitHub's /user/emails.
type gitHubEmail struct {
	Email    string `json:"email"`
	Primary  bool   `json:"primary"`
	Verified bool   `json:"verified"`
}

func (p *GitHubProvider) fetchUserEmails(ctx context.Context, token string) ([]gitHubEmail, error) {
	req, err := http.NewRequestWithContext(ctx, "GET", "https://api.github.com/user/emails", nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Accept", "application/json")

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("failed to fetch user emails: %d", resp.StatusCode)
	}

	var emails []gitHubEmail
	if err := json.NewDecoder(resp.Body).Decode(&emails); err != nil {
		return nil, err
	}
	return emails, nil
}

// errNoVerifiedEmail: the provider account has no address it has verified.
var errNoVerifiedEmail = fmt.Errorf("sign in with an account whose email address is verified")

// verifiedGitHubEmail picks the address to sign in with: the profile's own
// address if GitHub verified it, else the verified primary, else any
// verified address. Never an unverified one.
func verifiedGitHubEmail(profile string, emails []gitHubEmail) (string, error) {
	verified := func(address string) bool {
		for _, e := range emails {
			if e.Verified && strings.EqualFold(e.Email, address) {
				return true
			}
		}
		return false
	}
	if profile != "" && verified(profile) {
		return profile, nil
	}
	for _, e := range emails {
		if e.Primary && e.Verified && e.Email != "" {
			return e.Email, nil
		}
	}
	for _, e := range emails {
		if e.Verified && e.Email != "" {
			return e.Email, nil
		}
	}
	return "", errNoVerifiedEmail
}

type GoogleProvider struct {
	clientID     string
	clientSecret string
	redirectURL  string
}

func NewGoogleProvider(clientID, clientSecret, redirectURL string) *GoogleProvider {
	return &GoogleProvider{
		clientID:     clientID,
		clientSecret: clientSecret,
		redirectURL:  redirectURL,
	}
}

func (p *GoogleProvider) GetAuthURL(state string) string {
	return fmt.Sprintf("https://accounts.google.com/o/oauth2/v2/auth?client_id=%s&redirect_uri=%s&response_type=code&scope=openid%%20profile%%20email&state=%s",
		p.clientID, url.QueryEscape(p.redirectURL), state)
}

func (p *GoogleProvider) ExchangeCode(ctx context.Context, code string) (string, error) {
	data := url.Values{}
	data.Set("client_id", p.clientID)
	data.Set("client_secret", p.clientSecret)
	data.Set("code", code)
	data.Set("redirect_uri", p.redirectURL)
	data.Set("grant_type", "authorization_code")

	req, err := http.NewRequestWithContext(ctx, "POST", "https://oauth2.googleapis.com/token", strings.NewReader(data.Encode()))
	if err != nil {
		return "", err
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("google oauth token exchange failed: %d", resp.StatusCode)
	}

	var result struct {
		AccessToken string `json:"access_token"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return "", err
	}

	return result.AccessToken, nil
}

func (p *GoogleProvider) FetchUserInfo(ctx context.Context, token string) (*OAuthUserInfo, error) {
	req, err := http.NewRequestWithContext(ctx, "GET", "https://www.googleapis.com/oauth2/v2/userinfo", nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+token)

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("failed to fetch google user info: %d", resp.StatusCode)
	}

	var user struct {
		ID         string `json:"id"`
		Email      string `json:"email"`
		Name       string `json:"name"`
		GivenName  string `json:"given_name"`
		FamilyName string `json:"family_name"`
		Picture    string `json:"picture"`
		// Google's own check of the address: accounts are matched by email,
		// so an unverified one must never sign in.
		VerifiedEmail bool `json:"verified_email"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&user); err != nil {
		return nil, err
	}
	if !user.VerifiedEmail || user.Email == "" {
		return nil, errNoVerifiedEmail
	}

	firstName, lastName := user.GivenName, user.FamilyName
	if firstName == "" && lastName == "" {
		firstName, lastName = splitFullName(user.Name)
	}

	return &OAuthUserInfo{
		Email:          user.Email,
		ProviderUserID: user.ID,
		FirstName:      firstName,
		LastName:       lastName,
		AvatarURL:      user.Picture,
	}, nil
}

// splitFullName breaks a provider's single display-name string into first and
// last: first word vs everything after it ("Ada Lovelace King" → "Ada",
// "Lovelace King"). Single-word names land entirely in first.
func splitFullName(full string) (string, string) {
	parts := strings.SplitN(strings.TrimSpace(full), " ", 2)
	if len(parts) < 2 {
		return parts[0], ""
	}
	return parts[0], strings.TrimSpace(parts[1])
}

var _ OAuthProvider = (*GitHubProvider)(nil)
var _ OAuthProvider = (*GoogleProvider)(nil)
