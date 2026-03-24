use clap::Subcommand;

#[derive(Subcommand)]
pub enum Commands {
  /// Run the proxy server (default)
  Serve,
  /// Upgrade the binary to the latest release
  Upgrade,
}
