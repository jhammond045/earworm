// The EARWORM mark occasionally spends six of its letters on another word, then puts them back.
// The spare letter is the one the word does not use.

export const SOURCE = 'EARWORM';

export const LOGO_WORDS = ['MARROW', 'REMORA', 'REWARM', 'ROAMER', 'WARMER', 'WOMERA', 'WORMER'] as const;

export interface LogoPlan {
  /** Index in EARWORM for each letter of the target, in target order. */
  from: number[];
  /** Index in EARWORM of the letter the word does not use. */
  spare: number;
}

/** A plan when `word` can be spelled from distinct letters of EARWORM, leaving exactly one behind. */
export function logoPlan(word: string): LogoPlan | null {
  if (word.length !== SOURCE.length - 1) return null;
  const pools = new Map<string, number[]>();
  for (let i = 0; i < SOURCE.length; i++) {
    const list = pools.get(SOURCE[i]) ?? [];
    list.push(i);
    pools.set(SOURCE[i], list);
  }
  const from: number[] = [];
  for (const ch of word) {
    const list = pools.get(ch);
    const i = list?.shift();
    if (i === undefined) return null;
    from.push(i);
  }
  const used = new Set(from);
  let spare = -1;
  for (let i = 0; i < SOURCE.length; i++) if (!used.has(i)) spare = i;
  return spare >= 0 ? { from, spare } : null;
}

/** Move the glyphs. `plan` null puts every letter back in EARWORM. */
export function placeLogo(root: HTMLElement, plan: LogoPlan | null): void {
  const glyphs = [...root.children] as HTMLElement[];
  if (glyphs.length !== SOURCE.length) return;
  if (!plan) {
    for (const g of glyphs) {
      g.style.transform = '';
      g.style.opacity = '';
    }
    return;
  }
  const slot = new Map<number, number>();
  plan.from.forEach((src, i) => slot.set(src, i));
  glyphs.forEach((g, i) => {
    const k = slot.get(i);
    if (k === undefined) {
      g.style.transform = 'translateY(1.15em)';
      g.style.opacity = '0';
    } else {
      g.style.transform = `translateX(calc(${0.5 + k - i} * 100%))`;
      g.style.opacity = '1';
    }
  });
}

const HOLD_MS = 1600;
const RETURN_MS = 800;

/** Start the occasional rearrangement. Does nothing when the system asks for reduced motion. */
export function startLogo(root: HTMLElement, busy: () => boolean): () => void {
  if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) return () => {};
  root.replaceChildren(...[...SOURCE].map((ch) => {
    const s = document.createElement('span');
    s.textContent = ch;
    return s;
  }));
  let stopped = false;
  let timer = 0;
  const later = (ms: number) => new Promise<void>((resolve) => { timer = window.setTimeout(resolve, ms); });
  const wait = async (ms: number): Promise<boolean> => {
    const end = performance.now() + ms;
    while (!stopped && performance.now() < end) {
      if (busy()) return false;
      await later(Math.min(200, end - performance.now()));
    }
    return !stopped && !busy();
  };
  const loop = async () => {
    while (!stopped) {
      await later(20000 + Math.random() * 20000);
      if (stopped || busy()) continue;
      const word = LOGO_WORDS[Math.floor(Math.random() * LOGO_WORDS.length)];
      const plan = logoPlan(word);
      if (!plan) continue;
      placeLogo(root, plan);
      const held = await wait(HOLD_MS);
      placeLogo(root, null);
      if (held) await wait(RETURN_MS);
    }
  };
  void loop();
  return () => { stopped = true; window.clearTimeout(timer); placeLogo(root, null); };
}
