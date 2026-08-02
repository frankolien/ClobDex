//! The read surface: HTTP for snapshots, WebSocket for the live feed.
//!
//! actix runs `!Send` futures per worker, so handlers reach shared state through an
//! `Arc<Registry>` and never hold its lock across an await.

pub mod http;
pub mod view;
pub mod ws;

use std::sync::Arc;

use actix_cors::Cors;
use actix_web::{App, HttpServer, web};

use crate::registry::Registry;

/// Which origins may read this API from a browser.
///
/// Any of them, by default, and that is a decision rather than an oversight.
///
/// Everything here is public on-chain data served over `GET`. There is no authentication,
/// no cookie, no header carrying a secret, and nothing a request can change — so an origin
/// restriction would protect nothing and would only decide who gets to build a client.
/// Every public market-data API works this way for the same reason.
///
/// It also has to be permissive to be usable at all: a browser refuses a cross-origin read
/// without this header, and a UI on one port talking to an indexer on another is the normal
/// arrangement rather than the exception.
///
/// **This must be narrowed the moment anything here requires credentials.** Allowing any
/// origin *and* credentials is the combination that turns a read API into a way for any
/// page to act as its visitor.
fn cors() -> Cors {
    match std::env::var("ALLOWED_ORIGINS") {
        // A comma-separated allowlist, for a deployment that wants one anyway.
        Ok(origins) if !origins.trim().is_empty() => origins
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .fold(Cors::default(), |cors, origin| cors.allowed_origin(origin))
            .allowed_methods(["GET"])
            .allow_any_header()
            .max_age(3600),
        _ => Cors::default()
            .allow_any_origin()
            .allowed_methods(["GET"])
            .allow_any_header()
            .max_age(3600),
    }
}

/// Serves the read API until the process stops.
pub async fn serve(
    registry: Arc<Registry>,
    store: Arc<dyn crate::store::Store>,
    program_id: solana_pubkey::Pubkey,
    bind: &str,
) -> std::io::Result<()> {
    let state = web::Data::from(registry);
    let store = web::Data::new(store);
    // Needed to derive each market's vault signer, which is a PDA the TypeScript SDK
    // cannot compute for itself.
    let program_id = web::Data::new(program_id);

    HttpServer::new(move || {
        App::new()
            // Before every route, including the WebSocket upgrade: a browser will not even
            // attempt the socket without a successful preflight.
            .wrap(cors())
            .app_data(state.clone())
            .app_data(store.clone())
            .app_data(program_id.clone())
            .service(http::markets)
            .service(http::book)
            .service(http::trades)
            .service(http::health)
            .service(http::history)
            .service(http::candles)
            .service(http::window)
            .service(http::trader)
            .route("/v1/markets/{market}/stream", web::get().to(ws::stream))
    })
    .bind(bind)?
    .run()
    .await
}
