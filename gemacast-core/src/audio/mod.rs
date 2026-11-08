use opus::{Decoder, Encoder};

pub mod mixdown;
pub mod encryption;
pub use encryption::AudioSessionKey;
pub(crate) mod pcm_datagram;
pub mod resampler;
pub use resampler::CaptureResampler;
pub const OPUS_CHANNELS: u16 = 2;
pub const OPUS_SAMPLE_RATE: u32 = 48_000;

pub const OPUS_BITRATE: usize = 128_000;
pub const OPUS_FRAME_SIZE: usize = 480;
pub const OPUS_FRAME_SAMPLES: usize = OPUS_FRAME_SIZE * OPUS_CHANNELS as usize;

pub const MAX_OPUS_PACKET_SIZE: usize = 8000;
pub const SEQ_NUM_SIZE: usize = 8;
pub const FORMAT_FLAG_SIZE: usize = 1;

pub const FORMAT_OPUS: u8 = 0;
pub const FORMAT_UNCOMPRESSED: u8 = 1;
pub const FORMAT_SILENCE: u8 = 2;

/// Convert a **per-channel** frame count to milliseconds at a given sample rate.
///
/// The per-channel part is the whole reason this exists instead of being written inline
/// at each call site. Every platform quotes its buffer size in frames — cpal's
/// `FrameCount`, WASAPI's `GetBufferSize`, PipeWire's quantum — and a frame is one
/// sample *per channel*, so the period is `frames / rate` and never
/// `frames / (rate * channels)`. Dividing by the interleaved total reports half the true
/// period, and that is exactly the confusion [`OPUS_FRAME_SIZE`] and
/// [`OPUS_FRAME_SAMPLES`] invite: 480 is the frame count a platform API wants, 960 is
/// the length of the buffer holding it.
///
/// Returns `None` for a zero rate so a caller can log "rate unknown" rather than
/// `inf ms`, which reads as a broken device.
pub fn frames_to_ms(frames: u32, rate: u32) -> Option<f64> {
    if rate == 0 {
        return None;
    }

    Some(f64::from(frames) * 1000.0 / f64::from(rate))
}

pub fn create_opus_encoder_with_bitrate(bitrate: i32) -> Result<Encoder, opus::Error> {
    let mut encoder = Encoder::new(
        OPUS_SAMPLE_RATE,
        opus::Channels::Stereo,
        opus::Application::LowDelay,
    )?;

    encoder.set_bitrate(opus::Bitrate::Bits(bitrate))?;
    encoder.set_complexity(5)?;
    // CELT mode: treated as music/system audio, avoids speech/music detection overhead.
    encoder.set_signal(opus::Signal::Music)?;

    Ok(encoder)
}

pub fn create_opus_encoder() -> Result<Encoder, opus::Error> {
    create_opus_encoder_with_bitrate(OPUS_BITRATE as i32)
}

pub fn create_opus_decoder() -> Result<Decoder, opus::Error> {
    let decoder = Decoder::new(OPUS_SAMPLE_RATE, opus::Channels::Stereo)?;

    Ok(decoder)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod constants {
        use super::*;

        #[test]
        fn opus_frame_samples_should_equal_frame_size_times_channels() {
            assert_eq!(OPUS_FRAME_SAMPLES, OPUS_FRAME_SIZE * OPUS_CHANNELS as usize);
        }

        #[test]
        fn max_packet_size_should_accommodate_uncompressed_pcm_frame() {
            // Uncompressed PCM frame = OPUS_FRAME_SAMPLES * 4 bytes/f32 = 3840 bytes
            let uncompressed_size = OPUS_FRAME_SAMPLES * std::mem::size_of::<f32>();
            assert!(
                MAX_OPUS_PACKET_SIZE >= uncompressed_size,
                "MAX_OPUS_PACKET_SIZE ({}) must fit uncompressed frame ({})",
                MAX_OPUS_PACKET_SIZE,
                uncompressed_size
            );
        }
    }

    /// The frames-to-milliseconds conversion used by every backend's format log.
    mod frame_duration {
        use super::*;

        #[test]
        fn one_opus_frame_should_be_ten_milliseconds() {
            // The per-channel count, which is what a platform API is handed.
            assert_eq!(
                frames_to_ms(OPUS_FRAME_SIZE as u32, OPUS_SAMPLE_RATE),
                Some(10.0)
            );
        }

        #[test]
        fn the_interleaved_sample_count_should_read_as_twenty_milliseconds() {
            // Not a quirk to fix here — this is what asking for `OPUS_FRAME_SAMPLES`
            // frames actually buys, and the assertion exists so a caller that confuses
            // the two constants sees 20 ms in the log rather than a plausible 10.
            assert_eq!(
                frames_to_ms(OPUS_FRAME_SAMPLES as u32, OPUS_SAMPLE_RATE),
                Some(20.0)
            );
        }

        #[test]
        fn should_not_divide_by_the_channel_count() {
            // The discriminating case: a divisor of `rate * channels` would give 5.0.
            assert_eq!(
                frames_to_ms(OPUS_FRAME_SIZE as u32, OPUS_SAMPLE_RATE),
                Some(10.0)
            );
            assert_ne!(
                frames_to_ms(
                    OPUS_FRAME_SIZE as u32,
                    OPUS_SAMPLE_RATE * OPUS_CHANNELS as u32
                ),
                Some(10.0)
            );
        }

        #[test]
        fn should_refuse_a_zero_rate_rather_than_reporting_an_infinite_period() {
            assert_eq!(frames_to_ms(480, 0), None);
        }
    }

    mod codec_factories {
        use super::*;

        #[test]
        fn create_opus_encoder_should_succeed() {
            let encoder = create_opus_encoder();
            assert!(
                encoder.is_ok(),
                "Encoder creation failed: {:?}",
                encoder.unwrap_err()
            );
        }

        #[test]
        fn create_opus_decoder_should_succeed() {
            let decoder = create_opus_decoder();
            assert!(
                decoder.is_ok(),
                "Decoder creation failed: {:?}",
                decoder.unwrap_err()
            );
        }

        #[test]
        fn create_opus_encoder_with_custom_bitrate_should_succeed() {
            let encoder = create_opus_encoder_with_bitrate(64_000);
            assert!(
                encoder.is_ok(),
                "Custom bitrate encoder failed: {:?}",
                encoder.unwrap_err()
            );
        }

        #[test]
        fn encode_then_decode_should_produce_correct_sample_count() {
            let mut encoder = create_opus_encoder().unwrap();
            let mut decoder = create_opus_decoder().unwrap();

            let input = vec![0.1f32; OPUS_FRAME_SAMPLES];
            let mut opus_buf = vec![0u8; MAX_OPUS_PACKET_SIZE];
            let encoded_len = encoder.encode_float(&input, &mut opus_buf).unwrap();

            let mut output = vec![0.0f32; OPUS_FRAME_SAMPLES];
            let decoded_samples = decoder
                .decode_float(&opus_buf[..encoded_len], &mut output, false)
                .unwrap();

            assert_eq!(
                decoded_samples, OPUS_FRAME_SIZE,
                "Expected {} decoded frames, got {}",
                OPUS_FRAME_SIZE, decoded_samples
            );
        }
    }
}
