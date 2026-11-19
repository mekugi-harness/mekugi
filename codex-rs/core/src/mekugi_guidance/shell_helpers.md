These are shell commands on PATH. Use ordinary pipes. Options apply only to
commands whose usage lists them. Keep commands and tool results together; redirect
to a file only when the task needs that artifact. Use a file in `SESSION_TMP` for
substantial reusable scripts, and edit and run them in the same execution call.

Batch independent short operations. Run long commands in separate cells so they
can be waited on and stopped independently. A shell wait and the enclosing
`exec` wait are independent: continue the handle that actually yielded instead
of restarting the command. Leave room in both output budgets for any continuation
handle. An incomplete result is not full coverage; use the returned `next_call`.
