//! Engine ↔ GUI protocol.
//!
//! The formal contract between the live engine and any front-end: commands the
//! GUI sends, events the engine emits, and the snapshot payloads used for state
//! migration and the DX intel panel. These are plain data types; the actors
//! produce and consume them independently of GUI drawing.
//!
//! Nothing here touches `lib_wsjtx`/`lib_jtdx`.

use ft8rs::stream::session::StreamDecodeProvenance;
use ft8rs::stream::StreamDecodedMessage;
use ft8rs::SlotTimestamp;

use crate::reconfig::{EngineState, ReconfigOutcome};
use crate::soundcard::{InputChannel, SoundcardDeviceInfo, SoundcardFormatInfo};

/// Borrowed diagnostic data for a slot that started on a capture boundary.
pub struct CapturedSlot<'a> {
    pub timestamp: &'a SlotTimestamp,
    pub samples_12k: &'a [f32],
    pub config: &'a ft8rs::stream::session::StreamDecodeConfig,
    pub device_name: &'a str,
    pub format: &'a SoundcardFormatInfo,
    pub channel: InputChannel,
    pub native_samples: usize,
    pub xruns: u64,
    pub jitter_ms: i64,
}

/// Optional live diagnostics. Implementations run on the engine thread and
/// should keep callbacks short and avoid I/O.
pub trait MonitorObserver: Send + Sync {
    fn slot(&self, slot: CapturedSlot<'_>);
    fn event(&self, event: &EngineEvent);
}

/// Commands the GUI sends to the engine. The GUI submits a full desired
/// [`EngineState`]; the engine diffs it (see `reconfig::plan_reconfig`) and
/// decides what to do. There is no `OpenFile` — the GUI is monitor-only.
#[derive(Clone, Debug)]
pub enum EngineCommand {
    /// Begin monitoring with the given desired state.
    StartMonitor(EngineState),
    /// Stop monitoring; keep the last desired state.
    StopMonitor,
    /// Apply a new desired state while monitoring (engine diffs and reconfigures).
    ApplyState(EngineState),
    /// Re-enumerate input devices.
    RefreshDevices,
    /// Tear everything down and exit the engine thread.
    Shutdown,
}

/// Which staged decode produced a row. The wsjtx
/// and Hybrid profiles emit early partial decodes before the slot ends;
/// JTDX and DX produce only `Final`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DecodeStage {
    /// Early decode within the slot (low-latency wsjtx nzhsym=41 pass).
    Early,
    /// Final decode at slot end (nzhsym=50).
    Final,
}

/// A single decoded row delivered to the GUI, with provenance for the `a7`/AP
/// marker and the staged-decode origin.
#[derive(Clone, Debug)]
pub struct DecodeRecord {
    pub timestamp: SlotTimestamp,
    pub row: StreamDecodedMessage,
    pub provenance: StreamDecodeProvenance,
    pub stage: DecodeStage,
}

/// High-level engine status for the GUI status bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineStatus {
    /// Not monitoring.
    Idle,
    /// Waiting for the next UTC slot boundary (capture (re)start).
    Aligning,
    /// Actively capturing and decoding.
    Monitoring,
    /// Fatal engine error; monitoring stopped.
    Error(String),
}

/// Where the currently effective DX grid came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HisgridSource {
    None,
    /// Entered by the operator.
    User,
    /// Auto-harvested from a target-sender decode.
    Harvested,
}

/// Read-only snapshot of the DX target intel for the GUI panel.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DxContextSnapshot {
    pub target: String,
    pub foci: Vec<f64>,
    /// Observed transmit-slot parity (0/1) if known.
    pub tx_parity: Option<u8>,
    pub hisgrid: Option<String>,
    pub hisgrid_source: Option<HisgridSource>,
    pub dt: Option<f64>,
}

/// Migration payload exported from a session before a rebuild and imported into
/// the new one. Today only hash calls are provably safe to
/// migrate without touching `lib_*`; deeper buckets (A7/AP/evidence, DX intel)
/// are added here as the contract grows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub hash_calls: Vec<String>,
}

/// Events the engine emits to the GUI.
#[derive(Clone, Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "Preserve the public event API and avoid an extra allocation per decoded message"
)]
pub enum EngineEvent {
    /// Decode rows lost while the GUI was stalled, since its last drain.
    DecodeRowsDropped(u64),
    Status(EngineStatus),
    Decode(DecodeRecord),
    /// Internal event consumed by the live worker supervisor.
    A7Checkpoint {
        timestamp: SlotTimestamp,
        snapshot: ft8rs::stream::profile::A7Snapshot,
    },
    /// Internal JTDX progress, consumed by the live worker supervisor.
    JtdxCheckpoint {
        timestamp: SlotTimestamp,
        branch: ft8rs::stream::profile::JtdxBranch,
        update: ft8rs::stream::profile::JtdxCheckpoint,
        publish: bool,
    },
    SlotComplete {
        timestamp: SlotTimestamp,
        count: usize,
    },
    DevicesRefreshed(Vec<SoundcardDeviceInfo>),
    /// Per-slot captured-audio peak amplitude (0.0..=1.0) so the GUI can show an
    /// input level and the operator can tell silence (dead capture) from signal.
    InputLevel(f32),
    /// Actual opened format (device enumeration only describes the OS default).
    CaptureStarted {
        device_name: String,
        format: SoundcardFormatInfo,
    },
    /// Native channel peaks before extraction, and peak delivered to decoding.
    CaptureLevels {
        channel_peaks: Vec<f32>,
        peak: f32,
        /// Cumulative non-fatal capture discontinuity reports since this stream opened.
        xruns: u64,
    },
    /// A failed capture switch kept the previous device; reconcile the picker
    /// only if the operator has not requested a newer selection meanwhile.
    CaptureSelectionRejected {
        requested_device: Option<String>,
        requested_channel: InputChannel,
        active_device: Option<String>,
        active_channel: InputChannel,
    },
    /// Per-slot capture diagnostics (window length, driver block size/count, and
    /// arrival-time jitter) for locating capture/alignment problems from one live run.
    CaptureDiag {
        samples: usize,
        blocks: u32,
        avg_block: u32,
        jitter_ms: i64,
        sample_rate: u32,
    },
    DxContext(DxContextSnapshot),
    /// What a just-applied `ApplyState` actually did (level + reset/migrate buckets).
    Reconfigured(ReconfigOutcome),
    Error(String),
}
