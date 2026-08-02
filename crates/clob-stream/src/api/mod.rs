//! The read surface: HTTP for snapshots, WebSocket for the live feed.
//!
//! actix runs `!Send` futures per worker, so handlers reach shared state through an
//! `Arc<Registry>` and never hold its lock across an await.

pub mod http;
pub mod view;
pub mod ws;

use std::sync::Arc;

use actix_web::{App, HttpServer, web};

use crate::registry::Registry;

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
