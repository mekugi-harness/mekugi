package shellsyntax

import (
	"path/filepath"
	"strings"

	"mvdan.cc/sh/v3/syntax"
)

// StripLeadingCd recognizes a safe leading literal cd <dir> && <rest>,
// cd <dir>; <rest>, or newline-separated cd <dir>\n<rest> command, resolves
// the target directory against directory, and returns the remaining command
// text with the leading cd omitted.
//
// ||, subshells wrapping cd, dynamic targets, cd -, bare cd, options other
// than --, redirects, inline env assignments, shebangs, and commands that
// change directory again later in the remaining statements are left intact.
func StripLeadingCd(script, directory string) (rest, target string, ok bool) {
	operand, rest, ok := parseLeadingCd(script)
	if !ok {
		return "", "", false
	}
	target = operand
	if !filepath.IsAbs(target) {
		if !filepath.IsAbs(directory) {
			return "", "", false
		}
		target = filepath.Join(directory, target)
	}
	return rest, filepath.Clean(target), true
}

// LeadingCdOperand returns the raw literal directory operand of a strippable
// leading cd prefix without resolving it against a base directory.
func LeadingCdOperand(script string) (string, bool) {
	operand, _, ok := parseLeadingCd(script)
	return operand, ok
}

func parseLeadingCd(script string) (operand, rest string, ok bool) {
	if strings.TrimSpace(script) == "" || strings.HasPrefix(strings.TrimLeft(script, " \t"), "#!") {
		return "", "", false
	}
	program, err := syntax.NewParser(syntax.Variant(syntax.LangBash)).Parse(strings.NewReader(script), "")
	if err != nil || len(program.Stmts) == 0 {
		return "", "", false
	}
	steps := program.Stmts
	if len(program.Stmts) == 1 {
		steps, ok = andChain(program.Stmts[0])
		if !ok || len(steps) < 2 {
			return "", "", false
		}
	}
	operand, ok = leadingCdOperand(steps[0])
	if !ok {
		return "", "", false
	}
	for _, stmt := range steps[1:] {
		if containsDirChange(stmt) {
			return "", "", false
		}
	}
	offset := int(steps[1].Pos().Offset())
	if offset < 0 || offset >= len(script) {
		return "", "", false
	}
	rest = script[offset:]
	if strings.TrimSpace(rest) == "" {
		return "", "", false
	}
	return operand, rest, true
}

func andChain(stmt *syntax.Stmt) ([]*syntax.Stmt, bool) {
	var steps []*syntax.Stmt
	var collect func(*syntax.Stmt) bool
	collect = func(node *syntax.Stmt) bool {
		if node == nil {
			return false
		}
		if bin, ok := node.Cmd.(*syntax.BinaryCmd); ok && bin.Op == syntax.AndStmt {
			if node.Background || node.Coprocess || node.Disown || node.Negated || len(node.Redirs) != 0 {
				return false
			}
			return collect(bin.X) && collect(bin.Y)
		}
		steps = append(steps, node)
		return true
	}
	if !collect(stmt) || len(steps) < 2 {
		return nil, false
	}
	return steps, true
}

func leadingCdOperand(stmt *syntax.Stmt) (string, bool) {
	call, ok := stmt.Cmd.(*syntax.CallExpr)
	if !ok || stmt.Background || stmt.Coprocess || stmt.Disown || stmt.Negated ||
		len(stmt.Redirs) != 0 || len(call.Assigns) != 0 {
		return "", false
	}
	if len(call.Args) < 2 || len(call.Args) > 3 {
		return "", false
	}
	if name, ok := Literal(call.Args[0]); !ok || name != "cd" {
		return "", false
	}
	arg := call.Args[1]
	allowDash := false
	if len(call.Args) == 3 {
		if sep, ok := Literal(call.Args[1]); !ok || sep != "--" {
			return "", false
		}
		arg, allowDash = call.Args[2], true
	}
	target, ok := Literal(arg)
	if !ok || target == "" || strings.HasPrefix(target, "~") || !allowDash && strings.HasPrefix(target, "-") {
		return "", false
	}
	return target, true
}

func containsDirChange(stmt *syntax.Stmt) bool {
	found := false
	syntax.Walk(stmt, func(node syntax.Node) bool {
		if found {
			return false
		}
		call, ok := node.(*syntax.CallExpr)
		if !ok || len(call.Args) == 0 {
			return true
		}
		args := call.Args
		if first, literal := Literal(args[0]); literal && (first == "builtin" || first == "command") && len(args) > 1 {
			args = args[1:]
		}
		if name, literal := Literal(args[0]); literal {
			switch name {
			case "cd", "pushd", "popd":
				found = true
				return false
			}
		}
		return true
	})
	return found
}
