//! The engine: eight layers, the transport, the partial budget and the
//! master bus. One instance per host (worklet, export worker, benchmark).
//!
//! Each call renders in spans that never cross a control tick (every TICK
//! samples, counted from engine start, so output does not depend on call
//! length). A span is split again at every sequencer event, live trigger and
//! note off, so all of them land on their exact sample.

use crate::curve::CurvePool;
use crate::layer::Layer;
use crate::live::{Action, LiveCapture};
use crate::metro::Metro;
use crate::protocol::*;
use crate::seq::{EventList, Track, Transport};
use crate::voice::{MAX_PARTIALS, TICK};
use dsp::filters::DcBlocker;
use params as P;

/// A live trigger waiting for its sample offset in the current call.
#[derive(Clone, Copy, Debug, Default)]
struct LiveMsg {
    time: u32,
    on: bool,
    layer: u8,
    slot: u8,
    cents: f32,
    velocity: f32,
    /// Live follow: 0 none, 1 start the live voice or hit, 2 release it.
    follow: u8,
    hard: f32,
}

pub struct Engine {
    sr: f32,
    max_block: usize,
    input: Vec<f32>,
    out_l: Vec<f32>,
    out_r: Vec<f32>,
    control: Vec<u8>,
    control_count: u32,
    meter: Vec<f32>,
    staging: Vec<u8>,
    host_load: f32,
    table: BufferTable,

    layers: Vec<Layer>,
    track: Box<Track>,
    curves: Box<CurvePool>,
    transport: Transport,
    metro: Metro,
    events: EventList,
    live: Vec<LiveMsg>,
    capture: LiveCapture,
    /// Position of the current control tick in the call, and the call length.
    call_pos: usize,
    call_len: usize,
    /// Set by `process`; `render` runs without the live tier.
    live_on: bool,
    tick_phase: usize,

    budget: f32,
    budget_fixed: u32,
    overloads: u32,
    load_status: i32,
    selected: usize,
    blocks: u32,
    /// Samples processed since init (process and render both add to it).
    clock: u64,
    peak: (f32, f32),
    dc: (DcBlocker, DcBlocker),
}

#[inline]
fn soft_clip(x: f32) -> f32 {
    // Pade approximation of tanh, exactly 1 at |x| = 3.
    let x = x.clamp(-3.0, 3.0);
    (x * (27.0 + x * x) / (27.0 + 9.0 * x * x)).clamp(-1.0, 1.0)
}

impl Engine {
    pub fn new(sample_rate: f32, max_block: usize) -> Box<Engine> {
        let sr = if sample_rate.is_finite() {
            sample_rate.clamp(8000.0, 192_000.0)
        } else {
            48_000.0
        };
        let max_block = max_block.clamp(1, 1 << 16);
        let mut e = Box::new(Engine {
            sr,
            max_block,
            input: vec![0.0; max_block],
            out_l: vec![0.0; max_block],
            out_r: vec![0.0; max_block],
            control: vec![0; CONTROL_CAP * MSG_BYTES],
            control_count: 0,
            meter: vec![0.0; METER_LEN],
            staging: vec![0; STAGING_BYTES],
            host_load: 0.0,
            table: BufferTable {
                input: 0,
                output_l: 0,
                output_r: 0,
                max_block: 0,
                control: 0,
                control_cap: 0,
                control_count: 0,
                meter: 0,
                meter_len: 0,
                staging: 0,
                staging_cap: 0,
                host_load: 0,
                frames: 0,
                frames_cap: 0,
                frames_count: 0,
            },
            layers: (0..MAX_LAYERS).map(|i| Layer::new(sr, i)).collect(),
            track: Box::new(Track::new()),
            curves: Box::new(CurvePool::new()),
            transport: Transport::new(sr),
            metro: Metro::new(sr),
            events: EventList::new(),
            live: Vec::with_capacity(CONTROL_CAP + 4),
            capture: LiveCapture::new(sr, max_block),
            call_pos: 0,
            call_len: 1,
            live_on: false,
            tick_phase: 0,
            budget: BUDGET_MAX as f32,
            budget_fixed: 0,
            overloads: 0,
            load_status: LOAD_OK,
            selected: 0,
            blocks: 0,
            clock: 0,
            peak: (0.0, 0.0),
            dc: (DcBlocker::new(sr), DcBlocker::new(sr)),
        });
        let addr = |p: *const u8| p as usize as u32;
        e.table = BufferTable {
            input: addr(e.input.as_ptr() as *const u8),
            output_l: addr(e.out_l.as_ptr() as *const u8),
            output_r: addr(e.out_r.as_ptr() as *const u8),
            max_block: max_block as u32,
            control: addr(e.control.as_ptr()),
            control_cap: CONTROL_CAP as u32,
            control_count: addr(&e.control_count as *const u32 as *const u8),
            meter: addr(e.meter.as_ptr() as *const u8),
            meter_len: METER_LEN as u32,
            staging: addr(e.staging.as_ptr()),
            staging_cap: STAGING_BYTES as u32,
            host_load: addr(&e.host_load as *const f32 as *const u8),
            frames: addr(e.capture.frames.as_ptr() as *const u8),
            frames_cap: e.capture.frames_cap as u32,
            frames_count: addr(&e.capture.frames_count as *const u32 as *const u8),
        };
        e.meter[M_LIVE_SLOT] = -1.0;
        e.meter[M_ARMED] = -1.0;
        e
    }

    pub fn table(&self) -> &BufferTable {
        &self.table
    }
    pub fn sample_rate(&self) -> f32 {
        self.sr
    }
    pub fn output(&self) -> (&[f32], &[f32]) {
        (&self.out_l, &self.out_r)
    }
    pub fn meter(&self) -> &[f32] {
        &self.meter
    }
    pub fn input(&self) -> &[f32] {
        &self.input
    }
    pub fn input_mut(&mut self) -> &mut [f32] {
        &mut self.input
    }
    /// Frame records made by the last `process` (CURVE_STRIDE floats each).
    pub fn frame_records(&self) -> &[f32] {
        &self.capture.frames[..self.capture.frames_count as usize * crate::curve::CURVE_STRIDE]
    }
    pub fn set_host_load(&mut self, load: f32) {
        self.host_load = load;
    }

    /// Native helper (tests, tools): queue one control message.
    pub fn push(&mut self, ty: u16, layer: u8, index: u8, param: u16, value: f32, time: u32) {
        let i = self.control_count as usize;
        if i >= CONTROL_CAP {
            return;
        }
        let m = &mut self.control[i * MSG_BYTES..(i + 1) * MSG_BYTES];
        m[0..2].copy_from_slice(&ty.to_le_bytes());
        m[2] = layer;
        m[3] = index;
        m[4..6].copy_from_slice(&param.to_le_bytes());
        m[6..8].copy_from_slice(&0u16.to_le_bytes());
        m[8..12].copy_from_slice(&value.to_le_bytes());
        m[12..16].copy_from_slice(&time.to_le_bytes());
        self.control_count += 1;
    }

    /// Native helper: load a blob from a slice.
    pub fn load_bytes(&mut self, slot: u32, kind: u32, bytes: &[u8]) -> i32 {
        let r = self.load_inner(slot, kind, bytes);
        self.load_status = r;
        self.meter[M_LOAD_STATUS] = r as f32;
        r
    }

    fn load_inner(&mut self, slot: u32, kind: u32, bytes: &[u8]) -> i32 {
        match kind {
            LOAD_TRACK => {
                let uid = |i: usize| {
                    if i < self.track.n_sections() {
                        self.track.section(i).uid
                    } else {
                        0
                    }
                };
                let cur_uid = uid(self.transport.section());
                let jump_uid = self.transport.queued_jump().map_or(0, uid);
                match self.track.load(bytes) {
                    Ok(()) => {
                        self.transport.follow_uids(&self.track, cur_uid, jump_uid);
                        self.transport.track_changed(&self.track);
                        LOAD_OK
                    }
                    Err(crate::seq::LoadError::BadMagic) => LOAD_BAD_MAGIC,
                    Err(crate::seq::LoadError::TooBig) => LOAD_TOO_BIG,
                    Err(crate::seq::LoadError::Malformed) => LOAD_MALFORMED,
                }
            }
            LOAD_LAYER => {
                let s = slot as usize;
                if s >= MAX_LAYERS {
                    return LOAD_BAD_SLOT;
                }
                if bytes.len() != LAYER_WORDS * 4 {
                    return LOAD_MALFORMED;
                }
                let word = |i: usize| {
                    u32::from_le_bytes([
                        bytes[i * 4],
                        bytes[i * 4 + 1],
                        bytes[i * 4 + 2],
                        bytes[i * 4 + 3],
                    ])
                };
                if word(0) != LAYER_MAGIC {
                    return LOAD_BAD_MAGIC;
                }
                let mut w = [0.0f32; LAYER_WORDS];
                for (i, x) in w.iter_mut().enumerate() {
                    *x = f32::from_bits(word(i));
                }
                self.layers[s].load(&w);
                LOAD_OK
            }
            LOAD_GRAIN => {
                let s = slot as usize;
                if s >= MAX_LAYERS {
                    return LOAD_BAD_SLOT;
                }
                let hdr = GRAIN_HEADER_WORDS * 4;
                if bytes.len() < hdr {
                    return LOAD_MALFORMED;
                }
                let word = |i: usize| {
                    u32::from_le_bytes([
                        bytes[i * 4],
                        bytes[i * 4 + 1],
                        bytes[i * 4 + 2],
                        bytes[i * 4 + 3],
                    ])
                };
                if word(0) != GRAIN_MAGIC {
                    return LOAD_BAD_MAGIC;
                }
                let (rate, ref_cents, n) = (
                    f32::from_bits(word(1)),
                    f32::from_bits(word(2)),
                    word(3) as usize,
                );
                if n > GRAIN_MAX_SAMPLES {
                    return LOAD_TOO_BIG;
                }
                if bytes.len() != hdr + n * 4
                    || (n > 0 && !(rate.is_finite() && (1000.0..=384_000.0).contains(&rate)))
                {
                    return LOAD_MALFORMED;
                }
                // NaN means unpitched; any other value is a pitch in cents.
                let ref_cents = if ref_cents.is_finite() {
                    ref_cents.clamp(0.0, 13_000.0)
                } else {
                    f32::NAN
                };
                self.layers[s].load_grain(rate, ref_cents, n, &bytes[hdr..]);
                LOAD_OK
            }
            LOAD_STATES => {
                let s = slot as usize;
                if s >= MAX_LAYERS {
                    return LOAD_BAD_SLOT;
                }
                if bytes.len() < 8 {
                    return LOAD_MALFORMED;
                }
                let word = |i: usize| {
                    u32::from_le_bytes([
                        bytes[i * 4],
                        bytes[i * 4 + 1],
                        bytes[i * 4 + 2],
                        bytes[i * 4 + 3],
                    ])
                };
                if word(0) != STATES_MAGIC {
                    return LOAD_BAD_MAGIC;
                }
                let n = word(1) as usize;
                if n > MAX_STATES {
                    return LOAD_TOO_BIG;
                }
                if bytes.len() != 8 + n * STATE_WORDS * 4 {
                    return LOAD_MALFORMED;
                }
                let mut w = [0.0f32; MAX_STATES * STATE_WORDS];
                for (i, x) in w[..n * STATE_WORDS].iter_mut().enumerate() {
                    *x = f32::from_bits(word(2 + i));
                }
                self.layers[s].load_states(&w[..n * STATE_WORDS], n);
                LOAD_OK
            }
            LOAD_CURVE => match self.curves.load(slot as usize, bytes) {
                Ok(()) => LOAD_OK,
                Err(crate::curve::CurveError::BadSlot) => LOAD_BAD_SLOT,
                Err(crate::curve::CurveError::BadMagic) => LOAD_BAD_MAGIC,
                Err(crate::curve::CurveError::TooBig) => LOAD_TOO_BIG,
                Err(crate::curve::CurveError::Malformed) => LOAD_MALFORMED,
            },
            _ => LOAD_BAD_KIND,
        }
    }

    pub fn seek(&mut self, section: usize, pos: f64) {
        for l in self.layers.iter_mut() {
            l.release_sequenced();
        }
        self.transport.seek(&self.track, section, pos);
    }

    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    fn drain_control(&mut self, frames: usize) {
        let n = (self.control_count as usize).min(CONTROL_CAP);
        self.control_count = 0;
        self.live.clear();
        for i in 0..n {
            let m = &self.control[i * MSG_BYTES..(i + 1) * MSG_BYTES];
            let ty = u16::from_le_bytes([m[0], m[1]]);
            let layer = m[2] as usize;
            let index = m[3];
            let param = u16::from_le_bytes([m[4], m[5]]);
            let value = f32::from_le_bytes([m[8], m[9], m[10], m[11]]);
            let time = u32::from_le_bytes([m[12], m[13], m[14], m[15]])
                .min(frames.saturating_sub(1) as u32);
            let lay = layer < MAX_LAYERS;
            match ty {
                MSG_SET_PARAM if lay => self.layers[layer].set_param(index, param as usize, value),
                MSG_SET_MACRO if lay => self.layers[layer].set_macro(index as usize, value),
                MSG_SET_LAYER if lay => self.layers[layer].set_field(param, value),
                MSG_NOTE_ON | MSG_NOTE_OFF if lay => {
                    let velocity = if param == 0 {
                        1.0
                    } else {
                        (param as f32 / 1000.0).min(1.0)
                    };
                    self.live.push(LiveMsg {
                        time,
                        on: ty == MSG_NOTE_ON,
                        layer: layer as u8,
                        slot: index,
                        cents: value,
                        velocity,
                        follow: 0,
                        hard: 0.0,
                    });
                }
                MSG_ARM if lay => {
                    let was = self.capture.armed;
                    let now = if value > 0.5 {
                        layer as i32
                    } else if was == layer as i32 {
                        -1
                    } else {
                        was
                    };
                    if now != was {
                        if was >= 0 {
                            self.layers[was as usize].follow_release();
                        }
                        self.capture.arm(now);
                    }
                }
                MSG_SET_TUNE if lay => self.capture.set_tune(layer, param, value),
                MSG_SET_LIVE_KIT if lay => {
                    self.capture.set_kit(layer, index as usize, param, value)
                }
                MSG_RESET_SINGER => self.capture.reset_singer(),
                MSG_SET_METRO => self.metro.set(param, value),
                MSG_COUNT_IN => {
                    if !self.transport.playing() && !self.metro.counting() {
                        let bpm = if value.is_finite() && value >= 20.0 {
                            value
                        } else {
                            let b = self.transport.bpm();
                            if b >= 20.0 { b as f32 } else { 100.0 }
                        };
                        self.metro.start_count(bpm as f64);
                    }
                }
                MSG_PLAY => {
                    self.metro.cancel();
                    self.transport.play();
                }
                MSG_STOP => {
                    self.metro.cancel();
                    self.transport.stop();
                    for l in self.layers.iter_mut() {
                        l.release_sequenced();
                    }
                }
                MSG_JUMP => self.transport.queue_jump(index as usize),
                MSG_SET_BUDGET if value.is_finite() => {
                    self.budget_fixed = (value.max(0.0) as u32)
                        .min(MAX_LAYERS as u32 * VOICES_PER_LAYER as u32 * MAX_PARTIALS as u32);
                }
                MSG_SELECT_LAYER if lay => self.selected = layer,
                MSG_PANIC => {
                    for l in self.layers.iter_mut() {
                        l.panic();
                    }
                }
                _ => {}
            }
        }
        // Insertion sort: stable (keeps ring order for equal times) and,
        // unlike slice::sort, never allocates. Usually 0 to 2 entries.
        for i in 1..self.live.len() {
            let mut j = i;
            while j > 0 && self.live[j - 1].time > self.live[j].time {
                self.live.swap(j - 1, j);
                j -= 1;
            }
        }
    }

    /// Live: one render quantum with mic input in `input[..frames]`. Runs the
    /// live tier, then renders with the armed layer driven by what it heard.
    pub fn process(&mut self, frames: usize) {
        let frames = frames.min(self.max_block);
        self.adapt_budget();
        self.drain_control(frames);
        self.live_on = true;
        let armed = self.capture.armed;
        let beat = armed >= 0 && self.layers[armed as usize].kind == KIND_BEAT;
        let action = self.capture.process(&mut self.input[..frames], beat);
        if armed >= 0 {
            let msg = match action {
                Action::None => None,
                Action::Start {
                    slot,
                    velocity,
                    hard,
                } => Some((1, slot, velocity, hard)),
                Action::Release => Some((2, 0, 0.0, 0.0)),
            };
            if let Some((follow, slot, velocity, hard)) = msg {
                if self.live.len() < self.live.capacity() {
                    // Time 0: the sound starts in this quantum.
                    self.live.insert(
                        0,
                        LiveMsg {
                            time: 0,
                            on: true,
                            layer: armed as u8,
                            slot,
                            cents: f32::NAN,
                            velocity,
                            follow,
                            hard,
                        },
                    );
                }
            }
        }
        self.run_block(frames);
        self.capture.write_meters(&mut self.meter);
    }

    /// Offline: any length up to max_block, fixed budget, no input, no live tier.
    pub fn render(&mut self, frames: usize) -> usize {
        self.host_load = 0.0;
        let frames = frames.min(self.max_block);
        self.drain_control(frames);
        self.live_on = false;
        self.capture.frames_count = 0;
        self.run_block(frames)
    }

    fn adapt_budget(&mut self) {
        let load = self.host_load;
        if self.budget_fixed > 0 || !(load > 0.0) || !load.is_finite() {
            return;
        }
        if load > 0.8 {
            self.overloads += 1;
            self.budget = (self.budget * 0.85).max(BUDGET_MIN as f32);
            if self.budget <= BUDGET_MIN as f32 && load > 0.95 {
                self.steal_quietest();
            }
        } else if load < 0.6 {
            self.budget = (self.budget + 8.0).min(BUDGET_MAX as f32);
        }
    }

    /// Overload, last resort: choke the quietest voice (released ones first).
    fn steal_quietest(&mut self) {
        let mut best: Option<(bool, f32, usize, usize)> = None;
        for (li, l) in self.layers.iter().enumerate() {
            for (vi, v) in l.voices.iter().enumerate() {
                if !v.active() {
                    continue;
                }
                let key = (!v.released(), v.loudness(), li, vi);
                if best.is_none_or(|b| (key.0, key.1) < (b.0, b.1)) {
                    best = Some(key);
                }
            }
        }
        if let Some((_, _, li, vi)) = best {
            self.layers[li].voices[vi].choke();
            self.layers[li].steals += 1;
        }
    }

    fn run_block(&mut self, frames: usize) -> usize {
        self.call_len = frames.max(1);
        self.call_pos = 0;
        self.out_l[..frames].fill(0.0);
        self.out_r[..frames].fill(0.0);
        for l in self.layers.iter_mut() {
            l.peak = 0.0;
        }
        let mut live_i = 0;
        let mut pos = 0;
        while pos < frames {
            if self.tick_phase == 0 {
                self.call_pos = pos;
                self.tick();
            }
            let mut span = (TICK - self.tick_phase).min(frames - pos);

            // Count-in runs with the transport stopped. Clicks are mixed here;
            // the downbeat sample is left for the transport so the song and
            // the accented click start on the same sample.
            if self.live_on && self.metro.counting() {
                let (n, start) = self.metro.advance_count(span);
                if n > 0 {
                    self.render_count_span(pos, n);
                    self.tick_phase = (self.tick_phase + n) % TICK;
                    pos += n;
                }
                if !start {
                    continue;
                }
                self.transport.play();
                if pos >= frames {
                    break;
                }
                if n > 0 && self.tick_phase == 0 {
                    self.call_pos = pos;
                    self.tick();
                }
                span = (TICK - self.tick_phase).min(frames - pos);
                if span == 0 {
                    continue;
                }
            }

            self.events.clear();
            self.transport.advance(&self.track, span, &mut self.events);
            let beats_on = self.live_on && self.metro.on;
            let n_beats = if beats_on { self.transport.n_beats } else { 0 };
            let beat_off = self.transport.beat_off;
            let beat_index = self.transport.beat_index;
            let mut beat_i = 0;
            let mut ev_i = 0;
            let mut sub: usize = 0;
            loop {
                let next_seq = self
                    .events
                    .as_slice()
                    .get(ev_i)
                    .map_or(span, |e| e.offset as usize);
                let next_live = self
                    .live
                    .get(live_i)
                    .map_or(span, |m| (m.time as usize).saturating_sub(pos).min(span));
                let next_beat = if beat_i < n_beats {
                    beat_off[beat_i] as usize
                } else {
                    span
                };
                let next_off = self
                    .layers
                    .iter()
                    .map(|l| l.next_off())
                    .min()
                    .unwrap_or(u32::MAX) as usize;
                let stop = next_seq
                    .min(next_live)
                    .min(next_beat)
                    .min(sub.saturating_add(next_off))
                    .min(span)
                    .max(sub);
                if stop > sub {
                    self.paint(pos + sub, pos + stop);
                    sub = stop;
                }
                if sub >= span {
                    break;
                }
                let len = TICK - (self.tick_phase + sub);
                if beat_i < n_beats && beat_off[beat_i] as usize == sub {
                    let bar = self.metro.bar.max(1) as i32;
                    let accent = beat_index[beat_i].rem_euclid(bar) == 0;
                    self.fire_click(accent, false, len);
                    beat_i += 1;
                }
                while let Some(m) = self.live.get(live_i).copied() {
                    if (m.time as usize).saturating_sub(pos) > sub {
                        break;
                    }
                    live_i += 1;
                    if m.follow != 0 {
                        self.follow_msg(&m, len);
                        continue;
                    }
                    let l = &mut self.layers[m.layer as usize];
                    if m.on {
                        l.note_on_live(
                            m.slot as usize,
                            m.cents,
                            m.velocity,
                            &self.track,
                            &self.curves,
                            len,
                        );
                    } else {
                        l.note_off_live(m.cents);
                    }
                }
                while let Some(e) = self.events.as_slice().get(ev_i).copied() {
                    if e.offset as usize > sub {
                        break;
                    }
                    ev_i += 1;
                    let ev = *self.track.event(e.event as usize);
                    if let Some(l) = self.layers.get_mut(e.layer as usize) {
                        l.note_from_event(
                            &self.track,
                            &self.curves,
                            e.clip,
                            &ev,
                            e.dur_samples,
                            len,
                        );
                    }
                }
            }
            self.tick_phase = (self.tick_phase + span) % TICK;
            pos += span;
        }
        let (mut pl, mut pr) = (0.0f32, 0.0f32);
        for i in 0..frames {
            let l = soft_clip(self.dc.0.process(self.out_l[i]));
            let r = soft_clip(self.dc.1.process(self.out_r[i]));
            pl = pl.max(l.abs());
            pr = pr.max(r.abs());
            self.out_l[i] = l;
            self.out_r[i] = r;
        }
        self.peak = (pl, pr);
        self.clock += frames as u64;
        self.write_meters();
        frames
    }

    /// Live follow start or release on the armed layer, now.
    fn follow_msg(&mut self, m: &LiveMsg, len: usize) {
        let layer = m.layer as usize;
        if self.capture.armed != m.layer as i32 {
            return;
        }
        if m.follow == 2 {
            self.layers[layer].follow_release();
            return;
        }
        self.push_follow_record(self.call_pos);
        let l = &mut self.layers[layer];
        l.follow_start(
            m.slot as usize,
            m.velocity,
            m.hard,
            &self.track,
            &self.curves,
            len,
        );
    }

    /// Give the armed layer the live frame for the tick starting at `pos`.
    fn push_follow_record(&mut self, pos: usize) {
        if !self.live_on || self.capture.armed < 0 {
            return;
        }
        let at = (pos + TICK / 2) as f32 / self.call_len as f32;
        let (rec, gain) = self.capture.record(at);
        self.layers[self.capture.armed as usize].follow_set(&rec, gain);
    }

    fn render_span(&mut self, a: usize, b: usize) {
        let (l, r) = (&mut self.out_l[a..b], &mut self.out_r[a..b]);
        for layer in self.layers.iter_mut() {
            layer.render(l, r);
        }
    }

    /// Render layers for `[a, b)` and mix a ringing tick on top. The tick is
    /// live-only; `render` leaves `live_on` false so exports stay clean.
    fn paint(&mut self, a: usize, b: usize) {
        if b <= a {
            return;
        }
        self.render_span(a, b);
        if self.live_on {
            self.metro.mix(&mut self.out_l[a..b], &mut self.out_r[a..b]);
        }
        for l in self.layers.iter_mut() {
            l.count_offs((b - a) as u32);
        }
    }

    /// Count-in clicks in a span of `n` samples starting at `pos`. The click
    /// list was filled by `advance_count` and is not a transport beat.
    fn render_count_span(&mut self, pos: usize, n: usize) {
        let n_clicks = self.metro.n_clicks;
        let clicks = self.metro.clicks;
        let mut s = 0usize;
        for i in 0..n_clicks {
            let at = clicks[i].offset as usize;
            if at > n {
                break;
            }
            if at > s {
                self.paint(pos + s, pos + at);
            }
            let len = (TICK - self.tick_phase).saturating_sub(at).max(1);
            self.fire_click(clicks[i].accent, true, len);
            s = at;
        }
        if n > s {
            self.paint(pos + s, pos + n);
        }
    }

    /// A click. A kit voice is a real note on the chosen beat layer, so it
    /// uses that slot's sound. Anything else (or a muted layer) is a plain tick.
    /// Neither path writes the mic buffer.
    fn fire_click(&mut self, accent: bool, count_in: bool, len: usize) {
        if self.metro.kit {
            let li = self.metro.kit_layer;
            if let Some(l) = self.layers.get_mut(li) {
                if l.kind == KIND_BEAT && !l.mute {
                    let slot = self.metro.kit_slot;
                    let vel = if accent { 1.0 } else { 0.7 };
                    l.note_on_live(slot, f32::NAN, vel, &self.track, &self.curves, len.max(1));
                    return;
                }
            }
        }
        self.metro.trigger(accent, count_in);
    }

    /// Control tick: resolution and budget for every layer, then every voice.
    fn tick(&mut self) {
        self.update_morph();
        for l in self.layers.iter_mut() {
            l.control_tick();
        }
        if self.live_on {
            self.capture.advance_tick();
            self.push_follow_record(self.call_pos);
        }
        self.share_budget();
        let ups = self.transport.units_per_sample();
        for l in self.layers.iter_mut() {
            l.voices_tick(&self.track, &self.curves, ups);
        }
    }

    /// Resolution step 1: tell each layer with states which two to blend,
    /// from the morph lane of its clip in the current section. No clip or no
    /// marker: state 0.
    fn update_morph(&mut self) {
        let section = self.transport.section();
        let pos = self.transport.pos();
        let ups = self.transport.units_per_sample();
        let ms_per_unit = if ups > 0.0 {
            1000.0 / (ups * self.sr as f64)
        } else {
            0.0
        };
        for (li, l) in self.layers.iter_mut().enumerate() {
            if !l.has_states() {
                continue;
            }
            let b = self
                .track
                .clip_index_for(section, li)
                .and_then(|ci| self.track.morph_blend(ci, pos, ms_per_unit));
            match b {
                Some(b) => l.set_morph(b.from, b.to, b.x),
                None => l.set_morph(0, 0, 0.0),
            }
        }
    }

    pub fn budget(&self) -> u32 {
        if self.budget_fixed > 0 {
            self.budget_fixed
        } else {
            self.budget as u32
        }
    }

    /// Share the active-partial budget among sounding voices by loudness
    /// (water-filling: no voice gets more than it wants, the rest is shared
    /// among the others). Uses each voice's wanted count from the last tick.
    fn share_budget(&mut self) {
        let budget = self.budget() as f32;
        const N: usize = MAX_LAYERS * VOICES_PER_LAYER;
        let mut want = [0.0f32; N];
        let mut weight = [0.0f32; N];
        let mut total_want = 0.0;
        let mut n_active = 0;
        for (li, l) in self.layers.iter().enumerate() {
            let g = dsp::db_to_lin(l.level_db);
            for (vi, v) in l.voices.iter().enumerate() {
                if v.active() {
                    let k = li * VOICES_PER_LAYER + vi;
                    want[k] = v.wanted_partials().max(1) as f32;
                    weight[k] = (v.loudness() * g).max(1e-6);
                    total_want += want[k];
                    n_active += 1;
                }
            }
        }
        let idle_share = if total_want <= budget {
            MAX_PARTIALS
        } else {
            ((budget / (n_active + 1) as f32) as usize).clamp(2, MAX_PARTIALS)
        };
        if total_want <= budget {
            for l in self.layers.iter_mut() {
                l.max_partials = [MAX_PARTIALS; VOICES_PER_LAYER];
            }
            return;
        }
        let mut give = [0.0f32; N];
        let mut left = budget;
        let mut open = [false; N];
        for k in 0..N {
            open[k] = want[k] > 0.0;
        }
        for _ in 0..6 {
            let wsum: f32 = (0..N).filter(|&k| open[k]).map(|k| weight[k]).sum();
            if wsum <= 0.0 || left < 1.0 {
                break;
            }
            let mut used = 0.0;
            for k in 0..N {
                if !open[k] {
                    continue;
                }
                let share = left * weight[k] / wsum;
                let g = (give[k] + share).min(want[k]);
                used += g - give[k];
                give[k] = g;
                if give[k] >= want[k] {
                    open[k] = false;
                }
            }
            left -= used;
        }
        for (li, l) in self.layers.iter_mut().enumerate() {
            for vi in 0..VOICES_PER_LAYER {
                let k = li * VOICES_PER_LAYER + vi;
                l.max_partials[vi] = if want[k] > 0.0 {
                    (give[k].floor() as usize).clamp(2, MAX_PARTIALS)
                } else {
                    idle_share
                };
            }
        }
    }

    fn write_meters(&mut self) {
        self.blocks = self.blocks.wrapping_add(1);
        let m = &mut self.meter;
        m[M_BLOCKS] = self.blocks as f32;
        m[M_PLAYING] = self.transport.playing() as u8 as f32;
        m[M_SECTION] = self.transport.section() as f32;
        m[M_SECTION_POS] = self.transport.pos() as f32;
        m[M_REPEAT] = self.transport.repeat() as f32;
        m[M_BPM] = self.transport.bpm() as f32;
        m[M_CLOCK_LO] = (self.clock & ((1 << 20) - 1)) as f32;
        m[M_CLOCK_HI] = (self.clock >> 20) as f32;
        m[M_UNITS_PER_SAMPLE] = if self.transport.playing() {
            self.transport.units_per_sample() as f32
        } else {
            0.0
        };
        m[M_METRO] = if self.metro.on { 1.0 } else { 0.0 };
        m[M_COUNTIN] = self.metro.remaining() as f32;
        m[M_COUNT_SEQ] = self.metro.count_seq as f32;
        m[M_METRO_BEAT] = if self.metro.counting() {
            self.metro.shown_beat() as f32
        } else if self.transport.playing() && !self.transport.is_free() {
            let beat = ((self.transport.pos() + 1e-6) / 960.0).floor() as i32;
            beat.rem_euclid(self.metro.bar.max(1) as i32) as f32
        } else {
            -1.0
        };
        m[M_LOAD] = self.host_load;
        m[M_BUDGET] = if self.budget_fixed > 0 {
            self.budget_fixed as f32
        } else {
            self.budget.floor()
        };
        let (mut parts, mut voices, mut steals) = (0usize, 0usize, 0u32);
        for (i, l) in self.layers.iter().enumerate() {
            let n = l.sounding();
            voices += n;
            steals += l.steals;
            parts += l
                .voices
                .iter()
                .filter(|v| v.active())
                .map(|v| v.active_partials())
                .sum::<usize>();
            m[M_LAYER_PEAK + i] = l.peak;
            m[M_LAYER_VOICES + i] = n as f32;
        }
        m[M_ACTIVE_PARTIALS] = parts as f32;
        m[M_ACTIVE_VOICES] = voices as f32;
        m[M_STEALS] = steals as f32;
        m[M_OVERLOADS] = self.overloads as f32;
        m[M_LOAD_STATUS] = self.load_status as f32;
        m[M_PEAK_L] = self.peak.0;
        m[M_PEAK_R] = self.peak.1;
        m[M_SELECTED] = self.selected as f32;
        let l = &self.layers[self.selected];
        m[M_STORED..M_STORED + P::COUNT].copy_from_slice(l.stored());
        m[M_LIVE..M_LIVE + P::COUNT].copy_from_slice(&l.live);
        let mut u = [0.0f32; N_MACROS];
        P::project_macros(l.stored(), &l.live, &l.macros, &mut u);
        m[M_MACRO_LIVE..M_MACRO_LIVE + N_MACROS].copy_from_slice(&u);
    }
}

// ---------------------------------------------------------------- C ABI (TDD "Engine ABI")

#[unsafe(no_mangle)]
pub extern "C" fn ew_init(sample_rate: f32, max_block: u32) -> *mut Engine {
    Box::into_raw(Engine::new(sample_rate, max_block as usize))
}

/// # Safety
/// `e` must come from `ew_init`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ew_buffers(e: *mut Engine) -> *const BufferTable {
    unsafe { &(*e).table }
}

/// # Safety
/// `e` must come from `ew_init`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ew_process(e: *mut Engine, frames: u32) {
    unsafe { (*e).process(frames as usize) }
}

/// # Safety
/// `e` must come from `ew_init`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ew_render(e: *mut Engine, frames: u32) -> u32 {
    unsafe { (*e).render(frames as usize) as u32 }
}

/// # Safety
/// `e` must come from `ew_init`; `ptr..ptr+len` must be readable (normally
/// the staging area from `ew_buffers`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ew_load(
    e: *mut Engine,
    slot: u32,
    kind: u32,
    ptr: *const u8,
    len: u32,
) -> i32 {
    unsafe {
        let bytes = if ptr.is_null() || len == 0 {
            &[][..]
        } else {
            std::slice::from_raw_parts(ptr, len as usize)
        };
        (*e).load_bytes(slot, kind, bytes)
    }
}

/// # Safety
/// `e` must come from `ew_init`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ew_seek(e: *mut Engine, section: u32, ticks: f64) {
    unsafe { (*e).seek(section as usize, ticks) }
}

/// # Safety
/// `e` must come from `ew_init` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ew_free(e: *mut Engine) {
    if !e.is_null() {
        drop(unsafe { Box::from_raw(e) });
    }
}
