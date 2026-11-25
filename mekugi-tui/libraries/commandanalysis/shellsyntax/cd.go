package shellsyntax

import (
	"path/filepath"
	"strings"

	"mvdan.cc/sh/v3/syntax"
)

// StripLeadingCd recognizes a safe leading literal cd <dir> && <rest>
// command, resolves the target directory against directory, and returns the
// remaining command text with the leading cd ... && omitted.
//
// Semicolons, ||, subshells, dynamic targets, cd -, bare cd,
// options other than --, and commands that change directory again later
// in the remaining statements are left intact.
func StripLeadingCd(script, directory string) (rest, target string, ok bool) {
	if strings.TrimSpace(script) == "" || strings.HasPrefix(strings.TrimLeft(script, " \t"), "#!") {
		return "", "", false
	}
	program, err := syntax.NewParser(syntax.Variant(syntax.LangBash)).Parse(strings.NewReader(script), "")
	if err != nil || len(program.Stmts) != 1 {
		return "", "", false
	}
	steps, ok := andChain(program.Stmts[0])
	if !ok || len(steps) < 2 {
		return "", "", false
	}
	target, ok = leadingCdTarget(steps[0], directory)
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
	return rest, target, true
}

// andChain flattens a pure foreground a && b && ... statement without
// surrounding redirects, negation, or background modifiers.
func andChain(stmt *syntax.Stmt) ([]*syntax.Stmt, bool) {
	if stmt == nil || stmt.Background || stmt.Coprocess || stmt.Disown || stmt.Negated || len(stmt.Redirs) != 0 {
		return nil, false
	}
	binary, ok := stmt.Cmd.(*syntax.BinaryCmd)
	if !ok || binary.Op != syntax.AndStmt {
		return nil, false
	}
	var steps []*syntax.Stmt
	var collect func(*syntax.Stmt) bool
	collect = func(node *syntax.Stmt) bool {
		if node == nil {
			return false
		}
		if bin, isBin := node.Cmd.(*syntax.BinaryCmd); isBin && bin.Op == syntax.AndStmt {
			if node.Background || node.Coprocess || node.Disown || node.Negated || len(node.Redirs) != 0 {
				return false
			}
			return collect(bin.X) && collect(bin.Y)
		}
		steps = append(steps, node)
		return true
	}
	if !collect(stmt) {
		return nil, false
	}
	return steps, true
}

func leadingCdTarget(stmt *syntax.Stmt, directory string) (string, bool) {
	call, ok := stmt.Cmd.(*syntax.CallExpr)
	if !ok || stmt.Background || stmt.Coprocess || stmt.Disown || stmt.Negated ||
		len(stmt.Redirs) != 0 || len(call.Assigns) != 0 {
		return "", false
	}
	if len(call.Args) != 2 && len(call.Args) != 3 {
		return "", false
	}
	name, literal := Literal(call.Args[0])
	if !literal || name != "cd" {
		return "", false
	}
	var target string
	var targetLiteral bool
	if len(call.Args) == 2 {
		target, targetLiteral = Literal(call.Args[1])
		if !targetLiteral || target == "" || strings.HasPrefix(target, "-") || strings.HasPrefix(target, "~") {
			return "", false
		}
	} else {
		sep, sepLiteral := Literal(call.Args[1])
		target, targetLiteral = Literal(call.Args[2])
		if !sepLiteral || sep != "--" || !targetLiteral || target == "" || strings.HasPrefix(target, "~") {
			return "", false
		}
	}
	if !filepath.IsAbs(target) {
		if !filepath.IsAbs(directory) {
			return "", false
		}
		target = filepath.Join(directory, target)
	}
	return filepath.Clean(target), true
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
