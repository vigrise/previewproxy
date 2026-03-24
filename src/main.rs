use clap::Parser;
use previewproxy::modules::cli::{Cli, Commands};

fn main() {
  dotenvy::dotenv().ok();
  let cli = Cli::parse();
  cli.apply_to_env();

  let rt = tokio::runtime::Runtime::new().unwrap();

  match cli.command {
    Some(Commands::Upgrade) => {
      rt.block_on(async {
        if let Err(e) = previewproxy::modules::cli::subcommands::upgrade::run_upgrade().await {
          eprintln!("upgrade failed: {e}");
          std::process::exit(1);
        }
      });
    }
    None | Some(Commands::Serve) => {
      previewproxy::common::config::telemetry::setup_tracing();
      let cfg = previewproxy::common::config::Configuration::new();
      rt.block_on(async {
        use previewproxy::modules;
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
        let app = previewproxy::app::router(cfg.clone(), cache).await;
        let listener = tokio::net::TcpListener::bind(cfg.listen_address)
          .await
          .unwrap();
        tracing::info!("Listening on http://{}", cfg.listen_address);
        axum::serve(listener, app)
          .with_graceful_shutdown(previewproxy::common::config::shutdown::shutdown_signal())
          .await
          .unwrap();
      });
    }
  }
}
