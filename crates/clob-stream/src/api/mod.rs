//! The read surface.
//!
//! actix runs `!Send` futures per worker, so handlers reach shared state through an
//! `Arc<Registry>` and never hold its lock across an await.

pub mod http;

use std::sync::Arc;

use actix_web::{App, HttpServer, web};

use crate::registry::Registry;

/// Serves the read API until the process stops.
pub async fn serve(registry: Arc<Registry>, bind: &str) -> std::io::Result<()> {
    let state = web::Data::from(registry);

    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .service(http::markets)
            .service(http::book)
            .service(http::trades)
            .service(http::health)
    })
    .bind(bind)?
    .run()
    .await
}
