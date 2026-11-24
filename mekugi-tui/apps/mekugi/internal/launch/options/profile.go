package options

import (
	"errors"
	"slices"
	"strings"
)

// ProfileSelector removes only profile selectors, retaining operands and --.
func ProfileSelector(args []string) (string, []string, error) {
	var name string
	out := make([]string, 0, len(args))
	for i := 0; i < len(args); i++ {
		arg := args[i]
		if arg == "--" {
			out = append(out, args[i:]...)
			break
		}
		value, selected := "", false
		switch {
		case arg == "-p" || arg == "--profile":
			if i+1 == len(args) {
				return "", nil, errors.New("--profile requires a plain name")
			}
			i++
			value, selected = args[i], true
		case strings.HasPrefix(arg, "--profile="):
			value, selected = strings.TrimPrefix(arg, "--profile="), true
		case strings.HasPrefix(arg, "-p") && len(arg) > 2:
			value, selected = strings.TrimPrefix(strings.TrimPrefix(arg, "-p"), "="), true
		}
		if selected {
			if name != "" {
				return "", nil, errors.New("--profile may be supplied only once")
			}
			if value == "" || strings.ContainsFunc(value, func(r rune) bool {
				return !(r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || r == '_' || r == '-')
			}) {
				return "", nil, errors.New("--profile requires a plain alphanumeric, underscore or hyphen name")
			}
			name = value
			continue
		}
		out = append(out, arg)
		if CodexOptionOperand(arg) && i+1 < len(args) {
			i++
			out = append(out, args[i])
		}
	}
	return name, out, nil
}

// CodexOptionOperand identifies separate option values, never their contents.
func CodexOptionOperand(arg string) bool {
	return slices.Contains([]string{"-c", "--config", "--enable", "--disable", "-i", "--image", "-m", "--model", "-s", "--sandbox", "-a", "--ask-for-approval", "-C", "--cd", "--add-dir", "-o", "--output-last-message", "--output-schema", "--color", "--local-provider", "--thread-source"}, arg)
}

func EnvironmentValue(environment []string, key string) string {
	for _, entry := range slices.Backward(environment) {
		if value, ok := strings.CutPrefix(entry, key+"="); ok {
			return value
		}
	}
	return ""
}
