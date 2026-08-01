/**
 * Where the pointer is, normalised, for anything that wants to lean towards it.
 *
 * One listener for the whole page rather than one per effect, and it publishes a raw
 * target rather than a smoothed value. Smoothing belongs to the consumer: the hero drifts
 * slowly and a tilting panel snaps, and a single shared easing would make one of them
 * wrong. Each reads this and eases at its own rate inside its own frame loop.
 *
 * Zero means centred, which is also what a touch device and a page nobody has moved over
 * both report — so every effect degrades to "no parallax" rather than to a jump.
 */

const target = { x: 0, y: 0 };
let listening = false;

function onMove(event: PointerEvent): void {
  target.x = (event.clientX / window.innerWidth) * 2 - 1;
  target.y = (event.clientY / window.innerHeight) * 2 - 1;
}

function onLeave(): void {
  target.x = 0;
  target.y = 0;
}

/**
 * The pointer's position, from -1 to 1 on each axis.
 *
 * Returns a live object rather than a copy — it is read once per frame by things already
 * running a loop, and allocating a pair of numbers sixty times a second per effect is
 * garbage for no reason.
 */
export function pointerTarget(): Readonly<{ x: number; y: number }> {
  if (!listening) {
    listening = true;
    // Passive: this never calls preventDefault, and saying so keeps it off the path that
    // can delay a scroll.
    window.addEventListener("pointermove", onMove, { passive: true });
    window.addEventListener("pointerleave", onLeave, { passive: true });
    // A pointer that never moves again should not leave the page leaning. Touch fires
    // this immediately after a tap, which is what keeps phones centred.
    window.addEventListener("pointercancel", onLeave, { passive: true });
  }
  return target;
}

/** Moves `current` a fraction of the way to `to`. Frame-rate independent enough at 60–144Hz. */
export function ease(current: number, to: number, rate: number): number {
  return current + (to - current) * rate;
}

/**
 * How far through the viewport an element is, from 0 as it enters to 1 as it leaves.
 *
 * Used for scroll parallax. Clamped, so an element far above or below the fold does not
 * drive an effect to an extreme it was never designed to reach.
 */
export function viewProgress(element: Element): number {
  const box = element.getBoundingClientRect();
  const span = window.innerHeight + box.height;
  if (span === 0) return 0;
  return Math.min(1, Math.max(0, (window.innerHeight - box.top) / span));
}
