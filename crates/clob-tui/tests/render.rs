//! Drawing, without a terminal.
//!
//! `TestBackend` renders into a buffer, so the layout can be exercised in CI and the
//! failure mode a TUI is most prone to — a panel that panics on an empty state, or one that
//! silently renders nothing — becomes a test rather than something discovered by opening it.
//!
//! Run with `--nocapture` to read the frame.

use clob_tui::app::{App, Link};
use clob_tui::indexer::Event;
use clob_tui::ui;
use clob_tui::wire;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn level(price: u64, size: u64) -> wire::Level {
    wire::Level {
        price_in_ticks: price.to_string(),
        base_lots: size.to_string(),
    }
}

fn summary() -> wire::MarketSummary {
    wire::MarketSummary {
        market: "Co2FDvpvFr4NWFaLJN2Hb3QxdXkJ7CqxrYYCuNzh8ymY".into(),
        slot: 400_120,
        finalized_through: 400_118,
        base_mint: "So11111111111111111111111111111111111111112".into(),
        quote_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".into(),
        taker_fee_bps: 2,
        lots: wire::Lots {
            base_lots_per_base_unit: "1000".into(),
            tick_size_in_quote_lots_per_base_unit: "1000".into(),
            base_atoms_per_base_lot: "1000000".into(),
            quote_atoms_per_quote_lot: "1".into(),
        },
        best_bid_in_ticks: Some("151400".into()),
        best_ask_in_ticks: Some("151600".into()),
        spread_in_ticks: Some("200".into()),
        mid_price_in_ticks: Some("151500".into()),
        last_price_in_ticks: Some("151550".into()),
        bid_orders: 3,
        ask_orders: 2,
        seats: 2,
        trades_seen: 41,
    }
}

fn populated() -> App {
    let mut app = App::new(
        summary().market,
        "http://localhost:8080".into(),
        Some("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".into()),
        9,
        6,
    );
    app.handle(Event::Summary(Box::new(summary())));
    app.link = Link::Live;

    app.handle(Event::Feed(wire::Message::Snapshot {
        slot: 400_120,
        finalized_through: 400_118,
        bids: vec![
            level(151_400, 1_200),
            level(151_300, 3_400),
            level(151_100, 900),
        ],
        asks: vec![level(151_600, 800), level(151_800, 2_600)],
    }));

    app.handle(Event::Feed(wire::Message::Update {
        slot: 400_121,
        trades: vec![wire::Trade {
            slot: 400_121,
            price_in_ticks: "151600".into(),
            base_lots: "400".into(),
            taker_side: "bid".into(),
            maker_seat: 1,
            taker_seat: Some(2),
        }],
        bids: vec![level(151_400, 1_200), level(151_300, 3_400)],
        asks: vec![level(151_600, 400), level(151_800, 2_600)],
        finalized_through: 400_119,
    }));

    app.handle(Event::Position(Some(Box::new(wire::TraderView {
        seat: 1,
        base_lots_free: "4300".into(),
        base_lots_locked: "700".into(),
        quote_lots_free: "8999020".into(),
        quote_lots_locked: "980".into(),
        orders: vec![wire::OpenOrder {
            side: "bid".into(),
            price_in_ticks: "151400".into(),
            base_lots: "700".into(),
        }],
    }))));

    app
}

fn render(app: &App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| ui::draw(frame, app)).unwrap();

    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_populated_market_renders_every_panel() {
    let app = populated();
    let frame = render(&app, 110, 20);
    println!("\n{frame}\n");

    // Prices are rendered through the lot geometry, not printed as ticks.
    assert!(frame.contains("151.4"), "the best bid, as a price");
    assert!(frame.contains("151.6"), "the best ask");
    assert!(frame.contains("0.2 spread"), "the spread, in quote tokens");

    assert!(frame.contains("book"));
    assert!(frame.contains("fills"));
    assert!(frame.contains("position"));
    assert!(frame.contains("seat 1"));
    assert!(frame.contains("live"));

    // Ticks must never reach the screen raw — that is the number the program stores, not
    // the number a person reads.
    assert!(!frame.contains("151400"), "no raw ticks anywhere");
}

#[test]
fn an_empty_market_renders_rather_than_panicking() {
    // A new listing with no liquidity and no fills is an ordinary state, and it is the one
    // a viewer is most likely to be pointed at first.
    let mut app = App::new("MKT".into(), "http://localhost:8080".into(), None, 9, 6);
    app.handle(Event::Summary(Box::new(summary())));
    app.handle(Event::Feed(wire::Message::Snapshot {
        slot: 1,
        finalized_through: 0,
        bids: vec![],
        asks: vec![],
    }));

    let frame = render(&app, 110, 20);
    println!("\n{frame}\n");

    assert!(frame.contains("no resting liquidity"));
    assert!(frame.contains("nothing has traded"));
    assert!(frame.contains("pass --trader"));
}

#[test]
fn a_market_with_no_summary_yet_still_draws() {
    // The first frame is painted before anything has been fetched. Dashes are correct here;
    // a panic or a blank screen is not.
    let app = App::new("MKT".into(), "http://localhost:8080".into(), None, 9, 6);
    let frame = render(&app, 110, 20);
    assert!(frame.contains("—"), "unknown values render as dashes");
    assert!(frame.contains("connecting"));
}

#[test]
fn a_narrow_terminal_does_not_panic() {
    // Every panel divides the available width. A terminal narrow enough to make one of
    // those divisions zero is the classic way a TUI dies on somebody else's machine.
    let app = populated();
    for (width, height) in [(40, 10), (20, 6), (10, 4)] {
        let frame = render(&app, width, height);
        assert!(!frame.is_empty(), "at {width}x{height}");
    }
}

#[test]
fn a_lagged_feed_says_so_on_the_status_line() {
    // Silence and a gap look identical. The count is the only thing that distinguishes a
    // quiet market from a client that has been dropping messages.
    let mut app = populated();
    app.handle(Event::Feed(wire::Message::Lagged { missed: 12 }));
    app.link = Link::Connecting;

    let frame = render(&app, 110, 20);
    assert!(frame.contains("12 missed"));
}

#[test]
fn a_retraction_is_visible_after_it_happens_and_not_before() {
    // A permanent "0 retracted" trains the eye to skip the place where the number that
    // matters will appear.
    let app = populated();
    assert!(!render(&app, 110, 20).contains("retracted"));

    let mut app = app;
    app.handle(Event::Feed(wire::Message::Retract {
        slot: 400_121,
        trades: 1,
    }));
    let frame = render(&app, 110, 20);
    assert!(frame.contains("1 retracted"));
    // The tape is empty again. Checking for the price would prove nothing — 151.6 is also
    // the best ask, and it is legitimately still in the book.
    assert!(frame.contains("nothing has traded"), "the fill left the tape");
}
