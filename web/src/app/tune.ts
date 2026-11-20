// What a finished take writes onto a layer's Tune. The second pass already chose a root,
// a scale and a reference; the capture path and the Fill button both use this.

export interface TakeTune {
  root: number;
  scale: number;
  refCents: number;
}

/** Root 0..11, a whole scale index, and the reference cents clamped the way the capture page does. */
export function tuneFromTake(take: TakeTune): TakeTune {
  return {
    root: Math.max(0, Math.min(11, Math.round(take.root))),
    scale: Math.max(0, Math.round(take.scale)),
    refCents: Math.max(-50, Math.min(50, take.refCents)),
  };
}
