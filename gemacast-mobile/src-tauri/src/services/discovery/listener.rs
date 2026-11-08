use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokio::task::JoinHandle;

use crate::HEARTBEAT_CHECK_INTERVAL_SECS;
use crate::STREAMER_HEARTBEAT_TIMEOUT_SECS;
use crate::traits::FrontendNotifier;

use gemacast_core::domain::types::{ConnectionMode, DeviceId};

use super::dispatch::DispatchContext;

pub struct DiscoveryListener;

impl DiscoveryListener {
    pub fn spawn(
        listener: gemacast_core::network::PresenceListener,
        mut presence_message_rx: tokio::sync::mpsc::Receiver<(
            gemacast_core::control::messages::ControlMessage,
            std::net::SocketAddr,
        )>,
        notifier: Arc<dyn FrontendNotifier>,
        device_id: DeviceId,
        mode: ConnectionMode,
        is_streaming: Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut set = tokio::task::JoinSet::new();
            let ctx = DispatchContext::new(notifier.clone());

            let socket = listener.socket.clone();
            set.spawn(async move {
                if let Err(e) = listener.run_receive_loop().await {
                    tracing::error!("Discovery listener failed: {}", e);
                    std::process::exit(1);
                }
            });

            // Heartbeat watchdog — delegates tick logic to heartbeat::evict_stale_streamers
            let streamer_heartbeat_tracker = ctx.streamer_last_seen.clone();
            let notifier_for_watchdog = notifier.clone();
            set.spawn(async move {
                let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(
                    HEARTBEAT_CHECK_INTERVAL_SECS,
                ));
                loop {
                    interval.tick().await;
                    super::heartbeat::HeartbeatMonitor::evict_stale_streamers(
                        notifier_for_watchdog.as_ref(),
                        &streamer_heartbeat_tracker,
                        Duration::from_secs(STREAMER_HEARTBEAT_TIMEOUT_SECS),
                    );
                }
            });

            set.spawn(super::probe::SubnetProbe::run(
                socket,
                device_id.clone(),
                mode,
                is_streaming,
            ));


            while let Some((message, addr)) = presence_message_rx.recv().await {
                ctx.dispatch(message, addr, mode);
            }
        })
    }
}
