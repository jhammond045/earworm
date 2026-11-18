// Trial generation and scoring for the M4 blind match test. Pure: no DOM, no audio.
// The seeded shuffle is the M0 test page's, so a seed means the same thing here.

/** M4 gate: listeners pick the true source at least this often... */
export const GATE_PCT = 70;
/** ...across this many varied sources. */
export const GATE_SOURCES = 20;
export const MIN_SOURCES = 4;
export const CHOICES = 4;

export const SUGGESTED_KINDS = [
  'voice (sung vowel)', 'voice (spoken or hummed)', 'whistle', 'flute-like tone', 'plucked string', 'bell or chime',
  'drum hit', 'clap', 'hiss or breath', 'synth pad', 'bowed sound', 'brass or reed', 'piano-like note', 'click or tick',
];

export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export function shuffle<T>(a: readonly T[], rnd: () => number): T[] {
  const x = a.slice();
  for (let i = x.length - 1; i > 0; i--) {
    const j = Math.floor(rnd() * (i + 1));
    [x[i], x[j]] = [x[j], x[i]];
  }
  return x;
}

export interface Trial {
  /** Index of the source the match was made from. */
  target: number;
  /** Source indices in display order A to D; includes the target. */
  candidates: number[];
}

/** One trial per source, in shuffled order; each shows the true source and three others. */
export function makeTrials(nSources: number, seed: number): Trial[] {
  if (nSources < CHOICES) throw new RangeError(`need at least ${CHOICES} sources, have ${nSources}`);
  const rnd = mulberry32(seed);
  const all = Array.from({ length: nSources }, (_, i) => i);
  return shuffle(all, rnd).map((target) => {
    const others = shuffle(all.filter((i) => i !== target), rnd).slice(0, CHOICES - 1);
    return { target, candidates: shuffle([target, ...others], rnd) };
  });
}

export interface Outcome {
  source: string;
  chosen: string;
  correct: boolean;
  /** Letter the listener picked, A to D. */
  position: string;
}

export function outcomeFor(items: { name: string }[], trial: Trial, pos: number): Outcome {
  const chosen = trial.candidates[pos];
  return { source: items[trial.target].name, chosen: items[chosen].name, correct: chosen === trial.target, position: 'ABCD'[pos] };
}

export interface Score {
  right: number;
  total: number;
  percent: number;
  /** At or above the gate percentage (says nothing about how many sources were used). */
  atGate: boolean;
  /** Fewer sources than the gate wants: the percentage is not the gate result. */
  short: boolean;
}

export function score(outcomes: readonly Outcome[], nSources = outcomes.length): Score {
  const right = outcomes.filter((o) => o.correct).length;
  const total = outcomes.length;
  const percent = total ? (100 * right) / total : 0;
  return { right, total, percent, atGate: total > 0 && percent >= GATE_PCT, short: nSources < GATE_SOURCES };
}

export const SOUND_MODES = ['default', 'off', 'on'] as const;
export type SoundMode = (typeof SOUND_MODES)[number];

/** MatchOptions.useSound for a mode: undefined is the analysis default rule. */
export function useSoundFor(mode: SoundMode): boolean | undefined {
  return mode === 'default' ? undefined : mode === 'on';
}

export interface ReportSource {
  name: string;
  sampleRate: number;
  seconds: number;
  pitched: boolean;
  pitchCents: number;
  useSound: boolean;
  matchMs: number;
  renderMs: number;
}

export interface ReportInput {
  seed: number;
  soundMode: SoundMode;
  sources: ReportSource[];
  trials: Trial[];
  outcomes: Outcome[];
  renderRate: number;
  releaseS: number;
  when?: Date;
}

/** The JSON the page downloads. */
export function buildReport(r: ReportInput) {
  const s = score(r.outcomes, r.sources.length);
  return {
    engine: 'klont M4',
    seed: r.seed,
    settings: { influence: 1, mask: 'all', soundMode: r.soundMode, renderRate: r.renderRate, releaseS: r.releaseS, gateRule: soundGateNote(r.soundMode) },
    sources: r.sources,
    trials: r.trials.map((t) => ({ target: r.sources[t.target].name, candidates: t.candidates.map((c) => r.sources[c].name) })),
    picks: r.outcomes,
    right: s.right,
    total: s.total,
    percent: s.percent,
    gatePercent: GATE_PCT,
    gateSources: GATE_SOURCES,
    atGate: s.atGate,
    gateRun: r.soundMode === 'default' && !s.short,
    finishedAt: (r.when ?? new Date()).toISOString(),
  };
}

function soundGateNote(mode: SoundMode): string {
  return mode === 'default' ? 'default use_sound rule (the gate run)' : `use_sound ${mode} for all (exploration, not the gate run)`;
}
