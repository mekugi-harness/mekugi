package capture

// Source: apps/mekugi/internal/router/exec_observation.go:38:1250@1e6d58c6.

import (
	"cmp"
	"context"
	"errors"
	"io"
	"io/fs"
	"maps"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"time"

	changes "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/changes"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/commandanalysis"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/commandanalysis/execscope"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/modelprotocol/responses"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/evidence"
)

const (
	// execCaptureHold bounds how long a call item waits for its pre-call
	// capture. Unread paths become incomplete instead of delaying the host.
	execCaptureHold = 500 * time.Millisecond
	// MaxContentBytes bounds the text read on each side of a call.
	MaxContentBytes = 8 << 20
	// maxExecObservationBytes bounds the encoded pre-call capture that the call
	// record carries until its result arrives.
	maxExecObservationBytes = 8 << 20
	// maxExecRecordBytes keeps the encoded reviews of a derived record below
	// the store's record limit.

)

type FileKind = evidence.FileKind

const (
	Absent    FileKind = ""
	Text      FileKind = "text"
	Binary    FileKind = "binary"
	Symlink   FileKind = "symlink"
	Directory FileKind = "dir"
	Other     FileKind = "other"
)

// File is the bounded state of one path. Binary content keeps only
// size and hash; symlinks keep their target, never the target's content.
type File = evidence.File

// Omission records scope that was not captured at all.
type Omission struct {
	Origin string `json:",omitempty"`
	Path   string
	Reason string
	// Existing explicitly scoped fixer or formatter paths were not baselined. Their
	// after-state is checked against the observation clock before reporting.
	Deferred bool `json:",omitzero"`
}

// Observation is the durable pre-call capture of one observed call: a
// native exec_command, or all literal commands of one exec cell.
type Observation struct {
	// endpoints is an operation-local, already observed boundary. Interval
	// reconciliation must not replace it with a later filesystem read.
	Commands []commandanalysis.Command
	// CommandClasses retain pre-call classification for repeated literal
	// occurrences, without reclassifying providers after execution.
	CommandClasses []string `json:",omitempty"`
	Class          string
	Labels         []string `json:",omitempty"`
	Reason         string   `json:",omitempty"`
	Unresolved     bool     `json:",omitzero"`
	Files          []File
	Omitted        []Omission          `json:",omitempty"`
	Listings       []Listing           `json:",omitempty"`
	CodeMode       bool                `json:",omitzero"`
	Roots          []string            `json:",omitempty"`
	WindowStart    time.Time           `json:",omitzero"`
	Programs       []execscope.Program `json:",omitempty"`
	// Group identifies the response that emitted the call, so parallel
	// siblings finalized together share one record.
	Group string `json:",omitempty"`
	// Excluded paths belong to patch records of the same call.
	Excluded []string `json:",omitempty"`
	// RepeatedPaths names multiple potential writes in one call window. A
	// final equality cannot prove those intermediate attempts had no effects.
	RepeatedPaths bool `json:",omitzero"`
	// LiteralClass preserves pre-captured scope independently of uncertainty
	// about additional exec calls whose arguments are not yet resolved.
	LiteralClass string `json:",omitempty"`
	// Tree is the private workspace snapshot taken before a writer call. Its
	// comparison covers effects outside the source-named scope.
	Tree string `json:",omitempty"`
	// Checkpoints cover writer directories outside the workspace's
	// comparison, including nested repositories. Workspace remains the identity.
	Checkpoints map[string]string `json:",omitempty"`
	// CheckpointOmissions holds complete pre-writer inventories of physical
	// roots omitted from each checkpoint. Missing entries mean unknown coverage.
	// An empty inventory proves absence outside the tree; historical captures
	// without an inventory retain their conservative unknown-baseline behavior.
	CheckpointOmissions map[string][]string `json:",omitempty"`
}

// A successful mktemp report supplies an absent endpoint. Large temporary
// trees retain one coverage gap instead of acquiring their extracted contents;
// independently named writes remain eligible.
func (o *Observation) AddCreatedPath(path string, tree bool) {
	// Observation copies may share backing arrays with a running preview.
	o.Files, o.Listings = slices.Clip(o.Files), slices.Clip(o.Listings)
	if tree {
		err := boundTemporaryTree(path, time.Now().Add(execCaptureHold))
		if err != nil && !os.IsNotExist(err) {
			o.Omitted = append(slices.Clip(o.Omitted), Omission{Path: path, Reason: "temporary directory contents omitted: bounded tree acquisition"})
			return
		}
	}
	if !slices.ContainsFunc(o.Files, func(file File) bool { return file.Path == path }) {
		o.Files = append(o.Files, File{Path: path})
	}
	if tree && !slices.ContainsFunc(o.Listings, func(listing Listing) bool { return listing.Root == path }) {
		o.Listings = append(o.Listings, Listing{Root: path})
	}
}

// ReadDir batches cap directory enumeration too: WalkDir reads and sorts every
// child of a wide directory before its visitor can enforce an entry budget.
func boundTemporaryTree(root string, deadline time.Time) error {
	const limit = 256
	entries := 1
	directories := []string{root}
	for len(directories) != 0 {
		if entries > limit || time.Now().After(deadline) {
			return fs.ErrInvalid
		}
		directory := directories[len(directories)-1]
		directories = directories[:len(directories)-1]
		file, err := os.Open(directory)
		if err != nil {
			return err
		}
		for {
			children, err := file.ReadDir(min(32, limit+1-entries))
			entries += len(children)
			if entries > limit || time.Now().After(deadline) {
				_ = file.Close()
				return fs.ErrInvalid
			}
			for _, child := range children {
				if child.IsDir() {
					directories = append(directories, filepath.Join(directory, child.Name()))
				}
			}
			if err != nil {
				_ = file.Close()
				if errors.Is(err, io.EOF) {
					break
				}
				return err
			}
		}
	}
	return nil
}

// Member carries the original admission sequence independently of display order.
type Member struct {
	Observation
	Sequence uint64
}

// Outcome is the finalized status of an observed exec record.
type Outcome struct {
	DeletedDirectories []string `json:",omitempty"` // Captured directory roots absent after the call.
	Status             string
	Exit               *int `json:",omitempty"`
	Class              string
	Labels             []string `json:",omitempty"`
	Coverage           string
	CodeMode           bool `json:",omitzero"`
	// Scope lists the captured paths, the only paths the record compares
	// exactly.
	Scope       []string `json:",omitempty"`
	ScopeReason string   `json:",omitempty"`
	// Overlaps are calls whose windows overlapped this one on the same root.
	Overlaps []string `json:",omitempty"`
	// Background names still-running sessions for command-history diagnostics.
	Background []string `json:",omitempty"`
	// SharedWith names the record of a parallel sibling that holds this
	// call's effects.
	SharedWith string `json:",omitempty"`
}

func (o Observation) CommandClassesForCalls() []string {
	classes := slices.Clone(o.CommandClasses)
	// Older or timed-out captures without per-command facts can establish
	// neutrality only for their aggregate original scope.
	for len(classes) < len(o.Commands) {
		classes = append(classes, cmp.Or(o.LiteralClass, o.Class))
	}
	return classes
}

const (
	StatusCompleted = "completed"
	StatusFailed    = "failed"

	CoverageExact   = "exact"
	CoveragePartial = "partial"
)

// Options is the router state a capture consults.
type Options struct {
	// Evidence spills bounded file text into the admitted retained store.
	Evidence *evidence.Source
	// Workspaces supplies the invocation-owned fallback for unnamed effects.
	Workspaces *Workspaces
	// PreviewOnly keeps source-derived display baselines out of durable edits.
	PreviewOnly bool
	// Directory is the selected workspace for resolving explicit edit operands.
	Directory string
	// Excluded paths belong to patch records of the same call.
	Excluded []string
	// Changes scopes mchanges revert and apply by their recorded paths.
	Changes execscope.ChangeResolver
}

func (o *Observation) captureCheckpoint(ctx context.Context, workspaces *Workspaces, workspace, directory string, inventoryBudget int) int {
	checkpoint := workspaces.checkpoint(ctx, directory, true)
	tree := checkpoint.tree
	inventoryBytes := len(responses.MustMarshalJSON(checkpoint.omissions)) + len(responses.MustMarshalJSON(directory)) + 2
	if checkpoint.omissions != nil && inventoryBytes <= inventoryBudget {
		inventoryBudget -= inventoryBytes
		if o.CheckpointOmissions == nil {
			o.CheckpointOmissions = make(map[string][]string)
		}
		o.CheckpointOmissions[directory] = checkpoint.omissions
	}
	if directory == workspace {
		o.Tree = tree
	} else {
		if o.Checkpoints == nil {
			o.Checkpoints = make(map[string]string)
		}
		o.Checkpoints[directory] = tree
	}
	o.Roots = AddRoot(o.Roots, directory)
	if tree == "" {
		o.Unresolved = true
		o.Reason = strings.TrimPrefix(o.Reason+"; workspace checkpoint unavailable", "; ")
	}
	return inventoryBudget
}

// AddDirectoryCheckpoints acquires any independent directories already listed
// in the boundary witness, using the original observation's remaining budget.
func (o *Observation) AddDirectoryCheckpoints(ctx context.Context, workspaces *Workspaces, original *Observation, workspace string) {
	budget := maxExecObservationBytes - len(responses.MustMarshalJSON(original))
	for _, directory := range slices.Sorted(maps.Keys(o.Checkpoints)) {
		budget = o.captureCheckpoint(ctx, workspaces, workspace, directory, budget)
	}
}

func checkpointCovers(root, directory string) bool {
	if root == directory {
		return true
	}
	if !changes.PathWithin(directory, root) {
		return false
	}
	physicalRoot, rootErr := filepath.EvalSymlinks(root)
	physicalDirectory, directoryErr := filepath.EvalSymlinks(directory)
	if rootErr != nil || directoryErr != nil || !changes.PathWithin(physicalDirectory, physicalRoot) {
		return false
	}
	for path := physicalDirectory; path != physicalRoot; path = filepath.Dir(path) {
		if _, err := os.Lstat(filepath.Join(path, ".git")); !errors.Is(err, os.ErrNotExist) {
			return false
		}
	}
	return true
}

// NeedsDirectory distinguishes a separate repository or external working
// directory from one already covered by a pre-execution checkpoint.
func (o *Observation) NeedsDirectory(workspace, directory string) bool {
	if !filepath.IsAbs(directory) || checkpointCovers(workspace, directory) {
		return false
	}
	for root := range o.Checkpoints {
		if checkpointCovers(root, directory) {
			return false
		}
	}
	return true
}

// Isolate potentially stalled filesystem syscalls from forwarding while keeping
// outstanding auxiliary workers bounded across requests and threads.

// AddRoot adds root unless an existing root contains it, and drops
// roots it contains.
func AddRoot(roots []string, root string) []string {
	for _, existing := range roots {
		if changes.PathWithin(root, existing) {
			return roots
		}
	}
	roots = slices.DeleteFunc(roots, func(existing string) bool { return changes.PathWithin(existing, root) })
	return append(roots, root)
}

// scopePaths are the paths a capture claims: files it compares and roots it
// lists. Concurrent sweeps leave them to this record.
func (o Observation) ScopePaths() []string {
	var paths []string
	for _, file := range o.Files {
		paths = append(paths, file.Path)
	}
	for _, listing := range o.Listings {
		paths = append(paths, listing.Root)
	}
	for _, omission := range o.Omitted {
		if !omission.Deferred {
			paths = append(paths, omission.Path)
		}
	}
	return paths
}

// Listing names the files below one destination root before the call, so
// files a recursive copy or move creates there are found afterwards.
type Listing struct {
	Origin string `json:",omitempty"`
	Root   string
	// Entries maps each file's relative path to its stamp.
	Entries map[string]string `json:",omitempty"`
}

// removals are trees an earlier statement deletes or moves away.

// sourceLabel names the programs that made an exec record's edits, such as
// sed or python, so a review can tell them from stock patches.
func (o Observation) SourceLabel() string {
	var labels []string
	add := func(label string) {
		if label != "" && !slices.Contains(labels, label) {
			labels = append(labels, label)
		}
	}
	for _, label := range o.Labels {
		add(label)
	}
	for _, program := range o.Programs {
		if program.Direct {
			add(program.Label)
		}
	}
	if len(labels) == 0 {
		return "exec_command"
	}
	if len(labels) > 2 {
		labels = append(labels[:2], "…")
	}
	return strings.Join(labels, "+")
}
