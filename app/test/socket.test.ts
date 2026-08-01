import { test } from "node:test";
import assert from "node:assert/strict";

import { Feed } from "../src/lib/socket.ts";
import type { FeedState } from "../src/lib/feed.ts";
import type { WireMessage } from "../src/lib/wire.ts";

/**
 * A socket that does nothing until told to.
 *
 * Enough of the interface for the transport to drive, and no more — a full fake would be a
 * second WebSocket implementation, which is more code than the thing under test.
 */
class FakeSocket {
  static opened: FakeSocket[] = [];

  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  closed = false;

  readonly url: string;

  constructor(url: string) {
    this.url = url;
    FakeSocket.opened.push(this);
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.onclose?.();
  }

  deliver(message: WireMessage): void {
    this.onmessage?.({ data: JSON.stringify(message) });
  }

  deliverRaw(data: string): void {
    this.onmessage?.({ data });
  }
}

function subscribe(): { feed: Feed; states: FeedState[] } {
  FakeSocket.opened = [];
  const states: FeedState[] = [];
  const instance = new Feed({
    socketUrl: "ws://indexer",
    market: "MKT",
    onState: (state) => states.push(state),
    open: (url) => new FakeSocket(url) as unknown as WebSocket,
  });
  return { feed: instance, states };
}

const snapshot: WireMessage = {
  type: "snapshot",
  market: "MKT",
  slot: 10,
  finalized_through: 9,
  bids: [{ price_in_ticks: "98", base_lots: "10" }],
  asks: [{ price_in_ticks: "102", base_lots: "7" }],
};

test("it subscribes to the market it was given", () => {
  const { feed } = subscribe();
  feed.start();

  assert.equal(FakeSocket.opened.length, 1);
  assert.equal(FakeSocket.opened[0]?.url, "ws://indexer/v1/markets/MKT/stream");
  feed.stop();
});

test("messages drive the reducer", () => {
  const { feed, states } = subscribe();
  feed.start();
  FakeSocket.opened[0]?.deliver(snapshot);

  assert.equal(states.length, 1);
  assert.equal(states[0]?.status, "live");
  assert.equal(states[0]?.slot, 10);
  assert.equal(feed.current().bids[0]?.priceInTicks, 98n);
  feed.stop();
});

test("a gap reconnects, because only a snapshot fixes a gap", () => {
  // The protocol has no way to ask for a fresh snapshot on a live socket, and it does not
  // need one: a connection starts with a snapshot. Waiting would leave the book wrong by
  // an unknown amount forever — no later update contains the changes that were dropped.
  const { feed } = subscribe();
  feed.start();
  const first = FakeSocket.opened[0];
  first?.deliver(snapshot);
  first?.deliver({ type: "lagged", missed: 12 });

  assert.equal(first?.closed, true, "the socket is dropped");
  assert.equal(feed.current().status, "gapped");
  assert.equal(feed.current().missed, 12);
  feed.stop();
});

test("a close is shown rather than hidden", () => {
  // What is on screen was true when it arrived and is not being kept true. A page that
  // keeps rendering a frozen book without saying so is a page that lies quietly.
  const { feed } = subscribe();
  feed.start();
  FakeSocket.opened[0]?.deliver(snapshot);
  FakeSocket.opened[0]?.close();

  assert.equal(feed.current().status, "closed");
  assert.equal(feed.current().bids.length, 1, "the last known book is kept");
  feed.stop();
});

test("a frame it cannot parse drops the connection instead of the message", () => {
  // Skipping the frame would carry on with a book that silently stopped matching the
  // server. Reconnecting at least produces a fresh snapshot.
  const { feed } = subscribe();
  feed.start();
  FakeSocket.opened[0]?.deliver(snapshot);
  FakeSocket.opened[0]?.deliverRaw("{ not json");

  assert.equal(FakeSocket.opened[0]?.closed, true);
  feed.stop();
});

test("stopping cancels the reconnection rather than racing it", () => {
  // A component unmounting mid-backoff must not reopen a socket for a market nobody is
  // looking at any more.
  const { feed } = subscribe();
  feed.start();
  FakeSocket.opened[0]?.close();
  feed.stop();

  assert.equal(FakeSocket.opened.length, 1, "no socket was opened after stop");
});
