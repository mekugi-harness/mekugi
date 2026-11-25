package sessioncontrol

// InterruptTarget captures the host work observed by a consumer. Start keeps an
// admitted start identifiable through acknowledgement, commitment and turn start.
type InterruptTarget struct {
	Thread, Turn, Start string
}

type InterruptMode uint8

const (
	RestoreWaiting InterruptMode = iota
	ExpediteSteers
)
