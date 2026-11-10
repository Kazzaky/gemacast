use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::control::http_transport::build_https_client;
use crate::control::types::{ControlErrorResponse, PresenceResponse};
use crate::domain::error::{ControlError, GemaCastError};
use crate::domain::types::DeviceId;
use crate::network::Ports;

/// HTTPS client for the phone side of the control protocol.
pub struct HttpControlClient {
    pub(crate) bootstrap_client: reqwest::Client,
    pub(crate) base_url: String,
    pub(crate) request_timeout: Duration,
    pub(crate) credentials: Arc<Mutex<Option<ControlCredentials>>>,
}

#[derive(Clone)]
pub struct ControlCredentials {
    pub device_id: DeviceId,
    pub token: String,
    pub generation: crate::control::SessionGeneration,
    pub pc_device_id: DeviceId,
    pub pc_certificate_fingerprint: String,
}

impl std::fmt::Debug for ControlCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControlCredentials")
            .field("device_id", &self.device_id)
            .field("token", &"[REDACTED]")
            .field("generation", &self.generation)
            .field("pc_device_id", &self.pc_device_id)
            .field("pc_certificate_fingerprint", &self.pc_certificate_fingerprint)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_must_not_expose_the_bearer_token() {
        let token = "sensitive-session-token".to_string();
        let credentials = ControlCredentials {
            device_id: DeviceId("phone-1".into()),
            token: token.clone(),
            generation: crate::control::SessionGeneration(7),
            pc_device_id: DeviceId("pc-1".into()),
            pc_certificate_fingerprint: "ab".repeat(32),
        };

        let debug = format!("{credentials:?}");
        assert!(!debug.contains(&token));
        assert!(debug.contains("[REDACTED]"));
        assert!(debug.contains("phone-1"));
    }
}

/// Long-term player identity and persistent PC certificate pin storage.
pub trait DeviceAuthSigner: Send + Sync {
    fn public_key(&self) -> Result<String, String>;
    fn sign(&self, transcript: &[u8]) -> Result<String, String>;

    fn trusted_pc_fingerprint(&self, _pc_id: &DeviceId) -> Result<Option<String>, String> {
        Ok(None)
    }

    fn confirm_pc_identity(
        &self,
        pc_id: &DeviceId,
        pc_name: &str,
        fingerprint: &str,
        pairing_code: &str,
        requires_approval: bool,
    ) -> Result<bool, String>;

    fn remember_pc_identity(&self, _pc_id: &DeviceId, _fingerprint: &str) -> Result<(), String> {
        Ok(())
    }
}

impl HttpControlClient {
    pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(70);
    const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

    pub fn new(target_ip: IpAddr) -> Self {
        Self::with_timeout(target_ip, Self::DEFAULT_TIMEOUT)
    }

    pub fn with_timeout(target_ip: IpAddr, timeout: Duration) -> Self {
        Self::with_shared_credentials(target_ip, timeout, Arc::new(Mutex::new(None)))
    }

    pub fn with_shared_credentials(
        target_ip: IpAddr,
        timeout: Duration,
        credentials: Arc<Mutex<Option<ControlCredentials>>>,
    ) -> Self {
        let bootstrap_client = build_https_client(timeout, None).unwrap_or_else(|reason| {
            panic!("failed to initialize the mandatory HTTPS control client: {reason}")
        });
        let base_url = format!("https://{}:{}", target_ip, Ports::CONTROL);
        Self {
            bootstrap_client,
            base_url,
            request_timeout: timeout,
            credentials,
        }
    }

    pub(crate) fn credential_for(
        &self,
        device_id: Option<&DeviceId>,
    ) -> Option<ControlCredentials> {
        self.credentials
            .lock()
            .ok()
            .and_then(|credentials| credentials.clone())
            .filter(|credential| {
                device_id.is_none_or(|device_id| device_id == &credential.device_id)
            })
    }

    pub(crate) fn client_for(
        &self,
        credential: Option<&ControlCredentials>,
    ) -> Result<reqwest::Client, GemaCastError> {
        credential.map_or_else(
            || Ok(self.bootstrap_client.clone()),
            |credential| {
                build_https_client(
                    self.request_timeout,
                    Some(&credential.pc_certificate_fingerprint),
                )
                .map_err(|reason| ControlError::Rejected { reason }.into())
            },
        )
    }

    pub(crate) fn authorize(
        request: reqwest::RequestBuilder,
        credential: Option<&ControlCredentials>,
    ) -> reqwest::RequestBuilder {
        match credential {
            Some(credential) => request.bearer_auth(&credential.token),
            None => request,
        }
    }

    pub fn session_token(&self, device_id: &DeviceId) -> Option<String> {
        self.credential_for(Some(device_id))
            .map(|credentials| credentials.token)
    }

    pub fn session_credentials(&self, device_id: &DeviceId) -> Option<ControlCredentials> {
        self.credential_for(Some(device_id))
    }

    pub(crate) async fn ensure_success(
        response: reqwest::Response,
    ) -> Result<reqwest::Response, GemaCastError> {
        if response.status().is_success() {
            return Ok(response);
        }

        let status = response.status();
        let error = response
            .json::<ControlErrorResponse>()
            .await
            .unwrap_or_else(|_| ControlErrorResponse {
                code: "http_error".into(),
                message: format!("HTTP {status}"),
            });
        Err(ControlError::RemoteRejected {
            code: error.code,
            reason: error.message,
        }
        .into())
    }

    pub(crate) fn verify_presence_certificate(
        presence: &PresenceResponse,
        observed_fingerprint: &str,
    ) -> Result<String, GemaCastError> {
        let advertised = presence
            .pc_certificate_fingerprint
            .as_deref()
            .ok_or_else(|| ControlError::Rejected {
                reason: "streamer did not provide its PC certificate fingerprint".into(),
            })?;
        if !advertised.eq_ignore_ascii_case(observed_fingerprint) {
            return Err(ControlError::Rejected {
                reason: "streamer identity does not match the HTTPS certificate".into(),
            }
            .into());
        }
        Ok(advertised.to_ascii_lowercase())
    }
}
