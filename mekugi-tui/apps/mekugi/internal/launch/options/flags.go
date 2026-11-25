// Package options owns shared invocation parsing and local configuration.
package options

import (
	"errors"
	"flag"
	"fmt"
	"io"
	"strings"
)

// Source: apps/mekugi/internal/router/flags.go:12:160@[e448711c]

// Config is a parsed invocation configuration. It contains values only;
// execution derives effective output paths without modifying the selected options.
type Config struct {
	ANSIFaint string

	ChildReplySummary     bool
	CaptureOutput         string
	Debug                 bool
	RequireCustomProvider bool
	Profile               string

	childReplySummarySet bool
	debugSet             bool
}

// Parse selects the launch scope without opening resources.
func Parse(args []string) (Config, error) {
	var config Config
	flags := newFlags(io.Discard, &config)
	if err := flags.Parse(args); err != nil {
		return Config{}, fmt.Errorf("parse flags: %w", err)
	}
	flags.Visit(func(item *flag.Flag) {
		switch item.Name {
		case "child-reply-summary":
			config.childReplySummarySet = true
		case "debug":
			config.debugSet = true
		}
	})
	config.RequireCustomProvider = flags.NArg() == 1 && flags.Arg(0) == "custom-provider"

	switch {
	case flags.NArg() != 0 && !config.RequireCustomProvider:
		return Config{}, errors.New("positional arguments are not supported")
	}
	return config, nil
}

func newFlags(stderr io.Writer, config *Config) *flag.FlagSet {
	flags := flag.NewFlagSet("mekugi", flag.ContinueOnError)
	flags.SetOutput(stderr)
	flags.Usage = func() {
		fmt.Fprintln(stderr, "Usage: mekugi [flags] [Codex arguments...]")
		fmt.Fprintln(stderr, "       mekugi [flags] codex [Codex arguments...]")
		fmt.Fprintln(stderr, "       mekugi [flags] codex headless --yolo [-m MODEL] [-c KEY=VALUE] < prompt.txt")
		fmt.Fprintln(stderr, "       mekugi connector [--listen ADDRESS] [--socket PATH] [--token-file PATH] [--origin URL]")
		fmt.Fprintln(stderr, "       mekugi web [--listen ADDRESS]")
		flags.PrintDefaults()
	}
	flags.StringVar(&config.ANSIFaint, "ansi-faint", "auto", "dim text: auto (detect mosh), on (ANSI faint), or off (fixed colors)")
	flags.StringVar(&config.CaptureOutput, "capture-output", "", "optional sanitized capture JSONL path")
	flags.BoolVar(&config.ChildReplySummary, "child-reply-summary", true, "show child-authored summaries in Main with full replies in the dialog")
	flags.BoolVar(&config.Debug, "debug", false, "record diagnostics, capture, metrics, instructions, runtime reads, and AX report; print debug bundle path on exit")
	return flags
}

// SplitCommand parses invocation flags without consuming the following command or its arguments.
func SplitCommand(args []string) (routerArgs, command []string, err error) {
	flags := newFlags(io.Discard, new(Config))
	index := 0
	for index < len(args) {
		arg := args[index]
		if arg == "--" {
			index++
			break
		}
		if len(arg) < 2 || arg[0] != '-' {
			break
		}
		name := strings.TrimLeft(arg, "-")
		name, _, inline := strings.Cut(name, "=")
		item := flags.Lookup(name)
		if item == nil {
			if name == "h" || name == "help" {
				return nil, nil, flags.Parse(args)
			}
			break // Codex options belong to standalone mode.
		}
		index++
		boolean, isBoolean := item.Value.(interface{ IsBoolFlag() bool })
		if !inline && (!isBoolean || !boolean.IsBoolFlag()) && index < len(args) {
			index++
		}
	}
	if err := flags.Parse(args[:index]); err != nil {
		return nil, nil, err
	}
	command = args[index:]
	routerArgs = args[:index]
	return routerArgs, command, err
}

// HasModelOverride distinguishes a user's model selection from an automatic
// launch default, without interpreting option operands or prompt text.
func HasModelOverride(args []string) bool {
	for i := 0; i < len(args); i++ {
		arg := args[i]
		if arg == "--" {
			break
		}
		if arg == "-m" || arg == "--model" || strings.HasPrefix(arg, "--model=") || strings.HasPrefix(arg, "-m") {
			return true
		}
		var value string
		switch {
		case arg == "-c" || arg == "--config":
			if i+1 < len(args) {
				i++
				value = args[i]
			}
		case strings.HasPrefix(arg, "--config="):
			value = strings.TrimPrefix(arg, "--config=")
		case strings.HasPrefix(arg, "-c"):
			value = strings.TrimPrefix(strings.TrimPrefix(arg, "-c"), "=")
		default:
			switch arg {
			case "--enable", "--disable", "-i", "--image", "-p", "--profile", "-s", "--sandbox", "-a", "--ask-for-approval", "-C", "--cd", "--add-dir", "-o", "--output-last-message", "--output-schema", "--color", "--local-provider", "--thread-source":
				i++
			}
		}
		key, _, _ := strings.Cut(value, "=")
		if strings.Trim(strings.TrimSpace(key), `"'`) == "model" {
			return true
		}
	}
	return false
}

// PrintUsage describes the session-only launch interface.
func PrintUsage(w io.Writer) { newFlags(w, new(Config)).Usage() }
