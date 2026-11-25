package presentation

// CommitKey identifies only a commit named by observed output, in its proven directory.
type CommitKey struct{ Directory, Hash string }

// CommitReader owns bounded immutable-object reads for one attached Activity session.
// Request never waits for Git. Close cancels outstanding reads without waiting for them.
type CommitReader struct {
}
