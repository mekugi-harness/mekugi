package evidence

import (
	"context"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
)

type FileKind string

// File retains endpoint metadata without requiring its text to stay resident.
type File struct {
	Origin        string `json:",omitempty"`
	AlsoManaged   string `json:",omitempty"`
	Path          string
	Kind          FileKind `json:",omitempty"`
	Content       string   `json:",omitempty"`
	ContentRef    *Text    `json:",omitempty"`
	Size          int64    `json:",omitzero"`
	Hash          string   `json:",omitempty"`
	Link          string   `json:",omitempty"`
	Stamp         string   `json:",omitempty"`
	Error         string   `json:",omitempty"`
	WatchStamp    string   `json:"-"`
	WatchMoveOnly bool     `json:"-"`
	CopyOf        string   `json:",omitempty"`
}

func (f File) Text() (string, error) {
	if f.ContentRef != nil {
		return f.ContentRef.Read()
	}
	return f.Content, nil
}

func (f File) TextSize() int {
	if f.ContentRef != nil {
		return f.ContentRef.Size
	}
	return len(f.Content)
}

func (f File) TextDigest() string {
	if f.ContentRef != nil {
		return f.ContentRef.Digest
	}
	return digest([]byte(f.Content))
}

func (f *File) Spill(ctx context.Context, source *Source) error {
	if f.ContentRef != nil || f.Content == "" {
		return nil
	}
	ref, err := source.SaveText(ctx, f.Content)
	if err != nil {
		return err
	}
	f.Content, f.ContentRef = "", ref
	return nil
}

type PatchFile struct {
	BeforePath      string `json:",omitempty"`
	AfterPath       string `json:",omitempty"`
	Before          string `json:",omitempty"`
	BeforeRef       *Text  `json:",omitempty"`
	Exists          bool   `json:",omitzero"`
	Error           string `json:",omitempty"`
	TargetBefore    string `json:",omitempty"`
	TargetBeforeRef *Text  `json:",omitempty"`
	TargetExists    bool   `json:",omitzero"`
	TargetError     string `json:",omitempty"`
}

// Review keeps immutable path/status facts and direct counts alongside lazy text.
// Only Load returns a diffmodel value suitable for composition or painting.
type Review struct {
	diffmodel.ReviewFile
	Text    *Text `json:",omitempty"`
	Added   int   `json:",omitzero"`
	Removed int   `json:",omitzero"`
}

func Inline(file diffmodel.ReviewFile) Review { return Review{ReviewFile: file} }

func (r Review) Load() (diffmodel.ReviewFile, error) {
	file := r.ReviewFile
	if r.Text != nil {
		var err error
		file.Diff, err = r.Text.Read()
		if err != nil {
			return file, err
		}
	}
	return file, nil
}

func (r Review) LineCounts() (int, int) {
	if r.Text != nil {
		return r.Added, r.Removed
	}
	return r.ReviewFile.LineCounts()
}

func (r Review) DiffBytes() int {
	if r.Text != nil {
		return r.Text.Size
	}
	return len(r.Diff)
}
