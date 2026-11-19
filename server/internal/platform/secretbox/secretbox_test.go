package secretbox

import "testing"

func TestSecretsRoundTripAndNeedTheirPassphrase(t *testing.T) {
	sealed, err := Encrypt(`{"token":"lin_api_x"}`, "passphrase")
	if err != nil {
		t.Fatal(err)
	}
	opened, err := Decrypt(sealed, "passphrase")
	if err != nil || opened != `{"token":"lin_api_x"}` {
		t.Fatalf("round trip: %q %v", opened, err)
	}
	if _, err := Decrypt(sealed, "another"); err == nil {
		t.Fatal("opened with the wrong passphrase")
	}
	if _, err := Encrypt("x", ""); err == nil {
		t.Fatal("encrypted without a passphrase")
	}
}
