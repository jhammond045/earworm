// Helpers for the reload and crash checks: a stable text form of the document, its hash, and the first
// place two documents differ.

/** JSON with object keys in sorted order, so equal documents give equal text. */
export function stableStringify(v: unknown): string {
  return JSON.stringify(v, (_k, x) => {
    if (x && typeof x === 'object' && !Array.isArray(x)) {
      const o: Record<string, unknown> = {};
      for (const k of Object.keys(x as object).sort()) o[k] = (x as Record<string, unknown>)[k];
      return o;
    }
    return x;
  });
}

/** SHA-256 as hex when the browser has it (secure contexts), else a 32-bit FNV-1a hash. */
export async function hashText(text: string): Promise<string> {
  const subtle = globalThis.crypto?.subtle;
  if (subtle) {
    const d = await subtle.digest('SHA-256', new TextEncoder().encode(text));
    return [...new Uint8Array(d)].map((b) => b.toString(16).padStart(2, '0')).join('');
  }
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) { h ^= text.charCodeAt(i); h = Math.imul(h, 0x01000193) >>> 0; }
  return `fnv-${h.toString(16)}`;
}

const show = (v: unknown): string => { const s = JSON.stringify(v); return s === undefined ? 'undefined' : s.length > 60 ? `${s.slice(0, 57)}...` : s; };

/** The first path where `a` and `b` differ (like `sections[1].clips.lay_x.notes[3].t: 120 vs 121`), or null when equal. */
export function firstDifference(a: unknown, b: unknown, path = ''): string | null {
  if (a === b) return null;
  const arrA = Array.isArray(a), arrB = Array.isArray(b);
  if (a && b && typeof a === 'object' && typeof b === 'object' && arrA === arrB) {
    if (arrA) {
      const x = a as unknown[], y = b as unknown[];
      for (let i = 0; i < Math.max(x.length, y.length); i++) {
        const d = firstDifference(x[i], y[i], `${path}[${i}]`);
        if (d) return d;
      }
      return null;
    }
    const x = a as Record<string, unknown>, y = b as Record<string, unknown>;
    const keys = [...new Set([...Object.keys(x), ...Object.keys(y)])].sort();
    for (const k of keys) {
      const d = firstDifference(x[k], y[k], path ? `${path}.${k}` : k);
      if (d) return d;
    }
    return null;
  }
  if (typeof a === 'number' && typeof b === 'number' && Number.isNaN(a) && Number.isNaN(b)) return null;
  return `${path || '(root)'}: ${show(a)} vs ${show(b)}`;
}
