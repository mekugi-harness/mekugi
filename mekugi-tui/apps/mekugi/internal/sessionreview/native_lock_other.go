//go:build !unix

package sessionreview

func lockNativeStoreShared(string) (func(), error) {
	return func() {}, nil
}
