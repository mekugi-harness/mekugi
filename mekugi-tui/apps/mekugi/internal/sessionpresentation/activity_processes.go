package sessionpresentation

// Process identifies the actual host invocation, not a row whose retention or
// rendering lifetime can outlast the command.
type Process struct {
	Thread, Turn, Item, ID, Agent, Command string
}
