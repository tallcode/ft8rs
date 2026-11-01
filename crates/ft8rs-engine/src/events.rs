//! GUI mailbox without consumption backpressure: bounded decode rows, latest telemetry, reliable controls.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::protocol::EngineEvent;

const DECODE_CAP: usize = 1024;

#[derive(Default)]
struct Queue {
    events: VecDeque<EngineEvent>,
    decodes: usize,
    dropped: u64,
}

#[derive(Clone, Default)]
pub(crate) struct GuiEvents(Arc<Mutex<Queue>>);

impl GuiEvents {
    pub(crate) fn send(&self, event: EngineEvent) {
        let mut queue = self.0.lock().unwrap();
        if matches!(event, EngineEvent::Decode(_)) {
            if queue.decodes == DECODE_CAP {
                queue.dropped = queue.dropped.saturating_add(1);
                return;
            }
            queue.decodes += 1;
        } else if !matches!(
            event,
            EngineEvent::Error(_) | EngineEvent::CaptureSelectionRejected { .. }
        ) {
            // Snapshots replace their old value and move to the tail, preserving
            // their order relative to controls. Errors and rollbacks are retained.
            let kind = std::mem::discriminant(&event);
            queue
                .events
                .retain(|old| std::mem::discriminant(old) != kind);
        }
        queue.events.push_back(event);
    }

    pub(crate) fn try_recv(&self) -> Option<EngineEvent> {
        let mut queue = self.0.lock().unwrap();
        if let Some(event) = queue.events.pop_front() {
            if matches!(event, EngineEvent::Decode(_)) {
                queue.decodes -= 1;
            }
            return Some(event);
        }
        let dropped = std::mem::take(&mut queue.dropped);
        (dropped != 0).then_some(EngineEvent::DecodeRowsDropped(dropped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{DecodeRecord, DecodeStage, EngineStatus};
    use crate::soundcard::InputChannel;
    use ft8rs::stream::session::StreamDecodeProvenance;
    use ft8rs::{SlotTimestamp, StreamDecodedMessage};

    #[test]
    fn stalled_gui_retains_controls_and_latest_status_and_counts_lost_rows() {
        let mailbox = GuiEvents::default();
        let record = DecodeRecord {
            timestamp: SlotTimestamp::parse("120000").unwrap(),
            row: StreamDecodedMessage {
                freq: 1000.0,
                dt: 0.0,
                snr: -10.0,
                sync: 1.0,
                msg: "CQ K1ABC FN42".into(),
                itone: [0; 79],
            },
            provenance: StreamDecodeProvenance::Unknown,
            stage: DecodeStage::Final,
        };
        mailbox.send(EngineEvent::Status(EngineStatus::Monitoring));
        for _ in 0..DECODE_CAP + 7 {
            mailbox.send(EngineEvent::Decode(record.clone()));
            mailbox.send(EngineEvent::InputLevel(0.5));
        }
        mailbox.send(EngineEvent::Error("capture stopped".into()));
        mailbox.send(EngineEvent::CaptureSelectionRejected {
            requested_device: Some("new".into()),
            requested_channel: InputChannel::Mono,
            active_device: None,
            active_channel: InputChannel::Left,
        });
        mailbox.send(EngineEvent::Status(EngineStatus::Idle));
        let events: Vec<_> = std::iter::from_fn(|| mailbox.try_recv()).collect();
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, EngineEvent::Decode(_)))
                .count(),
            DECODE_CAP
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, EngineEvent::InputLevel(_)))
                .count(),
            1
        );
        assert!(events.iter().any(|e| matches!(e, EngineEvent::Error(_))));
        assert!(events
            .iter()
            .any(|e| matches!(e, EngineEvent::CaptureSelectionRejected { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, EngineEvent::Status(EngineStatus::Idle))));
        assert!(!events
            .iter()
            .any(|e| matches!(e, EngineEvent::Status(EngineStatus::Monitoring))));
        assert!(matches!(
            events.last(),
            Some(EngineEvent::DecodeRowsDropped(7))
        ));
        mailbox.send(EngineEvent::Decode(record));
        assert!(matches!(mailbox.try_recv(), Some(EngineEvent::Decode(_))));
        assert!(mailbox.try_recv().is_none());
    }
}
