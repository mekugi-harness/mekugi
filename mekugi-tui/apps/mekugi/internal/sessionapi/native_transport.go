package sessionapi

import "fmt"

type NativeExit struct {
	Code    int    `json:"code"`
	Message string `json:"message,omitempty"`
}

func (e *NativeExit) Error() string {
	if e.Message != "" {
		return e.Message
	}
	return fmt.Sprintf("session exited with status %d", e.Code)
}
