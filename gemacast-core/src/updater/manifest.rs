use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::domain::error::UpdaterError;

const SHA256_HEX_LENGTH: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableUpdate {
    pub version: String,
    pub download_url: String,
    pub sha256: String,
}

impl AvailableUpdate {
    pub fn artifact(&self) -> UpdateArtifact {
        UpdateArtifact::new(self.download_url.clone(), self.sha256.clone())
    }
}

#[derive(Debug, Clone)]
pub struct UpdateArtifact {
    pub download_url: String,
    pub sha256: String,
}

impl UpdateArtifact {
    pub fn new(download_url: impl Into<String>, sha256: impl Into<String>) -> Self {
        Self {
            download_url: download_url.into(),
            sha256: sha256.into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ReleaseManifest {
    version: String,
    platforms: HashMap<String, PlatformRelease>,
}

impl ReleaseManifest {
    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, UpdaterError> {
        serde_json::from_slice(bytes).map_err(UpdaterError::ManifestParseFailed)
    }

    pub(crate) fn newer_release_for(
        &self,
        installed_version: &semver::Version,
        platform_key: &str,
    ) -> Result<Option<AvailableUpdate>, UpdaterError> {
        let release_version = semver::Version::parse(&self.version).map_err(|source| {
            UpdaterError::InvalidManifestVersion {
                version: self.version.clone(),
                source,
            }
        })?;

        if release_version <= *installed_version {
            return Ok(None);
        }

        let platform_release =
            self.platforms
                .get(platform_key)
                .ok_or_else(|| UpdaterError::PlatformNotPublished {
                    platform_key: platform_key.to_string(),
                })?;

        Ok(Some(AvailableUpdate {
            version: self.version.clone(),
            download_url: platform_release.url.clone(),
            sha256: platform_release.verified_checksum(platform_key)?,
        }))
    }
}

#[derive(Debug, Clone, Deserialize)]
struct PlatformRelease {
    url: String,
    #[serde(default)]
    sha256: Option<String>,
}

impl PlatformRelease {
    fn verified_checksum(&self, platform_key: &str) -> Result<String, UpdaterError> {
        let checksum = self.sha256.as_deref().unwrap_or_default().trim();

        if checksum.is_empty() {
            return Err(UpdaterError::MissingChecksum {
                platform_key: platform_key.to_string(),
            });
        }

        if checksum.len() != SHA256_HEX_LENGTH
            || !checksum
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            return Err(UpdaterError::MalformedChecksum {
                platform_key: platform_key.to_string(),
                length: checksum.len(),
                expected_length: SHA256_HEX_LENGTH,
            });
        }

        Ok(checksum.to_ascii_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_CHECKSUM: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    fn manifest_with_checksum(checksum: &str) -> ReleaseManifest {
        let json = format!(
            r#"{{
                "version": "0.3.0",
                "platforms": {{
                    "windows-x86_64": {{
                        "url": "https://example.com/installer.msi"
                        {checksum}
                    }}
                }}
            }}"#
        );

        ReleaseManifest::from_bytes(json.as_bytes()).unwrap()
    }

    #[test]
    fn a_platform_release_normalizes_its_checksum() {
        let manifest = manifest_with_checksum(&format!(
            r#", "sha256": "{}""#,
            VALID_CHECKSUM.to_uppercase()
        ));
        let update = manifest
            .newer_release_for(&semver::Version::parse("0.2.0").unwrap(), "windows-x86_64")
            .unwrap()
            .unwrap();

        assert_eq!(update.sha256, VALID_CHECKSUM);
    }

    #[test]
    fn a_platform_release_without_a_checksum_is_rejected() {
        let error = manifest_with_checksum("")
            .newer_release_for(&semver::Version::parse("0.2.0").unwrap(), "windows-x86_64")
            .unwrap_err();

        assert!(
            matches!(error, UpdaterError::MissingChecksum { .. }),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn a_platform_release_with_an_empty_checksum_is_rejected() {
        let error = manifest_with_checksum(r#", "sha256": "  ""#)
            .newer_release_for(&semver::Version::parse("0.2.0").unwrap(), "windows-x86_64")
            .unwrap_err();

        assert!(
            matches!(error, UpdaterError::MissingChecksum { .. }),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn a_platform_release_with_a_malformed_checksum_is_rejected() {
        for checksum in [
            "abc123def456",
            "zz86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a0",
        ] {
            let error = manifest_with_checksum(&format!(r#", "sha256": "{checksum}""#))
                .newer_release_for(&semver::Version::parse("0.2.0").unwrap(), "windows-x86_64")
                .unwrap_err();

            assert!(
                matches!(error, UpdaterError::MalformedChecksum { .. }),
                "expected {checksum:?} to be rejected as malformed, got: {error}"
            );
        }
    }
}
