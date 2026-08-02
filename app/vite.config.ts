import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

/**
 * The trading application.
 *
 * A static bundle with no server half. Nothing here is rendered ahead of time and nothing
 * could be: the book changes every slot, the wallet exists only in the browser, and a
 * trading page has no crawler to satisfy. The marketing site is a separate deploy for the
 * opposite reason — see `web/`.
 *
 * The SDK is a `file:` dependency rather than a bundler alias. npm links it to `ts/sdk`,
 * whose `exports` map points at source, so Vite, Node and the type checker all reach the
 * same files through the same mechanism. An alias here would have been a second resolution
 * rule, and two rules that disagree produce a build that type-checks and a bundle that
 * does not run.
 */
export default defineConfig({
  plugins: [react()],
  build: {
    // The chart and the Solana client are the weight here. A ceiling that fails the build
    // is worth more than a warning nobody reads, but it has to be set where a real bundle
    // sits rather than where an empty one does.
    chunkSizeWarningLimit: 900,
  },
});
