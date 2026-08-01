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
    bind: &str,
) -> std::io::Result<()> {
    let state = web::Data::from(registry);
    let store = web::Data::new(store);

    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .app_data(store.clone())
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
