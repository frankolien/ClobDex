/**
 * Where this deploy points.
 *
 * Read once, at module load, and validated. A missing indexer URL is a build that was
 * configured wrong, and it should say so on the first paint rather than render an empty
 * markets table that looks exactly like a chain with nothing on it.
 *
 * Nothing else in the app reads `import.meta.env`, so every environment variable this
 * deploy depends on is listed here and in `.env.example`, and nowhere else.
 */

export interface Config {
  /** Base URL of a `clob-stream` instance, without a trailing slash. */
  readonly indexerUrl: string;
  /** WebSocket origin, derived from the indexer URL. */
  readonly indexerSocketUrl: string;
  /** The deployed program. */
  readonly programAddress: string;
  /** A Solana RPC endpoint, for sending what the SDK builds. */
  readonly rpcUrl: string;
  /** Which cluster this points at, for the badge that stops mainnet mistakes. */
  readonly cluster: "devnet" | "mainnet" | "localnet";
  /** The marketing site. This app is a separate deploy, so it is a URL and not a route. */
  readonly siteUrl: string;
}

function required(name: string, value: string | undefined): string {
  if (!value) {
    throw new Error(
      `${name} is not set. This deploy cannot reach its backend; see app/.env.example.`,
    );
  }
  return value.replace(/\/+$/, "");
}

function socketUrlOf(indexerUrl: string): string {
  // Same host, same path, other scheme. Deriving it means one variable to set and no way
  // to point the feed at a different instance from the one serving the snapshots — which
  // would show a book and a tape from two different markets' worth of state.
  return indexerUrl.replace(/^http/, "ws");
}

function clusterOf(value: string | undefined): Config["cluster"] {
  switch (value) {
    case "mainnet":
    case "devnet":
    case "localnet":
      return value;
    // Defaulting to devnet rather than mainnet: an unset variable should fail towards the
    // network where a mistake costs nothing.
    default:
      return "devnet";
  }
}

const indexerUrl = required("VITE_INDEXER_URL", import.meta.env.VITE_INDEXER_URL);

export const config: Config = {
  indexerUrl,
  indexerSocketUrl: socketUrlOf(indexerUrl),
  programAddress: required("VITE_PROGRAM_ADDRESS", import.meta.env.VITE_PROGRAM_ADDRESS),
  rpcUrl: required("VITE_RPC_URL", import.meta.env.VITE_RPC_URL),
  cluster: clusterOf(import.meta.env.VITE_CLUSTER),
  siteUrl: import.meta.env.VITE_SITE_URL ?? "https://clobdex.xyz",
};
