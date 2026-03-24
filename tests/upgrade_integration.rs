use previewproxy::modules::cli::subcommands::upgrade::{compare_versions, download_url};
use std::cmp::Ordering;

#[test]
fn test_download_url_contains_artifact_and_tag() {
  let url = download_url("1.99.0");
  assert!(
    url.starts_with("https://github.com/ViGrise/previewproxy/releases/download/v1.99.0/"),
    "unexpected url: {url}"
  );
  assert!(url.contains("previewproxy-"), "unexpected url: {url}");
}

#[test]
fn test_version_comparison_across_major_minor() {
  assert_eq!(compare_versions("1.3.0", "2.0.0"), Ordering::Less);
  assert_eq!(compare_versions("1.9.0", "1.10.0"), Ordering::Less);
  assert_eq!(compare_versions("1.3.0", "1.3.0"), Ordering::Equal);
}
