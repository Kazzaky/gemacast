//! Persistent self-signed identity for the encrypted PC control transport.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gemacast_core::control::device_auth::sha256_fingerprint;
use gemacast_core::domain::types::DeviceId;
use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};

#[derive(Clone)]
pub struct PcIdentity {
    certificate_der: Vec<u8>,
    private_key_der: Vec<u8>,
    fingerprint: String,
}

impl PcIdentity {
    pub fn load_default() -> io::Result<Self> {
        Self::load_or_create(
            crate::config::pc_identity_cert_path(),
            crate::config::pc_identity_key_path(),
        )
    }

    fn load_or_create(cert_path: PathBuf, key_path: PathBuf) -> io::Result<Self> {
        match (std::fs::read(&cert_path), std::fs::read(&key_path)) {
            (Ok(certificate_der), Ok(private_key_der)) => {
                match Self::from_der(certificate_der, private_key_der) {
                    Ok(identity) => Ok(identity),
                    Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                        tracing::warn!(
                            %error,
                            "PC identity files are invalid; quarantining them and generating a new identity"
                        );
                        quarantine_identity_file(&cert_path)?;
                        quarantine_identity_file(&key_path)?;
                        Self::generate(cert_path, key_path)
                    }
                    Err(error) => Err(error),
                }
            }
            (Err(cert_error), Err(key_error))
                if cert_error.kind() == io::ErrorKind::NotFound
                    && key_error.kind() == io::ErrorKind::NotFound =>
            {
                Self::generate(cert_path, key_path)
            }
            (Err(error), Ok(_)) if error.kind() == io::ErrorKind::NotFound => {
                quarantine_identity_file(&key_path)?;
                Self::generate(cert_path, key_path)
            }
            (Ok(_), Err(error)) if error.kind() == io::ErrorKind::NotFound => {
                quarantine_identity_file(&cert_path)?;
                Self::generate(cert_path, key_path)
            }
            (Err(error), _) | (_, Err(error)) => Err(error),
        }
    }

    fn generate(cert_path: PathBuf, key_path: PathBuf) -> io::Result<Self> {
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(|error| {
            io::Error::other(format!("failed to generate PC identity: {error}"))
        })?;
        let params = CertificateParams::new(["gemacast.local".to_string()]).map_err(|error| {
            io::Error::other(format!("failed to create PC certificate: {error}"))
        })?;
        let certificate = params
            .self_signed(&key_pair)
            .map_err(|error| io::Error::other(format!("failed to sign PC certificate: {error}")))?;
        let certificate_der = certificate.der().to_vec();
        let private_key_der = key_pair.serialize_der();

        save_private_file(&key_path, &private_key_der)?;
        if let Err(error) = save_file(&cert_path, &certificate_der) {
            let _ = std::fs::remove_file(&key_path);
            return Err(error);
        }
        Self::from_der(certificate_der, private_key_der)
    }

    fn from_der(certificate_der: Vec<u8>, private_key_der: Vec<u8>) -> io::Result<Self> {
        let signer = rustls::crypto::ring::sign::any_ecdsa_type(
            &rustls::pki_types::PrivateKeyDer::Pkcs8(private_key_der.clone().into()),
        )
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("PC identity private key cannot be used by TLS: {error}"),
            )
        })?;
        let certified_key = rustls::sign::CertifiedKey::new(
            vec![rustls::pki_types::CertificateDer::from(
                certificate_der.clone(),
            )],
            signer,
        );
        certified_key.keys_match().map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("PC certificate and private key do not match: {error}"),
            )
        })?;

        Ok(Self {
            fingerprint: sha256_fingerprint(&certificate_der),
            certificate_der,
            private_key_der,
        })
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn device_id(&self) -> DeviceId {
        DeviceId(format!("PC_{}", self.fingerprint))
    }

    pub fn tls_config(&self) -> io::Result<Arc<rustls::ServerConfig>> {
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|error| io::Error::other(format!("failed to enable TLS 1.2/1.3: {error}")))?
        .with_no_client_auth()
        .with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(
                self.certificate_der.clone(),
            )],
            rustls::pki_types::PrivateKeyDer::Pkcs8(self.private_key_der.clone().into()),
        )
        .map_err(|error| io::Error::other(format!("failed to configure PC TLS: {error}")))?;
        Ok(Arc::new(config))
    }
}

fn save_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("der.tmp");
    std::fs::write(&tmp_path, contents)?;
    std::fs::rename(tmp_path, path)
}

fn save_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
    }

    // Create the temporary private-key file with restrictive permissions from
    // the first filesystem operation. Do not write it with the default umask
    // and chmod only after rename: another local user could read that window.
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "PC identity key path has no file name")
    })?;
    let tmp_path = path.with_file_name(format!(
        "{}.tmp-{}-{stamp}",
        file_name.to_string_lossy(),
        std::process::id()
    ));

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp_path)?;
    if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all()) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(error);
    }
    drop(file);

    if let Err(error) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(error);
    }
    Ok(())
}

fn quarantine_identity_file(path: &Path) -> io::Result<()> {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "PC identity path has no valid file name",
        ));
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let quarantined = path.with_file_name(format!("{file_name}.invalid-{stamp}"));
    std::fs::rename(path, &quarantined)?;
    tracing::warn!(original = %path.display(), quarantined = %quarantined.display(), "Quarantined PC identity file");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn private_key_file_is_created_with_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "gemacast-pc-identity-permissions-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let key_path = root.join("key.der");
        let cert_path = root.join("cert.der");
        PcIdentity::load_or_create(cert_path, key_path.clone()).unwrap();

        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn identity_should_survive_reload_with_the_same_fingerprint() {
        let root = std::env::temp_dir().join(format!(
            "gemacast-pc-identity-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cert_path = root.join("cert.der");
        let key_path = root.join("key.der");
        let identity = PcIdentity::load_or_create(cert_path.clone(), key_path.clone()).unwrap();
        let reloaded = PcIdentity::load_or_create(cert_path, key_path).unwrap();

        assert_eq!(identity.fingerprint(), reloaded.fingerprint());
        assert_eq!(identity.device_id(), reloaded.device_id());
        identity.tls_config().unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn partial_identity_should_be_quarantined_and_regenerated() {
        let root = std::env::temp_dir().join(format!(
            "gemacast-pc-identity-partial-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cert_path = root.join("cert.der");
        let key_path = root.join("key.der");
        let original = PcIdentity::load_or_create(cert_path.clone(), key_path.clone()).unwrap();
        std::fs::remove_file(&key_path).unwrap();

        let regenerated = PcIdentity::load_or_create(cert_path.clone(), key_path).unwrap();
        assert_ne!(original.fingerprint(), regenerated.fingerprint());
        assert!(
            std::fs::read_dir(&root)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().contains("invalid-"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn key_only_identity_should_be_quarantined_and_regenerated() {
        let root = std::env::temp_dir().join(format!(
            "gemacast-pc-identity-key-only-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cert_path = root.join("cert.der");
        let key_path = root.join("key.der");
        let original = PcIdentity::load_or_create(cert_path.clone(), key_path.clone()).unwrap();
        std::fs::remove_file(&cert_path).unwrap();

        let regenerated = PcIdentity::load_or_create(cert_path, key_path.clone()).unwrap();
        assert_ne!(original.fingerprint(), regenerated.fingerprint());
        assert!(key_path.exists());
        assert!(
            std::fs::read_dir(&root)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .contains("key.der.invalid-"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_identity_should_be_quarantined_and_regenerated() {
        let root = std::env::temp_dir().join(format!(
            "gemacast-pc-identity-corrupt-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cert_path = root.join("cert.der");
        let key_path = root.join("key.der");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&cert_path, b"not a certificate").unwrap();
        std::fs::write(&key_path, b"not a key").unwrap();

        let identity = PcIdentity::load_or_create(cert_path, key_path).unwrap();
        identity.tls_config().unwrap();
        assert!(
            std::fs::read_dir(&root)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains("invalid-"))
                .count()
                == 2
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn mismatched_identity_should_be_quarantined_and_regenerated() {
        let root = std::env::temp_dir().join(format!(
            "gemacast-pc-identity-mismatch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let first_root = root.join("first");
        let second_root = root.join("second");
        let cert_path = root.join("cert.der");
        let key_path = root.join("key.der");
        let first =
            PcIdentity::load_or_create(first_root.join("cert.der"), first_root.join("key.der"))
                .unwrap();
        let second =
            PcIdentity::load_or_create(second_root.join("cert.der"), second_root.join("key.der"))
                .unwrap();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::copy(first_root.join("cert.der"), &cert_path).unwrap();
        std::fs::copy(second_root.join("key.der"), &key_path).unwrap();

        let regenerated = PcIdentity::load_or_create(cert_path, key_path).unwrap();
        assert_ne!(regenerated.fingerprint(), first.fingerprint());
        assert_ne!(regenerated.fingerprint(), second.fingerprint());
        let _ = std::fs::remove_dir_all(root);
    }
}
