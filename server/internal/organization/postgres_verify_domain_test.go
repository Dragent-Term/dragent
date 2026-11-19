package organization

import (
	"errors"
	"testing"
)

func TestDomainTokenMatches(t *testing.T) {
	records := []string{"some-other-txt-record", "the-real-token"}
	if !domainTokenMatches(records, "the-real-token") {
		t.Fatal("expected the token to be found among the TXT records")
	}
}

func TestDomainTokenMatchesRejectsMismatch(t *testing.T) {
	records := []string{"wrong-token", "also-wrong"}
	if domainTokenMatches(records, "the-real-token") {
		t.Fatal("token must not match an unrelated TXT record")
	}
}

func TestDomainTokenMatchesEmptyRecords(t *testing.T) {
	if domainTokenMatches(nil, "the-real-token") {
		t.Fatal("no records must never match")
	}
}

func TestVerifyDomainReturnsUnverifiedOnLookupError(t *testing.T) {
	// A lookup error is the real net.LookupTXT behavior for a domain that
	// simply hasn't published the record yet (NXDOMAIN-shaped). VerifyDomain
	// must treat that as "not verified" and return before ever touching the
	// database — proven here by calling it on a repository with a nil pool:
	// if it tried to write, this would panic instead of returning cleanly.
	repo := &PostgresRepository{}
	d := OrganizationDomain{Domain: "example.com", VerificationToken: "anything"}

	lookupErr := errors.New("no such host")
	fake := DNSLookupTXT(func(host string) ([]string, error) {
		if host != d.VerificationHost() {
			t.Fatalf("looked up %q, want %q", host, d.VerificationHost())
		}
		return nil, lookupErr
	})

	verified, err := repo.VerifyDomain(t.Context(), d, fake)
	if err != nil {
		t.Fatalf("a lookup error must not surface as a handler error: %v", err)
	}
	if verified {
		t.Fatal("a lookup error must never report verified")
	}
}

func TestVerifyDomainReturnsUnverifiedOnNoMatch(t *testing.T) {
	// Same nil-pool proof as above, for the "record exists but doesn't
	// match" case — this must also return before any database write.
	repo := &PostgresRepository{}
	d := OrganizationDomain{Domain: "example.com", VerificationToken: "the-real-token"}

	fake := DNSLookupTXT(func(host string) ([]string, error) {
		return []string{"unrelated-record"}, nil
	})

	verified, err := repo.VerifyDomain(t.Context(), d, fake)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if verified {
		t.Fatal("a non-matching record must never report verified")
	}
}

func TestVerificationHostIsDedicatedSubdomain(t *testing.T) {
	d := OrganizationDomain{Domain: "acme.com"}
	got := d.VerificationHost()
	want := "_dragent-challenge.acme.com"
	if got != want {
		t.Fatalf("VerificationHost() = %q, want %q", got, want)
	}
}
