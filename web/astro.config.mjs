import { defineConfig } from "astro/config";

/**
 * The marketing site.
 *
 * Static output, no adapter, no server. Everything here is content, so the whole site is
 * files on a CDN — which also means it can be pinned somewhere it outlives whoever is
 * paying for hosting. The trading app is a separate deploy for the opposite reason: it is
 * entirely live state and could not be rendered ahead of time if it wanted to.
 *
 * Astro ships zero JavaScript unless a component asks for it. Two things do — the hero
 * canvas and the scroll reveals — which together come to under three kilobytes, inlined.
 * The alternative is a framework whose job would be rendering text that never changes.
 */
export default defineConfig({
  // Canonical URLs and Open Graph tags are built from this, so it has to be the real
  // origin before launch. `example.com` is reserved by RFC 2606 and can never belong to
  // anyone, which is the point: a placeholder that resolves is a placeholder that ships.
  site: process.env.PUBLIC_SITE_URL ?? "https://example.com",
  output: "static",
  build: {
    // One stylesheet rather than a <link> per component. The whole thing is smaller than a
    // single web font, so the round trips cost more than the bytes.
    inlineStylesheets: "auto",
  },
});
