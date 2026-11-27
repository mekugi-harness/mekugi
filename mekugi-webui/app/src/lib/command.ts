import type { ThreadItem, Value } from "./protocol";

type CommandItem = Extract<ThreadItem, { type: "commandExecution" }>;

// Inline previews retain the last five nonblank lines, up to 512 UTF-8 bytes
// each. Details reads the complete aggregate, including lines clipped here.
const encoder = new TextEncoder();
const decoder = new TextDecoder();
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
      let cut = 512;
      while ((bytes[cut] & 0xc0) === 0x80) cut--;
      tail.unshift(decoder.decode(bytes.subarray(0, cut)) + "…");
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
        code: item.display?.segments[0] ?? item.display?.script ?? item.command,
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
