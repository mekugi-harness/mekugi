package executionobservation

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/commandanalysis"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/commandanalysis/execscope"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
	changecapture "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/capture"
)

// The historical wire schema has no live preview or request-local state.
// Conversion leaves those fields unset, just as reading retained JSON did.
// Legacy schema, decoded only when reading retained pre-redesign records.
type resolvedStockBaseline struct {
	Root    string
	Shell   string
	Files   []changecapture.File     `json:",omitempty"`
	Omitted []changecapture.Omission `json:",omitempty"`
}

func legacyBaseline(v history.Baseline) resolvedStockBaseline {
	return resolvedStockBaseline{
		Root:    v.Root,
		Shell:   v.Shell,
		Files:   legacySlice(v.Files, legacyFile),
		Omitted: legacySlice(v.Omitted, func(v history.Omission) changecapture.Omission { return changecapture.Omission(v) }),
	}
}

func legacyFile(v history.File) changecapture.File {
	return changecapture.File{
		Origin:      v.Origin,
		AlsoManaged: v.AlsoManaged,
		Path:        v.Path,
		Kind:        changecapture.FileKind(v.Kind),
		Content:     v.Content,
		Size:        v.Size,
		Hash:        v.Hash,
		Link:        v.Link,
		Stamp:       v.Stamp,
		Error:       v.Error,
		CopyOf:      v.CopyOf,
	}
}

func legacyHistory(v history.History) mekugiHistory {
	result := mekugiHistory{
		ToolName:             v.ToolName,
		PluginID:             v.PluginID,
		Script:               v.Script,
		Root:                 v.Root,
		ExecutingThread:      v.ExecutingThread,
		Caller:               v.Caller,
		Source:               v.Source,
		ChangeID:             v.ChangeID,
		ReviewFiles:          legacySlice(v.ReviewFiles, func(v history.ReviewFile) diffmodel.ReviewFile { return diffmodel.ReviewFile(v) }),
		NativePatches:        legacySlice(v.NativePatches, legacyNativePatch),
		ResolvedBaseline:     legacyPointer(v.ResolvedBaseline, legacyBaseline),
		ExecObservation:      legacyPointer(v.ExecObservation, legacyObservation),
		ExecOutcome:          legacyPointer(v.ExecOutcome, legacyOutcome),
		HostResults:          legacySlice(v.HostResults, func(v history.HostResult) binding.ToolResult { return binding.ToolResult(v) }),
		CommandSegments:      v.CommandSegments,
		CarrierName:          v.CarrierName,
		CarrierKind:          codeModeCarrierKind(v.CarrierKind),
		CarrierPayload:       v.CarrierPayload,
		Report:               v.Report,
		OutputWarning:        v.OutputWarning,
		TranslationError:     v.TranslationError,
		CorrelationID:        v.CorrelationID,
		Attempt:              v.Attempt,
		UpstreamItem:         v.UpstreamItem,
		ReplayCarrier:        v.ReplayCarrier,
		CommentaryMessageIDs: v.CommentaryMessageIDs,
		AlreadySatisfied:     v.AlreadySatisfied,
	}
	result.legacy = legacyMetadata(v)
	return result
}

func legacyMetadata(v history.History) *history.History {
	if v.ExecObservation != nil && v.ExecObservation.Inventory != nil ||
		v.ResolvedBaseline != nil && v.ResolvedBaseline.Inventory != nil ||
		v.ExecOutcome != nil && v.ExecOutcome.Unswept != "" {
		return new(v)
	}
	return nil
}

func legacyNativePatch(v history.NativePatch) changecapture.PatchObservation {
	return changecapture.PatchObservation{
		Input: v.Input,
		Files: legacySlice(v.Files, func(v history.NativeFile) changecapture.PatchFile {
			return changecapture.PatchFile{BeforePath: v.BeforePath, AfterPath: v.AfterPath, Before: v.Before, Exists: v.Exists, Error: v.Error, TargetBefore: v.TargetBefore, TargetExists: v.TargetExists, TargetError: v.TargetError}
		}),
	}
}

func legacyObservation(v history.Observation) changecapture.Observation {
	return changecapture.Observation{
		Commands:       legacySlice(v.Commands, func(v history.Command) commandanalysis.Command { return commandanalysis.Command(v) }),
		CommandClasses: v.CommandClasses,
		Class:          v.Class,
		Labels:         v.Labels,
		Reason:         v.Reason,
		Unresolved:     v.Unresolved,
		Files:          legacySlice(v.Files, legacyFile),
		Omitted:        legacySlice(v.Omitted, func(v history.Omission) changecapture.Omission { return changecapture.Omission(v) }),
		Listings:       legacySlice(v.Listings, func(v history.Listing) changecapture.Listing { return changecapture.Listing(v) }),
		CodeMode:       v.CodeMode,
		Roots:          v.Roots,
		WindowStart:    v.WindowStart,
		Programs:       legacySlice(v.Programs, func(v history.Program) execscope.Program { return execscope.Program(v) }),
		Group:          v.Group,
		Excluded:       v.Excluded,
		RepeatedPaths:  v.RepeatedPaths,
		LiteralClass:   v.LiteralClass,
		Tree:           v.Tree,
		Checkpoints:    v.Checkpoints,
	}
}

func legacySlice[A, B any](values []A, convert func(A) B) []B {
	if values == nil {
		return nil
	}
	result := make([]B, len(values))
	for i, value := range values {
		result[i] = convert(value)
	}
	return result
}

func legacyPointer[A, B any](value *A, convert func(A) B) *B {
	if value == nil {
		return nil
	}
	return new(convert(*value))
}

// Typed neutral publication and cache conversion; live-only fields stay local.
// Source: apps/mekugi/internal/router/legacy_call.go:14:143@d002a0dc, inverse conversion.
func retainedBaseline(v resolvedStockBaseline) history.Baseline {
	return history.Baseline{
		Root:    v.Root,
		Shell:   v.Shell,
		Files:   legacySlice(v.Files, retainedFile),
		Omitted: legacySlice(v.Omitted, func(v changecapture.Omission) history.Omission { return history.Omission(v) }),
	}
}

func retainedFile(v changecapture.File) history.File {
	return history.File{
		Origin:      v.Origin,
		AlsoManaged: v.AlsoManaged,
		Path:        v.Path,
		Kind:        string(v.Kind),
		Content:     v.Content,
		Size:        v.Size,
		Hash:        v.Hash,
		Link:        v.Link,
		Stamp:       v.Stamp,
		Error:       v.Error,
		CopyOf:      v.CopyOf,
	}
}

func retainedHistory(v mekugiHistory) history.History {
	result := history.History{
		ToolName:             v.ToolName,
		PluginID:             v.PluginID,
		Script:               v.Script,
		Root:                 v.Root,
		ExecutingThread:      v.ExecutingThread,
		Caller:               v.Caller,
		Source:               v.Source,
		ChangeID:             v.ChangeID,
		ReviewFiles:          legacySlice(v.ReviewFiles, func(v diffmodel.ReviewFile) history.ReviewFile { return history.ReviewFile(v) }),
		NativePatches:        legacySlice(v.NativePatches, retainedNativePatch),
		ResolvedBaseline:     legacyPointer(v.ResolvedBaseline, retainedBaseline),
		ExecObservation:      legacyPointer(v.ExecObservation, retainedObservation),
		ExecOutcome:          legacyPointer(v.ExecOutcome, retainedOutcome),
		HostResults:          legacySlice(v.HostResults, func(v binding.ToolResult) history.HostResult { return history.HostResult(v) }),
		CommandSegments:      v.CommandSegments,
		CarrierName:          v.CarrierName,
		CarrierKind:          string(v.CarrierKind),
		CarrierPayload:       v.CarrierPayload,
		Report:               v.Report,
		OutputWarning:        v.OutputWarning,
		TranslationError:     v.TranslationError,
		CorrelationID:        v.CorrelationID,
		Attempt:              v.Attempt,
		UpstreamItem:         v.UpstreamItem,
		ReplayCarrier:        v.ReplayCarrier,
		CommentaryMessageIDs: v.CommentaryMessageIDs,
		AlreadySatisfied:     v.AlreadySatisfied,
	}
	// Frozen metadata belongs to the historical record, not live capture.
	if old := v.legacy; old != nil {
		if old.ExecObservation != nil && result.ExecObservation != nil {
			result.ExecObservation.Inventory = old.ExecObservation.Inventory
		}
		if old.ResolvedBaseline != nil && result.ResolvedBaseline != nil {
			result.ResolvedBaseline.Inventory = old.ResolvedBaseline.Inventory
		}
		if old.ExecOutcome != nil && old.ExecOutcome.Unswept != "" {
			if result.ExecOutcome == nil {
				result.ExecOutcome = new(history.Outcome)
			}
			result.ExecOutcome.Unswept = old.ExecOutcome.Unswept
		}
	}
	return result
}

func retainedNativePatch(v changecapture.PatchObservation) history.NativePatch {
	return history.NativePatch{
		Input: v.Input,
		Files: legacySlice(v.Files, func(v changecapture.PatchFile) history.NativeFile {
			return history.NativeFile{BeforePath: v.BeforePath, AfterPath: v.AfterPath, Before: v.Before, Exists: v.Exists, Error: v.Error, TargetBefore: v.TargetBefore, TargetExists: v.TargetExists, TargetError: v.TargetError}
		}),
	}
}

func retainedObservation(v changecapture.Observation) history.Observation {
	return history.Observation{
		Commands:       legacySlice(v.Commands, func(v commandanalysis.Command) history.Command { return history.Command(v) }),
		CommandClasses: v.CommandClasses,
		Class:          v.Class,
		Labels:         v.Labels,
		Reason:         v.Reason,
		Unresolved:     v.Unresolved,
		Files:          legacySlice(v.Files, retainedFile),
		Omitted:        legacySlice(v.Omitted, func(v changecapture.Omission) history.Omission { return history.Omission(v) }),
		Listings:       legacySlice(v.Listings, func(v changecapture.Listing) history.Listing { return history.Listing(v) }),
		CodeMode:       v.CodeMode,
		Roots:          v.Roots,
		WindowStart:    v.WindowStart,
		Programs:       legacySlice(v.Programs, func(v execscope.Program) history.Program { return history.Program(v) }),
		Group:          v.Group,
		Excluded:       v.Excluded,
		RepeatedPaths:  v.RepeatedPaths,
		LiteralClass:   v.LiteralClass,
		Tree:           v.Tree,
		Checkpoints:    v.Checkpoints,
	}
}

func legacyOutcome(v history.Outcome) changecapture.Outcome {
	return changecapture.Outcome{
		DeletedDirectories: v.DeletedDirectories,
		Status:             v.Status,
		Exit:               v.Exit,
		Class:              v.Class,
		Labels:             v.Labels,
		Coverage:           v.Coverage,
		CodeMode:           v.CodeMode,
		Scope:              v.Scope,
		ScopeReason:        v.ScopeReason,
		Overlaps:           v.Overlaps,
		Background:         v.Background,
		SharedWith:         v.SharedWith,
	}
}

func retainedOutcome(v changecapture.Outcome) history.Outcome {
	return history.Outcome{
		DeletedDirectories: v.DeletedDirectories,
		Status:             v.Status,
		Exit:               v.Exit,
		Class:              v.Class,
		Labels:             v.Labels,
		Coverage:           v.Coverage,
		CodeMode:           v.CodeMode,
		Scope:              v.Scope,
		ScopeReason:        v.ScopeReason,
		Overlaps:           v.Overlaps,
		Background:         v.Background,
		SharedWith:         v.SharedWith,
	}
}
