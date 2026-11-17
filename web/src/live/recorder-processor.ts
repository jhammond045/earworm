/// <reference types="audioworklet" />
// Records the raw mic (before the engine's high-pass) for Live follow's
// record mode, so a take can be run through the live path later exactly as
// the worklet would have run it. Posts chunks of CHUNK samples while
// recording; on stop posts the remainder and { done: true }.

const CHUNK = 8192;

class RecorderProcessor extends AudioWorkletProcessor {
  private on = false;
  private buf = new Float32Array(CHUNK);
  private n = 0;

  constructor() {
    super();
    this.port.onmessage = (ev) => {
      if (ev.data === 'start') { this.on = true; this.n = 0; }
      else if (ev.data === 'stop' && this.on) {
        this.on = false;
        this.flush();
        this.port.postMessage({ done: true });
      }
    };
  }

  private flush(): void {
    if (this.n === 0) return;
    const chunk = this.buf.slice(0, this.n);
    this.port.postMessage({ chunk }, [chunk.buffer]);
    this.n = 0;
  }

  process(inputs: Float32Array[][]): boolean {
    const inp = inputs[0]?.[0];
    if (!this.on || !inp) return true;
    for (let i = 0; i < inp.length; i++) {
      this.buf[this.n++] = inp[i];
      if (this.n === CHUNK) this.flush();
    }
    return true;
  }
}

registerProcessor('earworm-recorder', RecorderProcessor);
