package shellinstrumentation

// Source: internal/execsegment/hook.go:1:168@[465a8ff3] Bash instrumentation

// Guard marks a shell that already decided whether to track its command, so
// nested command shells started by that command are never tracked.
const Guard = "MEKUGI_EXEC_TRACK"
