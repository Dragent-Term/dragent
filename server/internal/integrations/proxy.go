package integrations

import (
	"bytes"
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"strings"
	"syscall"
	"time"
)

const (
	// What the app may send to a service in one request.
	maxCallBytes = 2 << 20
	// What the server reads of a service's answer.
	maxAnswerBytes = 8 << 20
	callTimeout    = 25 * time.Second
)

// Call is a request the app built for a service, without its key.
type Call struct {
	Method string `json:"method"`
	// The whole address, query included.
	URL string `json:"url"`
	// Only Accept, Content-Type and the version headers some services
	// want are passed on.
	Headers map[string]string `json:"headers"`
	// Base64 of the body, if there is one.
	Body string `json:"body"`
}

// Answer is what the service said back.
type Answer struct {
	Status      int    `json:"status"`
	ContentType string `json:"content_type"`
	// Base64 of the body.
	Body string `json:"body"`
}

// refusedError is a call the server won't send, and why.
type refusedError struct{ reason string }

func (e *refusedError) Error() string { return e.reason }

func refuse(format string, args ...any) error {
	return &refusedError{reason: fmt.Sprintf(format, args...)}
}

var passedHeaders = []string{"Accept", "Content-Type", "Notion-Version", "API-Version"}

var allowedMethods = map[string]bool{
	http.MethodGet:   true,
	http.MethodPost:  true,
	http.MethodPut:   true,
	http.MethodPatch: true,
}

// build makes the request to send: the call checked against where the
// connection's service lives, with the key added.
func build(ctx context.Context, p provider, call Call, secrets, config map[string]string) (*http.Request, error) {
	method := strings.ToUpper(strings.TrimSpace(call.Method))
	if !allowedMethods[method] {
		return nil, refuse("%s requests aren't sent", method)
	}
	base, err := p.base(config)
	if err != nil {
		return nil, refuse("the connection's address isn't valid")
	}
	target, err := url.Parse(call.URL)
	if err != nil || target.Host == "" {
		return nil, refuse("the request has no valid address")
	}
	// Only to this service, the way it was connected: same scheme, host
	// and port, under its path.
	prefix := strings.TrimSuffix(base.EscapedPath(), "/")
	path := target.EscapedPath()
	if target.User != nil ||
		!strings.EqualFold(target.Scheme, base.Scheme) ||
		!strings.EqualFold(target.Host, base.Host) ||
		(prefix != "" && path != prefix && !strings.HasPrefix(path, prefix+"/")) {
		return nil, refuse("that address isn't %s's", p.key)
	}
	var body io.Reader
	if call.Body != "" {
		raw, err := base64.StdEncoding.DecodeString(call.Body)
		if err != nil {
			return nil, refuse("the request body isn't base64")
		}
		body = bytes.NewReader(raw)
	}
	req, err := http.NewRequestWithContext(ctx, method, target.String(), body)
	if err != nil {
		return nil, refuse("the request can't be built")
	}
	for _, name := range passedHeaders {
		for key, value := range call.Headers {
			if strings.EqualFold(key, name) {
				req.Header.Set(name, value)
			}
		}
	}
	req.Header.Set("User-Agent", "Dragent")
	p.authorize(req, secrets, config)
	return req, nil
}

func send(ctx context.Context, client *http.Client, p provider, call Call, secrets, config map[string]string) (Answer, error) {
	ctx, cancel := context.WithTimeout(ctx, callTimeout)
	defer cancel()
	req, err := build(ctx, p, call, secrets, config)
	if err != nil {
		return Answer{}, err
	}
	resp, err := client.Do(req)
	if err != nil {
		var blocked *blockedAddressError
		if errors.As(err, &blocked) {
			return Answer{}, refuse("%s", blocked.Error())
		}
		return Answer{}, errors.New(strings.ReplaceAll(err.Error(), req.URL.RawQuery, "…"))
	}
	defer resp.Body.Close()
	raw, err := io.ReadAll(io.LimitReader(resp.Body, maxAnswerBytes))
	if err != nil {
		return Answer{}, fmt.Errorf("reading the answer: %w", err)
	}
	return Answer{
		Status:      resp.StatusCode,
		ContentType: resp.Header.Get("Content-Type"),
		Body:        base64.StdEncoding.EncodeToString(raw),
	}, nil
}

type blockedAddressError struct{ address string }

func (e *blockedAddressError) Error() string {
	return "that service's address is on a private network, which the server doesn't reach"
}

// publicAddress reports whether ip is on the public internet: not this
// machine, a private or carrier network, or the cloud's metadata service.
func publicAddress(ip net.IP) bool {
	if ip == nil || ip.IsLoopback() || ip.IsPrivate() || ip.IsUnspecified() ||
		ip.IsLinkLocalUnicast() || ip.IsLinkLocalMulticast() || ip.IsInterfaceLocalMulticast() ||
		ip.IsMulticast() {
		return false
	}
	// 100.64.0.0/10, shared by carriers (and some VPNs).
	if v4 := ip.To4(); v4 != nil && v4[0] == 100 && v4[1]&0xc0 == 64 {
		return false
	}
	return true
}

// serviceClient checks every address it connects to, after DNS, so a
// self-hosted service's address can't point the server at its own
// network.
func serviceClient() *http.Client {
	dialer := &net.Dialer{
		Timeout: 10 * time.Second,
		Control: func(_, address string, _ syscall.RawConn) error {
			host, _, err := net.SplitHostPort(address)
			if err != nil {
				return err
			}
			if !publicAddress(net.ParseIP(host)) {
				return &blockedAddressError{address: host}
			}
			return nil
		},
	}
	transport := &http.Transport{
		Proxy:                 nil,
		DialContext:           dialer.DialContext,
		TLSHandshakeTimeout:   10 * time.Second,
		ResponseHeaderTimeout: callTimeout,
		MaxIdleConnsPerHost:   4,
	}
	return &http.Client{
		Transport: transport,
		Timeout:   callTimeout,
		// A redirect could leave the service: don't follow any.
		CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse },
	}
}
