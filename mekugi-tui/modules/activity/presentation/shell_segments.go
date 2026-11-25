package presentation

import (
	"slices"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/commandanalysis/shellsyntax"
	"mvdan.cc/sh/v3/syntax"
)

type shellSegment struct {
	Start, End int
	Source     string
}

func splitShellSegments(script string) ([]shellSegment, bool) {
	program, err := syntax.NewParser(syntax.Variant(syntax.LangBash)).Parse(strings.NewReader(script), "")
	if err != nil || strings.Contains(script, "__mekugi_") || !shellSegmentTrackable(program) {
		return nil, false
	}
	if len(program.Stmts) > 0 && hasShellSetup(program) {
		return []shellSegment{{Start: 0, End: len(script), Source: strings.TrimRight(script, "\r\n")}}, true
	}
	var segments []shellSegment
	var add func(*syntax.Stmt)
	add = func(stmt *syntax.Stmt) {
		if binary, ok := stmt.Cmd.(*syntax.BinaryCmd); ok && (binary.Op == syntax.AndStmt || binary.Op == syntax.OrStmt) &&
			!stmt.Negated && len(stmt.Redirs) == 0 {
			add(binary.X)
			add(binary.Y)
			return
		}
		if conditional, ok := stmt.Cmd.(*syntax.IfClause); ok && !stmt.Negated && len(stmt.Redirs) == 0 {
			for clause := conditional; clause != nil; clause = clause.Else {
				for _, child := range clause.Cond {
					add(child)
				}
				for _, child := range clause.Then {
					add(child)
				}
			}
			return
		}
		start, end := int(stmt.Pos().Offset()), int(shellStmtEnd(stmt).Offset())
		segments = append(segments, shellSegment{Start: start, End: end, Source: shellsyntax.StatementSource(stmt, script[start:end])})
	}
	for _, stmt := range program.Stmts {
		add(stmt)
	}
	if len(segments) == 0 || len(segments) > 1 && !shellTrapsFirst(program.Stmts) {
		return nil, false
	}
	return segments, true
}

func hasShellSetup(program *syntax.File) bool {
	setup := false
	syntax.Walk(program, func(node syntax.Node) bool {
		if setup {
			return false
		}
		switch node := node.(type) {
		case *syntax.Subshell, *syntax.CmdSubst, *syntax.ProcSubst:
			return false
		case *syntax.BinaryCmd:
			if node.Op == syntax.Pipe || node.Op == syntax.PipeAll {
				return false
			}
		case *syntax.DeclClause, *syntax.FuncDecl:
			setup = true
		case *syntax.CallExpr:
			if len(node.Args) == 0 {
				setup = len(node.Assigns) > 0
				break
			}
			args := node.Args
			if args[0].Lit() == "builtin" || args[0].Lit() == "command" {
				args = args[1:]
			}
			if len(args) > 0 {
				switch args[0].Lit() {
				case "export", "readonly", "declare", "typeset", "local", "set", "shopt",
					"trap", "source", ".", "unset", "alias", "unalias", "umask":
					setup = true
				}
			}
		}
		return !setup
	})
	return setup
}

func shellStmtEnd(stmt *syntax.Stmt) syntax.Pos {
	end := stmt.Position
	later := func(pos syntax.Pos) {
		if pos.Offset() > end.Offset() {
			end = pos
		}
	}
	switch cmd := stmt.Cmd.(type) {
	case nil:
	case *syntax.BinaryCmd:
		later(shellStmtEnd(cmd.Y))
	case *syntax.TimeClause:
		if cmd.Stmt == nil {
			later(cmd.End())
		} else {
			later(shellStmtEnd(cmd.Stmt))
		}
	default:
		later(cmd.End())
	}
	for _, redirect := range stmt.Redirs {
		if redirect.Hdoc != nil {
			later(redirect.Word.End())
		} else {
			later(redirect.End())
		}
	}
	return end
}

func shellTrapsFirst(stmts []*syntax.Stmt) bool {
	for _, stmt := range stmts {
		switch cmd := stmt.Cmd.(type) {
		case *syntax.FuncDecl:
			continue
		case nil, *syntax.CallExpr, *syntax.DeclClause, *syntax.LetClause, *syntax.TestClause, *syntax.ArithmCmd, *syntax.ForClause, *syntax.CaseClause:
			return true
		case *syntax.BinaryCmd:
			return shellTrapsFirst([]*syntax.Stmt{cmd.X})
		case *syntax.Block:
			return shellTrapsFirst(cmd.Stmts)
		case *syntax.IfClause:
			return shellTrapsFirst(cmd.Cond)
		case *syntax.WhileClause:
			return shellTrapsFirst(cmd.Cond)
		}
		return false
	}
	return false
}

var shellUntrackedBuiltins = []string{"wait", "jobs", "fg", "bg", "disown", "trap", "coproc", "suspend", "caller"}
var shellUntrackedParameters = []string{"!", "PIPESTATUS", "_", "LINENO", "BASH_LINENO", "BASH_COMMAND", "BASH_SOURCE", "FUNCNAME", "BASH_EXECUTION_STRING"}

func shellSegmentTrackable(program *syntax.File) bool {
	ok := true
	var visit func(node syntax.Node, function bool) bool
	visit = func(node syntax.Node, function bool) bool {
		switch node := node.(type) {
		case *syntax.Stmt:
			if node.Background || node.Coprocess || node.Disown {
				ok = false
			}
		case *syntax.FuncDecl:
			if node.Body != nil {
				syntax.Walk(node.Body, func(child syntax.Node) bool { return ok && visit(child, true) })
			}
			return false
		case *syntax.CoprocClause:
			ok = false
		case *syntax.ParamExp:
			if node.Param != nil && slices.Contains(shellUntrackedParameters, node.Param.Value) {
				ok = false
			}
		case *syntax.CallExpr:
			if len(node.Args) == 0 {
				break
			}
			name := node.Args[0].Lit()
			switch {
			case slices.Contains(shellUntrackedBuiltins, name):
				ok = false
			case name == "return" && !function:
				ok = false
			case name == "exec" && len(node.Args) > 1:
				ok = false
			case name == "set" || name == "shopt":
				ok = !shellTracing(node)
			}
		}
		return ok
	}
	syntax.Walk(program, func(node syntax.Node) bool { return ok && visit(node, false) })
	return ok
}

func shellTracing(call *syntax.CallExpr) bool {
	for _, word := range call.Args[1:] {
		arg := word.Lit()
		if arg == "" {
			return true
		}
		if strings.Contains(arg, "xtrace") || strings.Contains(arg, "verbose") || strings.Contains(arg, "extdebug") || strings.Contains(arg, "functrace") {
			return true
		}
		if (strings.HasPrefix(arg, "-") || strings.HasPrefix(arg, "+")) && !strings.HasPrefix(arg, "--") && strings.ContainsAny(arg[1:], "xvT") {
			return true
		}
	}
	return false
}
