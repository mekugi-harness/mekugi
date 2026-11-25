package sessionpresentation

type ApprovalIntent struct {
	Version Version
	Request string
	Choice  int
	Reason  string
}
