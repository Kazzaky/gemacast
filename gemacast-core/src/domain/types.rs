use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::fmt;
use std::net::SocketAddr;

pub const MIN_OPUS_BITRATE_BPS: i32 = 6_000;
pub const MAX_OPUS_BITRATE_BPS: i32 = 512_000;

/// Validated streamer-side encoding mode.
///
/// The public control protocol intentionally keeps using `Option<i32>` for
/// backwards compatibility (`None` means raw PCM). Production code converts to
/// this type at the boundary so an invalid bitrate can never reach Opus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioBitrate {
    Uncompressed,
    Opus(i32),
}

impl AudioBitrate {
    pub fn from_wire(value: Option<i32>) -> Result<Self, crate::domain::error::GemaCastError> {
        match value {
            None => Ok(Self::Uncompressed),
            Some(bps) if (MIN_OPUS_BITRATE_BPS..=MAX_OPUS_BITRATE_BPS).contains(&bps) => {
                Ok(Self::Opus(bps))
            }
            Some(bps) => Err(crate::domain::error::AudioError::InvalidBitrate {
                value: bps,
                min: MIN_OPUS_BITRATE_BPS,
                max: MAX_OPUS_BITRATE_BPS,
            }
            .into()),
        }
    }

    pub fn to_wire(self) -> Option<i32> {
        match self {
            Self::Uncompressed => None,
            Self::Opus(bps) => Some(bps),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for DeviceId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for DeviceId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl From<String> for DeviceId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl DeviceId {
    pub fn new() -> Self {
        DeviceId(format!(
            "PC_{}",
            whoami::hostname().unwrap_or("UNKNOWN".to_string())
        ))
    }
}

impl Default for DeviceId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportType {
    Wifi,
    Usb,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AudioSource {
    #[default]
    Desktop,
    Process {
        pid: u32,
        name: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub has_audio_session: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamerCapabilities {
    pub supports_process_capture: bool,
    #[serde(default)]
    pub supports_volume_sync: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredDevice {
    pub device_id: DeviceId,
    pub device_name: String,
    pub addr: std::net::SocketAddr,
    #[serde(skip)]
    pub last_seen: std::time::Instant,
    pub is_offline: bool,
    pub transport: Option<TransportType>,
}

/// Audio destination used by the streamer capture pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TargetId {
    Udp(SocketAddr),
    Tcp(DeviceId),
}

impl DiscoveredDevice {
    pub fn from_presence(
        streamer_id: DeviceId,
        streamer_name: String,
        is_offline: bool,
        addr: std::net::SocketAddr,
        transport: Option<TransportType>,
    ) -> Self {
        Self {
            device_id: streamer_id,
            device_name: streamer_name,
            last_seen: std::time::Instant::now(),
            addr,
            is_offline,
            transport,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionMode {
    #[default]
    Wifi,
    Usb,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionModes {
    pub wifi: bool,
    pub usb: bool,
}

pub fn get_available_connection_modes() -> ConnectionModes {
    ConnectionModes {
        wifi: true,
        usb: true,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JitterConfig {
    pub min_depth_ms: u32,
    pub comfort_cap_ms: u32,
    pub peak_decay_halflife_ms: u32,
    pub resume_threshold_pct: f32,
    #[serde(default)]
    pub static_target_ms: Option<u32>,
}

impl Default for JitterConfig {
    fn default() -> Self {
        Self {
            min_depth_ms: 0,
            comfort_cap_ms: 0,
            peak_decay_halflife_ms: 1000,
            resume_threshold_pct: 0.0,
            static_target_ms: None,
        }
    }
}

/// The detected network link type for one side of the connection.
///
/// Used at connection time to determine how aggressive the jitter buffer
/// can be. The [`LinkPair`] combines both sides and picks the weaker link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NetworkLink {
    /// USB tethering (direct cable, RNDIS/ETHERNET transport).
    UsbTether,
    /// WiFi on 5 GHz or 6 GHz band (clean, low jitter).
    Wifi5Ghz,
    /// WiFi on 2.4 GHz band (congested, higher jitter).
    #[serde(rename = "wifi2_4Ghz")]
    Wifi2_4Ghz,
    /// Wired Ethernet.
    Ethernet,
    /// WiFi but band could not be determined.
    WifiUnknown,
    /// Could not determine the link type.
    Unknown,
}

impl NetworkLink {
    /// Quality ranking: lower is better. Used by [`LinkPair::effective_link`]
    /// to pick the weaker side.
    pub fn quality_rank(self) -> u8 {
        match self {
            Self::UsbTether => 0, // Best: direct cable
            Self::Ethernet => 1,              // Excellent: wired LAN
            Self::Wifi5Ghz => 2,              // Great: clean RF band
            Self::WifiUnknown => 3,           // Unknown WiFi: assume mediocre
            Self::Wifi2_4Ghz => 4,            // Poor: congested 2.4 GHz
            Self::Unknown => 5,               // Worst: no information
        }
    }
}

/// Network link information from both sides of the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkPair {
    pub phone: NetworkLink,
    pub pc: NetworkLink,
}

impl LinkPair {
    /// Returns the effective link quality, determining the bottleneck.
    ///
    /// Resolves asymmetrical knowledge (e.g. if the Phone knows it's on 5GHz,
    /// but the PC lacks permissions and reports `WifiUnknown`, we trust the Phone's 5GHz).
    pub fn effective_link(&self) -> NetworkLink {
        let has = |link: NetworkLink| self.phone == link || self.pc == link;

        // 1. Direct USB tethering bypasses radio conditions.
        if has(NetworkLink::UsbTether) {
            return NetworkLink::UsbTether;
        }

        // 2. The worst known physical bottleneck dominates.
        if has(NetworkLink::Wifi2_4Ghz) {
            return NetworkLink::Wifi2_4Ghz;
        }

        // 3. Resolve 5GHz vs Unknowns.
        if has(NetworkLink::Wifi5Ghz) {
            // If the other side is Ethernet, or Unknown (lacks metadata), the bottleneck is 5GHz.
            return NetworkLink::Wifi5Ghz;
        }

        // 4. Resolve Ethernet vs Unknowns.
        if has(NetworkLink::Ethernet) {
            if has(NetworkLink::WifiUnknown) || has(NetworkLink::Unknown) {
                // E.g., PC is Ethernet, but Phone is Unknown. Phones don't use Ethernet,
                // so the phone is almost certainly on Wi-Fi. We must assume a generic Wi-Fi link.
                return NetworkLink::WifiUnknown;
            }
            return NetworkLink::Ethernet;
        }

        // 5. Fallbacks
        if has(NetworkLink::WifiUnknown) {
            return NetworkLink::WifiUnknown;
        }

        NetworkLink::Unknown
    }
}

impl JitterConfig {
    /// Create a `JitterConfig` optimised for the detected [`LinkPair`].
    ///
    /// This is the "Option A: full override" approach — when the user has
    /// the preset set to "Auto", the network-aware profile **replaces** the
    /// generic Auto config entirely.
    pub fn for_link_pair(pair: LinkPair) -> Self {
        match pair.effective_link() {
            // Direct USB tethering.
            // Direct USB tethering can deliver audio in USB-transit batches. The
            // 30–100ms profile provides enough headroom without chasing unrealistic
            // sub-millisecond scheduling jitter.
            NetworkLink::UsbTether => Self {
                min_depth_ms: 30,
                comfort_cap_ms: 100,
                peak_decay_halflife_ms: 500,
                resume_threshold_pct: 0.2,
                static_target_ms: None,
            },
            // Aggressive — wired Ethernet. No DTIM, no scan cycle, no power-save
            // batching: the only jitter is switch queuing, so the buffer can sit
            // genuinely low.
            NetworkLink::Ethernet => Self {
                min_depth_ms: 5,
                comfort_cap_ms: 200,
                peak_decay_halflife_ms: 800,
                resume_threshold_pct: 0.25,
                static_target_ms: None,
            },
            // Clean 5 GHz Wi-Fi. Split out from Ethernet because it is subject to
            // Android DTIM batching, which Ethernet is not.
            //
            // `min_depth_ms: 5` (1 frame) was actively harmful: it let the probe
            // walk the target down to a single frame, which is a *guaranteed*
            // starvation on any real link, and that first starvation is what seeded
            // the learned-floor ratchet in the field log. 30ms is the shallowest
            // depth that is actually survivable.
            //
            // `comfort_cap_ms: 400` covers the observed 100-200ms DTIM
            // inter-cluster gaps with headroom. Raising the cap is only safe
            // because the depth signal (`max_gap_frames`) and the learned floor now
            // both decay on their own — under the old one-way ratchet a higher cap
            // just relocated where the target got permanently stuck.
            NetworkLink::Wifi5Ghz => Self {
                min_depth_ms: 30,
                comfort_cap_ms: 400,
                peak_decay_halflife_ms: 800,
                resume_threshold_pct: 0.25,
                static_target_ms: None,
            },
            // Conservative — congested 2.4 GHz band.
            //
            // `peak_decay_halflife_ms: 0` selects the *adaptive* (stability-aware)
            // peak-decay path in `JitterStats` rather than a fixed half-life. On
            // 2.4 GHz this matters: a fixed 15s half-life pinned `ema_peak` — and
            // therefore the target depth — near its spike level for many seconds
            // after congestion cleared, so the observed latency ballooned and stuck.
            // The adaptive path sheds the peak within seconds once the link goes
            // quiet, while still clamping to a slow 15s decay during a *verified*
            // unstable regime (two macro-spikes within 10s).
            //
            // `comfort_cap_ms: 800` is a deliberate "cover the gap, pay the
            // latency" choice. The Router A field log shows real delivery gaps up
            // to ~600ms; at the old 500ms cap the buffer starved ~30 times in 55s
            // because it *could not* be sized to survive them. 800ms lets the
            // buffer actually cover the worst observed gap, and the self-decaying
            // gap window means it only costs that latency while the gaps persist.
            NetworkLink::Wifi2_4Ghz => Self {
                min_depth_ms: 30,
                comfort_cap_ms: 800,
                peak_decay_halflife_ms: 0,
                resume_threshold_pct: 0.5,
                static_target_ms: None,
            },
            // Fallback — unknown WiFi or completely unknown link
            NetworkLink::WifiUnknown | NetworkLink::Unknown => Self {
                min_depth_ms: 30,
                comfort_cap_ms: 1000,
                peak_decay_halflife_ms: 0,
                resume_threshold_pct: 0.25,
                static_target_ms: None,
            },
        }
    }

    /// Returns `true` if this config looks like the "Auto" preset sentinel.
    ///
    /// Auto is uniquely identified by `peak_decay_halflife_ms == 0` (fully
    /// adaptive) with no static target. This is used by the LinkPair cache
    /// to decide whether to re-apply the network-aware override.
    pub fn is_auto_sentinel(&self) -> bool {
        self.peak_decay_halflife_ms == 0 && self.static_target_ms.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod device_id {
        use super::*;

        #[test]
        fn display_should_output_inner_string() {
            let id = DeviceId("PC_MYHOST".to_string());
            assert_eq!(id.to_string(), "PC_MYHOST");
        }

        #[test]
        fn as_ref_should_return_inner_str() {
            let id = DeviceId("test_dev".to_string());
            let s: &str = id.as_ref();
            assert_eq!(s, "test_dev");
        }

        #[test]
        fn from_string_should_construct_device_id() {
            let id = DeviceId::from("hello".to_string());
            assert_eq!(id.0, "hello");
        }

        #[test]
        fn serde_should_round_trip_as_transparent_string() {
            let id = DeviceId("PC_123".to_string());
            let json = serde_json::to_string(&id).unwrap();
            assert_eq!(json, "\"PC_123\"");
            let parsed: DeviceId = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, id);
        }
    }

    mod audio_source {
        use super::*;

        #[test]
        fn default_should_be_desktop() {
            assert_eq!(AudioSource::default(), AudioSource::Desktop);
        }

        #[test]
        fn desktop_should_round_trip_through_json() {
            let src = AudioSource::Desktop;
            let json = serde_json::to_string(&src).unwrap();
            let parsed: AudioSource = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, AudioSource::Desktop);
        }

        #[test]
        fn process_should_round_trip_with_camel_case_fields() {
            let src = AudioSource::Process {
                pid: 1234,
                name: "chrome.exe".to_string(),
            };
            let json = serde_json::to_string(&src).unwrap();
            assert!(
                json.contains("\"type\":\"process\"") || json.contains("\"type\": \"process\""),
                "Expected tagged type field, got: {json}"
            );
            let parsed: AudioSource = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, src);
        }
    }

    mod transport_type {
        use super::*;

        #[test]
        fn serde_should_use_lowercase_variant_names() {
            let parsed: TransportType = serde_json::from_str("\"wifi\"").unwrap();
            assert_eq!(parsed, TransportType::Wifi);
        }
    }

    mod connection_mode {
        use super::*;

        #[test]
        fn default_should_be_wifi() {
            assert_eq!(ConnectionMode::default(), ConnectionMode::Wifi);
        }
    }

    mod jitter_config {
        use super::*;

        #[test]
        fn default_should_have_expected_field_values() {
            let config = JitterConfig::default();
            assert_eq!(config.min_depth_ms, 0);
            assert_eq!(config.comfort_cap_ms, 0);
            assert_eq!(config.peak_decay_halflife_ms, 1000);
            assert_eq!(config.resume_threshold_pct, 0.0);
            assert!(config.static_target_ms.is_none());
        }

        #[test]
        fn serde_should_default_static_target_to_none_when_absent() {
            let json = r#"{
                "minDepthMs": 10,
                "comfortCapMs": 200,
                "peakDecayHalflifeMs": 3500,
                "resumeThresholdPct": 0.75
            }"#;
            let config: JitterConfig = serde_json::from_str(json).unwrap();
            assert_eq!(config.min_depth_ms, 10);
            assert_eq!(config.comfort_cap_ms, 200);
            assert!(config.static_target_ms.is_none());
        }

        #[test]
        fn serde_should_round_trip_with_static_target() {
            let config = JitterConfig {
                min_depth_ms: 5,
                comfort_cap_ms: 100,
                peak_decay_halflife_ms: 2000,
                resume_threshold_pct: 0.5,
                static_target_ms: Some(50),
            };
            let json = serde_json::to_string(&config).unwrap();
            let parsed: JitterConfig = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, config);
        }
    }

    mod network_link {
        use super::*;

        #[test]
        fn quality_rank_order_should_be_cable_best_unknown_worst() {
            assert!(NetworkLink::UsbTether.quality_rank() < NetworkLink::Ethernet.quality_rank());
            assert!(NetworkLink::Ethernet.quality_rank() < NetworkLink::Wifi5Ghz.quality_rank());
            assert!(NetworkLink::Wifi5Ghz.quality_rank() < NetworkLink::WifiUnknown.quality_rank());
            assert!(
                NetworkLink::WifiUnknown.quality_rank() < NetworkLink::Wifi2_4Ghz.quality_rank()
            );
            assert!(NetworkLink::Wifi2_4Ghz.quality_rank() < NetworkLink::Unknown.quality_rank());
        }

        #[test]
        fn serde_should_round_trip_all_variants() {
            let variants = [
                NetworkLink::UsbTether,
                NetworkLink::Wifi5Ghz,
                NetworkLink::Wifi2_4Ghz,
                NetworkLink::Ethernet,
                NetworkLink::WifiUnknown,
                NetworkLink::Unknown,
            ];
            for v in variants {
                let json = serde_json::to_string(&v).unwrap();
                let parsed: NetworkLink = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed, v, "Round-trip failed for {v:?} (json: {json})");
            }
        }

        #[test]
        fn wifi_2_4ghz_should_serialize_with_underscore() {
            let json = serde_json::to_string(&NetworkLink::Wifi2_4Ghz).unwrap();
            assert_eq!(json, "\"wifi2_4Ghz\"");
        }
    }

    mod link_pair {
        use super::*;

        #[test]
        fn effective_link_should_return_weaker_side() {
            let pair = LinkPair {
                phone: NetworkLink::Wifi5Ghz,
                pc: NetworkLink::Wifi2_4Ghz,
            };
            assert_eq!(pair.effective_link(), NetworkLink::Wifi2_4Ghz);
        }

        #[test]
        fn effective_link_should_return_phone_when_phone_is_weaker() {
            let pair = LinkPair {
                phone: NetworkLink::Wifi2_4Ghz,
                pc: NetworkLink::Ethernet,
            };
            assert_eq!(pair.effective_link(), NetworkLink::Wifi2_4Ghz);
        }

        #[test]
        fn effective_link_should_return_phone_when_equal() {
            let pair = LinkPair {
                phone: NetworkLink::Wifi5Ghz,
                pc: NetworkLink::Wifi5Ghz,
            };
            // Equal ranks → phone is returned (>= comparison)
            assert_eq!(pair.effective_link(), NetworkLink::Wifi5Ghz);
        }

        #[test]
        fn effective_link_ethernet_pc_with_unknown_phone() {
            let pair = LinkPair {
                phone: NetworkLink::Unknown,
                pc: NetworkLink::Ethernet,
            };
            // PC is on Ethernet but the phone can't be (phones don't use Ethernet),
            // so it's almost certainly on Wi-Fi of an unknown band → assume a
            // generic Wi-Fi link rather than trusting the PC's wire.
            assert_eq!(pair.effective_link(), NetworkLink::WifiUnknown);
        }

        #[test]
        fn serde_should_round_trip() {
            let pair = LinkPair {
                phone: NetworkLink::Wifi5Ghz,
                pc: NetworkLink::Ethernet,
            };
            let json = serde_json::to_string(&pair).unwrap();
            let parsed: LinkPair = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, pair);
        }
    }

    mod for_link_pair {
        use super::*;

        #[test]
        fn both_5ghz_should_produce_aggressive() {
            let pair = LinkPair {
                phone: NetworkLink::Wifi5Ghz,
                pc: NetworkLink::Wifi5Ghz,
            };
            let config = JitterConfig::for_link_pair(pair);
            assert_eq!(config.min_depth_ms, 30);
            assert_eq!(config.comfort_cap_ms, 400);
            assert_eq!(config.peak_decay_halflife_ms, 800);
        }

        /// Ethernet is no longer folded in with 5 GHz: it has no DTIM batching,
        /// so it keeps the shallow floor and tight cap that 5 GHz had to give up.
        #[test]
        fn ethernet_should_keep_the_shallowest_profile() {
            let pair = LinkPair {
                phone: NetworkLink::Ethernet,
                pc: NetworkLink::Ethernet,
            };
            let config = JitterConfig::for_link_pair(pair);
            assert_eq!(config.min_depth_ms, 5);
            assert_eq!(config.comfort_cap_ms, 200);
            assert!(
                config.comfort_cap_ms
                    < JitterConfig::for_link_pair(LinkPair {
                        phone: NetworkLink::Wifi5Ghz,
                        pc: NetworkLink::Wifi5Ghz,
                    })
                    .comfort_cap_ms,
            );
        }

        #[test]
        fn mixed_wifi_should_use_2_4ghz_profile() {
            let pair = LinkPair {
                phone: NetworkLink::Wifi5Ghz,
                pc: NetworkLink::Wifi2_4Ghz,
            };
            let config = JitterConfig::for_link_pair(pair);
            assert_eq!(config.min_depth_ms, 30);
            assert_eq!(config.comfort_cap_ms, 800);
            assert_eq!(config.peak_decay_halflife_ms, 0);
        }

        #[test]
        fn ethernet_pc_with_2_4ghz_phone_should_use_conservative() {
            let pair = LinkPair {
                phone: NetworkLink::Wifi2_4Ghz,
                pc: NetworkLink::Ethernet,
            };
            let config = JitterConfig::for_link_pair(pair);
            assert_eq!(config.min_depth_ms, 30);
            assert_eq!(config.comfort_cap_ms, 800);
            assert_eq!(config.resume_threshold_pct, 0.5);
        }

        #[test]
        fn unknown_link_should_produce_fallback_auto() {
            let pair = LinkPair {
                phone: NetworkLink::Unknown,
                pc: NetworkLink::Ethernet,
            };
            let config = JitterConfig::for_link_pair(pair);
            assert_eq!(config.min_depth_ms, 30);
            assert_eq!(config.comfort_cap_ms, 1000);
            assert_eq!(config.peak_decay_halflife_ms, 0);
        }

        #[test]
        fn wifi_unknown_should_produce_fallback_auto() {
            let pair = LinkPair {
                phone: NetworkLink::WifiUnknown,
                pc: NetworkLink::WifiUnknown,
            };
            let config = JitterConfig::for_link_pair(pair);
            assert_eq!(config.peak_decay_halflife_ms, 0);
            assert_eq!(config.comfort_cap_ms, 1000);
        }

        #[test]
        fn all_profiles_should_have_no_static_target() {
            let pairs = [
                (NetworkLink::Wifi5Ghz, NetworkLink::Wifi5Ghz),
                (NetworkLink::Wifi2_4Ghz, NetworkLink::Wifi2_4Ghz),
                (NetworkLink::Unknown, NetworkLink::Unknown),
            ];
            for (phone, pc) in pairs {
                let config = JitterConfig::for_link_pair(LinkPair { phone, pc });
                assert!(
                    config.static_target_ms.is_none(),
                    "for_link_pair should always produce adaptive config (no static target)"
                );
            }
        }
    }

    mod is_auto_sentinel {
        use super::*;

        #[test]
        fn auto_config_should_be_detected() {
            let config = JitterConfig {
                min_depth_ms: 25,
                comfort_cap_ms: 1000,
                peak_decay_halflife_ms: 0,
                resume_threshold_pct: 0.25,
                static_target_ms: None,
            };
            assert!(config.is_auto_sentinel());
        }

        #[test]
        fn non_auto_config_should_not_be_detected() {
            let config = JitterConfig {
                min_depth_ms: 10,
                comfort_cap_ms: 200,
                peak_decay_halflife_ms: 3500,
                resume_threshold_pct: 0.75,
                static_target_ms: None,
            };
            assert!(!config.is_auto_sentinel());
        }

        #[test]
        fn no_buffer_should_not_be_auto_sentinel() {
            // No Buffer has staticTargetMs: Some(0), distinguishing it from Auto
            let config = JitterConfig {
                min_depth_ms: 0,
                comfort_cap_ms: 0,
                peak_decay_halflife_ms: 0,
                resume_threshold_pct: 0.0,
                static_target_ms: Some(0),
            };
            assert!(!config.is_auto_sentinel());
        }
    }
}
