// Shared validation errors and path helpers for the R2 mirror.
package relay

import (
	"errors"
	"io"
	"net/http"
	"path"
	"strings"
)

var (
	errInvalidOwner     = errors.New("invalid relay owner")
	errTooManyTabs      = errors.New("too many tabs")
	errTooManyWorktrees = errors.New("too many worktrees")
	errTooManyListings  = errors.New("too many file listings")
	errDuplicateListing = errors.New("duplicate file listing")
	errTooManyEntries   = errors.New("too many listing entries")
	errTooManyFiles     = errors.New("too many stored files")
	errDuplicateContent = errors.New("duplicate file content")
	errInvalidPath      = errors.New("invalid file path")
	errInvalidEntry     = errors.New("invalid file entry")
	errFileTooLarge     = errors.New("file content is too large")
	errBinaryContent    = errors.New("file content must be text")
)

// readLimited reads the whole request body up to max bytes (inclusive
// slack): it returns an error when the body exceeds max.
func readLimited(r *http.Request, max int) ([]byte, error) {
	raw, err := io.ReadAll(io.LimitReader(r.Body, int64(max)+1))
	if err != nil {
		return nil, err
	}
	if len(raw) > max {
		return nil, errors.New("body too large")
	}
	return raw, nil
}

// cleanPubPath normalizes a published mirror path for comparison.
func cleanPubPath(p string) string {
	p = strings.TrimSpace(p)
	if p == "" {
		return ""
	}
	return path.Clean(p)
}

// validPubPath accepts absolute, traversal-free mirror paths. The relay
// mirror is rooted at the host's filesystem view; ".." and empty paths are
// never addressable, and the host must already have resolved symlinks (they
// are never followed for listings).
func validPubPath(p string) bool {
	if p == "" || len(p) > maxPathLength {
		return false
	}
	if !path.IsAbs(p) {
		return false
	}
	cleaned := path.Clean(p)
	if cleaned != p {
		return false
	}
	for _, seg := range strings.Split(p, "/") {
		if seg == ".." {
			return false
		}
	}
	return true
}
