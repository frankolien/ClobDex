/**
 * Scroll reveals, and nothing else.
 *
 * An IntersectionObserver rather than a scroll listener: the browser decides when an
 * element crosses the threshold, off the main thread, instead of this file recomputing
 * geometry on every frame of a scroll.
 *
 * Each element is unobserved once it has appeared. A reveal that plays again on the way
 * back up is a page that will not stay still while it is being read.
 */

/** How far into the viewport an element must come before it counts as seen. */
const MARGIN = "0px 0px -12% 0px";

export function observeReveals(): void {
  const targets = document.querySelectorAll<HTMLElement>(".reveal");
  if (targets.length === 0) return;

  // No observer, or motion the visitor asked not to see: show everything immediately.
  // Failing towards visible is the only acceptable direction — the alternative is a page
  // that hides its own content because a feature was missing.
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (reduced || !("IntersectionObserver" in window)) {
    for (const target of targets) target.classList.add("in");
    return;
  }

  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        entry.target.classList.add("in");
        observer.unobserve(entry.target);
      }
    },
    { rootMargin: MARGIN, threshold: 0.01 },
  );

  for (const target of targets) observer.observe(target);
}
