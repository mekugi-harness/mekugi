package sessioncontrol

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/observation"
)

// NativeDelivery is a passive, identity-validated native response item. It has
// no dispatch settings, wait claims or work admission semantics.
type NativeDelivery struct {
	Thread, ID, Sender, Recipient, Kind, Message string
	Outcome                                      string
}

// NativeMessage covers the native agent_message item and tagged contextual
// message form. Provider-authored text without native provenance is not traffic.
type NativeMessage observation.NativeMessage
