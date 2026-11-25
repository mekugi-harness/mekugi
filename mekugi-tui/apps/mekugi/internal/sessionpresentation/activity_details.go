package sessionpresentation

import (
	"reflect"
	"slices"

	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/feed"
	activity "github.com/mekugi-harness/mekugi/mekugi-tui/modules/activity/presentation"
)

type DetailID struct{ Attachment, Selection uint64 }

// DetailItem publishes page membership and output availability without a handle.
type DetailItem struct {
	activity.DetailPage
	Process              *Process `json:",omitzero"`
	OutputVersion        uint64
	Retained, OutputLive bool
}

type DetailView struct {
	Revision  uint64
	ID        DetailID
	Pages     []DetailItem
	Processes bool
}

type DetailOutputRequest struct {
	ID      DetailID
	Page    string
	Version uint64
}

type DetailOutput struct {
	DetailOutputRequest
	View feed.OutputView
}

type RunOutputRange struct {
	First, Last int
	Available   bool
}

type detailPage struct {
	activity.DetailPage
}

// Details owns one renderer attachment's static detail selection.
type Details struct {
	id                 DetailID
	sequence, revision uint64
	pages              []detailPage
}

// OpenDetails stores source-free static pages.
func (d *Details) OpenDetails(version Version, pages []activity.DetailPage) DetailID {
	d.sequence++
	d.id = DetailID{version.Attachment, d.sequence}
	d.pages = nil
	if _, ok := d.refreshDetails(d.id, pages); !ok {
		d.CloseDetails(d.id)
	}
	return d.id
}

func (d *Details) CloseDetails(id DetailID) {
	if d.id == id {
		d.id, d.pages = DetailID{}, nil
	}
}

func (d *Details) refreshDetails(id DetailID, requested []activity.DetailPage) (uint64, bool) {
	if d.id != id || d.id.Selection == 0 {
		return 0, false
	}
	previous := make(map[string]detailPage, len(d.pages))
	var details []activity.DetailPage
	for _, page := range d.pages {
		previous[page.ID] = page
		details = append(details, page.DetailPage)
	}
	if requested != nil {
		details = requested
	}
	if slices.ContainsFunc(details, func(page activity.DetailPage) bool {
		return page.Source != 0 || page.PreparedSource != 0 || page.Output != 0
	}) {
		return 0, false
	}
	details = activity.RefreshDetails(details, nil)

	pages := make([]detailPage, len(details))
	for i, detail := range details {
		prior := previous[detail.ID]
		detail.Revision = prior.Revision
		if !reflect.DeepEqual(detail, prior.DetailPage) {
			d.revision++
			detail.Revision = d.revision
			detail.Value = activity.CloneValues([]activity.Value{detail.Value})[0]
		}
		pages[i] = detailPage{detail}
	}
	if !reflect.DeepEqual(d.pages, pages) {
		d.revision++
		d.pages = pages
	}
	return d.revision, true
}

func (d *Details) DetailView(id DetailID) (DetailView, bool) {
	if d.id != id || d.id.Selection == 0 {
		return DetailView{}, false
	}
	view := DetailView{Revision: d.revision, ID: d.id, Pages: make([]DetailItem, len(d.pages))}
	for i, page := range d.pages {
		item := DetailItem{DetailPage: page.DetailPage}
		if page.Prepared != nil {
			content := *page.Prepared
			content.Calls = slices.Clone(content.Calls)
			item.Prepared = &content
		}
		item.Value = activity.CloneValues([]activity.Value{item.Value})[0]
		view.Pages[i] = item
	}
	return view, true
}
