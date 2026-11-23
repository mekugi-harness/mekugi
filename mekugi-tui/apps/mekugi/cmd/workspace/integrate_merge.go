package main

import (
	"errors"
	"fmt"
	"os"
	"path"
	"path/filepath"
	"slices"
	"strings"
)

func integrationIdle(root string) error {
	args := []string{"rev-parse", "--path-format=absolute"}
	for _, entry := range []string{"MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD", "BISECT_START", "rebase-merge", "rebase-apply", "sequencer"} {
		args = append(args, "--git-path", entry)
	}
	paths, err := integrationGit(root, args...)
	if err != nil {
		return err
	}
	for path := range strings.SplitSeq(paths, "\n") {
		if _, err := os.Lstat(path); err == nil {
			return fmt.Errorf("Git operation in progress in %s (%s); resolve it before integration", root, filepath.Base(path))
		} else if !errors.Is(err, os.ErrNotExist) {
			return err
		}
	}
	return nil
}

// Some merge strategies overwrite ignored files even with --no-overwrite-ignore.
// Compare path components, including file/directory collisions, before either merge.
func protectIntegrationLocalFiles(root, incoming string) error {
	local, err := integrationGitOutput(root, "ls-files", "--others", "--directory", "--no-empty-directory", "-z")
	if err != nil || local == "" {
		return err
	}
	tree, err := integrationGitOutput(root, "ls-tree", "-r", "--name-only", "-z", incoming)
	if err != nil {
		return err
	}
	paths := strings.Split(strings.TrimSuffix(local, "\x00"), "\x00")
	for i, name := range paths {
		paths[i] = strings.TrimSuffix(name, "/")
	}
	slices.Sort(paths)
	for name := range strings.SplitSeq(strings.TrimSuffix(tree, "\x00"), "\x00") {
		if name == "" {
			continue
		}
		if i, _ := slices.BinarySearch(paths, name+"/"); i < len(paths) && strings.HasPrefix(paths[i], name+"/") {
			return fmt.Errorf("incoming path %q overlaps local untracked path %q in %s", name, paths[i], root)
		}
		for candidate := name; candidate != "."; candidate = path.Dir(candidate) {
			if _, found := slices.BinarySearch(paths, candidate); found {
				return fmt.Errorf("incoming path %q overlaps local untracked path %q in %s", name, candidate, root)
			}
		}
	}
	return nil
}

func mergeIntegration(root, ref string) error {
	if err := integrationIdle(root); err != nil {
		return err
	}
	incoming, err := integrationGit(root, "rev-parse", "--verify", "--end-of-options", ref+"^{commit}")
	if err != nil {
		return err
	}
	if err := protectIntegrationLocalFiles(root, incoming); err != nil {
		return err
	}
	return integrationCommand(root, "git", "merge", "--no-ff", "--no-edit", "--no-squash",
		"--no-autostash", "--no-overwrite-ignore", "--no-rerere-autoupdate", "--", incoming)
}
