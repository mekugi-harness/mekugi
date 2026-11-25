package history

import (
	"bytes"
	"crypto/sha256"
	jsonv1 "encoding/json"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/recordkind"
)

// Record is a retained host call envelope stored in sessionhistory.
type Record struct {
	Version      int
	Workspace    string
	CallID       string
	Commentary   bool
	CaptureOrder uint64 `json:",omitzero"`
	History      History
}

type History struct {
	ToolName             string
	PluginID             string
	Script               string
	Root                 string
	ExecutingThread      string `json:",omitempty"`
	Caller               string `json:",omitempty"`
	Source               string `json:",omitempty"`
	ChangeID             string
	ReviewFiles          []ReviewFile
	NativePatches        []NativePatch `json:",omitempty"`
	ResolvedBaseline     *Baseline     `json:",omitempty"`
	ExecObservation      *Observation  `json:",omitempty"`
	ExecOutcome          *Outcome      `json:",omitempty"`
	HostResults          []HostResult  `json:",omitempty"`
	CommandSegments      *Segments     `json:",omitempty"`
	CarrierName          string
	CarrierKind          string
	CarrierPayload       string
	Report               string
	OutputWarning        string
	TranslationError     string
	CorrelationID        string
	Attempt              int
	UpstreamItem         map[string]jsonv1.RawMessage
	ReplayCarrier        bool
	CommentaryMessageIDs []string
	AlreadySatisfied     bool
}

type ReviewFile struct {
	BeforePath string
	AfterPath  string
	Diff       string
	Incomplete string `json:",omitzero"`
	Directory  bool   `json:",omitzero"`
	Binary     bool   `json:",omitzero"`
	Link       bool   `json:",omitzero"`
	CopyFrom   string `json:",omitzero"`
	Origin     string `json:",omitzero"`
	OriginNote string `json:",omitzero"`
}

type NativePatch struct {
	Input string
	Files []NativeFile
}

type NativeFile struct {
	BeforePath   string `json:",omitempty"`
	AfterPath    string `json:",omitempty"`
	Before       string `json:",omitempty"`
	Exists       bool   `json:",omitzero"`
	Error        string `json:",omitempty"`
	TargetBefore string `json:",omitempty"`
	TargetExists bool   `json:",omitzero"`
	TargetError  string `json:",omitempty"`
}

type Baseline struct {
	Root      string
	Shell     string
	Files     []File     `json:",omitempty"`
	Omitted   []Omission `json:",omitempty"`
	Inventory *Inventory `json:",omitempty"`
}

type Command struct {
	Command string
	Workdir string
	Shell   string `json:",omitempty"`
}

type File struct {
	Origin      string `json:",omitempty"`
	AlsoManaged string `json:",omitempty"`
	Path        string
	Kind        string `json:",omitempty"`
	Content     string `json:",omitempty"`
	Size        int64  `json:",omitzero"`
	Hash        string `json:",omitempty"`
	Link        string `json:",omitempty"`
	Stamp       string `json:",omitempty"`
	Error       string `json:",omitempty"`
	CopyOf      string `json:",omitempty"`
}

type Omission struct {
	Origin   string `json:",omitempty"`
	Path     string
	Reason   string
	Deferred bool `json:",omitzero"`
}

type Observation struct {
	Commands       []Command
	CommandClasses []string `json:",omitempty"`
	Class          string
	Labels         []string `json:",omitempty"`
	Reason         string   `json:",omitempty"`
	Unresolved     bool     `json:",omitzero"`
	Files          []File
	Omitted        []Omission        `json:",omitempty"`
	Listings       []Listing         `json:",omitempty"`
	CodeMode       bool              `json:",omitzero"`
	Roots          []string          `json:",omitempty"`
	WindowStart    time.Time         `json:",omitzero"`
	Programs       []Program         `json:",omitempty"`
	Group          string            `json:",omitempty"`
	Excluded       []string          `json:",omitempty"`
	RepeatedPaths  bool              `json:",omitzero"`
	LiteralClass   string            `json:",omitempty"`
	Inventory      *Inventory        `json:",omitempty"`
	Tree           string            `json:",omitempty"`
	Checkpoints    map[string]string `json:",omitempty"`
}

type Outcome struct {
	DeletedDirectories []string `json:",omitempty"`
	OutputRef          string   `json:",omitempty"`
	Status             string
	Exit               *int `json:",omitempty"`
	Class              string
	Labels             []string `json:",omitempty"`
	Coverage           string
	CodeMode           bool     `json:",omitzero"`
	Scope              []string `json:",omitempty"`
	ScopeReason        string   `json:",omitempty"`
	Unswept            string   `json:",omitempty"`
	Overlaps           []string `json:",omitempty"`
	Background         []string `json:",omitempty"`
	SharedWith         string   `json:",omitempty"`
}

type Listing struct {
	Origin  string `json:",omitempty"`
	Root    string
	Entries map[string]string `json:",omitempty"`
}

type Inventory struct {
	Version      int                       `json:",omitzero"`
	Directories  map[string]DirectoryStamp `json:",omitempty"`
	Root         string
	Entries      map[string]string `json:",omitempty"`
	Blobs        map[string]string `json:",omitempty"`
	Files        []File            `json:",omitempty"`
	Repositories []string          `json:",omitempty"`
	Pruned       []string          `json:",omitempty"`
	Omitted      []Omission        `json:",omitempty"`
}

type DirectoryStamp struct {
	Exists   bool
	Stamp    string
	Digest   string
	Complete bool
}

type Program struct {
	Label  string
	Direct bool `json:",omitzero"`
	VCS    bool `json:",omitzero"`
}

type HostResult struct {
	CallID   string
	Tool     string
	Status   string
	ExitCode *int `json:",omitempty"`
}

type Segments struct {
	Command string
	Exit    int
	Output  [32]byte
	Parts   []Segment
}

type Segment struct {
	Timing  Timing `json:",omitzero"`
	Source  string
	Skipped bool
	Exit    int
	Output  *string
}

type Timing struct {
	Started   time.Time `json:"started,omitzero"`
	Ended     time.Time `json:"ended,omitzero"`
	ElapsedNS int64     `json:"elapsed_ns,omitzero"`
}

func Name(workspace, callID string, commentary bool) string {
	prefix := "call-"
	if commentary {
		prefix = "commentary-"
	}
	return prefix + fmt.Sprintf("%x.json", sha256.Sum256(fmt.Appendf(nil, "%t\x00%s\x00%s", commentary, workspace, callID)))
}

func ValidName(name string, commentary bool) bool {
	prefix := "call-"
	if commentary {
		prefix = "commentary-"
	}
	return recordkind.HashedName(prefix, ".json")(name)
}

// DecodeRecord validates a version-1 call envelope against its expected identity.
func DecodeRecord(workspace, callID string, commentary bool, data []byte, limit int) (Record, error) {
	if len(data) > limit {
		return Record{}, errors.New("replay record exceeds size limit")
	}
	var r Record
	dec := jsontext.NewDecoder(bytes.NewReader(data))
	if err := json.UnmarshalDecode(dec, &r, jsonv1.DefaultOptionsV1(), json.RejectUnknownMembers(true)); err != nil {
		return r, fmt.Errorf("decode replay record: %w", err)
	}
	if err := json.UnmarshalDecode(dec, new(any), jsonv1.DefaultOptionsV1()); !errors.Is(err, io.EOF) {
		return r, errors.New("trailing replay record data")
	}
	if r.Version != 1 || r.Workspace != workspace || r.CallID != callID || r.Commentary != commentary {
		return r, errors.New("replay record identity/version mismatch")
	}
	return r, nil
}
