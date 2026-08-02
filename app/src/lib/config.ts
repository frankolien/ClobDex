/**
 * Where this deploy points.
 *
 * Read once, at module load, and validated. Nothing else in the app reads
 * `import.meta.env`, so every environment variable this deploy depends on is listed here
 * and in `.env.example`, and nowhere else.
 *
 * # Dev falls back, production does not
 *
 * `npm run dev` on a fresh clone should work without copying a file first, and a production
 * build that was never configured should fail loudly rather than point somewhere plausible
 * and wrong. Both are served by one rule:
 *
 * > A fallback may only be a fact about this repository — where our own dev servers listen,
 * > which program this repository deploys. Never a guess about somebody's deployment.
 *
 * That rule exists because it was broken once. An earlier version defaulted the marketing
 * site's address to an invented domain, which turned out to be registered and parked for
 * sale, so every link to it went to a stranger's listing page. The only symptom was a TLS
 * error that says nothing about a missing variable.
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
  /** The marketing site, if this build was told where it is. */
  readonly siteUrl: string | null;
}

/** Where `cargo run -p clob-stream` binds unless `BIND` says otherwise. */
const LOCAL_INDEXER = "http://localhost:8080";

/** Where `npm run dev` serves the marketing site from. */
const LOCAL_SITE = "http://localhost:4321";

/** The program this repository deploys to devnet, as recorded in `.env.example`. */
const DEVNET_PROGRAM = "DaNh1Gk3xCLwzHhFQZTgLZuvUMS8YyfPqzs9ZgqFqhTe";

const PUBLIC_DEVNET_RPC = "https://api.devnet.solana.com";

/**
 * A setting, with a fallback that only applies while developing.
 *
 * See the module docs for what a fallback is allowed to be.
 */
function setting(name: string, value: string | undefined, whileDeveloping: string): string {
  if (value) return value.replace(/\/+$/, "");
  if (import.meta.env.DEV) return whileDeveloping;
  throw new Error(
    `${name} is not set. This deploy cannot reach its backend; see app/.env.example.`,
  );
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

const indexerUrl = setting("VITE_INDEXER_URL", import.meta.env.VITE_INDEXER_URL, LOCAL_INDEXER);

export const config: Config = {
  indexerUrl,
  indexerSocketUrl: socketUrlOf(indexerUrl),
  programAddress: setting(
    "VITE_PROGRAM_ADDRESS",
    import.meta.env.VITE_PROGRAM_ADDRESS,
    DEVNET_PROGRAM,
  ),
  rpcUrl: setting("VITE_RPC_URL", import.meta.env.VITE_RPC_URL, PUBLIC_DEVNET_RPC),
  cluster: clusterOf(import.meta.env.VITE_CLUSTER),
  // The one setting with no production fallback at all. An absent marketing site is a
  // missing link; a wrong one is a link to somebody else.
  siteUrl: import.meta.env.VITE_SITE_URL ?? (import.meta.env.DEV ? LOCAL_SITE : null),
};
