package terminalview

import (
	"strconv"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/diffmodel"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/changehistory/presentation"
)

func liveDiffHighlightChunk(key, path, hunk string) presentation.Chunk {
	diff := "--- " + strconv.Quote(path) + "\n+++ " + strconv.Quote(path) + "\n" + hunk
	return presentation.Chunk{Key: key, Review: diffmodel.ReviewFile{BeforePath: path, AfterPath: path, Diff: diff}}
}
