mod cli;

use clap::Parser;
use previewproxy::{app, common::config, modules};

fn main() {
  dotenvy::dotenv().ok();
  let cli = cli::Cli::parse();
  cli.apply_to_env();

  config::telemetry::setup_tracing();
  let cfg = config::Configuration::new();

  let rt = tokio::runtime::Runtime::new().unwrap();
  rt.block_on(async {
    let cache = modules::cache::manager::CacheManager::new(&cfg);
    let cache_clone = cache.clone();
    let interval = cfg.cache_cleanup_interval_secs;
    tokio::spawn(async move {
      let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval));
      loop {
        ticker.tick().await;
        cache_clone.run_cleanup().await;
      }
    });
    let app = app::router(cfg.clone(), cache).await;
    let listener = tokio::net::TcpListener::bind(cfg.listen_address)
      .await
      .unwrap();
    tracing::info!("Listening on http://{}", cfg.listen_address);
    axum::serve(listener, app)
      .with_graceful_shutdown(config::shutdown::shutdown_signal())
      .await
      .unwrap();
  });
}
