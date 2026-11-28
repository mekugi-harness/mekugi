import type { Status, Value } from "./protocol";

export function progressText(value: Value, label: (name: string) => string): string {
  const targets = value.waits?.map(
    (wait) => `${label(wait.name)}${wait.status ? `: ${wait.status}` : ""}`,
  );
  return `${value.label || value.body || ""}${targets?.length ? ` ${targets.join(", ")}` : ""}${value.waitTimeout ? ` (timeout ${value.waitTimeout})` : ""}${value.waitReason ? `: ${value.waitReason}` : ""}`;
}

// Elapsed grammar follows the terminal renderer: 850ms, 4.1s, 1m 19s, 2h 05m.
// Whole seconds count down like the playback clock, so the two agree.
export function duration(ms: number): string {
  if (ms < 1000) return `${Math.max(0, Math.round(ms))}ms`;
  if (ms < 10_000) return `${(Math.floor(ms / 100) / 10).toFixed(1)}s`;
  const seconds = Math.floor(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${String(seconds % 60).padStart(2, "0")}s`;
  return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, "0")}m`;
}

const pad = (n: number) => String(n).padStart(2, "0");

export function date(unixMs: number): string {
  const d = new Date(unixMs);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export function tokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}

export function cost(usd: number): string {
  return `$${usd.toFixed(2)}`;
}

export function tps(rate: number): string {
  return `${rate.toFixed(1)} tok/s`;
}

// Usage that is partly unknown is a lower bound, as 33.72M+.
export function atLeast(text: string, partial?: boolean): string {
  return partial ? `${text}+` : text;
}

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10240 ? 1 : 0)} KiB`;
  return `${(n / 1024 / 1024).toFixed(1)} MiB`;
}

export function basename(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  return trimmed.slice(trimmed.lastIndexOf("/") + 1) || trimmed;
}

// A session status as one line: its text, a finished turn's duration, and why
// it failed.
export function statusText(s: Status): string {
  let text = s.text ?? "";
  if (s.completed != null) text += ` in ${duration(s.completed)}`;
  if (s.failure) text += `: ${s.failure}`;
  return text;
}
