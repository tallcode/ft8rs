use ft8rs::stream::ProfileStreamDecodeSession;
use ft8rs::{DecodeProfile, SlotTimestamp, StreamDecodeConfig, StreamDecodedMessage};

/// Future FT8 engines implement this interface and register a factory below.
/// The corpus/runner/expectations do not need their implementation types.
pub trait Decoder {
    fn decode(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
        on_message: &mut dyn FnMut(&StreamDecodedMessage) -> Result<(), String>,
    ) -> Result<Vec<StreamDecodedMessage>, String>;
}

impl Decoder for ProfileStreamDecodeSession {
    fn decode(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
        on_message: &mut dyn FnMut(&StreamDecodedMessage) -> Result<(), String>,
    ) -> Result<Vec<StreamDecodedMessage>, String> {
        self.decode_slot_streaming_at(timestamp, samples, on_message)
    }
}

#[derive(Clone, Copy)]
pub struct Adapter {
    pub name: &'static str,
    pub supports_a8: bool,
    build: fn(StreamDecodeConfig) -> Box<dyn Decoder>,
    pub target_only: bool,
    pub native_baseline: bool,
    pub preserves_native: bool,
}

pub const ADAPTERS: [Adapter; 5] = [
    Adapter {
        name: "wsjtx",
        supports_a8: true,
        target_only: false,
        native_baseline: true,
        preserves_native: false,
        build: |c| native(c, DecodeProfile::Wsjtx),
    },
    Adapter {
        name: "jtdx",
        supports_a8: false,
        target_only: false,
        native_baseline: true,
        preserves_native: false,
        build: |c| native(c, DecodeProfile::Jtdx),
    },
    Adapter {
        name: "hybrid",
        supports_a8: true,
        target_only: false,
        native_baseline: false,
        preserves_native: true,
        build: |c| native(c, DecodeProfile::Hybrid),
    },
    Adapter {
        name: "hybrid-exp",
        supports_a8: true,
        target_only: false,
        native_baseline: false,
        preserves_native: true,
        build: |mut c| {
            c.hybrid_experiment = true;
            native(c, DecodeProfile::Hybrid)
        },
    },
    Adapter {
        name: "dx",
        supports_a8: true,
        target_only: true,
        native_baseline: false,
        preserves_native: false,
        build: |c| native(c, DecodeProfile::Dx),
    },
];

impl Adapter {
    pub fn session(self, mut config: StreamDecodeConfig) -> Box<dyn Decoder> {
        // Deterministic file-mode search, no wall-clock start gate.
        config.cooperative_realtime = false;
        (self.build)(config)
    }
}

fn native(mut config: StreamDecodeConfig, profile: DecodeProfile) -> Box<dyn Decoder> {
    config.profile = profile;
    Box::new(ProfileStreamDecodeSession::new(config))
}
