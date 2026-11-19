package tasks

import (
	"testing"

	"github.com/jackc/pgx/v5/pgtype"
)

func TestTaskLabelsAreStoredAsPostgresArrays(t *testing.T) {
	for _, test := range []struct {
		name   string
		labels []string
		count  int
	}{
		{name: "omitted"},
		{name: "empty", labels: []string{}},
		{name: "blank labels removed", labels: []string{"", "  "}},
		{name: "normalized labels", labels: []string{" Bug ", "bug", "UI"}, count: 2},
	} {
		t.Run(test.name, func(t *testing.T) {
			labels, err := validateLabels(test.labels)
			if err != nil {
				t.Fatal(err)
			}
			codec := pgtype.NewMap()
			encoded, err := codec.Encode(pgtype.TextArrayOID, pgtype.BinaryFormatCode, labels, nil)
			if err != nil {
				t.Fatal(err)
			}
			// tasks.labels is NOT NULL. pgx encodes a nil slice as SQL NULL,
			// which overrides the column default and rejects the whole create.
			if encoded == nil {
				t.Fatal("labels encode as SQL NULL; tasks.labels requires a non-null array")
			}
			var stored []string
			if err := codec.Scan(pgtype.TextArrayOID, pgtype.BinaryFormatCode, encoded, &stored); err != nil {
				t.Fatal(err)
			}
			if len(stored) != test.count {
				t.Fatalf("stored %v; want %d normalized labels", stored, test.count)
			}
		})
	}
}
