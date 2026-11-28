package sessionapi

const (
	inputLimit     = 1 << 20
	UploadLimit    = 32 << 20
	InputBodyLimit = (UploadLimit+2)/3*4 + inputLimit
)

// InputError is a permanent preparation error, not a retryable admission refusal.
type InputError struct {
	Message string
	Limit   bool
}

func (e *InputError) Error() string { return e.Message }
