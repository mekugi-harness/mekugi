package executionobservation

import (
	"context"
	"path/filepath"
	"slices"
	"strings"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/commandanalysis"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/commandanalysis/execscope"
	changecapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"
)

// NativeBoundaryHold bounds pre-execution baseline acquisition.
const NativeBoundaryHold = 500 * time.Millisecond

func nativeBoundaryID(call, item string) string { return call + ":native-boundary:" + item }

func shellCallDirectory(workspace, directory string) string {
	if !filepath.IsAbs(directory) {
		directory = filepath.Join(workspace, directory)
	}
	return filepath.Clean(directory)
}

func plannedShellCall(o *changecapture.Observation, workspace string, command commandanalysis.Command) bool {
	directory := shellCallDirectory(workspace, command.Workdir)
	return slices.ContainsFunc(o.Commands, func(planned commandanalysis.Command) bool {
		return planned.Command == command.Command && planned.Workdir == directory &&
			(command.Shell == "" || planned.Shell == command.Shell)
	})
}

// loadNativeBoundaries joins pre-execution evidence with actual native calls.
// Absent trace/baseline evidence never becomes a learned post-edit before-state.
func (p *Preparer) loadNativeBoundaries(ctx context.Context, workspace, callID string, h *mekugiHistory) error {
	if p.Retained.Store == nil {
		return nil
	}
	remaining := slices.Clone(h.NativePatches)
	var patches []changecapture.PatchObservation
	for _, tool := range h.nativeCell.Calls() {
		if tool.Tool != applyPatchToolName && tool.Tool != nativeExecCommandToolName {
			continue
		}
		baseline, found, err := lookupObservation(ctx, p.Retained, workspace, nativeBoundaryID(callID, tool.ID))
		if err != nil {
			return err
		}
		if found && baseline.CorrelationID != callID+"\x00boundary:"+tool.ID {
			found = false
		}
		if tool.Tool == applyPatchToolName {
			if i := slices.IndexFunc(remaining, func(p changecapture.PatchObservation) bool { return p.Input == tool.Input }); i >= 0 {
				patch := remaining[i]
				// Only a witnessed terminal-writer cut supersedes the eager
				// literal baseline; unrelated paths keep their earliest bytes.
				if found && baseline.ExecObservation != nil && len(baseline.NativePatches) == 1 {
					patch.Files = slices.Clone(patch.Files)
					for i, before := range patch.Files {
						for _, actual := range baseline.NativePatches[0].Files {
							if actual.BeforePath != before.BeforePath || actual.AfterPath != before.AfterPath {
								continue
							}
							path := before.BeforePath
							if path == "" {
								path = before.AfterPath
							}
							if slices.Contains(baseline.ExecObservation.Excluded, path) {
								patch.Files[i].Before, patch.Files[i].BeforeRef, patch.Files[i].Exists, patch.Files[i].Error = actual.Before, actual.BeforeRef, actual.Exists, actual.Error
							}
							if before.AfterPath != before.BeforePath && slices.Contains(baseline.ExecObservation.Excluded, before.AfterPath) {
								patch.Files[i].TargetBefore, patch.Files[i].TargetBeforeRef, patch.Files[i].TargetExists, patch.Files[i].TargetError = actual.TargetBefore, actual.TargetBeforeRef, actual.TargetExists, actual.TargetError
							}
						}
					}
				}
				patches = append(patches, patch)
				remaining = slices.Delete(remaining, i, i+1)
			} else if found && len(baseline.NativePatches) == 1 && baseline.NativePatches[0].Input == tool.Input {
				patches = append(patches, baseline.NativePatches[0])
			} else if tool.LocalEnvironment {
				// Actual targets with no witnessed before-state remain incomplete.
				// Never learn a baseline from bytes read after the native edit.
				patch, err := changecapture.PatchBoundary(tool.Input, workspace, nil)
				if err == nil {
					patches = append(patches, patch)
				}
			}
		} else if o := h.ExecObservation; o != nil {
			copy := *o
			if found && baseline.ExecObservation != nil {
				if len(baseline.ExecObservation.Checkpoints) != 0 {
					copy = changecapture.Merge([]changecapture.Member{{Observation: copy}, {Observation: *baseline.ExecObservation, Sequence: 1}})
					copy.Files = o.Files // named files still follow the native boundary rule below.
				}
				if baseline.ExecObservation.Unresolved {
					copy.Unresolved = true
					reason := baseline.ExecObservation.Reason
					if reason != "" && !strings.Contains(copy.Reason, reason) {
						copy.Reason = strings.TrimPrefix(copy.Reason+"; "+reason, "; ")
					}
				}
				copy.Files = rebaseNativeFiles(copy.Files, baseline.ExecObservation.Files)
				if len(baseline.ExecObservation.Roots) != 0 {
					copy.Roots = slices.Clone(copy.Roots)
					for _, root := range baseline.ExecObservation.Roots {
						copy.Roots = changecapture.AddRoot(copy.Roots, root)
					}
				}
				// The first native writer can witness named targets the eager
				// classifier couldn't resolve from the model's source alone.
				for _, file := range baseline.ExecObservation.Files {
					if !slices.ContainsFunc(copy.Files, func(before changecapture.File) bool { return before.Path == file.Path }) {
						copy.Files = append(copy.Files, file)
					}
				}
			}
			if tool.LocalEnvironment && o.LiteralClass != "" {
				directory := shellCallDirectory(workspace, tool.Command.Workdir)
				witnessed := found && baseline.ExecObservation != nil
				if !witnessed && !plannedShellCall(o, workspace, tool.Command) ||
					copy.NeedsDirectory(workspace, directory) &&
						(!witnessed || baseline.ExecObservation.Class != execscope.Neutral.String()) {
					copy.Unresolved = true
					const reason = "native working directory checkpoint unavailable"
					if !strings.Contains(copy.Reason, reason) {
						copy.Reason = strings.TrimPrefix(copy.Reason+"; "+reason, "; ")
					}
				}
			}
			h.ExecObservation = &copy
		}
	}
	h.NativePatches = append(patches, remaining...)
	if h.ExecObservation != nil {
		copy := *h.ExecObservation
		copy.Excluded = slices.Clone(copy.Excluded)
		for _, patch := range h.NativePatches {
			for _, file := range patch.Files {
				if file.BeforePath != "" {
					copy.Excluded = append(copy.Excluded, file.BeforePath)
				}
				if file.AfterPath != "" {
					copy.Excluded = append(copy.Excluded, file.AfterPath)
				}
			}
		}
		h.ExecObservation = &copy
	}
	return nil
}

func rebaseNativeFiles(files []changecapture.File, boundary []changecapture.File) []changecapture.File {
	files = slices.Clone(files)
	for _, file := range boundary {
		for i := range files {
			if files[i].Path == file.Path {
				before := file
				before.Origin, before.AlsoManaged = files[i].Origin, files[i].AlsoManaged
				files[i] = before
			}
		}
	}
	return files
}
