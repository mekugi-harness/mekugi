package terminalui

// SGR mouse reports can span reads. Consume the whole report, including ignored
// clicks and malformed payloads, so its bytes never become review commands.
type Mouse struct {
	Active  bool
	Payload string
}
