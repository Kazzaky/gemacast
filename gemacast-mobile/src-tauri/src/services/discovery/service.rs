//! Pure service functions for discovery, decoupled from Tauri.
//!
//! These functions take trait references as parameters, making the
//! transport classification and network identity logic fully testable.

use crate::traits::{NetworkInfoProvider, PlatformService};
use gemacast_core::domain::types::{ConnectionModes, DeviceId, NetworkLink};

/// Network state returned to the frontend.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkState {
    pub local_ip: String,
    pub network_id: String,
    pub modes: ConnectionModes,
}

pub struct DiscoveryService<'a> {
    network: &'a dyn NetworkInfoProvider,
    platform: &'a dyn PlatformService,
}

impl<'a> DiscoveryService<'a> {
    pub fn new(network: &'a dyn NetworkInfoProvider, platform: &'a dyn PlatformService) -> Self {
        Self { network, platform }
    }

    pub fn local_ip(&self) -> Result<String, String> {
        get_local_ip(self.network)
    }

    pub fn network_identifier(&self) -> Result<String, String> {
        get_network_identifier(self.network)
    }

    pub fn connection_status(&self) -> Result<ConnectionModes, String> {
        get_connection_status(self.network, self.platform)
    }

    pub fn network_state(&self) -> Result<NetworkState, String> {
        get_network_state(self.network, self.platform)
    }

    pub fn forget_pc_identity(&self, pc_id: &DeviceId) -> Result<(), String> {
        forget_pc_identity(self.platform, pc_id)
    }

    pub fn paired_pc_ids(&self) -> Result<Vec<DeviceId>, String> {
        paired_pc_ids(self.platform)
    }

    pub fn phone_link(&self, mode: &str) -> NetworkLink {
        detect_phone_link(self.network, self.platform, mode)
    }
}

/// Get the local IP address as a string.
fn get_local_ip(network: &dyn NetworkInfoProvider) -> Result<String, String> {
    network.get_local_ip().map(|ip| ip.to_string())
}

/// Build a network identifier string from the default interface.
///
/// Format: `"{interface_name}_{mac}_{ip}"`.
fn get_network_identifier(network: &dyn NetworkInfoProvider) -> Result<String, String> {
    let iface = network.get_default_interface()?;
    let mac = iface
        .mac_addr
        .unwrap_or_else(|| "00:00:00:00:00:00".to_string());
    let ip = if let Some(ip) = iface.ipv4.first() {
        std::net::IpAddr::V4(*ip).to_string()
    } else if let Some(ip) = iface.ipv6.first() {
        std::net::IpAddr::V6(*ip).to_string()
    } else {
        "no-ip".to_string()
    };
    Ok(format!("{}_{}_{}", iface.name, mac, ip))
}

/// Tethering state reported by the Android layer, third field of the transport
/// string (`"<transports>|<tether>"`).
///
/// Present because a tethering phone's *active* network is its upstream — often
/// cellular — so `NetworkCapabilities` reports neither WIFI nor ETHERNET, and a
/// Wi-Fi hotspot is otherwise indistinguishable from a USB cable. Classified in
/// Kotlin by `TetherClassifier` (a pure function with JVM tests) because the
/// Android APIs that would answer directly are all `@SystemApi`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TetherState {
    Hotspot,
    UsbTether,
    None,
}

/// Parse the tether field out of a transport string. Absent field → `None`, so
/// an older Kotlin layer that does not send it still behaves as before.
impl TetherState {
    pub fn from_transport(transport: &str) -> Self {
        match transport.split('|').nth(1).unwrap_or("") {
            "HOTSPOT" => Self::Hotspot,
            "USB_TETHER" => Self::UsbTether,
            _ => Self::None,
        }
    }
}

/// Determine which connection modes are available.
///
/// Checks the platform transport type (Android JNI) and enriches
/// with local network interface information.
fn get_connection_status(
    network: &dyn NetworkInfoProvider,
    platform: &dyn PlatformService,
) -> Result<ConnectionModes, String> {
    let mut modes = gemacast_core::domain::types::get_available_connection_modes();

    // Platform-specific transport detection (Android JNI)
    if let Ok(transport_str) = platform.get_transport_type() {
        modes.wifi = false;
        modes.usb = false;

        let parts: Vec<&str> = transport_str.split('|').collect();
        let network_type = parts.first().unwrap_or(&"");
        for transport in network_type.split(',') {
            if transport == "WIFI" || transport.starts_with("WIFI:") {
                modes.wifi = true;
            } else if transport == "ETHERNET" {
                modes.usb = true;
            }
        }

        // A hotspot is an IP-routable radio link, i.e. Wi-Fi mode. Without this
        // the Wi-Fi button stayed disabled and USB was the only option, which
        // forced the cable jitter profile onto a radio link.
        match TetherState::from_transport(&transport_str) {
            TetherState::Hotspot => modes.wifi = true,
            TetherState::UsbTether => modes.usb = true,
            TetherState::None => {}
        }
    }

    // Enrich with interface classification
    let interfaces = network.get_interfaces();
    for iface in interfaces {
        if iface.is_wifi && !iface.ipv4.is_empty() {
            modes.wifi = true;
        }
        if iface.is_usb && !iface.ipv4.is_empty() {
            modes.usb = true;
        }
    }

    Ok(modes)
}

/// Get a combined network state snapshot.
fn get_network_state(
    network: &dyn NetworkInfoProvider,
    platform: &dyn PlatformService,
) -> Result<NetworkState, String> {
    let local_ip = network
        .get_local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string());

    let network_id = get_network_identifier(network).unwrap_or_else(|_| local_ip.clone());

    let modes = get_connection_status(network, platform).unwrap_or(ConnectionModes {
        wifi: true,
        usb: false,
    });

    Ok(NetworkState {
        local_ip,
        network_id,
        modes,
    })
}

/// Remove one locally stored PC certificate pin.
fn forget_pc_identity(platform: &dyn PlatformService, pc_id: &DeviceId) -> Result<(), String> {
    platform.forget_pc_identity(pc_id)
}

/// Return the locally stored PC identities used by the paired-PC settings UI.
fn paired_pc_ids(platform: &dyn PlatformService) -> Result<Vec<DeviceId>, String> {
    platform.paired_pc_ids()
}

/// Detect the phone's network link type from platform transport info.
///
/// Combines the user-selected connection mode with the Android JNI transport
/// string (e.g., `"WIFI:5180"`) to determine the best description
/// of the phone's network link quality.
fn detect_phone_link(
    network: &dyn NetworkInfoProvider,
    platform: &dyn PlatformService,
    mode: &str, // "wifi" or "usb"
) -> NetworkLink {
    let transport_str = platform.get_transport_type().unwrap_or_default();
    tracing::info!(
        mode,
        transport = %transport_str,
        "Detecting phone network link"
    );

    // USB mode → Always USB Tether
    if mode == "usb" {
        tracing::info!(link = ?NetworkLink::UsbTether, "Phone link detected (USB mode)");
        return NetworkLink::UsbTether;
    }

    // 3. WIFI mode → Only look for WiFi transports from Android JNI
    if mode == "wifi" && !transport_str.is_empty() {
        // A hotspot answers before the STA frequency is read, because those two
        // radios are not the same link. In Wi-Fi+hotspot concurrent mode
        // `WifiInfo.getFrequency()` reports the *upstream* channel this phone is
        // a client of, which says nothing about the AP the PC is attached to.
        //
        // We report `WifiUnknown` rather than assuming a band, and that is what
        // makes the band knowable at all: the PC *is* a client of this AP, so it
        // measures the real channel (`netsh wlan show interfaces` and friends)
        // and `LinkPair::effective_link()` resolves the pair in the PC's favour —
        // its rule 3 returns `Wifi5Ghz` when the other side is `WifiUnknown`.
        // Asserting `Wifi2_4Ghz` here would instead hit rule 2 and let a guess
        // override that measurement, costing latency on a clean 5 GHz hotspot.
        if TetherState::from_transport(&transport_str) == TetherState::Hotspot {
            tracing::info!(
                link = ?NetworkLink::WifiUnknown,
                "Phone link detected (Wi-Fi hotspot; band is measured by the PC side)"
            );
            return NetworkLink::WifiUnknown;
        }

        let parts: Vec<&str> = transport_str.split('|').collect();
        let network_type = parts.first().unwrap_or(&"");

        let mut best_wifi: Option<(NetworkLink, u32)> = None;

        for transport in network_type.split(',') {
            // "WIFI:5180" → parse frequency in MHz
            if let Some(freq_str) = transport.strip_prefix("WIFI:") {
                if let Ok(freq) = freq_str.parse::<u32>() {
                    let link = if freq >= 4000 {
                        NetworkLink::Wifi5Ghz
                    } else {
                        NetworkLink::Wifi2_4Ghz
                    };
                    best_wifi = Some((link, freq));
                } else if best_wifi.is_none() {
                    best_wifi = Some((NetworkLink::WifiUnknown, 0));
                }
            } else if transport == "WIFI" && best_wifi.is_none() {
                best_wifi = Some((NetworkLink::WifiUnknown, 0));
            }
        }

        if let Some((link, freq)) = best_wifi {
            if freq > 0 {
                tracing::info!(
                    ?link,
                    freq_mhz = freq,
                    "Phone link detected (WiFi frequency)"
                );
            } else {
                tracing::info!(?link, "Phone link detected (WiFi, no freq)");
            }
            return link;
        }
    }

    // 4. Fallback: check interfaces for WiFi/USB
    let interfaces = network.get_interfaces();
    for iface in interfaces {
        if iface.is_usb && !iface.ipv4.is_empty() {
            tracing::info!(link = ?NetworkLink::UsbTether, iface = %iface.name, "Phone link detected (interface)");
            return NetworkLink::UsbTether;
        }
        if iface.is_wifi && !iface.ipv4.is_empty() {
            tracing::info!(link = ?NetworkLink::WifiUnknown, iface = %iface.name, "Phone link detected (interface)");
            return NetworkLink::WifiUnknown;
        }
    }

    tracing::warn!("Could not detect phone network link, falling back to Unknown");
    NetworkLink::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::mocks::*;
    use crate::traits::InterfaceInfo;

    #[test]
    fn get_network_identifier_formats_correctly() {
        let network = MockNetworkInfoProvider::new().with_default_interface(InterfaceInfo {
            name: "wlan0".to_string(),
            mac_addr: Some("AA:BB:CC:DD:EE:FF".to_string()),
            ipv4: vec!["192.168.1.100".parse().unwrap()],
            ipv6: vec![],
            is_wifi: true,
            is_usb: false,
        });

        let result = get_network_identifier(&network).unwrap();
        assert_eq!(result, "wlan0_AA:BB:CC:DD:EE:FF_192.168.1.100");
    }

    #[test]
    fn get_network_identifier_uses_default_mac_when_missing() {
        let network = MockNetworkInfoProvider::new().with_default_interface(InterfaceInfo {
            name: "eth0".to_string(),
            mac_addr: None,
            ipv4: vec!["10.0.0.1".parse().unwrap()],
            ipv6: vec![],
            is_wifi: false,
            is_usb: false,
        });

        let result = get_network_identifier(&network).unwrap();
        assert!(result.contains("00:00:00:00:00:00"));
    }

    #[test]
    fn get_connection_status_parses_wifi_transport() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("WIFI");

        let modes = get_connection_status(&network, &platform).unwrap();
        assert!(modes.wifi);
        assert!(!modes.usb);
    }

    #[test]
    fn forget_pc_identity_delegates_to_platform() {
        let platform = MockPlatformService::new();
        let pc_id = DeviceId("pc-1".into());
        forget_pc_identity(&platform, &pc_id).unwrap();
        assert!(platform.calls.lock().unwrap().iter().any(|call| {
            matches!(call, PlatformCall::ForgetPcIdentity { pc_id: observed } if observed == &pc_id)
        }));
    }

    #[test]
    fn paired_pc_ids_delegates_to_platform() {
        let expected = vec![DeviceId("pc-1".into()), DeviceId("pc-2".into())];
        let platform = MockPlatformService::new().with_paired_pc_ids(expected.clone());

        assert_eq!(paired_pc_ids(&platform).unwrap(), expected);
        assert!(
            platform
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| matches!(call, PlatformCall::PairedPcIds))
        );
    }


    #[test]
    fn get_connection_status_detects_ethernet_as_usb() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("ETHERNET");

        let modes = get_connection_status(&network, &platform).unwrap();
        assert!(!modes.wifi);
        assert!(modes.usb);
    }

    #[test]
    fn get_connection_status_enriches_from_interfaces() {
        let network = MockNetworkInfoProvider::new().with_interfaces(vec![InterfaceInfo {
            name: "wlan0".to_string(),
            mac_addr: None,
            ipv4: vec!["192.168.1.100".parse().unwrap()],
            ipv6: vec![],
            is_wifi: true,
            is_usb: false,
        }]);
        let platform = MockPlatformService::new(); // get_transport_type returns Err

        let modes = get_connection_status(&network, &platform).unwrap();
        assert!(modes.wifi);
    }

    #[test]
    fn get_connection_status_parses_combined_transports() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("WIFI,ETHERNET");

        let modes = get_connection_status(&network, &platform).unwrap();
        assert!(modes.wifi);
        assert!(modes.usb);
        assert!(modes.adb);
    }

    #[test]
    fn get_connection_status_handles_wifi_with_frequency() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("WIFI:5180");

        let modes = get_connection_status(&network, &platform).unwrap();
        assert!(modes.wifi, "WIFI:5180 should still enable wifi mode");
        assert!(!modes.adb);
    }

    // ---------------------------------------------------------------
    // Hotspot vs USB tether
    // ---------------------------------------------------------------

    #[test]
    fn parse_tether_state_reads_the_third_field() {
        assert_eq!(
            TetherState::from_transport("NONE|HOTSPOT"),
            TetherState::Hotspot
        );
        assert_eq!(
            TetherState::from_transport("WIFI:5180|USB_TETHER"),
            TetherState::UsbTether
        );
        assert_eq!(
            TetherState::from_transport("WIFI"),
            TetherState::None
        );
        assert_eq!(TetherState::from_transport(""), TetherState::None);
    }

    #[test]
    fn a_hotspot_enables_wifi_mode_rather_than_usb() {
        // The reported bug. A tethering phone's active network is its cellular
        // upstream, so neither WIFI nor ETHERNET is reported and `wifi` used to
        // stay false — leaving USB as the only selectable mode for a radio link.
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("NONE|ADB_OFF|HOTSPOT");

        let modes = get_connection_status(&network, &platform).unwrap();
        assert!(modes.wifi, "A Wi-Fi hotspot must enable Wi-Fi mode");
        assert!(!modes.usb, "A Wi-Fi hotspot is not a cable");
    }

    #[test]
    fn a_usb_tether_still_enables_usb_mode() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("NONE|USB_TETHER");

        let modes = get_connection_status(&network, &platform).unwrap();
        assert!(modes.usb, "A USB tether must enable USB mode");
    }

    #[test]
    fn detect_phone_link_reports_unknown_band_for_a_hotspot_so_the_pc_measurement_wins() {
        // In Wi-Fi+hotspot concurrent mode the STA frequency describes the
        // *upstream* link, not the AP the PC is attached to. Claiming a band here
        // would beat the PC's real channel measurement in `effective_link`.
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("WIFI:5180|HOTSPOT");

        let link = detect_phone_link(&network, &platform, "wifi");
        assert_eq!(
            link,
            NetworkLink::WifiUnknown,
            "A hotspot must not assert the upstream STA band as its own"
        );
    }

    #[test]
    fn detect_phone_link_still_reads_the_band_without_a_hotspot() {
        // The guard must be scoped to hotspots only: an ordinary Wi-Fi client
        // still gets its band from the JNI frequency.
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("WIFI:5180|ADB_OFF");

        let link = detect_phone_link(&network, &platform, "wifi");
        assert_eq!(link, NetworkLink::Wifi5Ghz);
    }

    #[test]
    fn a_hotspot_pair_resolves_to_the_band_the_pc_measured() {
        // End to end over the two crates: phone cannot know the band, PC can,
        // and the pair must land on the PC's measurement rather than a fallback.
        use gemacast_core::domain::types::LinkPair;

        let pair_5 = LinkPair {
            phone: NetworkLink::WifiUnknown,
            pc: NetworkLink::Wifi5Ghz,
        };
        assert_eq!(pair_5.effective_link(), NetworkLink::Wifi5Ghz);

        let pair_24 = LinkPair {
            phone: NetworkLink::WifiUnknown,
            pc: NetworkLink::Wifi2_4Ghz,
        };
        assert_eq!(pair_24.effective_link(), NetworkLink::Wifi2_4Ghz);

        // Neither side could measure: falls back to the conservative Wi-Fi
        // profile, never to the 100 ms-capped cable profile.
        let pair_unknown = LinkPair {
            phone: NetworkLink::WifiUnknown,
            pc: NetworkLink::WifiUnknown,
        };
        assert_eq!(pair_unknown.effective_link(), NetworkLink::WifiUnknown);
    }

    // ---------------------------------------------------------------
    // detect_phone_link
    // ---------------------------------------------------------------


    #[test]
    fn detect_phone_link_wifi_5ghz_frequency() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("WIFI:5180|ADB_OFF");

        let link = detect_phone_link(&network, &platform, "wifi");
        assert_eq!(link, NetworkLink::Wifi5Ghz);
    }

    #[test]
    fn detect_phone_link_wifi_2_4ghz_frequency() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("WIFI:2412");

        let link = detect_phone_link(&network, &platform, "wifi");
        assert_eq!(link, NetworkLink::Wifi2_4Ghz);
    }

    #[test]
    fn detect_phone_link_wifi_6ghz_treated_as_5ghz() {
        let network = MockNetworkInfoProvider::new();
        // 6 GHz channel, e.g., 5955 MHz
        let platform = MockPlatformService::new().with_transport_type("WIFI:5955");

        let link = detect_phone_link(&network, &platform, "wifi");
        assert_eq!(link, NetworkLink::Wifi5Ghz);
    }

    #[test]
    fn detect_phone_link_wifi_no_frequency() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("WIFI|ADB_OFF");

        let link = detect_phone_link(&network, &platform, "wifi");
        assert_eq!(link, NetworkLink::WifiUnknown);
    }

    #[test]
    fn detect_phone_link_ethernet_transport_is_usb_tether() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("ETHERNET|ADB_ON");

        let link = detect_phone_link(&network, &platform, "usb");
        assert_eq!(link, NetworkLink::UsbTether);
    }

    #[test]
    fn detect_phone_link_usb_mode_without_transport() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new(); // get_transport_type returns Err

        let link = detect_phone_link(&network, &platform, "usb");
        assert_eq!(link, NetworkLink::UsbTether);
    }

    #[test]
    fn detect_phone_link_fallback_to_interfaces() {
        let network = MockNetworkInfoProvider::new().with_interfaces(vec![InterfaceInfo {
            name: "wlan0".to_string(),
            mac_addr: None,
            ipv4: vec!["192.168.1.100".parse().unwrap()],
            ipv6: vec![],
            is_wifi: true,
            is_usb: false,
        }]);
        let platform = MockPlatformService::new(); // get_transport_type returns Err

        let link = detect_phone_link(&network, &platform, "wifi");
        assert_eq!(link, NetworkLink::WifiUnknown);
    }

    #[test]
    fn detect_phone_link_no_info_returns_unknown() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new(); // get_transport_type returns Err

        let link = detect_phone_link(&network, &platform, "wifi");
        assert_eq!(link, NetworkLink::Unknown);
    }
}
