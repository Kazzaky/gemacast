use thiserror::Error as ThisError;

#[derive(ThisError, Debug)]
pub enum GemaCastError {
    #[error(transparent)]
    Protocol(#[from] ProtocolError),

    #[error(transparent)]
    Audio(#[from] AudioError),

    #[error(transparent)]
    Network(#[from] NetworkError),

    #[error(transparent)]
    Control(#[from] ControlError),

    #[error(transparent)]
    Updater(#[from] UpdaterError),
}

#[derive(ThisError, Debug)]
pub enum ProtocolError {
    #[error("unsupported audio format: {0}")]
    UnsupportedAudioFormat(u8),

    #[error("invalid PCM datagram: {reason}")]
    InvalidPcmDatagram { reason: &'static str },

    #[error("packet too short: expected at least {min} bytes, got {got}")]
    PacketTooShort { got: usize, min: usize },

    #[error("declared packet length {declared} exceeds buffer length {available}")]
    PacketLengthExceedsBuffer { declared: usize, available: usize },

    #[error("packet payload is too large: maximum {max} bytes, got {got}")]
    PacketPayloadTooLarge { got: usize, max: usize },

}

#[derive(Debug, Clone, Copy)]
pub enum StreamDirection {
    Input,
    Output,
}

impl std::fmt::Display for StreamDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input => write!(f, "input"),
            Self::Output => write!(f, "output"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum CodecDirection {
    Encoder,
    Decoder,
}

impl std::fmt::Display for CodecDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encoder => write!(f, "encoder"),
            Self::Decoder => write!(f, "decoder"),
        }
    }
}

#[derive(ThisError, Debug)]
pub enum AudioError {
    #[error("audio host is not available")]
    HostUnavailable(#[source] cpal::HostUnavailable),

    #[error("no default output device available")]
    NoOutputDevice,

    #[error("failed to get default stream config from output device")]
    StreamConfigUnavailable(#[from] cpal::DefaultStreamConfigError),

    #[error("failed to build {direction} stream on output device")]
    BuildStreamFailed {
        direction: StreamDirection,
        #[source]
        source: cpal::BuildStreamError,
    },

    #[error("failed to play {direction} stream")]
    PlayStreamFailed {
        direction: StreamDirection,
        #[source]
        source: cpal::PlayStreamError,
    },

    #[error("failed to pause {direction} stream")]
    PauseStreamFailed {
        direction: StreamDirection,
        #[source]
        source: cpal::PauseStreamError,
    },

    #[error("cpal stream error")]
    StreamError(#[source] cpal::StreamError),

    #[error("failed to create Opus {direction}")]
    OpusInitFailed {
        direction: CodecDirection,
        #[source]
        source: opus::Error,
    },

    #[error("invalid audio bitrate {value} bps (expected {min}..={max}, or null for uncompressed)")]
    InvalidBitrate { value: i32, min: i32, max: i32 },

    #[error("Opus {direction} failed")]
    OpusCodecFailed {
        direction: CodecDirection,
        #[source]
        source: opus::Error,
    },

    #[cfg(target_os = "windows")]
    #[error("Windows API error: {0}")]
    WindowsApi(#[from] windows::core::Error),

    #[error("per-process audio capture is not available on this platform")]
    ProcessCaptureUnavailable,

    #[error("process with PID {0} not found or not producing audio")]
    ProcessNotFound(u32),

    #[error("failed to create capture instance for source: {0}")]
    CaptureInstanceFailed(String),

    #[error("compressed stream has no Opus encoder")]
    MissingOpusEncoder,

    #[error("capture pool is full (max {max} concurrent captures)")]
    CapturePoolExhausted { max: usize },

    #[error("audio resampling failed: {0}")]
    ResampleFailed(String),

    /// A capture backend negotiated a format the pipeline cannot consume.
    ///
    /// The pipeline's contract is fixed at 48 kHz stereo interleaved `f32` (see
    /// [`crate::ports::capture`]). Where a platform can be resampled or downmixed
    /// into that shape, the adapter does so and this error never appears. It is
    /// raised only when adaptation is impossible — a planar layout, a non-`f32`
    /// sample type the decoder does not handle, or a degenerate descriptor such as
    /// zero channels.
    #[error(
        "unsupported capture format: {rate} Hz, {channels} channel(s) — expected 48000 Hz stereo"
    )]
    UnsupportedCaptureFormat { rate: u32, channels: usize },

    /// A frame handed to the encoder was not exactly one packet's worth of samples.
    ///
    /// 960 interleaved `f32` values is a wire-protocol invariant, not a preference:
    /// the player decodes an uncompressed payload by dividing its byte count by 4,
    /// and the jitter buffer's whole geometry assumes one packet is 10 ms. A short
    /// frame would produce a packet the player silently mis-frames, so this is
    /// rejected at the seam rather than transmitted.
    #[error("invalid frame length: {got} samples, expected {expected}")]
    InvalidFrameLength { got: usize, expected: usize },

    #[error("source is not actively subscribed")]
    SourceNotSubscribed,

    #[cfg(target_os = "android")]
    #[error("Oboe failed to open {direction} stream: {message}")]
    OboeStreamBuildFailed {
        direction: StreamDirection,
        message: String,
    },

    #[cfg(target_os = "android")]
    #[error("Oboe failed to start {direction} stream: {message}")]
    OboeStreamStartFailed {
        direction: StreamDirection,
        message: String,
    },

    #[cfg(target_os = "android")]
    #[error("Oboe failed to {action} {direction} stream: {message}")]
    OboeStreamControlFailed {
        action: &'static str,
        direction: StreamDirection,
        message: String,
    },

    #[error("playback control channel is unavailable")]
    PlaybackControlUnavailable,

    #[error("playback control failed: {0}")]
    PlaybackControlFailed(String),

    #[cfg(target_os = "linux")]
    #[error("PipeWire error: {0}")]
    PipeWireError(String),

    #[cfg(target_os = "linux")]
    #[error("PipeWire connection failed: {0}")]
    PipeWireConnectionFailed(String),

    #[cfg(target_os = "macos")]
    #[error("ScreenCaptureKit error: {0}")]
    ScreenCaptureKitError(String),

    #[cfg(target_os = "macos")]
    #[error("ScreenCaptureKit permission denied — grant Screen Recording in System Settings")]
    ScreenCapturePermissionDenied,
}

#[derive(ThisError, Debug)]
pub enum NetworkError {
    #[error("failed to configure audio priority: {operation}")]
    AudioPriorityFailed {
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("no non-loopback network address is available")]
    LocalAddressUnavailable,

    #[error("failed to bind socket on {addr}")]
    SocketBindFailed {
        addr: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to configure socket option: {option}")]
    SocketOptionFailed {
        option: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to send packet")]
    SendFailed(#[source] std::io::Error),

    #[error("failed to receive packet")]
    RecvFailed(#[source] std::io::Error),

    #[error("failed to clone socket")]
    SocketCloneFailed(#[source] std::io::Error),

    #[error("failed to enable broadcast")]
    EnableBroadcastFailed(#[source] std::io::Error),

    #[error("failed to serialize discovery payload")]
    Serialization(#[from] serde_json::Error),

    #[error("connection lost or streamer stopped transmitting")]
    ConnectionLost,

    #[error("no active connection for device {0}")]
    DeviceNotConnected(String),

    #[error("failed to register mDNS service: {0}")]
    MdnsRegisterFailed(#[source] mdns_sd::Error),
}

#[derive(ThisError, Debug)]
pub enum ControlError {
    #[error("failed to serialize control message")]
    Serialization(#[from] serde_json::Error),

    #[error("failed to send control message to {addr}")]
    SendFailed {
        addr: String,
        #[source]
        source: std::io::Error,
    },

    #[error("request timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    #[error("streamer rejected the request: {reason}")]
    Rejected { reason: String },

    #[error("streamer rejected the request: {reason} ({code})")]
    RemoteRejected { code: String, reason: String },

    #[error("failed to start control server")]
    ServerStartFailed(#[source] std::io::Error),

    #[error("HTTP request failed: {0}")]
    HttpRequestFailed(String),

    #[error("WebSocket connection failed: {reason}")]
    WebSocketFailed { reason: String },
}

#[derive(ThisError, Debug)]
pub enum UpdaterError {
    #[error("invalid current version '{version}'")]
    InvalidCurrentVersion {
        version: String,
        #[source]
        source: semver::Error,
    },

    #[error("invalid version '{version}' in updater manifest")]
    InvalidManifestVersion {
        version: String,
        #[source]
        source: semver::Error,
    },

    #[error("failed to fetch updater manifest")]
    ManifestRequestFailed(#[source] reqwest::Error),

    #[error("updater manifest request failed with HTTP status {status}")]
    ManifestHttpStatus {
        status: reqwest::StatusCode,
        #[source]
        source: reqwest::Error,
    },

    #[error("failed to read updater manifest body")]
    ManifestBodyReadFailed(#[source] reqwest::Error),

    #[error("failed to parse updater manifest")]
    ManifestParseFailed(#[source] serde_json::Error),

    #[error("no update is published for platform '{platform_key}'")]
    PlatformNotPublished { platform_key: String },

    #[error("updater manifest entry for '{platform_key}' carries no sha256 digest")]
    MissingChecksum { platform_key: String },

    #[error(
        "updater manifest entry for '{platform_key}' has a malformed sha256 digest \
         ({length} chars, expected {expected_length} hex digits)"
    )]
    MalformedChecksum {
        platform_key: String,
        length: usize,
        expected_length: usize,
    },

    #[error("failed to request update artifact")]
    ArtifactRequestFailed(#[source] reqwest::Error),

    #[error("update artifact request failed with HTTP status {status}")]
    ArtifactHttpStatus {
        status: reqwest::StatusCode,
        #[source]
        source: reqwest::Error,
    },

    #[error("failed to read update artifact stream")]
    ArtifactStreamFailed(#[source] reqwest::Error),

    #[error("failed to create update file at {path}")]
    CreateUpdateFile {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to write update file at {path}")]
    WriteUpdateFile {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to sync update file at {path}")]
    SyncUpdateFile {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("sha256 mismatch for {path}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        path: String,
        expected: String,
        actual: String,
    },

    #[error("failed to read update cache directory at {path}")]
    ReadCacheDirectory {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to read an entry in update cache directory at {path}")]
    ReadCacheEntry {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to remove stale update file at {path}")]
    RemoveStaleUpdate {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    mod protocol_error {
        use super::*;

        #[test]
        fn packet_too_short_should_display_expected_and_actual_sizes() {
            let err = ProtocolError::PacketTooShort { got: 3, min: 9 };
            let msg = err.to_string();
            assert!(
                msg.contains("at least 9") && msg.contains("got 3"),
                "Expected sizes in message, got: {msg}"
            );
        }

        #[test]
        fn should_convert_into_gemacast_error_via_from() {
            let err = ProtocolError::PacketTooShort { got: 0, min: 9 };
            let outer: GemaCastError = err.into();
            assert!(
                matches!(outer, GemaCastError::Protocol(_)),
                "Expected GemaCastError::Protocol, got: {outer:?}"
            );
        }
    }

    mod network_error {
        use super::*;

        #[test]
        fn connection_lost_should_display_descriptive_message() {
            let err = NetworkError::ConnectionLost;
            assert!(
                err.to_string().contains("connection lost"),
                "Expected 'connection lost' in: {}",
                err
            );
        }

        #[test]
        fn local_address_unavailable_should_describe_the_missing_address() {
            let err = NetworkError::LocalAddressUnavailable;

            assert!(err.to_string().contains("network address"));
        }

        #[test]
        fn device_not_connected_should_include_device_id() {
            let err = NetworkError::DeviceNotConnected("phone_42".to_string());
            assert!(
                err.to_string().contains("phone_42"),
                "Expected device id in: {}",
                err
            );
        }

        #[test]
        fn should_convert_into_gemacast_error_via_from() {
            let err = NetworkError::ConnectionLost;
            let outer: GemaCastError = err.into();
            assert!(
                matches!(outer, GemaCastError::Network(_)),
                "Expected GemaCastError::Network, got: {outer:?}"
            );
        }
    }

    mod audio_error {
        use super::*;

        #[test]
        fn no_output_device_should_display_descriptive_message() {
            let err = AudioError::NoOutputDevice;
            assert!(
                err.to_string().contains("no default output device"),
                "Expected 'no default output device' in: {}",
                err
            );
        }
    }

    mod control_error {
        use super::*;

        #[test]
        fn timeout_should_include_duration_in_display() {
            let err = ControlError::Timeout { timeout_ms: 3000 };
            assert!(
                err.to_string().contains("3000"),
                "Expected timeout value in: {}",
                err
            );
        }

        #[test]
        fn should_convert_into_gemacast_error_via_from() {
            let err = ControlError::Timeout { timeout_ms: 500 };
            let outer: GemaCastError = err.into();
            assert!(
                matches!(outer, GemaCastError::Control(_)),
                "Expected GemaCastError::Control, got: {outer:?}"
            );
        }

        #[test]
        fn tcp_write_failure_should_identify_the_operation_and_address() {
            let err = NetworkError::TcpWriteFailed {
                addr: "127.0.0.1:23557".into(),
                operation: "ADB audio handshake",
                source: std::io::Error::new(std::io::ErrorKind::BrokenPipe, "closed"),
            };
            let message = err.to_string();

            assert!(message.contains("ADB audio handshake"));
            assert!(message.contains("127.0.0.1:23557"));
        }
    }

    mod updater_error {
        use super::*;

        #[test]
        fn checksum_mismatch_should_include_expected_and_actual_digests() {
            let error = UpdaterError::ChecksumMismatch {
                path: "update.apk".to_string(),
                expected: "expected".to_string(),
                actual: "actual".to_string(),
            };

            assert!(error.to_string().contains("expected"));
            assert!(error.to_string().contains("actual"));
        }

        #[test]
        fn updater_errors_should_convert_into_gemacast_errors() {
            let error = UpdaterError::PlatformNotPublished {
                platform_key: "android".to_string(),
            };
            let outer: GemaCastError = error.into();

            assert!(matches!(outer, GemaCastError::Updater(_)));
        }
    }

    mod direction_display {
        use super::*;

        #[test]
        fn stream_direction_input_should_display_lowercase() {
            assert_eq!(StreamDirection::Input.to_string(), "input");
        }

        #[test]
        fn stream_direction_output_should_display_lowercase() {
            assert_eq!(StreamDirection::Output.to_string(), "output");
        }

        #[test]
        fn codec_direction_encoder_should_display_lowercase() {
            assert_eq!(CodecDirection::Encoder.to_string(), "encoder");
        }

        #[test]
        fn codec_direction_decoder_should_display_lowercase() {
            assert_eq!(CodecDirection::Decoder.to_string(), "decoder");
        }
    }
}
