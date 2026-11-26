package scope

// Store owns memory for one application scope. Its zero value is ready for use.
// The application serializes access and replaces the store when the scope ends.
// Do not copy a store after use.
type Store struct{}
