package launch

import (
	json "encoding/json/v2"
	"strings"
	"time"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/composer"

	noticepresentation "github.com/mekugi-harness/mekugi/mekugi-tui/modules/notifications/presentation"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessioncontrol"

	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/appserver"
)

func (session *nativeSession) titleCommand(attached *attachedPresentation, picker *composerPicker, editor **composer.Editor, clock func() time.Time, side **sessioncontrol.Side, overlay bool, text string) bool {
	if fields := strings.Fields(text); len(fields) == 0 || fields[0] != "/title" {
		return false
	}
	name := strings.TrimSpace(strings.TrimPrefix(text, "/title"))
	if name == "" {
		setNotice(&attached.feedback, presentationTime(clock), "Usage: /title <title>", false)
		return true
	}
	if session.Client.Replacement.Pending() {
		setNotice(&attached.feedback, presentationTime(clock), "Wait for the session to be ready", false)
		return true
	}
	session.deleteDraftRange(attached, picker, editor, (*side), clock, overlay, 0, len((*editor).Text))
	setNotice(&attached.feedback, presentationTime(clock), "", false)
	renameSessionTitle(&session.Client, &attached.feedback, clock, name)
	return true
}

func renameSessionTitle(control *sessioncontrol.Client[appServerRequest], feedback *noticepresentation.Model, clock func() time.Time, name string) {
	control.Names.Manual(control.Thread, name)
	if control.Thread == "" {
		return
	}
	requestSessionTitle(control, feedback, clock, control.Names.Next(control.Thread))
}

func requestSessionTitle(control *sessioncontrol.Client[appServerRequest], feedback *noticepresentation.Model, clock func() time.Time, save *sessioncontrol.NameSave) {
	if save == nil {
		return
	}
	_, err := control.Requests.Send("thread/name/set", map[string]any{"threadId": save.Thread, "name": save.Name}, "", appServerRequest{title: save})
	if err != nil {
		control.Names.Complete(*save)
		setNotice(feedback, presentationTime(clock), "Session title could not be saved", false)
	}
}

func sessionTitleMessage(control *sessioncontrol.Client[appServerRequest], feedback *noticepresentation.Model, clock func() time.Time, title *string, m appserver.Message, update *sessioncontrol.NameSave) bool {
	if update != nil {
		latest := control.Names.Complete(*update)
		if m.Error != nil {
			if update.Thread == control.Thread && latest {
				setNotice(feedback, presentationTime(clock), "Session title could not be saved: "+m.Error.Message, false)
			}
		} else {
			confirmSessionTitle(control.Thread, title, update.Thread, update.Name)
		}
		requestSessionTitle(control, feedback, clock, control.Names.Next(update.Thread))
		return true
	}
	if m.Method != "thread/name/updated" {
		return false
	}
	var params struct {
		ThreadID   string `json:"threadId"`
		ThreadName string `json:"threadName"`
	}
	if json.Unmarshal(m.Params, &params) == nil {
		confirmSessionTitle(control.Thread, title, params.ThreadID, params.ThreadName)
	}
	return true
}

func confirmSessionTitle(main string, title *string, thread, name string) {
	if thread != "" && thread == main {
		*title = name
	}
}
