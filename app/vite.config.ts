import { fileURLToPath } from "node:url";

import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

/**
 * The trading application.
 *
 * A static bundle with no server half. Nothing here is rendered ahead of time and nothing
 * could be: the book changes every slot, the wallet exists only in the browser, and a
 * trading page has no crawler to satisfy. The marketing site is a separate deploy for the
 * opposite reason — see `web/`.
 */
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      // Source, not a built package. Mirrors the `paths` entry in tsconfig so the type
      // checker and the bundler resolve the SDK the same way; two rules that disagree
      // produce a build that type-checks and a bundle that does not run.
      "@clobdex/sdk": fileURLToPath(new URL("../ts/sdk/src/index.ts", import.meta.url)),
    },
  },
  build: {
    // Fail the build rather than ship a bundle nobody meant to make this large. The chart
    // is the only heavy dependency and it is 35 kB.
    chunkSizeWarningLimit: 400,
  },
});
