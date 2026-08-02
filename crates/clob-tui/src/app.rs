//! Everything on screen, and nothing about how it got there.
//!
//! The socket task and the poller push [`Event`](crate::indexer::Event)s in; this folds them
//! into state; [`ui`](crate::ui) reads it. Keeping the three apart is what lets the feed be
//! tested without a terminal and the terminal be exercised without a network.

use clob_book::LotConfig;

use crate::feed::{Action, Feed};
use crate::indexer::Event;
use crate::lots;
use crate::wire;

/// How the socket is doing, for the status line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    Connecting,
    Live,
    Down(String),
}

pub struct App {
    /// The market being watched.
    pub market: String,
    /// Where the indexer is, for the status line.
    pub indexer: String,
    /// The wallet whose position is shown, if one was named.
    pub trader: Option<String>,

    pub feed: Feed,
    pub link: Link,

    /// The market's parameters, once the summary has been read.
    pub summary: Option<wire::MarketSummary>,
    /// Tick and lot geometry, derived from the summary and validated.
    pub lots: Option<LotConfig>,
    /// The watched wallet's position, or `None` for no seat.
    pub position: Option<wire::TraderView>,

    /// Decimals for display. Not on the wire, so they are given or assumed.
    pub base_decimals: u32,
    pub quote_decimals: u32,

    /// Set when the user asks to leave.
    pub quit: bool,
}

impl App {
    pub fn new(
        market: String,
        indexer: String,
        trader: Option<String>,
        base_decimals: u32,
        quote_decimals: u32,
    ) -> Self {
        Self {
            market,
            indexer,
            trader,
            feed: Feed::default(),
            link: Link::Connecting,
            summary: None,
            lots: None,
            position: None,
            base_decimals,
            quote_decimals,
            quit: false,
        }
    }

    /// Folds one event into state.
    ///
    /// Returns whether the socket should be re-established, which happens only when the
    /// server says this subscriber fell behind.
    pub fn handle(&mut self, event: Event) -> Action {
        match event {
            Event::Feed(message) => return self.feed.apply(message),
            Event::Connected => self.link = Link::Live,
            Event::Disconnected(why) => self.link = Link::Down(why),
            Event::Summary(summary) => {
                // The geometry is validated rather than trusted: it arrived over a network
                // from a process that decoded it out of an account, and every price on
                // screen rests on the exactness invariant. A configuration that fails is
                // left unset, which renders as dashes instead of as wrong numbers.
                self.lots = lots::config(&summary.lots).ok();
                self.summary = Some(*summary);
            }
            Event::Position(position) => self.position = position.map(|boxed| *boxed),
        }
        Action::None
    }

    /// A price, or a dash while the geometry is unknown.
    pub fn price(&self, ticks: Option<u64>) -> String {
        match (&self.lots, ticks) {
            (Some(config), Some(ticks)) => lots::price(config, ticks, self.quote_decimals),
            _ => "—".into(),
        }
    }

    /// A size in base lots, or a dash.
    pub fn size(&self, base_lots: u64) -> String {
        match &self.lots {
            Some(config) => lots::size(config, base_lots, self.base_decimals),
            None => "—".into(),
        }
    }

    /// A quote-lot amount, or a dash.
    pub fn quote(&self, quote_lots: u64) -> String {
        match &self.lots {
            Some(config) => lots::quote(config, quote_lots, self.quote_decimals),
            None => "—".into(),
        }
    }
}
