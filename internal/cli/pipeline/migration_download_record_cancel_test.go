package pipeline

import (
	"bytes"
	"context"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"os"
	"path/filepath"
	"reflect"
	"testing"
)

var migrationUpdateDownloadRecordReadCancel = flag.Bool("migration-update-download-record-read-cancel", false, "capture frozen Go cancellation during record reads before validation diagnostics")

type downloadRecordReadCancelCase struct {
	Name                string  `json:"name"`
	InputHex            string  `json:"input_hex"`
	Mode                Mode    `json:"mode"`
	ClassifierReadBytes int     `json:"classifier_read_bytes"`
	CanceledDuringRead  bool    `json:"canceled_during_read"`
	Error               string  `json:"error"`
	Canceled            bool    `json:"canceled"`
	Stderr              string  `json:"stderr"`
	ActionIDs           []int64 `json:"action_ids"`
}

func TestMigrationDownloadRecordReadCancelPreemptsParsedValidationDiagnostics(t *testing.T) {
	var rows []downloadRecordReadCancelCase
	for _, input := range []struct{ name, value string }{
		{"unsupported-type", `{"id":"42","type":"user","url":"x"}` + "\n"},
		{"invalid-id", `{"id":" 42 ","type":"artwork","url":"x"}` + "\n"},
	} {
		row := downloadRecordReadCancelCase{Name: input.name, InputHex: hex.EncodeToString([]byte(input.value)), ActionIDs: []int64{}}
		ctx, cancel := context.WithCancel(context.Background())
		reader := &downloadRecordOwnedReader{data: []byte(input.value), mode: "plain"}
		mode, replay, _, err := resolveTextOrRecord(reader)
		if err != nil {
			t.Fatal(err)
		}
		row.Mode, row.ClassifierReadBytes = mode, reader.position
		reader.onRead = func() { row.CanceledDuringRead = true; cancel() }
		var diagnostics bytes.Buffer
		err = ConsumeActionRecords(ctx, replay, &diagnostics, "download", "skip",
			map[string]struct{}{"artwork": {}, "illust": {}, "manga": {}, "ugoira": {}},
			func(_ context.Context, id int64) error { row.ActionIDs = append(row.ActionIDs, id); return nil },
			func(err error) error { return err },
		)
		cancel()
		row.Error, row.Canceled, row.Stderr = downloadRecordErrorText(err), errors.Is(err, context.Canceled), diagnostics.String()
		if !row.Canceled || !row.CanceledDuringRead || row.Mode != RecordMode || row.ClassifierReadBytes != 1 || row.Stderr != "" || len(row.ActionIDs) != 0 {
			t.Fatalf("%s canceled read contract: %+v", row.Name, row)
		}
		rows = append(rows, row)
	}
	path := filepath.Join("..", "..", "..", "crates", "pixiv-cli", "tests", "fixtures", "download_record_read_cancel.json")
	if *migrationUpdateDownloadRecordReadCancel {
		data, err := json.MarshalIndent(rows, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, append(data, '\n'), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var expected []downloadRecordReadCancelCase
	if err := json.Unmarshal(data, &expected); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(rows, expected) {
		t.Fatal("frozen canceled record read validation contracts differ")
	}
}
