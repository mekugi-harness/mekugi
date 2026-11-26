package sessionapi

import (
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionpresentation"
	activitypresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
	changepresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/prompt"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/livepreview/preview"
)

// Frame is one detached renderer input. Launch builds it from admitted owners;
// the server derives snapshots and patches from consecutive frames.
type Frame struct {
	Session    Session
	Playback   *Playback         // Absent for a live attachment.
	Input      *Input            // Absent for a replay.
	Prompts    *Prompts          // Absent for a replay.
	Questions  []prompt.CallView // Detached live questions, including native-only metadata.
	Attachment uint64            // A change restarts entry sequences.
	Status     sessionpresentation.Label
	FamilyRoot string                            // Admitted logical family, independent of runtime lifetime.
	Members    []sessionpresentation.Participant // Admitted coordination facts, never transcript history.
	Main       *activitypresentation.Publication
	// Agents is the child feed: each agent's own activity, and the roster.
	Agents    *activitypresentation.Publication
	Review    *changepresentation.ReviewView
	Live      []preview.Preview
	LiveScope preview.Scope
	// ChildReplySummary is the invocation's child_reply_summary feature.
	ChildReplySummary bool
}

type Session struct {
	Thread    string `json:"thread"`
	Caller    string `json:"caller,omitzero"`
	Role      string `json:"role,omitzero"`
	Title     string `json:"title,omitzero"`
	Workspace string `json:"workspace,omitzero"`
	Started   int64  `json:"started,omitzero"`
}

// Input is what a live session's input can do now.
type Input struct {
	Reason    string `json:"reason,omitzero"` // Why Main cannot take input now.
	Stoppable bool   `json:"stoppable,omitzero"`
	Waiting   int    `json:"waiting,omitzero"` // Questions and approvals Main waits on.
}

type Prompts struct {
	Questions []QuestionCall `json:"questions"`
	Approvals []Approval     `json:"approvals"`
}

type QuestionCall struct {
	Blocking  bool       `json:"blocking"`
	ID        string     `json:"id"`
	Agent     string     `json:"agent,omitzero"`
	Questions []Question `json:"questions"`
}

type Question struct {
	ID      string   `json:"id"`
	Header  string   `json:"header,omitzero"`
	Text    string   `json:"text,omitzero"`
	Options []Option `json:"options,omitempty"`
	Secret  bool     `json:"secret,omitzero"`
}

type Option struct {
	Preview     string `json:"preview,omitzero"`
	Recommended bool   `json:"recommended,omitzero"`
	Label       string `json:"label"`
	Description string `json:"description,omitzero"`
}

type Approval struct {
	ID      string   `json:"id"`
	Agent   string   `json:"agent,omitzero"`
	Title   string   `json:"title"`
	Subject string   `json:"subject,omitzero"`
	Details []string `json:"details"`
	Choices []Choice `json:"choices"`
}

type Choice struct {
	ID    int    `json:"id"`
	Label string `json:"label"`
	Deny  bool   `json:"deny,omitzero"`
}

// Summary is a session's entry in the session list.
type Summary struct {
	Replay    bool
	Thread    string
	Title     string
	Workspace string
	Started   time.Time
	Status    sessionpresentation.Label
	Ready     bool // Main can accept the initial hosted message. Private to the hub.
	Waiting   int  // Questions and approvals Main waits on.
}

type Playback struct {
	Position  int64   `json:"position"` // Recorded milliseconds.
	Until     int64   `json:"until"`
	Speed     float64 `json:"speed"`
	Paused    bool    `json:"paused,omitzero"`
	Simulated bool    `json:"simulated,omitzero"`
}

type status struct {
	Text      string `json:"text,omitzero"`
	Failure   string `json:"failure,omitzero"`
	Alert     bool   `json:"alert,omitzero"`
	Running   bool   `json:"running,omitzero"`
	Completed *int64 `json:"completed,omitzero"`
}

type entry struct {
	Traffic        *activitypresentation.Traffic       `json:"traffic,omitzero"`
	Identities     []sessionpresentation.AgentIdentity `json:"identities,omitzero"`
	Position       int                                 `json:"position"`
	Turn           uint64                              `json:"turn,omitzero"`
	Revision       uint64                              `json:"revision,omitzero"`
	Content        *ContentReference                   `json:"content,omitzero"`
	AutoContent    bool                                `json:"autoContent,omitzero"`
	Spans          []activitypresentation.TextSpan     `json:"spans,omitzero"`
	Images         []string                            `json:"images,omitzero"`
	Seq            uint64                              `json:"seq"`
	Agent          string                              `json:"agent,omitzero"`
	Kind           string                              `json:"kind,omitzero"`
	Outcome        string                              `json:"outcome,omitzero"`
	Text           string                              `json:"text,omitzero"`
	Observed       int64                               `json:"observed,omitzero"`
	Running        bool                                `json:"running,omitzero"`
	Assignment     bool                                `json:"assignment,omitzero"`
	AssignmentTo   string                              `json:"assignmentTo,omitzero"`
	AssignmentText string                              `json:"assignmentText,omitzero"`
	Values         []value                             `json:"values,omitzero"`
}

type value struct {
	Params           []value                          `json:"params,omitzero"`
	Failed           bool                             `json:"failed,omitzero"`
	Kind             string                           `json:"kind,omitzero"`
	Verb             string                           `json:"verb,omitzero"`
	BatchExit        bool                             `json:"batchExit,omitzero"` // A compound command's combined result.
	Label            string                           `json:"label,omitzero"`
	Sources          []string                         `json:"sources,omitzero"` // Programs that made a file edit, outside its label.
	Path             string                           `json:"path,omitzero"`
	AutoReload       bool                             `json:"autoReload,omitzero"`
	Approval         string                           `json:"approval,omitzero"`
	Code             string                           `json:"code,omitzero"`
	CodePreview      string                           `json:"codePreview,omitzero"`
	DetailCode       string                           `json:"detailCode,omitzero"`
	DetailsAvailable *activitypresentation.Disclosure `json:"detailsAvailable,omitzero"`
	Lang             string                           `json:"lang,omitzero"`
	Fenced           bool                             `json:"fenced,omitzero"`
	Body             string                           `json:"body,omitzero"`
	Details          string                           `json:"details,omitzero"` // Supporting child reply content without section delimiters.
	Summary          string                           `json:"summary,omitzero"` // A child reply's own summary of Body.
	Reads            []read                           `json:"reads,omitzero"`
	Subsumed         bool                             `json:"subsumed,omitzero"`
	Effect           bool                             `json:"effect,omitzero"`
	Pending          bool                             `json:"pending,omitzero"`
	Skipped          bool                             `json:"skipped,omitzero"`
	HiddenEdits      int                              `json:"hiddenEdits,omitzero"` // Count of retained rows folded from this index.
	CodeLines        int                              `json:"codeLines,omitzero"`
	TailLines        int                              `json:"tailLines,omitzero"`
	FoldSource       bool                             `json:"foldSource,omitzero"`
	Duration         int64                            `json:"duration,omitzero"`
	Thought          int64                            `json:"thought,omitzero"`
	Workdir          string                           `json:"workdir,omitzero"`
	From             string                           `json:"from,omitzero"` // Caller paths of a message's ends.
	To               string                           `json:"to,omitzero"`
	Questions        []question                       `json:"questions,omitzero"`
	Waits            []wait                           `json:"waits,omitzero"`
	WaitReason       string                           `json:"waitReason,omitzero"`
	WaitTimeout      string                           `json:"waitTimeout,omitzero"`
	ExitCode         *int                             `json:"exitCode,omitzero"`
	EditOutcome      string                           `json:"editOutcome,omitzero"`
	Running          bool                             `json:"running,omitzero"`
	Live             bool                             `json:"live,omitzero"`
	Tail             []string                         `json:"tail,omitzero"`
	TailOmitted      int                              `json:"tailOmitted,omitzero"`
	TailClipped      bool                             `json:"tailClipped,omitzero"`
	SourceOmitted    bool                             `json:"sourceOmitted,omitzero"`
	Changes          []change                         `json:"changes,omitzero"`
	Hook             *hook                            `json:"hook,omitzero"`
}

type read struct {
	Path   string   `json:"path"`
	Ranges []string `json:"ranges,omitzero"`
}

type question struct {
	Text   string `json:"text,omitzero"`
	Answer string `json:"answer,omitzero"`
}

type wait struct {
	Name   string `json:"name"`
	Status string `json:"status,omitzero"`
}

type change struct {
	Verb    string `json:"verb,omitzero"`
	Code    string `json:"code,omitzero"`
	Label   string `json:"label,omitzero"`
	Added   int    `json:"added,omitzero"`
	Removed int    `json:"removed,omitzero"`
	Note    string `json:"note,omitzero"`
	Footer  bool   `json:"footer,omitzero"`
}

type hook struct {
	Status   string `json:"status,omitzero"`
	HasError bool   `json:"hasError,omitzero"`
}

type roster struct {
	Next    *PageCursor `json:"next,omitzero"`
	Total   int         `json:"total"`
	Agents  []agent     `json:"agents"`
	Added   int         `json:"added,omitzero"` // Net workspace lines, when known.
	Removed int         `json:"removed,omitzero"`
}

type agent struct {
	sessionpresentation.AgentIdentity
	Identities    []sessionpresentation.AgentIdentity `json:"identities,omitzero"`
	Content       *ContentReference                   `json:"content,omitzero"`
	Thread        string                              `json:"thread,omitzero"` // Runtime identity; admission remains server-owned.
	NonAttachable bool                                `json:"nonAttachable,omitzero"`
	Role          string                              `json:"role,omitzero"`
	Depth         int                                 `json:"depth,omitzero"`
	State         string                              `json:"state"`
	Wait          *value                              `json:"wait,omitzero"`
	Activity      *entry                              `json:"activity,omitzero"` // Shared roster summary, without retained source/output.
	Turns         uint64                              `json:"turns,omitzero"`
	Roundtrips    uint64                              `json:"roundtrips,omitzero"`
	Work          *int64                              `json:"work,omitzero"`
	WorkSince     int64                               `json:"workSince,omitzero"`
	LastResponse  int64                               `json:"lastResponse,omitzero"`
	Cost          *float64                            `json:"cost,omitzero"`
	CostPartial   bool                                `json:"costPartial,omitzero"`
	InputTokens   *uint64                             `json:"inputTokens,omitzero"`
	OutputTokens  *uint64                             `json:"outputTokens,omitzero"`
	UsagePartial  bool                                `json:"usagePartial,omitzero"`
	Context       *uint64                             `json:"context,omitzero"`
	Window        uint64                              `json:"window,omitzero"`
	Added         *int                                `json:"added,omitzero"`
	Removed       *int                                `json:"removed,omitzero"`
	Skills        []string                            `json:"skills,omitzero"`
	Items         []string                            `json:"items,omitzero"`
}

// projectEntry carries child reply summaries only when summaries is set.

// projectRoster follows the terminal roster's facts: display names, the
// lifecycle state, work timers, usage and the lines each agent edited.
