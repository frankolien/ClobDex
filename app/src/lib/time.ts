/**
 * Slots, and the one place this application pretends they are clock time.
 *
 * The indexer is slot-native throughout, deliberately: a trade carries a slot, and
 * mapping slots to timestamps means trusting the cluster's block times, which drift and
 * are occasionally revised. `clob-stream`'s candle module puts it plainly — a candle whose
 * boundary moves is worse than one measured in a slightly odd unit, and callers that want
 * minutes should convert at the edge, where the fudge is visible.
 *
 * This module is that edge, and this comment is that visibility. Everything here is an
 * approximation built on one assumption:
 *
 * > slots arrive every {@link SLOT_MS} milliseconds.
 *
 * They do not, exactly. Slots are skipped, leaders vary, and the cluster runs slightly
 * behind its target under load. So:
 *
 * - **Fine for**: chart axis labels, "3 minutes ago", choosing a window to request.
 * - **Not fine for**: anything a person would reconcile against an exchange statement, or
 *   any two timestamps subtracted to produce a rate.
 *
 * Nothing sent to the chain and nothing stored passes through here. The worst a mistake
 * can do is mislabel an axis.
 */

/** The cluster's target slot time. */
export const SLOT_MS = 400;

/** Slots in roughly twenty-four hours, matching the indexer's own window default. */
export const SLOTS_PER_DAY = 216_000;

/** Slots in roughly one hour. */
export const SLOTS_PER_HOUR = 9_000;

/**
 * A known correspondence between a slot and a wall clock.
 *
 * Every conversion is relative to one of these rather than to an absolute epoch, so the
 * error is proportional to the distance from the anchor instead of to the age of the
 * chain. Anchor on the newest slot seen and a chart's right-hand edge is right even
 * though its left-hand edge drifts.
 */
export interface Anchor {
  readonly slot: number;
  readonly unixMs: number;
}

/**
 * Pins the slot you just saw to the moment you saw it.
 *
 * The clock is the caller's, so a test supplies its own and this module holds no dependency
 * on the current time.
 */
export function anchorAt(slot: number, unixMs: number): Anchor {
  return { slot, unixMs };
}

/** Approximate wall-clock time of a slot, in milliseconds. */
export function timeOf(anchor: Anchor, slot: number): number {
  return anchor.unixMs + (slot - anchor.slot) * SLOT_MS;
}

/** Approximate wall-clock time of a slot, in whole seconds — what a chart's axis takes. */
export function secondsOf(anchor: Anchor, slot: number): number {
  return Math.floor(timeOf(anchor, slot) / 1000);
}

/** The slot nearest a wall-clock time. Inverse of {@link timeOf}, with the same error. */
export function slotOf(anchor: Anchor, unixMs: number): number {
  return anchor.slot + Math.round((unixMs - anchor.unixMs) / SLOT_MS);
}

/** How many slots span a duration. At least one, so a window is never empty. */
export function slotsIn(milliseconds: number): number {
  return Math.max(1, Math.round(milliseconds / SLOT_MS));
}

/**
 * How long ago a slot was, in words.
 *
 * Deliberately coarse. Rendering "1.4s ago" from an approximation states a precision the
 * approximation does not have; "just now" is both friendlier and more honest.
 */
export function ago(anchor: Anchor, slot: number, nowMs: number): string {
  const seconds = Math.max(0, Math.round((nowMs - timeOf(anchor, slot)) / 1000));
  if (seconds < 5) return "just now";
  if (seconds < 60) return `${seconds}s ago`;

  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;

  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}
