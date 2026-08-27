use super::*;

#[test]
fn validates_interleaved_buffers() {
    assert_eq!(AudioFormat::new(0, 2), Err(AudioError::InvalidFormat));
    assert_eq!(
        AudioBuffer::new(format(), Timestamp::ZERO, vec![0.0]),
        Err(AudioError::SamplesNotInterleaved {
            samples: 1,
            channels: 2
        })
    );
    assert_eq!(
        AudioBuffer::new(format(), Timestamp::ZERO, vec![f32::NAN, 0.0]),
        Err(AudioError::NonFiniteSample)
    );
}

#[test]
fn audio_device_metadata_keeps_provider_default_route() {
    let mut device =
        AudioDeviceInfo::new("mic", "Microphone", AudioDeviceKind::Input).expect("device");
    assert!(!device.is_default());

    device.set_default(true);

    assert!(device.is_default());
}

#[test]
fn audio_formats_keep_standard_channel_layouts() {
    assert_eq!(
        AudioFormat::new(48_000, 1).expect("mono format").layout(),
        AudioChannelLayout::Mono
    );
    assert_eq!(
        AudioFormat::new(48_000, 2).expect("stereo format").layout(),
        AudioChannelLayout::Stereo
    );
    assert_eq!(
        AudioFormat::new(48_000, 3).expect("2.1 format").layout(),
        AudioChannelLayout::TwoPointOne
    );
    assert_eq!(
        AudioFormat::new(48_000, 4).expect("quad format").layout(),
        AudioChannelLayout::Quad
    );
    assert_eq!(
        AudioFormat::new(48_000, 6).expect("5.1 format").layout(),
        AudioChannelLayout::FivePointOne
    );
    assert_eq!(
        AudioFormat::new(48_000, 8).expect("7.1 format").layout(),
        AudioChannelLayout::SevenPointOne
    );
    assert_eq!(
        AudioFormat::new(48_000, 7)
            .expect("discrete format")
            .layout(),
        AudioChannelLayout::Discrete(7)
    );
    assert_eq!(
        AudioFormat::with_layout(48_000, AudioChannelLayout::FivePointOne)
            .expect("named format")
            .channels(),
        6
    );
    assert_eq!(
        AudioFormat::with_layout(48_000, AudioChannelLayout::Discrete(0)),
        Err(AudioError::InvalidFormat)
    );
}

#[test]
fn simulated_monitor_sink_validates_format_and_lifecycle() {
    let provider = SimulatedAudioProvider::new();
    let devices = provider.discover_outputs().expect("monitor catalog");
    assert_eq!(devices[0].kind(), AudioDeviceKind::Output);
    let mut output = provider
        .open_output(devices[0].id(), format())
        .expect("monitor output");
    assert_eq!(output.state(), AudioOutputState::Stopped);
    output
        .write_block(&buffer(&[0.1, -0.1]))
        .expect("write block");
    assert_eq!(output.state(), AudioOutputState::Running);
    let other_format = AudioFormat::new(44_100, 2).expect("other format");
    let other = AudioBuffer::silence(other_format, Timestamp::ZERO, 1).expect("other buffer");
    assert!(matches!(
        output.write_block(&other),
        Err(AudioDeviceError::Audio(AudioError::FormatMismatch { .. }))
    ));
    output.stop();
    assert_eq!(output.state(), AudioOutputState::Stopped);
}
