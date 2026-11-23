// Help bubbles (docs/ui.md "Design language"): explanatory text sits behind a small "?" button
// instead of on the page. A bubble is a native popover, so Esc and a click outside close it and
// the keyboard reaches it; it is placed beside its button by hand, which works in every browser
// that has popovers, with or without CSS anchor positioning.

let count = 0;

/** A "?" button and its bubble. `about` names what it explains, for the button's accessible name. */
export function helpButton(text: string, about: string): HTMLElement {
  const id = `help-${++count}`;
  const wrap = document.createElement('span');
  wrap.className = 'help-wrap';
  const b = document.createElement('button');
  b.type = 'button';
  b.className = 'help';
  b.textContent = '?';
  b.setAttribute('aria-label', `About ${about}`);
  b.setAttribute('popovertarget', id);
  const bubble = document.createElement('div');
  bubble.id = id;
  bubble.className = 'bubble';
  bubble.setAttribute('popover', '');
  bubble.textContent = text;
  wrap.append(b, bubble);
  return wrap;
}

/** Put an open bubble next to the button that opened it, inside the window. */
function place(bubble: HTMLElement): void {
  const button = document.querySelector<HTMLElement>(`[popovertarget="${bubble.id}"]`);
  if (!button) return;
  const r = button.getBoundingClientRect();
  const w = Math.min(bubble.offsetWidth, window.innerWidth - 16);
  const h = bubble.offsetHeight;
  const left = Math.max(8, Math.min(r.left + r.width / 2 - w / 2, window.innerWidth - w - 8));
  // Below the button if it fits, else above it; either way kept inside the window.
  const below = r.bottom + 8;
  const want = below + h > window.innerHeight - 8 && r.top - h - 8 > 8 ? r.top - h - 8 : below;
  const top = Math.max(8, Math.min(want, window.innerHeight - h - 8));
  bubble.style.left = `${Math.round(left)}px`;
  bubble.style.top = `${Math.round(top)}px`;
}

/** Placement for every bubble on the page, those in the HTML and those built later. */
export function placeBubbles(): void {
  document.addEventListener('toggle', (e) => {
    const t = e.target;
    if (t instanceof HTMLElement && t.classList.contains('bubble') && (e as ToggleEvent).newState === 'open') place(t);
  }, true);
}
