import type { ThreadItem, Value } from "./protocol";

type CommandItem = Extract<ThreadItem, { type: "commandExecution" }>;

export function unwrapShellCommand(raw: string): string {
  const trimmed = raw.trim();
  const match = /^(?:\/\S+\/)?(?:bash|sh|zsh|dash)\s+-(?:lc|c)\s+([\s\S]+)$/.exec(trimmed);
  if (!match) return raw;
  const arg = match[1].trim();
  if (arg.length >= 2 && arg.startsWith("'") && arg.endsWith("'")) {
    return arg.slice(1, -1).replace(/'\\''/g, "'");
  }
  if (arg.length >= 2 && arg.startsWith('"') && arg.endsWith('"')) {
    return arg
      .slice(1, -1)
      .replace(/\\(["\\$`])/g, "$1")
      .replace(/\\n/g, "\n");
  }
  if (!/\s/.test(arg)) return arg;
  return raw;
}

// Inline previews retain the last five nonblank lines, up to 512 UTF-8 bytes
// each. Details reads the complete aggregate, including lines clipped here.
const encoder = new TextEncoder();
function commandTail(output: string): Pick<Value, "tail" | "tailOmitted" | "tailClipped"> {
  const tail: string[] = [];
  let tailClipped = false;
  let end = output.length;
  while (end > 0 && tail.length < 5) {
    const start = output.lastIndexOf("\n", end - 1) + 1;
    const line = output.slice(start, end).replace(/\r$/, "").split("\r").at(-1)!.trimEnd();
    end = start - 1;
    if (!line.trim()) continue;
    const bytes = encoder.encode(line.slice(0, 513));
    if (bytes.length > 512) {
      tailClipped = true;
      // A streaming decode drops an unfinished code point at the cut.
      tail.unshift(new TextDecoder().decode(bytes.subarray(0, 512), { stream: true }) + "…");
    } else {
      tail.unshift(line);
    }
  }
  let tailOmitted = 0;
  for (let i = 0; i <= end; i++) if (output[i] === "\n") tailOmitted++;
  return { tail: tail.length ? tail : undefined, tailOmitted, tailClipped };
}

export function commandValues(item: CommandItem): Value[] {
  const workdir = item.display?.effectiveCwd || item.cwd || undefined;
  const result: Value = {
    kind: "op",
    verb: "Run",
    workdir,
    running: item.status === "inProgress",
    failed: item.status === "failed" || item.status === "declined",
    exitCode: item.exitCode ?? undefined,
    duration: item.durationMs ?? undefined,
    ...commandTail(item.aggregatedOutput ?? ""),
  };
  const segments =
    item.display?.segments ?? (item.commandActions ?? []).map((action) => action.command);
  if (segments.length < 2)
    return [
      {
        ...result,
        code: item.display?.segments[0] ?? unwrapShellCommand(item.command),
        lang: "bash",
      },
    ];
  // Source steps describe the script, not which steps executed.
  // Native completion and output belong only to the whole invocation.
  return [
    ...segments.map((code) => ({
      kind: "op",
      verb: "Run",
      code,
      lang: "bash",
      workdir,
    })),
    { ...result, batchExit: true },
  ];
}
