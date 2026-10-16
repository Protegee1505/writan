export type DateFormat = "relative" | "absolute";

export function fmtDate(ts: string | null | undefined, mode: DateFormat = "relative"): string {
  if (!ts) return "";
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return "";
  if (mode === "absolute") {
    return d.toLocaleString(undefined, {
      month: "short",
      day: "numeric",
      year: d.getFullYear() === new Date().getFullYear() ? undefined : "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  }
  const diff = Date.now() - d.getTime();
  const min = Math.round(diff / 60000);
  if (min < 1) return "just now";
  if (min < 60) return `${min}m ago`;
  const h = Math.round(min / 60);
  if (h < 24) return `${h}h ago`;
  const days = Math.round(h / 24);
  if (days < 7) return `${days}d ago`;
  return d.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year: d.getFullYear() === new Date().getFullYear() ? undefined : "numeric",
  });
}

export interface HighlightPart {
  text: string;
  hit: boolean;
}

/** Split text into parts around case-insensitive occurrences of query. */
export function highlightParts(text: string, query: string): HighlightPart[] {
  const q = query.trim();
  if (!q) return [{ text, hit: false }];
  const parts: HighlightPart[] = [];
  const lower = text.toLowerCase();
  const needle = q.toLowerCase();
  let index = 0;
  while (index < text.length) {
    const found = lower.indexOf(needle, index);
    if (found === -1) {
      parts.push({ text: text.slice(index), hit: false });
      break;
    }
    if (found > index) parts.push({ text: text.slice(index, found), hit: false });
    parts.push({ text: text.slice(found, found + q.length), hit: true });
    index = found + q.length;
  }
  return parts;
}
