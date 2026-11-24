package accounting

import (
	"github.com/mekugi-harness/mekugi/mekugi-tui/contracts/inference"
)

// RoundOutput decodes the last measured sample in historical usage records.
type RoundOutput struct {
	StartedUnixNano int64
	Throughput      inference.OutputThroughput
}
