package execscope

import (
	"path/filepath"
	"strings"

	"mvdan.cc/sh/v3/expand"
	"mvdan.cc/sh/v3/syntax"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/commandanalysis/shellsyntax"
)

// SessionScratchRedirects resolves direct output redirects against the scratch
// binding witnessed at tool entry. Stop before a shell construct that might
// change that binding rather than guessing its value later in the script.
func SessionScratchRedirects(command, shell, root string) []string {
	if !filepath.IsAbs(root) || !strings.Contains(command, "SESSION_TMP") {
		return nil
	}
	switch shellsyntax.InterpreterIdentity(shell) {
	case "bash", "sh":
	default:
		return nil
	}
	program, err := syntax.NewParser(syntax.Variant(syntax.LangBash)).Parse(strings.NewReader(command), "")
	if err != nil {
		return nil
	}
	var paths []string
	for _, stmt := range program.Stmts {
		call, ok := stmt.Cmd.(*syntax.CallExpr)
		if !ok || len(call.Assigns) != 0 || len(call.Args) == 0 {
			break
		}
		stable := true
		syntax.Walk(stmt, func(node syntax.Node) bool {
			switch value := node.(type) {
			case *syntax.ArithmExp:
				stable = false
			case *syntax.ParamExp:
				if value.Exp != nil && (value.Exp.Op == syntax.AssignUnset || value.Exp.Op == syntax.AssignUnsetOrNull) {
					stable = false
				}
			}
			return stable
		})
		if !stable {
			break
		}
		for _, redirect := range stmt.Redirs {
			switch redirect.Op {
			case syntax.RdrOut, syntax.AppOut, syntax.RdrClob, syntax.RdrAll, syntax.AppAll, syntax.RdrInOut:
				if path := sessionScratchOperand(redirect.Word, root); path != "" {
					paths = append(paths, path)
				}
			}
		}
		name, _ := shellsyntax.Literal(call.Args[0])
		switch name {
		case "cat", "mkdir", "tee", "touch", "echo", "set", "cd", "true", "false", ":":
			// These do not assign a shell variable in the parent shell.
		default:
			return paths
		}
	}
	return paths
}

func sessionScratchOperand(word *syntax.Word, root string) string {
	if word == nil {
		return ""
	}
	found, valid := false, true
	syntax.Walk(word, func(node syntax.Node) bool {
		switch node := node.(type) {
		case *syntax.Word, *syntax.DblQuoted, *syntax.SglQuoted, *syntax.Lit, nil:
		case *syntax.ParamExp:
			valid = valid && node.Param.Value == "SESSION_TMP" && !node.Excl && !node.Length && !node.Width &&
				node.Index == nil && node.Slice == nil && node.Repl == nil && node.Names == 0 && node.Exp == nil
			found = valid
			return false
		default:
			valid = false
		}
		return valid
	})
	if !found || !valid {
		return ""
	}
	// Fields respects quoting, field splitting and escapes. Never perform
	// globbing (no ReadDir) or run a substitution to find a target.
	fields, err := expand.Fields(&expand.Config{Env: expand.ListEnviron("SESSION_TMP=" + root)}, word)
	if err != nil || len(fields) != 1 || strings.ContainsAny(fields[0], "*?[") {
		return ""
	}
	path := filepath.Clean(fields[0])
	if relative, err := filepath.Rel(root, path); err == nil && relative != "." &&
		relative != ".." && !strings.HasPrefix(relative, ".."+string(filepath.Separator)) {
		return path
	}
	return ""
}
