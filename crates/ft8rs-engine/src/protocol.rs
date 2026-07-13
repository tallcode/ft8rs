//! Engine ↔ GUI protocol (P1).
//!
//! The formal contract between the live engine and any front-end: commands the
//! GUI sends, events the engine emits, and the snapshot payloads used for state
//! migration and the DX intel panel. These are plain data types; the actors (P2)
//! produce and consume them. Defining them now (rather than as GUI details)
//! keeps the boundary stable.
//!
//! Nothing here touches `lib_wsjtx`/`lib_jtdx`.

use ft8rs::stream::session::StreamDecodeProvenance;
use ft8rs::stream::StreamDecodedMessage;
use ft8rs::SlotTimestamp;

use crate::reconfig::{EngineState, ReconfigOutcome};
use crate::soundcard::SoundcardDeviceInfo;

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

/// Which staged decode produced a row (decision 12). The wsjtx
/// profile emits early partial decodes before the slot ends; other profiles
/// produce only `Final`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// Where the currently effective DX grid came from (decision 4, option C).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HisgridSource {
    None,
    /// Entered by the operator.
    User,
    /// Auto-harvested from a target-sender decode.
    Harvested,
}

/// Read-only snapshot of the DX target intel for the GUI panel (P4 fills it).
#[derive(Clone, Debug, Default, PartialEq)]
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
pub enum EngineEvent {
    Status(EngineStatus),
    Decode(DecodeRecord),
    SlotComplete {
        timestamp: SlotTimestamp,
        count: usize,
    },
    DevicesRefreshed(Vec<SoundcardDeviceInfo>),
    /// Per-slot captured-audio peak amplitude (0.0..=1.0) so the GUI can show an
    /// input level and the operator can tell silence (dead capture) from signal.
    InputLevel(f32),
    DxContext(DxContextSnapshot),
    /// What a just-applied `ApplyState` actually did (level + reset/migrate buckets).
    Reconfigured(ReconfigOutcome),
    Error(String),
}
