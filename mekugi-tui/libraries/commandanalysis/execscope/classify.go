// Package execscope derives bounded write scopes without executing the user's program.
package execscope

import "time"

const ListingLimit = 4096

// Input carries the execution metadata selected by the request.
type Input struct {
	Command      string
	Directory    string
	Shell        string
	Deadline     time.Time
	Changes      ChangeResolver
	AuthoredOnly bool
}
