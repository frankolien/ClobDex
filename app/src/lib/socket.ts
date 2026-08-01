/**
 * The transport under the feed reducer.
 *
 * Everything that decides what the screen shows lives in {@link ./feed.ts} and is pure.
 * This file only carries bytes and decides when to reconnect — kept apart so the hard part
 * is testable and this part is small enough to read.
 *
 * # A gap is closed by reconnecting
 *
 * When the server sends `lagged`, this subscriber's changes were dropped and the book on
 * screen is wrong by an unknown amount. The protocol has no way to ask for a new snapshot
 * on a live socket, and it does not need one: a connection begins with a snapshot. So a
 * gap closes the socket and opens another, which is the shortest path to a book that is
 * true again.
 *
 * Deliberately not "wait and hope". The missing changes are missing; no later update
 * contains them.
 */

import { type FeedState, initial, reduce } from "./feed.ts";
import type { WireMessage } from "./wire.ts";

/** How long to wait before the first reconnection attempt. */
const BASE_DELAY_MS = 500;

/**
 * The longest wait between attempts.
 *
 * Capped so a market that comes back after an outage is picked up in seconds rather than
 * whenever an unbounded backoff next happens to fire.
 */
const MAX_DELAY_MS = 10_000;

/** What the caller supplies so a test can hand over something that is not a network. */
export type SocketFactory = (url: string) => WebSocket;

export interface FeedOptions {
  /** WebSocket origin of a `clob-stream` instance. */
  readonly socketUrl: string;
  /** The market to subscribe to. */
  readonly market: string;
  /** Called with a new state after every message and every status change. */
  readonly onState: (state: FeedState) => void;
  /** Defaults to the platform's `WebSocket`. */
  readonly open?: SocketFactory;
}

/**
 * A subscription to one market.
 *
 * Reconnects on its own, indefinitely, with a bounded backoff. A trading page left open
 * across a laptop sleeping should come back by itself; the alternative is a book frozen
 * at whatever it was hours ago, which looks exactly like a market nobody is trading.
 */
export class Feed {
  private socket: WebSocket | null = null;
  private state: FeedState = initial;
  private attempts = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private stopped = false;

  // A parameter property would emit runtime code, which `erasableSyntaxOnly` forbids so
  // that Node can run this source by stripping types rather than compiling it.
  private readonly options: FeedOptions;

  constructor(options: FeedOptions) {
    this.options = options;
  }

  /** The last state published. */
  current(): FeedState {
    return this.state;
  }

  start(): void {
    this.stopped = false;
    this.connect();
  }

  /** Closes the socket and cancels any pending reconnection. */
  stop(): void {
    this.stopped = true;
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    this.socket?.close();
    this.socket = null;
  }

  private publish(state: FeedState): void {
    this.state = state;
    this.options.onState(state);
  }

  private connect(): void {
    if (this.stopped) return;

    const open = this.options.open ?? ((url: string) => new WebSocket(url));
    const url = `${this.options.socketUrl}/v1/markets/${this.options.market}/stream`;
    const socket = open(url);
    this.socket = socket;

    socket.onopen = () => {
      // Reset only once a connection is actually established. Resetting when one is
      // attempted turns a server that accepts and immediately drops into a tight loop.
      this.attempts = 0;
    };

    socket.onmessage = (event: MessageEvent) => {
      let message: WireMessage;
      try {
        message = JSON.parse(String(event.data)) as WireMessage;
      } catch {
        // A frame this cannot parse is a version skew, not a network fault. Dropping the
        // frame and carrying on shows a stale book; reconnecting at least re-snapshots.
        socket.close();
        return;
      }

      this.publish(reduce(this.state, message));

      // The reducer decides a gap happened; the transport decides what to do about it.
      if (this.state.status === "gapped") socket.close();
    };

    socket.onclose = () => {
      if (this.socket === socket) this.socket = null;
      if (this.stopped) return;

      // "closed" rather than "live": what is on screen was true when it arrived and is
      // not being kept true. A page that hides the difference is a page that lies quietly.
      if (this.state.status !== "gapped") {
        this.publish({ ...this.state, status: "closed" });
      }
      this.scheduleReconnect();
    };

    socket.onerror = () => socket.close();
  }

  private scheduleReconnect(): void {
    const delay = Math.min(BASE_DELAY_MS * 2 ** this.attempts, MAX_DELAY_MS);
    this.attempts += 1;
    this.timer = setTimeout(() => {
      this.timer = null;
      this.connect();
    }, delay);
  }
}
