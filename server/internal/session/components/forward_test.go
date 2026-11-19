package components

import "testing"

func TestForwardRegistryScopesAndExpiresForwards(t *testing.T) {
	r := NewForwardRegistry()
	f, err := r.Create("session-a", 3000, "frontend")
	if err != nil || f.Port != 3000 || f.Label != "frontend" {
		t.Fatalf("create: %+v, %v", f, err)
	}
	if _, ok := r.Get(f.ID); !ok {
		t.Fatal("forward should be present")
	}
	if err := r.Close(f.ID, "session-b"); err == nil {
		t.Fatal("different owner must not close forward")
	}
	if err := r.Close(f.ID, "session-a"); err != nil {
		t.Fatal(err)
	}
	if _, ok := r.Get(f.ID); ok {
		t.Fatal("forward should be closed")
	}
}

func TestForwardRegistryListsOnlyOwner(t *testing.T) {
	r := NewForwardRegistry()
	first, _ := r.Create("device-a", 3000, "frontend")
	_, _ = r.Create("device-b", 5432, "database")
	listed := r.List("device-a")
	if len(listed) != 1 || listed[0].ID != first.ID {
		t.Fatalf("owner forwards = %#v", listed)
	}
}

func TestForwardRegistryReusesOwnerPort(t *testing.T) {
	r := NewForwardRegistry()
	first, _ := r.Create("device-a", 3000, "frontend")
	second, _ := r.Create("device-a", 3000, "frontend again")
	if first.ID != second.ID || len(r.List("device-a")) != 1 {
		t.Fatalf("duplicate owner port created two forwards: %#v %#v", first, second)
	}
}

func TestClosingSharedSessionDoesNotCloseDeviceForward(t *testing.T) {
	r := NewForwardRegistry()
	deviceForward, _ := r.Create("device\x00user\x00mac", 3000, "frontend")
	_, _ = r.Create("shared-session", 5173, "legacy")
	r.CloseOwner("shared-session")
	if _, ok := r.Get(deviceForward.ID); !ok {
		t.Fatal("ending terminal sharing closed a device-scoped port forward")
	}
}

func TestForwardRegistryRejectsInvalidPorts(t *testing.T) {
	r := NewForwardRegistry()
	if _, err := r.Create("s", 0, ""); err == nil {
		t.Fatal("port zero must be rejected")
	}
}

func TestForwardRegistryAuthorizesOnlyItsOwnToken(t *testing.T) {
	r := NewForwardRegistry()
	forward, err := r.Create("session-a", 5173, "Vite")
	if err != nil {
		t.Fatal(err)
	}
	if forward.Token == "" {
		t.Fatal("forward token is empty")
	}
	if got, ok := r.Authorize(forward.ID, forward.Token); !ok || got.ID != forward.ID {
		t.Fatal("forward token should authorize its own forward")
	}
	if _, ok := r.Authorize(forward.ID, "wrong-token"); ok {
		t.Fatal("wrong forward token was authorized")
	}
}
