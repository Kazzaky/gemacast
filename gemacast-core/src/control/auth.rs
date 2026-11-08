use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::domain::types::DeviceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct SessionGeneration(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedSession {
    pub device_id: DeviceId,
    pub generation: SessionGeneration,
}

pub struct PendingSession {
    device_id: DeviceId,
    token: String,
    generation: SessionGeneration,
}

impl PendingSession {
    /// Derive the per-session audio key from the same random token sent only
    /// over the certificate-pinned TLS control channel.
    pub fn audio_key(&self) -> crate::audio::AudioSessionKey {
        crate::audio::AudioSessionKey::from_session_token(&self.token)
            .expect("SessionAuthorizer always creates a 256-bit hexadecimal token")
    }

    pub fn generation(&self) -> SessionGeneration {
        self.generation
    }
}

#[derive(Debug, Clone)]
struct SessionRecord {
    token: String,
    generation: SessionGeneration,
}

#[derive(Default)]
struct AuthorizationState {
    sessions: HashMap<DeviceId, SessionRecord>,
    pending: HashMap<String, PendingApproval>,
    next_generation: u64,
}

const PENDING_APPROVAL_TTL: Duration = Duration::from_secs(65);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingApprovalStatus {
    Pending,
    Approved,
    Rejected,
}

struct PendingApproval {
    device_id: DeviceId,
    status: PendingApprovalStatus,
    created_at: Instant,
}

/// In-memory per-device authorization state.
///
/// Tokens intentionally die with the PC process. A reconnect rotates both the
/// token and generation, invalidating delayed requests and stale WebSockets.
#[derive(Clone)]
pub struct SessionAuthorizer {
    state: Arc<Mutex<AuthorizationState>>,
    pending_ttl: Duration,
}

impl Default for SessionAuthorizer {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(AuthorizationState::default())),
            pending_ttl: PENDING_APPROVAL_TTL,
        }
    }
}

impl SessionAuthorizer {
    fn prune_pending(&self, state: &mut AuthorizationState) {
        let ttl = self.pending_ttl;
        state
            .pending
            .retain(|_, request| request.created_at.elapsed() <= ttl);
    }

    pub fn pending_request_id(&self) -> Result<String, String> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes)
            .map_err(|error| format!("failed to generate request id: {error}"))?;
        Ok(hex::encode(bytes))
    }

    pub fn create_pending(&self, device_id: DeviceId) -> Result<String, String> {
        let request_id = self.pending_request_id()?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "authorization state is unavailable".to_string())?;
        self.prune_pending(&mut state);
        state
            .pending
            .retain(|_, request| request.device_id != device_id);
        state.pending.insert(
            request_id.clone(),
            PendingApproval {
                device_id,
                status: PendingApprovalStatus::Pending,
                created_at: Instant::now(),
            },
        );
        Ok(request_id)
    }

    pub fn pending_status(
        &self,
        request_id: &str,
        device_id: &DeviceId,
    ) -> Option<PendingApprovalStatus> {
        self.state.lock().ok().and_then(|mut state| {
            self.prune_pending(&mut state);
            state
                .pending
                .get(request_id)
                .filter(|request| &request.device_id == device_id)
                .map(|request| request.status)
        })
    }

    pub fn resolve_pending(&self, request_id: &str, approved: bool) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        self.prune_pending(&mut state);
        let Some(request) = state.pending.get_mut(request_id) else {
            return false;
        };
        if request.status != PendingApprovalStatus::Pending {
            return false;
        }
        request.status = if approved {
            PendingApprovalStatus::Approved
        } else {
            PendingApprovalStatus::Rejected
        };
        true
    }

    pub fn remove_pending(&self, request_id: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.pending.remove(request_id);
        }
    }

    pub fn cancel_pending_for_device(&self, device_id: &DeviceId) -> usize {
        let Ok(mut state) = self.state.lock() else {
            return 0;
        };
        self.prune_pending(&mut state);
        let before = state.pending.len();
        state
            .pending
            .retain(|_, request| &request.device_id != device_id);
        before - state.pending.len()
    }

    pub fn issue(&self, device_id: DeviceId) -> Result<(String, SessionGeneration), String> {
        let pending = self.prepare(device_id)?;
        self.commit(pending)
    }

    /// Allocate credentials without invalidating the device's current session.
    /// The caller commits only after the replacement stream has started.
    pub fn prepare(&self, device_id: DeviceId) -> Result<PendingSession, String> {
        let mut token_bytes = [0u8; 32];
        getrandom::fill(&mut token_bytes)
            .map_err(|error| format!("failed to generate session token: {error}"))?;
        let token = hex::encode(token_bytes);

        let mut state = self
            .state
            .lock()
            .map_err(|_| "authorization state is unavailable".to_string())?;
        state.next_generation = state.next_generation.wrapping_add(1).max(1);
        let generation = SessionGeneration(state.next_generation);
        Ok(PendingSession {
            device_id,
            token,
            generation,
        })
    }

    pub fn commit(&self, pending: PendingSession) -> Result<(String, SessionGeneration), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "authorization state is unavailable".to_string())?;
        state.sessions.insert(
            pending.device_id,
            SessionRecord {
                token: pending.token.clone(),
                generation: pending.generation,
            },
        );
        Ok((pending.token, pending.generation))
    }

    pub fn authenticate(&self, device_id: &DeviceId, token: &str) -> Option<AuthorizedSession> {
        let state = self.state.lock().ok()?;
        let record = state.sessions.get(device_id)?;
        constant_time_eq(record.token.as_bytes(), token.as_bytes()).then(|| AuthorizedSession {
            device_id: device_id.clone(),
            generation: record.generation,
        })
    }

    pub fn authenticate_token(&self, token: &str) -> Option<AuthorizedSession> {
        let state = self.state.lock().ok()?;
        state.sessions.iter().find_map(|(device_id, record)| {
            constant_time_eq(record.token.as_bytes(), token.as_bytes()).then(|| AuthorizedSession {
                device_id: device_id.clone(),
                generation: record.generation,
            })
        })
    }

    pub fn is_current(&self, device_id: &DeviceId, generation: SessionGeneration) -> bool {
        self.state.lock().ok().and_then(|state| {
            state
                .sessions
                .get(device_id)
                .map(|record| record.generation)
        }) == Some(generation)
    }

    pub fn has_session(&self, device_id: &DeviceId) -> bool {
        self.state
            .lock()
            .is_ok_and(|state| state.sessions.contains_key(device_id))
    }

    pub fn revoke(&self, device_id: &DeviceId, generation: Option<SessionGeneration>) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if generation.is_some_and(|generation| {
            state
                .sessions
                .get(device_id)
                .map(|record| record.generation)
                != Some(generation)
        }) {
            return false;
        }
        state.sessions.remove(device_id).is_some()
    }

    pub fn revoke_all(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.sessions.clear();
            state.pending.clear();
        }
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issuing_a_new_session_should_invalidate_the_old_token_and_generation() {
        let authorizer = SessionAuthorizer::default();
        let device_id = DeviceId("phone-1".into());
        let (old_token, old_generation) = authorizer.issue(device_id.clone()).unwrap();
        let (new_token, new_generation) = authorizer.issue(device_id.clone()).unwrap();

        assert_ne!(old_token, new_token);
        assert_ne!(old_generation, new_generation);
        assert!(authorizer.authenticate(&device_id, &old_token).is_none());
        assert!(authorizer.authenticate(&device_id, &new_token).is_some());
        assert!(!authorizer.is_current(&device_id, old_generation));
        assert!(authorizer.is_current(&device_id, new_generation));
    }

    #[test]
    fn stale_revocation_should_not_remove_the_current_session() {
        let authorizer = SessionAuthorizer::default();
        let device_id = DeviceId("phone-1".into());
        let (_, old_generation) = authorizer.issue(device_id.clone()).unwrap();
        let (token, new_generation) = authorizer.issue(device_id.clone()).unwrap();

        assert!(!authorizer.revoke(&device_id, Some(old_generation)));
        assert!(authorizer.authenticate(&device_id, &token).is_some());
        assert!(authorizer.revoke(&device_id, Some(new_generation)));
        assert!(authorizer.authenticate(&device_id, &token).is_none());
    }

    #[test]
    fn one_devices_token_should_not_authorize_another_device() {
        let authorizer = SessionAuthorizer::default();
        let phone_one = DeviceId("phone-1".into());
        let phone_two = DeviceId("phone-2".into());
        let (token, _) = authorizer.issue(phone_one.clone()).unwrap();

        assert!(authorizer.authenticate(&phone_one, &token).is_some());
        assert!(authorizer.authenticate(&phone_two, &token).is_none());
    }

    #[test]
    fn pending_approval_should_be_bound_to_the_requesting_device() {
        let authorizer = SessionAuthorizer::default();
        let phone_one = DeviceId("phone-1".into());
        let phone_two = DeviceId("phone-2".into());
        let request_id = authorizer.create_pending(phone_one.clone()).unwrap();

        assert_eq!(
            authorizer.pending_status(&request_id, &phone_one),
            Some(PendingApprovalStatus::Pending)
        );
        assert_eq!(authorizer.pending_status(&request_id, &phone_two), None);
        assert!(authorizer.resolve_pending(&request_id, true));
        assert_eq!(
            authorizer.pending_status(&request_id, &phone_one),
            Some(PendingApprovalStatus::Approved)
        );
    }

    #[test]
    fn expired_pending_approval_should_reject_late_dialog_results() {
        let authorizer = SessionAuthorizer {
            pending_ttl: Duration::ZERO,
            ..SessionAuthorizer::default()
        };
        let device_id = DeviceId("phone-1".into());
        let request_id = authorizer.create_pending(device_id.clone()).unwrap();

        assert_eq!(authorizer.pending_status(&request_id, &device_id), None);
        assert!(!authorizer.resolve_pending(&request_id, true));
    }

    #[test]
    fn cancelling_a_device_should_remove_only_its_pending_approval() {
        let authorizer = SessionAuthorizer::default();
        let phone_one = DeviceId("phone-1".into());
        let phone_two = DeviceId("phone-2".into());
        let request_one = authorizer.create_pending(phone_one.clone()).unwrap();
        let request_two = authorizer.create_pending(phone_two.clone()).unwrap();

        assert_eq!(authorizer.cancel_pending_for_device(&phone_one), 1);
        assert_eq!(authorizer.pending_status(&request_one, &phone_one), None);
        assert_eq!(
            authorizer.pending_status(&request_two, &phone_two),
            Some(PendingApprovalStatus::Pending)
        );
    }

    #[test]
    fn has_session_should_follow_issue_and_revoke() {
        let authorizer = SessionAuthorizer::default();
        let device_id = DeviceId("phone-1".into());

        assert!(!authorizer.has_session(&device_id));
        let (_, generation) = authorizer.issue(device_id.clone()).unwrap();
        assert!(authorizer.has_session(&device_id));
        assert!(authorizer.revoke(&device_id, Some(generation)));
        assert!(!authorizer.has_session(&device_id));
    }
}
