/// <reference types="audioworklet" />
// Round-trip test processor. Plays `count` chirps spaced `spacing` frames
// apart starting at frame `lead`, and records the input into a buffer at the
// same frame index, all inside process(), so the recording index and the
// playback index share one clock. The page finds each chirp in the recording
// by cross-correlation; the lag is the device round trip.

interface LoopbackOptions {
  click: Float32Array;
  spacing: number;
  count: number;
  lead: number;
  total: number;
}

class LoopbackProcessor extends AudioWorkletProcessor {
  private readonly o: LoopbackOptions;
  private readonly rec: Float32Array;
  private pos = 0;

  constructor(options: AudioWorkletNodeOptions) {
    super();
    this.o = options.processorOptions as LoopbackOptions;
    this.rec = new Float32Array(this.o.total);
  }

  process(inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
    const { click, spacing, count, lead, total } = this.o;
    const out = outputs[0][0];
    const inp = inputs[0]?.[0];
    for (let i = 0; i < out.length; i++) {
      const idx = this.pos + i;
      if (idx >= total) break;
      this.rec[idx] = inp ? inp[i] : 0;
      const rel = idx - lead;
      if (rel >= 0) {
        const k = Math.floor(rel / spacing);
        if (k < count) {
          const j = rel - k * spacing;
          if (j < click.length) out[i] = click[j];
        }
      }
    }
    this.pos += out.length;
    if (this.pos >= total) {
      this.port.postMessage({ rec: this.rec, sampleRate }, [this.rec.buffer]);
      return false;
    }
    return true;
  }
}

registerProcessor('loopback', LoopbackProcessor);
