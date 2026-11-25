package presets

type Stage string

const (
	Quick   Stage = "presets"
	Edit    Stage = "preset-edit"
	Label   Stage = "preset-label"
	Slots   Stage = "preset-slots"
	Confirm Stage = "preset-confirm"
)

// Flow is view-local. Storage results are polled by the application, never a renderer.
type Flow struct {
	Stage                                 Stage
	Slots                                 [10]Slot
	Input, Label, Seed, Settings, Problem string
	Slot                                  int
	Mode                                  string
	Pending, Loaded                       bool
}
