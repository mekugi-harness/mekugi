import type { Entry, Value } from "./protocol";

export interface WorkdirRow {
  entry: Entry;
  views?: Entry[];
  thoughts?: Entry[];
}

export type WorkdirSegment<T extends WorkdirRow> =
  | { kind: "row"; row: T; key: number }
  | { kind: "group"; workdir: string; rows: T[]; key: string };

const EDIT_VERBS = new Set(["Create", "Edit", "Delete", "Move"]);
const PROSE_KINDS = new Set(["text", "final", "message"]);

function stripTrailingSlashes(path: string): string {
  if (path === "/") return "/";
  return path.replace(/\/+$/, "");
}

function isLocalRelativePath(rel: string): boolean {
  if (!rel || rel.startsWith("/")) return false;
  let depth = 0;
  let hasSegment = false;
  for (const segment of rel.split("/")) {
    if (!segment || segment === ".") continue;
    if (segment === "..") {
      depth--;
      if (depth < 0) return false;
    } else {
      depth++;
      hasSegment = true;
    }
  }
  return hasSegment && depth > 0;
}

function leadingCodeSpan(text: string): { content: string; end: number } | undefined {
  if (!text.startsWith("`")) return undefined;
  let ticks = 0;
  while (text[ticks] === "`") ticks++;
  const fence = "`".repeat(ticks);
  const close = text.indexOf(fence, ticks);
  if (close < 0) return undefined;
  let content = text.slice(ticks, close);
  if (content.length >= 2 && content.startsWith(" ") && content.endsWith(" ") && content.trim()) {
    content = content.slice(1, -1);
  }
  return { content, end: close + ticks };
}

function quoteCode(text: string): string {
  let longest = 0;
  for (const match of text.matchAll(/`+/g)) {
    if (match[0].length > longest) longest = match[0].length;
  }
  const fence = "`".repeat(longest + 1);
  const pad = text.startsWith("`") || text.endsWith("`") ? " " : "";
  return `${fence}${pad}${text}${pad}${fence}`;
}

// Normalizes a workdir relative to the thread's default workspace, returning ""
// when the directory is empty or matches the default workspace.
export function normalizeWorkdir(workdir: string | undefined, workspace?: string): string {
  if (!workdir) return "";
  const trimmed = stripTrailingSlashes(workdir.trim());
  if (!trimmed || trimmed === ".") return "";
  const root = workspace ? stripTrailingSlashes(workspace.trim()) : "";
  if (root) {
    if (trimmed === root) return "";
    if (trimmed.startsWith(`${root}/`)) {
      const rel = trimmed.slice(root.length + 1);
      return isLocalRelativePath(rel) ? rel : "";
    }
  }
  return trimmed;
}

export function isEditOperation(value: Value): boolean {
  return value.kind === "op" && (!!value.sources?.length || EDIT_VERBS.has(value.verb ?? ""));
}

export function relativeToDir(path: string, dir: string, workspace?: string): string {
  const normDir = normalizeWorkdir(dir, workspace);
  if (!normDir || !path) return "";
  let candidate = path;
  const root = workspace ? stripTrailingSlashes(workspace.trim()) : "";
  if (root && !normDir.startsWith("/") && candidate.startsWith(`${root}/`)) {
    candidate = candidate.slice(root.length + 1);
  }
  const prefix = normDir === "/" ? "/" : `${normDir}/`;
  if (!candidate.startsWith(prefix)) return "";
  const rel = candidate.slice(prefix.length);
  return isLocalRelativePath(rel) ? rel : "";
}

// Returns the edit target displayed relative to dir, or "" when the edit does
// not belong within dir. Move paths require both endpoints to lie inside dir.
export function editWithin(value: Value, dir: string, workspace?: string): string {
  if (!isEditOperation(value)) return "";
  const rawPath = value.path || leadingCodeSpan(value.label ?? "")?.content || "";
  if (!rawPath) return "";
  if (value.verb === "Move") {
    const arrow = rawPath.indexOf(" => ");
    if (arrow >= 0) {
      const before = relativeToDir(rawPath.slice(0, arrow), dir, workspace);
      const after = relativeToDir(rawPath.slice(arrow + 4), dir, workspace);
      return before && after ? `${before} => ${after}` : "";
    }
  }
  return relativeToDir(rawPath, dir, workspace);
}

// Rewrites only a leading code-span path in an edit label relative to dir.
export function relativeEditLabel(value: Value, dir: string, workspace?: string): string {
  const raw = value.label ?? "";
  if (!raw || value.path) return raw;
  const span = leadingCodeSpan(raw);
  if (!span) return raw;
  const rel = editWithin(value, dir, workspace);
  if (!rel) return raw;
  return quoteCode(rel) + raw.slice(span.end);
}

function isPendingBatchExit(value: Value): boolean {
  return !!(
    value.batchExit &&
    !value.code &&
    !value.codeLines &&
    !value.tailLines &&
    !value.tail?.length &&
    value.exitCode === undefined &&
    !value.duration &&
    !value.running
  );
}

function isReasoningRow(row: WorkdirRow): boolean {
  return !!row.thoughts || row.entry.kind === "reasoning";
}

function isActivityRow(row: WorkdirRow): boolean {
  const { entry } = row;
  if (entry.turn || entry.traffic || row.views || isReasoningRow(row)) return false;
  if (PROSE_KINDS.has(entry.kind ?? "") || entry.kind === "hook" || entry.kind === "progress") {
    return false;
  }
  const values = entry.values;
  if (!values?.length) return false;
  if (values[0]?.kind === "compaction") return false;
  if (values.every((value) => value.kind === "progress" && (value.from || value.waits?.length))) {
    return false;
  }
  return true;
}

function rowWorkdir(row: WorkdirRow, workspace?: string): string {
  if (!isActivityRow(row)) return "";
  const values = (row.entry.values ?? []).filter((value) => !isPendingBatchExit(value));
  if (!values.length) return "";
  let dir = "";
  let sawDefaultCommand = false;
  for (const value of values) {
    const current = normalizeWorkdir(value.workdir, workspace);
    if (current) {
      if (dir && dir !== current) return "";
      dir = current;
    } else if (!value.batchExit && !isEditOperation(value)) {
      sawDefaultCommand = true;
    }
  }
  if (!dir || sawDefaultCommand) return "";
  return dir;
}

function canAdoptEditRow(row: WorkdirRow, dir: string, workspace?: string): boolean {
  if (!isActivityRow(row)) return false;
  const values = (row.entry.values ?? []).filter((value) => !isPendingBatchExit(value));
  if (!values.length) return false;
  return values.every((value) => editWithin(value, dir, workspace) !== "");
}

// Labels consecutive runs of at least two activity rows sharing the same
// non-default workdir. Neighboring edit rows whose visible paths all lie within
// that directory join the run, and collapsed reasoning rows between members of
// the same group stay inside it without splitting the sticky header.
export function workdirGroups<T extends WorkdirRow>(
  rows: readonly T[],
  workspace?: string,
): string[] {
  const dirs = rows.map((row) => rowWorkdir(row, workspace));
  if (!dirs.some(Boolean)) return dirs;

  const adopt = (start: number, step: -1 | 1, dir: string): number => {
    let at = start;
    while (at >= 0 && at < rows.length && dirs[at] === "") {
      let probe = at;
      while (
        probe >= 0 &&
        probe < rows.length &&
        dirs[probe] === "" &&
        isReasoningRow(rows[probe])
      ) {
        probe += step;
      }
      if (probe < 0 || probe >= rows.length || dirs[probe] !== "") break;
      if (!canAdoptEditRow(rows[probe], dir, workspace)) break;
      for (let k = at; k !== probe + step; k += step) {
        dirs[k] = dir;
      }
      at = probe + step;
    }
    return at;
  };

  const bridgeReasoning = () => {
    for (let i = 0; i < rows.length; i++) {
      if (dirs[i] !== "" || !isReasoningRow(rows[i])) continue;
      let end = i;
      while (end < rows.length && dirs[end] === "" && isReasoningRow(rows[end])) end++;
      const left = i > 0 ? dirs[i - 1] : "";
      const right = end < rows.length ? dirs[end] : "";
      if (left && left === right) {
        for (let k = i; k < end; k++) dirs[k] = left;
      }
      i = end - 1;
    }
  };

  bridgeReasoning();

  // Adopt neighboring edit rows before and after each workdir run.
  for (let i = 0; i < rows.length;) {
    const dir = dirs[i];
    if (!dir) {
      i++;
      continue;
    }
    adopt(i - 1, -1, dir);
    while (i < rows.length && dirs[i] === dir) {
      i = adopt(i + 1, 1, dir);
    }
  }

  // Re-bridge reasoning rows in case edit adoption connected two runs of the same dir.
  bridgeReasoning();

  // A workdir heading only appears for runs of at least two activity rows.
  for (let start = 0; start < dirs.length;) {
    const dir = dirs[start];
    if (!dir) {
      start++;
      continue;
    }
    let end = start + 1;
    let activityCount = isReasoningRow(rows[start]) ? 0 : 1;
    while (end < dirs.length && dirs[end] === dir) {
      if (!isReasoningRow(rows[end])) activityCount++;
      end++;
    }
    if (activityCount < 2) {
      for (let k = start; k < end; k++) dirs[k] = "";
    }
    start = end;
  }

  return dirs;
}

export function segmentWorkdirRows<T extends WorkdirRow>(
  rows: readonly T[],
  groups: readonly string[],
): WorkdirSegment<T>[] {
  const segments: WorkdirSegment<T>[] = [];
  for (let i = 0; i < rows.length;) {
    const dir = groups[i] ?? "";
    if (!dir) {
      segments.push({ kind: "row", row: rows[i], key: rows[i].entry.seq });
      i++;
      continue;
    }
    const grouped: T[] = [];
    while (i < rows.length && (groups[i] ?? "") === dir) {
      grouped.push(rows[i]);
      i++;
    }
    segments.push({
      kind: "group",
      workdir: dir,
      rows: grouped,
      key: `${dir}:${grouped[0].entry.seq}`,
    });
  }
  return segments;
}
