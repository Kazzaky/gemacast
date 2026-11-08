/// One port per Gemacast service, so firewall rules stay simple.
///
/// Keep these in 1024..32768. Below 1024 needs root; 32768 and up is ephemeral
/// range on Linux and Android (49152 on Windows and macOS), and the OS will hand
/// a port in there to someone else's outbound socket before we can bind it.
pub struct Ports;

impl Ports {
    /// UDP presence broadcast, PC -> network. Carries `Presence` and `Probe` only.
    pub const DISCOVERY: u16 = 23555;

    /// HTTPS/WSS control channel, phone -> PC (Axum).
    pub const CONTROL: u16 = 23559;

    /// UDP real-time audio, PC -> phone.
    pub const AUDIO_UDP: u16 = 23556;
}
