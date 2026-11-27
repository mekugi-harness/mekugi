# Terminal snapshot procedures

Terminal appearance changes require actual-renderer `TestUISnapshot` fixtures through
`testsupport/uisnapshot` in the owner's `testdata/snapshots/`. Fix time, theme, dimensions, and other
nondeterministic inputs. Run commands from the repository root using the
[build and test](development.md#build-and-test) runner (which also documents the staged-tree snapshot commit gate).

```sh
just test-ui-snapshots ./mekugi-tui/apps/mekugi/internal/launch \
  '^TestUISnapshotNativeSessionShortcuts$'
```

Checks are offline, include the snapshot harness, and need neither Codex nor model requests.
Both `just test-ui-snapshots` and `just update-ui-snapshots` accept a package
selector followed by an optional test name pattern. The default selector,
`@snapshots`, discovers packages declaring `TestUISnapshot` or harness
`TestSnapshot` tests, including platform-tagged files. Go selects which can run.
An explicit package path bypasses discovery; `@workspace` compiles every member
package. Omit the pattern to select all UI snapshot cases in those packages.

An unfiltered `just test-ui-snapshots` also fails if a checked-in baseline
was not asserted by any test. A package or test-name selection does not run
the repository-wide orphan check.

- `testsupport/uisnapshot.Assert` strips ANSI only: spacing, blank lines, wrapping and borders
  remain exact. Text comparisons do not prove color/style correctness.
- `AssertTerminal` paints into a fixed-width VT emulator and retains canonical ANSI as quoted rows,
  covering colors, emphasis, links, padding and style restoration independently of equivalent escape
  sequences. Include a following plain row to detect style leaks. Theme-sensitive cases cover
  terminal/dark/light; code fills also cover a reported background.
- A missing/changed fixture fails and writes a sibling `.txt.new` candidate plus a unified diff;
  the baseline stays unchanged. Review the candidate against the requested behavior and the
  owner's design guide, as its user would see it. Accept it only for an intended change by moving
  it over the fixture or running the selected update recipe; a difference outside the intended
  change is a regression to fix in the code. Omit the pattern to update all
  UI snapshot cases in the selected packages.
- Updates require `MEKUGI_UPDATE_UI_SNAPSHOTS=1`; check recipes clear it. A pass or explicit update
  removes stale candidates. Rerun checks after accepting a baseline.
- Keep copy/state/bounds/interaction/PTY checks separate. Git attributes exempt only snapshot text
  from trailing-space/final-blank-row warnings; ordinary sources still use `git diff --check`.
