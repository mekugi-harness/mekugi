package executionobservation

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"errors"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/requestexecution"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/modelprotocol/responses"

	codexresponses "github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/responses"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/sessionhistory/history"
)

// Native reviewers borrow only immutable compaction context. They do not publish
// a retained scope, acquire reload sources, or enter execution observation.
func (p *Preparer) restoreApprovalReviewContext(ctx context.Context, request *codexresponses.Request, input requestexecution.Preparation) error {
	raw := bytes.TrimSpace(request.Fields["input"])
	if len(raw) == 0 || raw[0] != '[' {
		return nil
	}
	var items []jsontext.Value
	if err := json.Unmarshal(raw, &items); err != nil {
		return err
	}
	var release func()
	defer func() {
		if release != nil {
			release()
		}
	}()
	for index, rawItem := range items {
		var item struct {
			Type      string `json:"type"`
			Reference string `json:"encrypted_content"`
		}
		if err := json.Unmarshal(rawItem, &item); err != nil {
			return err
		}
		if (item.Type != "compaction" && item.Type != "compaction_summary") || !strings.HasPrefix(item.Reference, history.CompactSummaryPrefix) {
			continue
		}
		if release == nil {
			metadata := input.Metadata
			if p == nil || p.Retained.Store == nil || !input.MetadataValid || input.Thread == "" || metadata.ActivityIdentityInvalid || metadata.ThreadID != "" && metadata.ThreadID != input.Thread {
				return errors.New("approval review compaction recovery requires admitted identity and storage")
			}
			store := p.Retained.Store
			scope, exists, err := store.ReadScope(input.Thread)
			if err != nil {
				return err
			}
			if exists && (metadata.ParentThreadID != "" && metadata.ParentThreadID != scope.Parent || metadata.ForkedFromThreadID != "" && metadata.ForkedFromThreadID != scope.Fork) {
				return errors.New("approval review retained scope conflicts with host ancestry")
			}
			namespace, err := store.NamespaceForThread(input.Thread)
			if err != nil {
				return err
			}
			ctx = context.WithValue(ctx, storageSessionKey{}, storageSessionIdentity{Thread: input.Thread, Namespace: namespace})
			// Existing bindings and frozen allocation ranges remain authoritative.
			// Only an unbound reviewer borrows prospective host ancestry.
			if !exists {
				// Internal reviewers can carry a parent without a subagent kind.
				if metadata.ParentThreadID != "" && metadata.SubagentKind == "" {
					metadata.SubagentKind = "guardian"
				}
				ctx, err = prepareHandleScope(ctx, store, nil, metadata)
				if err != nil {
					return err
				}
			}
			release, err = store.SnapshotLease(ctx)
			if err != nil {
				return err
			}
		}
		record, err := p.readCompact(ctx, item.Reference)
		if err != nil {
			return err
		}
		items[index] = responses.MustMarshalJSON(map[string]any{"type": "message", "role": "assistant", "content": []map[string]string{{"type": "output_text", "text": record.Summary}}})
	}
	if release != nil {
		projected, err := json.Marshal(items)
		if err != nil {
			return err
		}
		request.Fields["input"] = projected
	}
	return nil
}

// Read under reconcileVisibleInput's snapshot lease. Prospective inheritance
// uses authenticated host ancestry only until its frozen scope is published.
func (p *Preparer) readCompact(ctx context.Context, reference string) (history.CompactRecord, error) {
	identity, _ := ctx.Value(storageSessionKey{}).(storageSessionIdentity)
	namespace := identity.Namespace
	store := p.Retained.Store
	if store == nil || identity.Thread == "" {
		return history.CompactRecord{}, errors.New("compaction recovery has no admitted thread")
	}
	_, exists, err := store.ReadScope(identity.Thread)
	if err != nil {
		return history.CompactRecord{}, err
	}
	if !exists && identity.Fork != "" {
		namespace, err = store.NamespaceForThread(identity.Fork)
		if err != nil {
			return history.CompactRecord{}, err
		}
	}
	return history.ReadCompactRecord(store, namespace, reference)
}

// Prewarm projects immutable history without publishing a scope or running readers.
func (p *Preparer) prepareCompactInput(ctx context.Context, request *codexresponses.Request, session, thread string, metadata codexresponses.TurnMetadata, prewarm bool) error {
	if p == nil || p.Retained.Store == nil || thread == "" || metadata.ActivityIdentityInvalid || metadata.ThreadID != "" && metadata.ThreadID != thread {
		return errors.New("compaction recovery identity or storage is unavailable")
	}
	namespace, err := p.Retained.Store.NamespaceForThread(thread)
	if err != nil {
		return err
	}
	ctx = context.WithValue(ctx, storageSessionKey{}, storageSessionIdentity{Thread: thread, Namespace: namespace})
	ctx, err = prepareHandleScope(ctx, p.Retained.Store, p.Retained.Writes, metadata)
	if err != nil {
		return err
	}
	release, err := p.Retained.Store.SnapshotLease(ctx)
	if err != nil {
		return err
	}
	defer release()
	var items []map[string]jsontext.Value
	if err := json.Unmarshal(request.Fields["input"], &items); err != nil {
		return err
	}
	var contextReference string
	for _, item := range items {
		kind, reference := responses.StringField(item, "type"), responses.StringField(item, "encrypted_content")
		if (kind != "compaction" && kind != "compaction_summary") || !strings.HasPrefix(reference, history.CompactSummaryPrefix) {
			continue
		}
		record, err := p.readCompact(ctx, reference)
		if err != nil {
			return err
		}
		contextReference = record.Reference
		if prewarm {
			for key := range item {
				delete(item, key)
			}
			item["type"], item["role"] = responses.MustMarshalJSON("message"), responses.MustMarshalJSON("assistant")
			item["content"] = responses.MustMarshalJSON([]map[string]string{{"type": "output_text", "text": record.Summary}})
		}
	}
	if prewarm {
		request.Fields["input"], err = json.Marshal(items)
		return err
	}
	release()
	ctx, release, err = beginRetainedSession(ctx, p.Retained.Store, p.Retained.Writes, p.Retained.Limits, thread, session)
	if err != nil {
		return err
	}
	defer release()
	ctx, err = prepareHandleScope(ctx, p.Retained.Store, p.Retained.Writes, metadata)
	if err != nil {
		return err
	}
	workspace, valid := usableRoutingDirectory(metadata.Directories)
	if len(metadata.Directories) != 0 && !valid {
		return errors.New("invalid compaction recovery workspace")
	}
	_, err = p.reconcileVisibleInput(ctx, request, workspace, workspace+"\x00"+thread)
	if err == nil && metadata.RequestKind == responses.Turn && metadata.ContextWindowID == "" {
		p.Control.ObserveCompactionContext(thread, contextReference)
	}
	return err
}
