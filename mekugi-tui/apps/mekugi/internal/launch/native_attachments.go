package launch

import (
	"context"
	"errors"
	"image"
	_ "image/gif"
	_ "image/jpeg"
	_ "image/png"
	"io"
	"os"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/sourceanalysis/sourceparse"
)

func snapshotNativeImage(ctx context.Context, path string) (_ string, err error) {
	source, err := sourceparse.Open(path)
	if err != nil {
		return "", &sessionapi.InputError{Message: "The native image source could not be opened."}
	}
	defer source.Close()
	stop := context.AfterFunc(ctx, func() { _ = source.Close() })
	defer stop()
	info, err := source.Stat()
	if err != nil || !info.Mode().IsRegular() {
		return "", &sessionapi.InputError{Message: "A native image source must be a regular file."}
	}
	_, format, err := image.DecodeConfig(source)
	if err != nil {
		return "", &sessionapi.InputError{Message: "The native image source is not a supported image."}
	}
	if _, err := source.Seek(0, io.SeekStart); err != nil {
		return "", err
	}
	file, err := os.CreateTemp("", "mekugi-upload-*."+format)
	if err != nil {
		return "", err
	}
	defer func() {
		if err != nil {
			_ = os.Remove(file.Name())
		}
	}()
	_, err = io.Copy(file, source)
	err = errors.Join(err, file.Close(), ctx.Err())
	if err != nil {
		return "", err
	}
	return file.Name(), nil
}
