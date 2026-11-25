package evidence

import (
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"errors"
	"fmt"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/activitydetail"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/recordkind"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

var manifestName = recordkind.HashedName("change-evidence-", ".json")
var fileName = recordkind.HashedName("change-file-", ".json")

// Snapshot is the feature-owned acquisition/review value. Observation and
// Baseline contain their owner's metadata only; file collections are separate.
type Snapshot struct {
	SourceAnchor  activitydetail.SourceAnchor
	Observation   jsontext.Value
	Files         []File
	Baseline      jsontext.Value
	BaselineFiles []File
	Patches       []Patch
	Reviews       []Review
}

type Patch struct {
	Input string
	Files []PatchFile
}

type patchRefs struct {
	Input string
	Files []string
}

// Manifest text is bounded metadata. It never expands the referenced texts.
type Manifest struct {
	SourceAnchor  activitydetail.SourceAnchor `json:",omitzero"`
	Version       int
	Observation   jsontext.Value `json:",omitempty"`
	Files         []string       `json:",omitempty"`
	Baseline      jsontext.Value `json:",omitempty"`
	BaselineFiles []string       `json:",omitempty"`
	Patches       []patchRefs    `json:",omitempty"`
	Reviews       []string       `json:",omitempty"`
}

type fileRecord struct {
	Version int
	File    *File      `json:",omitempty"`
	Patch   *PatchFile `json:",omitempty"`
	Review  *Review    `json:",omitempty"`
}

func (r fileRecord) texts() []*Text {
	var texts []*Text
	if r.File != nil {
		texts = append(texts, r.File.ContentRef)
	}
	if r.Patch != nil {
		texts = append(texts, r.Patch.BeforeRef, r.Patch.TargetBeforeRef)
	}
	if r.Review != nil {
		texts = append(texts, r.Review.Text)
	}
	return texts
}

func (r fileRecord) validate(source *Source) error {
	kinds := 0
	if r.File != nil {
		kinds++
		if len(r.File.Content) > InlineBytes || r.File.Content != "" && r.File.ContentRef != nil {
			return errors.New("conflicting endpoint text")
		}
	}
	if r.Patch != nil {
		kinds++
		if len(r.Patch.Before) > InlineBytes || len(r.Patch.TargetBefore) > InlineBytes || r.Patch.Before != "" && r.Patch.BeforeRef != nil || r.Patch.TargetBefore != "" && r.Patch.TargetBeforeRef != nil {
			return errors.New("conflicting patch text")
		}
	}
	if r.Review != nil {
		kinds++
		if len(r.Review.Diff) > InlineBytes || r.Review.Diff != "" && r.Review.Text != nil || r.Review.Added < -1 || r.Review.Removed < -1 {
			return errors.New("conflicting review text")
		}
	}
	if r.Version != 1 || kinds != 1 {
		return errors.New("invalid file evidence kind/version")
	}
	for _, text := range r.texts() {
		if text == nil {
			continue
		}
		// Dependency inspection has no reader authority. Validate using a
		// detached handle, then attach only for an admitted read.
		if err := text.Bind(source); err != nil {
			return err
		}
	}
	return nil
}

func (m Manifest) dependencies() ([]string, error) {
	if (m.SourceAnchor.Call == "") != (m.SourceAnchor.Segment == 0) || m.SourceAnchor.Segment < 0 {
		return nil, errors.New("invalid capture source anchor")
	}
	if m.Version != 1 {
		return nil, errors.New("invalid capture manifest version")
	}
	refs := append(append(append([]string{}, m.Files...), m.BaselineFiles...), m.Reviews...)
	for _, patch := range m.Patches {
		refs = append(refs, patch.Files...)
	}
	for _, ref := range refs {
		if !fileName(ref) {
			return nil, errors.New("invalid capture file dependency")
		}
	}
	return refs, nil
}

func RecordKinds() []recordkind.Spec {
	return []recordkind.Spec{
		{ID: "change-stage", Match: stagedName, MaxRecordBytes: ChunkBytes, Dependencies: func(record recordkind.Record) ([]string, error) {
			var staged stagedText
			if err := decode(record.Name, record.Data, "change-stage-", &staged); err != nil {
				return nil, err
			}
			if err := staged.Text.Bind(nil); err != nil {
				return nil, err
			}
			return staged.Text.Parts, nil
		}},
		{ID: "change-evidence", Match: manifestName, MaxRecordBytes: history.MaxRecordBytes, Dependencies: func(record recordkind.Record) ([]string, error) {
			var m Manifest
			if err := decode(record.Name, record.Data, "change-evidence-", &m); err != nil {
				return nil, err
			}
			return m.dependencies()
		}},
		{ID: "change-file", Match: fileName, MaxRecordBytes: ChunkBytes, Dependencies: func(record recordkind.Record) ([]string, error) {
			var r fileRecord
			if err := decode(record.Name, record.Data, "change-file-", &r); err != nil {
				return nil, err
			}
			if err := r.validate(nil); err != nil {
				return nil, err
			}
			var refs []string
			for _, text := range r.texts() {
				if text != nil {
					refs = append(refs, text.Parts...)
				}
			}
			return refs, nil
		}},
		{ID: "change-text", Match: textName, MaxRecordBytes: ChunkBytes},
	}
}

func decode(name string, data []byte, prefix string, value any) error {
	if prefix+digest(data)+".json" != name {
		return fmt.Errorf("invalid retained evidence digest: %s", name)
	}
	return json.Unmarshal(data, value, json.RejectUnknownMembers(true))
}

func Open(source *Source, name string) (Manifest, error) {
	var m Manifest
	if !manifestName(name) {
		return m, errors.New("invalid capture evidence reference/reader")
	}
	data, err := source.read(name, history.MaxRecordBytes)
	if err != nil {
		return m, err
	}
	if err := decode(name, data, "change-evidence-", &m); err != nil {
		return m, err
	}
	_, err = m.dependencies()
	return m, err
}

func IsManifest(name string) bool { return manifestName(name) }

func readFile(source *Source, name string, budget *int) (fileRecord, error) {
	var r fileRecord
	if !fileName(name) {
		return r, errors.New("invalid file evidence reference")
	}
	data, err := source.read(name, ChunkBytes)
	if err != nil {
		return r, err
	}
	if len(data) > ChunkBytes {
		return r, errors.New("file evidence exceeds its metadata bound")
	}
	*budget -= len(data)
	if *budget < 0 {
		return r, errors.New("capture file metadata exceeds its bound")
	}
	if err := decode(name, data, "change-file-", &r); err != nil {
		return r, err
	}
	return r, r.validate(source)
}

func ReadFiles(source *Source, refs []string) ([]File, error) {
	budget := history.MaxRecordBytes
	return readFiles(source, refs, &budget)
}

func readFiles(source *Source, refs []string, budget *int) ([]File, error) {
	var files []File
	for _, ref := range refs {
		r, err := readFile(source, ref, budget)
		if err != nil {
			return nil, err
		}
		if r.File == nil {
			return nil, errors.New("expected endpoint evidence")
		}
		files = append(files, *r.File)
	}
	return files, nil
}

func ReadReviews(source *Source, refs []string) ([]Review, error) {
	budget := history.MaxRecordBytes
	return readReviews(source, refs, &budget)
}

func readReviews(source *Source, refs []string, budget *int) ([]Review, error) {
	var files []Review
	for _, ref := range refs {
		r, err := readFile(source, ref, budget)
		if err != nil {
			return nil, err
		}
		if r.Review == nil {
			return nil, errors.New("expected review evidence")
		}
		files = append(files, *r.Review)
	}
	return files, nil
}
