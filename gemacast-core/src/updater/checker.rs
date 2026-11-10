use reqwest::Client;
use std::time::Duration;

use super::AvailableUpdate;
use super::downloader::approved_redirect_url;
use super::manifest::ReleaseManifest;
use super::retry::RetryPolicy;
use crate::domain::error::UpdaterError;

const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_MANIFEST_REDIRECTS: usize = 5;

/// Latest release manifest published by the personal Gemacast fork.
pub const UPDATER_URL: &str =
    "https://github.com/Kazzaky/gemacast/releases/latest/download/updater.json";

pub struct UpdateChecker {
    manifest_url: String,
    http_client: Client,
    retry_policy: RetryPolicy,
}

impl UpdateChecker {
    pub fn latest_release() -> Self {
        let http_client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= MAX_MANIFEST_REDIRECTS {
                    return attempt.error(std::io::Error::other(
                        "too many updater manifest redirects",
                    ));
                }
                if approved_redirect_url(attempt.url()) {
                    attempt.follow()
                } else {
                    attempt.error(std::io::Error::other(
                        "updater manifest redirect must remain HTTPS on an approved GitHub host",
                    ))
                }
            }))
            .build()
            .expect("failed to build restricted updater manifest HTTP client");

        Self {
            manifest_url: UPDATER_URL.to_string(),
            http_client,
            retry_policy: RetryPolicy::for_network_requests(),
        }
    }

    pub async fn find_available_update(
        &self,
        current_version: &str,
        platform_key: &str,
    ) -> Result<Option<AvailableUpdate>, UpdaterError> {
        let installed_version = semver::Version::parse(current_version).map_err(|source| {
            UpdaterError::InvalidCurrentVersion {
                version: current_version.to_string(),
                source,
            }
        })?;

        tracing::info!("Checking for updates (current: v{installed_version})...");

        let manifest = self.fetch_latest_manifest().await?;
        let update = manifest.newer_release_for(&installed_version, platform_key)?;

        if let Some(update) = &update {
            tracing::info!("Update available: v{}", update.version);
        } else {
            tracing::info!("App is up to date (v{installed_version})");
        }

        Ok(update)
    }

    async fn fetch_latest_manifest(&self) -> Result<ReleaseManifest, UpdaterError> {
        let manifest_json = self.retry_policy.run(|| self.fetch_manifest_json()).await?;

        ReleaseManifest::from_json(&manifest_json)
    }

    async fn fetch_manifest_json(&self) -> Result<Vec<u8>, UpdaterError> {
        let mut response = self
            .http_client
            .get(&self.manifest_url)
            .send()
            .await
            .map_err(UpdaterError::ManifestRequestFailed)?
            .error_for_status()
            .map_err(|error| match error.status() {
                Some(status) => UpdaterError::ManifestHttpStatus {
                    status,
                    source: error,
                },
                None => UpdaterError::ManifestRequestFailed(error),
            })?;

        if response
            .content_length()
            .is_some_and(|length| length > MAX_MANIFEST_BYTES as u64)
        {
            return Err(UpdaterError::ManifestTooLarge {
                max_bytes: MAX_MANIFEST_BYTES,
            });
        }

        let mut manifest = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(UpdaterError::ManifestBodyReadFailed)?
        {
            if manifest.len().saturating_add(chunk.len()) > MAX_MANIFEST_BYTES {
                return Err(UpdaterError::ManifestTooLarge {
                    max_bytes: MAX_MANIFEST_BYTES,
                });
            }
            manifest.extend_from_slice(&chunk);
        }

        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::UPDATER_URL;

    #[test]
    fn updater_manifest_comes_from_personal_fork() {
        assert_eq!(
            UPDATER_URL,
            "https://github.com/Kazzaky/gemacast/releases/latest/download/updater.json"
        );
    }
}
