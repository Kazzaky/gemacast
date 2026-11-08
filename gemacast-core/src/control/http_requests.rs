use crate::control::http_client::HttpControlClient;
use crate::control::http_transport::request_error;
use crate::control::tls::response_certificate_fingerprint;
use crate::control::types::{
    ChangeBitrateReq, ChangeSourceReq, DisconnectReq, PresenceResponse, ProbeReq,
    ProcessListResponse, SourcesResponse,
};
use crate::domain::error::{ControlError, GemaCastError};
use crate::domain::types::{AudioSource, DeviceId, ProcessInfo, StreamerCapabilities};

impl HttpControlClient {
    pub async fn send_disconnect_request(&self, device_id: DeviceId) -> Result<(), GemaCastError> {
        let credential = self.credential_for(Some(&device_id));
        let client = self.client_for(credential.as_ref())?;
        let response = Self::authorize(
            client.post(format!("{}/disconnect", self.base_url)),
            credential.as_ref(),
        )
        .json(&DisconnectReq { device_id })
        .send()
        .await
        .map_err(request_error)?;
        Self::ensure_success(response).await?;
        if let Ok(mut credentials) = self.credentials.lock() {
            *credentials = None;
        }
        Ok(())
    }

    pub async fn request_audio_sources(
        &self,
    ) -> Result<(Vec<AudioSource>, StreamerCapabilities), GemaCastError> {
        let credential = self.credential_for(None);
        let client = self.client_for(credential.as_ref())?;
        let response = Self::authorize(
            client.get(format!("{}/sources", self.base_url)),
            credential.as_ref(),
        )
        .send()
        .await
        .map_err(request_error)?;
        let response: SourcesResponse = Self::ensure_success(response)
            .await?
            .json()
            .await
            .map_err(request_error)?;
        Ok((response.sources, response.capabilities))
    }

    pub async fn send_change_source_request(
        &self,
        device_id: DeviceId,
        source: AudioSource,
    ) -> Result<(), GemaCastError> {
        let credential = self.credential_for(Some(&device_id));
        let client = self.client_for(credential.as_ref())?;
        let response = Self::authorize(
            client.post(format!("{}/change-source", self.base_url)),
            credential.as_ref(),
        )
        .json(&ChangeSourceReq { device_id, source })
        .send()
        .await
        .map_err(request_error)?;
        Self::ensure_success(response).await?;
        Ok(())
    }

    pub async fn send_change_bitrate_request(
        &self,
        device_id: DeviceId,
        bitrate: Option<i32>,
    ) -> Result<(), GemaCastError> {
        let credential = self.credential_for(Some(&device_id));
        let client = self.client_for(credential.as_ref())?;
        let response = Self::authorize(
            client.post(format!("{}/change-bitrate", self.base_url)),
            credential.as_ref(),
        )
        .json(&ChangeBitrateReq { device_id, bitrate })
        .send()
        .await
        .map_err(request_error)?;
        Self::ensure_success(response).await?;
        Ok(())
    }

    pub async fn send_probe(
        &self,
        device_id: Option<DeviceId>,
    ) -> Result<PresenceResponse, GemaCastError> {
        let credential = self.credential_for(device_id.as_ref());
        let client = self.client_for(credential.as_ref())?;
        let response = Self::authorize(
            client.post(format!("{}/probe", self.base_url)),
            credential.as_ref(),
        )
        .json(&ProbeReq { device_id })
        .send()
        .await
        .map_err(request_error)?;
        let observed_fingerprint = response_certificate_fingerprint(&response)
            .map_err(|reason| ControlError::Rejected { reason })?;
        let presence = Self::ensure_success(response)
            .await?
            .json::<PresenceResponse>()
            .await
            .map_err(request_error)?;
        Self::verify_presence_certificate(&presence, &observed_fingerprint)?;
        Ok(presence)
    }

    pub async fn request_process_list(&self) -> Result<Vec<ProcessInfo>, GemaCastError> {
        let credential = self.credential_for(None);
        let client = self.client_for(credential.as_ref())?;
        let response = Self::authorize(
            client.get(format!("{}/processes", self.base_url)),
            credential.as_ref(),
        )
        .send()
        .await
        .map_err(request_error)?;
        let response: ProcessListResponse = Self::ensure_success(response)
            .await?
            .json()
            .await
            .map_err(request_error)?;
        Ok(response.processes)
    }
}
