/** Fuzzy score of `query` against `text` (higher is better), or null if no match. */
export function fuzzyScore(query: string, text: string): { score: number; hits: number[] } | null {
  if (!query) return { score: 0, hits: [] };
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  const hits: number[] = [];
  let ti = 0;
  let score = 0;
  let prev = -2;
  const nameStart = t.lastIndexOf("/") + 1;
  for (const ch of q) {
    if (ch === " ") continue;
    const i = t.indexOf(ch, ti);
    if (i < 0) return null;
    hits.push(i);
    score += 1;
    if (i === prev + 1) score += 4; // consecutive
    if (i === 0 || "/-_. ".includes(t[i - 1]) || (text[i] >= "A" && text[i] <= "Z")) score += 3; // word start
    if (i >= nameStart) score += 2; // in the file name
    prev = i;
    ti = i + 1;
  }
  score -= t.length * 0.02;
  if (t.slice(nameStart).startsWith(q)) score += 10;
  return { score, hits };
}

export function fuzzyFilter<T>(items: T[], query: string, key: (t: T) => string, limit = 60) {
  const out: { item: T; score: number; hits: number[] }[] = [];
  for (const item of items) {
    const r = fuzzyScore(query, key(item));
    if (r) out.push({ item, ...r });
  }
  out.sort((a, b) => b.score - a.score);
  return out.slice(0, limit);
}

export function Highlight({ text, hits }: { text: string; hits: number[] }) {
  const set = new Set(hits);
  return (
    <>
      {[...text].map((c, i) => (set.has(i) ? <b key={i} className="hit">{c}</b> : <span key={i}>{c}</span>))}
    </>
  );
}
