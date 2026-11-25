package sessionpresentation

const SideCaller = "/oneoff"

type SideToken struct {
	ID    uint64
	Key   string
	Label string
}

type CloseSideIntent struct {
	Version  Version
	Key      string
	Exchange uint64
}
