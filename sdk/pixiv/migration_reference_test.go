package pixiv_test

import (
	"bytes"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"testing"

	"github.com/FlanChanXwO/pixiv-cli/sdk/pixiv"
)

var migrationUpdateReference = flag.Bool("migration-update-reference", false, "capture fixed Go page reference contracts")

func TestMigrationPageReferencesMatchFrozenParsingAndCanonicalURLs(t *testing.T) {
	type row struct {
		Input     *string         `json:"input"`
		Reference pixiv.Reference `json:"reference"`
		Canonical string          `json:"canonical"`
		Error     json.RawMessage `json:"error"`
	}
	rows := []row{}
	for _, input := range []string{
		"", " \t\n", "42", "+42", "-1", "0", "9223372036854775807", "9223372036854775808", "garbage", "https://[invalid", "https://www.pixiv.net:bad/artworks/42",
		"https://www.pixiv.net/artworks/42", "https://pixiv.net/artworks/42", "https://WWW.PIXIV.NET/artworks/42", " HTTPS://www.pixiv.net/artworks/42 ", "https://www.pixiv.net:/artworks/42", "https://www.pixiv.net:443/artworks/42", "https://fake@www.pixiv.net/artworks/42", "http://www.pixiv.net/artworks/42", "https://pixiv.net.evil.invalid/artworks/42",
		"https://www.pixiv.net/ja/artworks/42?tracking=fake#fragment", "https://www.pixiv.net/zh-TW/artworks/42", "https://www.pixiv.net/ABCDEFGH-abcdefgh/artworks/42", "https://www.pixiv.net/a/artworks/42", "https://www.pixiv.net/abcdefghi/artworks/42", "https://www.pixiv.net/en-US-x/artworks/42", "https://www.pixiv.net/en_Us/artworks/42",
		"https://www.pixiv.net///artworks/42///", "https://www.pixiv.net/artworks//42", "https://www.pixiv.net/artworks/../artworks/42", "https://www.pixiv.net/%61rtworks/%34%32", "https://www.pixiv.net/artworks/%2B42", "https://www.pixiv.net/artworks/00042", "https://www.pixiv.net/artworks/0", "https://www.pixiv.net/artworks/-1", "https://www.pixiv.net/artworks/9223372036854775808", "https://www.pixiv.net/artworks/%zz", "https://www.pixiv.net/artworks/42?bad=%zz", "https://www.pixiv.net/artworks/42#%zz", "https://www.pixiv.net/member_illust.php?illust_id=42",
		"https://www.pixiv.net/novel/show.php?id=42", "https://www.pixiv.net/en/novel/show.php?id=%2B42&extra=x", "https://www.pixiv.net/novel/show.php?id=42&id=43", "https://www.pixiv.net/novel/show.php?id=42&bad=%zz", "https://www.pixiv.net/novel/show.php?id=42;extra=x", "https://www.pixiv.net/novel/show.php?id=42&extra=x;y", "https://www.pixiv.net/novel/show.php?%69d=00042", "https://www.pixiv.net/novel/show.php?id=+42", "https://www.pixiv.net/novel/show.php?id=0", "https://www.pixiv.net/novel/show.php", "https://www.pixiv.net/novel/42",
		"https://www.pixiv.net/users/42", "https://www.pixiv.net/users/42/artworks", "https://www.pixiv.net/users/42/bookmarks/artworks", "https://www.pixiv.net/users/42/bookmarks/novels", "https://www.pixiv.net/user/7/series/42", "https://www.pixiv.net/novel/series/42", "https://www.pixiv.net/ja/novel/series/42", "https://www.pixiv.net/user/0/series/42", "https://www.pixiv.net/user/7/series/0",
		"https://www.pixiv.net/novel/show.php#?id=42",
	} {
		row := row{Input: &input}
		ref, err := pixiv.ParseURL(input)
		row.Reference = ref
		if err == nil {
			row.Canonical, err = ref.CanonicalURL()
		}
		if err != nil {
			data, marshalErr := json.Marshal(err)
			if marshalErr != nil {
				t.Fatal(marshalErr)
			}
			row.Error = data
		}
		rows = append(rows, row)
	}
	for _, ref := range []pixiv.Reference{{}, {Kind: pixiv.ReferenceKindArtwork, ID: -1}, {Kind: pixiv.ReferenceKindArtwork, ID: 42, OwnerUserID: 7}, {Kind: pixiv.ReferenceKindArtworkSeries, ID: 42}, {Kind: pixiv.ReferenceKindArtworkSeries, ID: 42, OwnerUserID: -1}, {Kind: pixiv.ReferenceKindArtworkSeries, ID: 42, OwnerUserID: 7}, {Kind: "future", ID: 42}} {
		row := row{Reference: ref}
		canonical, err := ref.CanonicalURL()
		row.Canonical = canonical
		if err != nil {
			data, marshalErr := json.Marshal(err)
			if marshalErr != nil {
				t.Fatal(marshalErr)
			}
			row.Error = data
		}
		rows = append(rows, row)
	}
	data, err := json.MarshalIndent(rows, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	path := filepath.Join("..", "..", "docs", "migration", "contracts", "page-references.json")
	if *migrationUpdateReference {
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(data, want) {
		t.Fatal("page references differ from fixed Go reference")
	}
}
