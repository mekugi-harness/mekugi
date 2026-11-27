export const detailLinks = Symbol("detail links");

export type LineRange = { first: number; last: number };
export type DetailLink =
  | { kind: "file"; path: string; label: string; range?: LineRange }
  | { kind: "run"; itemId: string; range?: LineRange };

function suffix(target: string): { path: string; range?: LineRange } {
  const match = /(?:#L([1-9]\d*)(?:-L([1-9]\d*))?|:([1-9]\d*)(?:-([1-9]\d*))?)$/.exec(target);
  if (!match) return { path: target };
  const first = Number(match[1] ?? match[3]);
  const last = Number(match[2] ?? match[4] ?? first);
  if (!Number.isSafeInteger(first) || !Number.isSafeInteger(last) || last < first) {
    return { path: target };
  }
  return { path: target.slice(0, match.index), range: { first, last } };
}

export function parseDetailLink(target: string): DetailLink | undefined {
  const runPrefix = "mekugi:run/";
  if (target.startsWith(runPrefix)) {
    const { path, range } = suffix(target.slice(runPrefix.length));
    if (!/^exec-[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(path)) return;
    return { kind: "run", itemId: path, range };
  }
  if (target.startsWith("file:")) {
    if (!target.startsWith("file:/")) return;
    try {
      const url = new URL(target);
      if (url.hostname || url.search) return;
      const { path, range } = suffix(url.hash);
      if (path) return;
      return { kind: "file", path: decodeURIComponent(url.pathname), label: target, range };
    } catch {
      return;
    }
  }
  const { path, range } = suffix(target.replace(/^@/, ""));
  if (!path || path.includes("://") || /^[#?]/.test(path)) return;
  return { kind: "file", path, label: target, range };
}

export function detailRange(text: string, range: LineRange, command = false) {
  // After the retention marker, the backend no longer knows the original
  // line numbers. The line directly before it may have been cut mid-line.
  const omission = command ? /\n\.\.\. \d+ bytes omitted \.\.\.\r?\n/.exec(text) : null;
  const cutoff = omission ? text.lastIndexOf("\n", omission.index - 1) + 1 : text.length;
  let first = 0;
  for (let line = 1; line < range.first; line++) {
    const end = text.indexOf("\n", first);
    if (end < 0 || end >= cutoff) return;
    first = end + 1;
  }
  if (first >= cutoff) return;
  let end = first;
  for (let line = range.first; line <= range.last; line++) {
    end = text.indexOf("\n", end);
    if (end < 0) {
      if (line < range.last) return;
      end = text.length;
    } else if (line < range.last) {
      end++;
    }
    if (end > cutoff || (end === text.length && line < range.last)) return;
  }
  const markEnd = text[end - 1] === "\r" ? end - 1 : end;
  return {
    before: text.slice(0, first),
    marked: text.slice(first, markEnd),
    after: text.slice(markEnd),
  };
}
