package components

import (
	"context"
	"testing"
)

func TestMemoryStoreSessionLifecycle(t *testing.T) {
	s := NewMemoryStore()
	ctx := context.Background()
	rec := SessionRecord{ID: "s1", Secret: "sec", ReconnectToken: "rt", HostUser: "u1"}
	if err := s.CreateSession(ctx, rec); err != nil {
		t.Fatal(err)
	}
	got, ok, _ := s.GetSession(ctx, "s1")
	if !ok || got != rec {
		t.Fatalf("get session: ok=%v rec=%+v", ok, got)
	}
	_ = s.DeleteSession(ctx, "s1")
	if _, ok, _ := s.GetSession(ctx, "s1"); ok {
		t.Fatal("session should be gone after delete")
	}
}

func TestMemoryStoreShareLinkResolveAndRevoke(t *testing.T) {
	s := NewMemoryStore()
	ctx := context.Background()
	link := ShareLink{SessionID: "s1", Secret: "tok", Access: AccessControl}
	if err := s.PutShareLink(ctx, "tok", link); err != nil {
		t.Fatal(err)
	}
	got, ok, _ := s.ResolveShareLink(ctx, "tok")
	if !ok || got != link {
		t.Fatalf("resolve: ok=%v link=%+v", ok, got)
	}
	_ = s.DeleteShareLink(ctx, "tok")
	if _, ok, _ := s.ResolveShareLink(ctx, "tok"); ok {
		t.Fatal("link should be gone after revoke")
	}
}
