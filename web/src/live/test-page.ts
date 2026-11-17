// Listening-test checklist for Live follow. Answers live in localStorage.

import { download } from '../common/wav';

// Inside live-session.html (?embed=1): hide the links to the other page.
if (new URLSearchParams(location.search).has('embed')) document.documentElement.classList.add('embedded');

interface Step { id: string; title: string; do: string; listen: string; rate?: string }
const STEPS: Step[] = [
  { id: 'vowels', title: 'Sustained vowels', do: 'Mouth depth 100%. Sing "aaa, eee, ooo" on one note, holding each.', listen: 'Do the three vowels sound different?', rate: '1 = identical, 5 = clearly different' },
  { id: 'speech', title: 'Spoken phrase, 16 then 24 bands', do: 'Mouth depth 100%. Say a short sentence with 16 bands, then again with 24.', listen: 'Can you make out the words? Which band count is better? Say which one in the note.', rate: '1 = unintelligible, 5 = clear (rate the better one)' },
  { id: 'swell', title: 'Swell', do: 'Hum "mmmwaaa", starting closed and getting louder.', listen: 'Does it get louder and brighter as you open?', rate: '1 = no change, 5 = follows well' },
  { id: 'snap', title: 'Scale: robotic vs natural', do: 'Sing up a scale twice. First Snap 100%, Tune speed 0. Then Snap 50%, Tune speed 120 ms.', listen: 'First should sound like robotic steps, second more natural. Is the difference clear and useful?', rate: '1 = no difference, 5 = both modes useful' },
  { id: 'slides', title: 'Slides', do: 'Snap 100%. Slide between two notes with Slides set to glide, then gliss, then jump.', listen: 'Does each do what its name says? Which would you keep?', rate: '1 = broken, 5 = all three work' },
  { id: 'staccato', title: 'Staccato', do: 'Sing "ta ta ta ta" at a steady pace.', listen: 'Does each note start right when you sing it?', rate: '1 = laggy or mushy, 5 = immediate' },
  { id: 'silence', title: 'Silence and breath', do: 'Stop singing. Breathe quietly. Wait a few seconds.', listen: 'Is silence silent? Any chatter, clicks or stuck notes?', rate: '1 = chattering, 5 = silent' },
];

const KEY = 'earworm-live-test-v1';
type Answers = Record<string, string>;
let answers: Answers = {};
try { answers = JSON.parse(localStorage.getItem(KEY) || '{}'); } catch { answers = {}; }
const save = () => { try { localStorage.setItem(KEY, JSON.stringify(answers)); } catch { /* storage blocked: answers stay in memory */ } };

function radios(name: string, values: string[], labels?: string[]): HTMLFieldSetElement {
  const fs = document.createElement('fieldset');
  const lg = document.createElement('legend');
  lg.className = 'small muted';
  fs.appendChild(lg);
  values.forEach((v, i) => {
    const l = document.createElement('label');
    const r = document.createElement('input');
    r.type = 'radio'; r.name = name; r.value = v;
    r.checked = answers[name] === v;
    r.addEventListener('change', () => { answers[name] = v; save(); });
    l.append(r, ' ' + (labels?.[i] ?? v));
    fs.appendChild(l);
  });
  return fs;
}

function note(name: string): HTMLElement {
  const wrap = document.createElement('div');
  const l = document.createElement('label');
  l.className = 'small muted';
  l.textContent = 'Note';
  const t = document.createElement('textarea');
  t.id = name;
  l.htmlFor = name;
  t.value = answers[name] ?? '';
  t.addEventListener('input', () => { answers[name] = t.value; save(); });
  wrap.append(l, t);
  return wrap;
}

const root = document.getElementById('steps')!;
STEPS.forEach((s, i) => {
  const p = document.createElement('section');
  p.className = 'panel step';
  const h = document.createElement('h2');
  h.textContent = `${i + 1}. ${s.title}`;
  const d = document.createElement('p');
  d.textContent = s.do;
  const q = document.createElement('p');
  q.className = 'muted';
  q.textContent = 'Listen for: ' + s.listen;
  const r = radios(s.id + '.rating', ['1', '2', '3', '4', '5']);
  r.querySelector('legend')!.textContent = 'Rating: ' + (s.rate ?? '1 to 5');
  p.append(h, d, q, r, note(s.id + '.note'));
  root.appendChild(p);
});

const fin = document.createElement('section');
fin.className = 'panel step';
const fh = document.createElement('h2');
fh.textContent = `${STEPS.length + 1}. Overall`;
const o = radios('overall.rating', ['1', '2', '3', '4', '5']);
o.querySelector('legend')!.textContent = 'Does it feel like playing an instrument? 1 = no, 5 = yes';
const dl = radios('overall.delay', ['none', 'slight', 'distracting']);
dl.querySelector('legend')!.textContent = 'Felt delay';
fin.append(fh, o, dl, note('overall.note'));
root.appendChild(fin);

document.getElementById('download')!.addEventListener('click', () => {
  const out = { date: new Date().toISOString(), userAgent: navigator.userAgent, answers };
  download(new Blob([JSON.stringify(out, null, 2)], { type: 'application/json' }), 'earworm-live-test.json');
});
document.getElementById('clear')!.addEventListener('click', () => {
  if (!confirm('Clear all answers?')) return;
  answers = {};
  save();
  location.reload();
});
