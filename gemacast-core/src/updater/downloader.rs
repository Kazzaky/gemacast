use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

use super::UpdateArtifact;
use super::retry::RetryPolicy;
use crate::domain::error::UpdaterError;

const RELEASE_PATH_PREFIX: &str = "/Kazzaky/gemacast/releases/download/";

pub(crate) fn approved_redirect_url(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(
            url.host_str(),
            Some("github.com" | "release-assets.githubusercontent.com")
        )
}

fn validate_artifact_url(raw: &str) -> Result<(), UpdaterError> {
    let url = reqwest::Url::parse(raw).map_err(|_| UpdaterError::InvalidArtifactUrl {
        reason: "malformed URL",
    })?;
    if url.scheme() != "https" {
        return Err(UpdaterError::InvalidArtifactUrl {
            reason: "HTTPS is required",
        });
    }
    if url.host_str() != Some("github.com")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || !url.path().starts_with(RELEASE_PATH_PREFIX)
    {
        return Err(UpdaterError::InvalidArtifactUrl {
            reason: "only this fork's GitHub Releases download paths are accepted",
        });
    }
    Ok(())
}

pub struct UpdateDownloader {
    http_client: reqwest::Client,
    retry_policy: RetryPolicy,
}

impl Default for UpdateDownloader {
    fn default() -> Self {
        // GitHub release downloads redirect to release-assets.githubusercontent.com.
        // Refuse HTTP downgrade and any unexpected redirect host.
        let http_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 {
                    return attempt.error(std::io::Error::other("too many update redirects"));
                }
                let url = attempt.url();
                if approved_redirect_url(url) {
                    attempt.follow()
                } else {
                    attempt.error(std::io::Error::other(
                        "update redirect must remain HTTPS on an approved GitHub host",
                    ))
                }
            }))
            .build()
            .expect("failed to build restricted update HTTP client");
        Self {
            http_client,
            retry_policy: RetryPolicy::for_network_requests(),
        }
    }
}

impl UpdateDownloader {
    pub async fn download(
        &self,
        artifact: &UpdateArtifact,
        destination: &Path,
        progress_sender: Option<mpsc::Sender<u8>>,
    ) -> Result<(), UpdaterError> {
        let response = self.request_artifact(&artifact.download_url).await?;
        let content_length = response.content_length();
        let mut stream = response.bytes_stream();
        let mut downloaded_file = DownloadedArtifact::create(destination)?;
        let mut progress = DownloadProgress::new(content_length, progress_sender);

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(UpdaterError::ArtifactStreamFailed)?;
            downloaded_file.append(&chunk)?;
            progress.record(chunk.len() as u64);
        }

        // Only publish the final APK path after the complete file has been synced
        // and its SHA-256 digest matches the signed release manifest.
        let downloaded_bytes = downloaded_file.publish(&artifact.sha256, destination)?;
        progress.complete();

        tracing::info!(
            "Downloaded and verified update to {} ({downloaded_bytes} bytes)",
            destination.display()
        );

        Ok(())
    }

    async fn request_artifact(&self, url: &str) -> Result<reqwest::Response, UpdaterError> {
        self.retry_policy
            .run(|| self.request_artifact_once(url))
            .await
    }

    async fn request_artifact_once(&self, url: &str) -> Result<reqwest::Response, UpdaterError> {
        validate_artifact_url(url)?;
        self.http_client
            .get(url)
            .send()
            .await
            .map_err(UpdaterError::ArtifactRequestFailed)?
            .error_for_status()
            .map_err(|error| match error.status() {
                Some(status) => UpdaterError::ArtifactHttpStatus {
                    status,
                    source: error,
                },
                None => UpdaterError::ArtifactRequestFailed(error),
            })
    }
}

struct DownloadedArtifact {
    path: PathBuf,
    file: Option<std::fs::File>,
    checksum: Sha256,
    bytes_written: u64,
}

impl DownloadedArtifact {
    fn create(destination: &Path) -> Result<Self, UpdaterError> {
        use std::time::{SystemTime, UNIX_EPOCH};

        let file_name = destination.file_name().ok_or_else(|| {
            UpdaterError::CreateUpdateFile {
                path: destination.display().to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "update destination has no file name",
                ),
            }
        })?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temporary_path = destination.with_file_name(format!(
            "{}.download-{}-{stamp}.part",
            file_name.to_string_lossy(),
            std::process::id()
        ));

        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        let file = options.open(&temporary_path).map_err(|source| {
            UpdaterError::CreateUpdateFile {
                path: temporary_path.display().to_string(),
                source,
            }
        })?;

        Ok(Self {
            path: temporary_path,
            file: Some(file),
            checksum: Sha256::new(),
            bytes_written: 0,
        })
    }

    fn append(&mut self, chunk: &[u8]) -> Result<(), UpdaterError> {
        self.file
            .as_mut()
            .expect("staging file remains open until publication")
            .write_all(chunk)
            .map_err(|source| UpdaterError::WriteUpdateFile {
                path: self.path.display().to_string(),
                source,
            })?;
        self.checksum.update(chunk);
        self.bytes_written += chunk.len() as u64;
        Ok(())
    }

    fn publish(
        mut self,
        expected_checksum: &str,
        destination: &Path,
    ) -> Result<u64, UpdaterError> {
        self.file
            .as_ref()
            .expect("staging file remains open until publication")
            .sync_all()
            .map_err(|source| UpdaterError::SyncUpdateFile {
                path: self.path.display().to_string(),
                source,
            })?;

        let actual_checksum = hex::encode(self.checksum.clone().finalize());
        if !actual_checksum.eq_ignore_ascii_case(expected_checksum) {
            return Err(UpdaterError::ChecksumMismatch {
                path: destination.display().to_string(),
                expected: expected_checksum.to_string(),
                actual: actual_checksum,
            });
        }

        // Close the staging file before renaming; Windows does not allow an
        // open file handle to be moved in every sharing configuration.
        self.file.take();

        // std::fs::rename replaces an existing file atomically on Unix but not
        // on Windows. Remove an older verified APK first on Windows.
        if destination.exists() {
            std::fs::remove_file(destination).map_err(|source| {
                UpdaterError::CommitUpdateFile {
                    path: destination.display().to_string(),
                    source,
                }
            })?;
        }
        std::fs::rename(&self.path, destination).map_err(|source| {
            UpdaterError::CommitUpdateFile {
                path: destination.display().to_string(),
                source,
            }
        })?;

        Ok(self.bytes_written)
    }
}

impl Drop for DownloadedArtifact {
    fn drop(&mut self) {
        // Close first so the partial file can also be removed on Windows.
        self.file.take();
        let _ = std::fs::remove_file(&self.path);
    }
}

struct DownloadProgress {
    total_bytes: Option<u64>,
    downloaded_bytes: u64,
    last_reported_percent: u8,
    sender: Option<mpsc::Sender<u8>>,
}

impl DownloadProgress {
    fn new(total_bytes: Option<u64>, sender: Option<mpsc::Sender<u8>>) -> Self {
        Self {
            total_bytes,
            downloaded_bytes: 0,
            last_reported_percent: 0,
            sender,
        }
    }

    fn record(&mut self, bytes: u64) {
        self.downloaded_bytes += bytes;

        let Some(total_bytes) = self.total_bytes.filter(|total| *total > 0) else {
            return;
        };

        let percent =
            ((self.downloaded_bytes as f64 / total_bytes as f64) * 100.0).min(100.0) as u8;
        if percent != self.last_reported_percent {
            self.last_reported_percent = percent;
            self.send(percent);
        }
    }

    fn complete(&self) {
        self.send(100);
    }

    fn send(&self, percent: u8) {
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(percent);
        }
    }
}

#[cfg(test)]
mod url_tests {
    use super::{approved_redirect_url, validate_artifact_url};

    #[test]
    fn accepts_only_https_release_assets_from_this_fork() {
        assert!(validate_artifact_url(
            "https://github.com/Kazzaky/gemacast/releases/download/v0.6.0/gemacast-mobile-universal.apk"
        ).is_ok());
    }

    #[test]
    fn rejects_http_artifact_urls() {
        assert!(validate_artifact_url(
            "http://github.com/Kazzaky/gemacast/releases/download/v0.6.0/app.apk"
        ).is_err());
    }

    #[test]
    fn accepts_only_https_github_release_redirect_hosts() {
        for url in [
            "https://github.com/Kazzaky/gemacast/releases/download/v0.6.0/app.apk",
            "https://release-assets.githubusercontent.com/github-production-release-asset/123/app.apk",
        ] {
            assert!(approved_redirect_url(&reqwest::Url::parse(url).unwrap()));
        }
        for url in [
            "http://release-assets.githubusercontent.com/app.apk",
            "https://example.com/app.apk",
            "https://user@github.com/app.apk",
            "https://github.com:444/app.apk",
        ] {
            assert!(!approved_redirect_url(&reqwest::Url::parse(url).unwrap()));
        }
    }

    #[test]
    fn rejects_other_hosts_and_repositories() {
        for url in [
            "https://example.com/Kazzaky/gemacast/releases/download/v0.6.0/app.apk",
            "https://github.com/someone-else/gemacast/releases/download/v0.6.0/app.apk",
            "https://user@github.com/Kazzaky/gemacast/releases/download/v0.6.0/app.apk",
            "https://github.com:444/Kazzaky/gemacast/releases/download/v0.6.0/app.apk",
        ] {
            assert!(validate_artifact_url(url).is_err(), "accepted {url}");
        }
    }

    #[test]
    fn only_a_checksum_verified_staging_file_is_published() {
        use sha2::{Digest, Sha256};

        let root = std::env::temp_dir().join(format!(
            "gemacast-update-publish-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let destination = root.join("gemacast-update.apk");
        let payload = b"verified update payload";
        let expected = hex::encode(Sha256::digest(payload));

        let mut staged = DownloadedArtifact::create(&destination).unwrap();
        let staging_path = staged.path.clone();
        staged.append(payload).unwrap();
        assert!(!destination.exists());
        assert!(staging_path.exists());

        staged.publish(&expected, &destination).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), payload);
        assert!(!staging_path.exists());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn checksum_failure_removes_staging_file_and_never_publishes_apk() {
        let root = std::env::temp_dir().join(format!(
            "gemacast-update-reject-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let destination = root.join("gemacast-update.apk");

        let mut staged = DownloadedArtifact::create(&destination).unwrap();
        let staging_path = staged.path.clone();
        staged.append(b"tampered payload").unwrap();

        assert!(staged.publish(&"00".repeat(32), &destination).is_err());
        assert!(!destination.exists());
        assert!(!staging_path.exists());

        let _ = std::fs::remove_dir_all(root);
    }
}

