//! Persisted preferences and validation before submitting engine state.
use ft8rs::stream::session::{DecodeProfile, StreamDecodeConfig};
use ft8rs_engine::reconfig::EngineState;
use ft8rs_engine::report::UdpConfig;
use ft8rs_engine::soundcard::{InputChannel, SoundcardDeviceInfo};

use super::decode_log::DisplayFilters;
use super::norm;
use crate::i18n::Language;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum SettingsTab {
    General,
    Audio,
    Decode,
    Frequency,
    Output,
    Compare,
    Advanced,
}

pub(super) struct Preferences {
    pub(super) mycall: String,
    pub(super) hiscall: String,
    pub(super) hisgrid: String,
    pub(super) nfqso: String,
    pub(super) language: Language,
    pub(super) profile: DecodeProfile,
    pub(super) swl: bool,
    pub(super) hybrid_experiment: bool,
    pub(super) nfa: String,
    pub(super) nfb: String,
    pub(super) mygrid: String,
    pub(super) udp_on: bool,
    pub(super) udp_host: String,
    pub(super) udp_port: String,
    pub(super) hide_hash: bool,
    pub(super) display_filters: DisplayFilters,
    pub(super) udp_in_on: bool,
    pub(super) udp_in_host: String,
    pub(super) udp_in_port: String,
    pub(super) udp_in_tol: f64,
    pub(super) selected_device: Option<String>,
    pub(super) input_channel: InputChannel,
}

impl Preferences {
    pub(super) fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        // Restore IDs; legacy names migrate after enumeration only if unique.
        // Missing or ambiguous selections are reported instead of falling back.
        let load = |key: &str, default: &str| -> String {
            storage
                .and_then(|st| st.get_string(key))
                .unwrap_or_else(|| default.to_string())
        };
        let load_bool = |key: &str, default: bool| -> bool {
            storage
                .and_then(|st| st.get_string(key))
                .map(|value| value == "1")
                .unwrap_or(default)
        };
        let language = Language::parse(&load("language", "auto"));
        let device = load("device", "");
        let profile =
            DecodeProfile::parse(&load("profile", "wsjtx")).unwrap_or(DecodeProfile::Wsjtx);
        let mycall = load("mycall", "");

        Self {
            language,
            mycall,
            hiscall: load("hiscall", ""),
            hisgrid: load("hisgrid", ""),
            nfqso: load("nfqso", "0"),
            profile,
            swl: load_bool("swl", false),
            hybrid_experiment: load_bool("hybrid_experiment", false),
            nfa: load("nfa", "200"),
            nfb: load("nfb", "3000"),
            mygrid: load("mygrid", ""),
            udp_on: load_bool("udp_on", false),
            udp_host: load("udp_host", "127.0.0.1"),
            udp_port: load("udp_port", "2238"),
            hide_hash: load_bool("hide_hash", false),
            display_filters: DisplayFilters {
                hide_contest: load_bool("hide_contest", true),
                hide_telemetry: load_bool("hide_telemetry", true),
            },
            udp_in_on: load_bool("udp_in_on", true),
            udp_in_host: load("udp_in_host", "127.0.0.1"),
            udp_in_port: load("udp_in_port", "2237"),
            udp_in_tol: load("udp_in_tol", "7").trim().parse().unwrap_or(7.0),
            selected_device: (!device.is_empty()).then_some(device),
            // Preserve the operator's channel choice; Mono negotiates a native
            // mono stream and otherwise takes the first capture channel.
            input_channel: InputChannel::parse(&load("audio_channel", "mono")).unwrap_or_default(),
        }
    }

    pub(super) fn save(&self, storage: &mut dyn eframe::Storage) {
        let bs = |value: bool| if value { "1" } else { "0" }.to_string();
        storage.set_string("language", self.language.as_str().to_owned());
        storage.set_string("mycall", self.mycall.clone());
        storage.set_string("hiscall", self.hiscall.clone());
        storage.set_string("hisgrid", self.hisgrid.clone());
        storage.set_string("nfqso", self.nfqso.clone());
        storage.set_string("mygrid", self.mygrid.clone());
        storage.set_string("profile", self.profile.as_str().to_string());
        storage.set_string("swl", bs(self.swl));
        storage.set_string("hybrid_experiment", bs(self.hybrid_experiment));
        storage.set_string("nfa", self.nfa.clone());
        storage.set_string("nfb", self.nfb.clone());
        storage.set_string("udp_on", bs(self.udp_on));
        storage.set_string("udp_host", self.udp_host.clone());
        storage.set_string("udp_port", self.udp_port.clone());
        storage.set_string("hide_hash", bs(self.hide_hash));
        storage.set_string("hide_contest", bs(self.display_filters.hide_contest));
        storage.set_string("hide_telemetry", bs(self.display_filters.hide_telemetry));
        storage.set_string("udp_in_on", bs(self.udp_in_on));
        storage.set_string("udp_in_host", self.udp_in_host.clone());
        storage.set_string("udp_in_port", self.udp_in_port.clone());
        storage.set_string("udp_in_tol", self.udp_in_tol.to_string());
        storage.set_string("device", self.selected_device.clone().unwrap_or_default());
        storage.set_string("audio_channel", self.input_channel.as_str().to_string());
    }
    pub(super) fn build_state(
        &self,
        devices: &[SoundcardDeviceInfo],
    ) -> Result<EngineState, String> {
        let mut config = StreamDecodeConfig {
            profile: self.profile,
            mycall: norm(&self.mycall),
            mygrid: norm(&self.mygrid),
            hiscall: norm(&self.hiscall),
            hisgrid: norm(&self.hisgrid),
            swl: self.swl,
            hybrid_experiment: self.hybrid_experiment,
            nagain: false,
            filter: false,
            hide_dupes: false,
            hide_hash: self.hide_hash,
            ..StreamDecodeConfig::default()
        };
        (config.nfa, config.nfb) = parse_frequency_range(&self.nfa, &self.nfb)?;
        let nfqso = self
            .nfqso
            .trim()
            .parse::<f64>()
            .map_err(|_| "RX frequency must be a number".to_owned())?;
        config.nfqso =
            ft8rs::stream::validation::normalize_rx_frequency(nfqso, config.nfa, config.nfb)?;
        Ok(EngineState {
            device: self
                .selected_device
                .as_deref()
                .map(|selector| {
                    let migrated = ft8rs_engine::soundcard::migrate_device_name(selector, devices)?;
                    if !migrated.starts_with("id:") {
                        return Err(format!(
                            "Saved input '{selector}' is unavailable; select an input device"
                        ));
                    }
                    Ok(migrated)
                })
                .transpose()?,
            channel: self.input_channel,
            config,
            udp: self.udp_config()?,
        })
    }

    fn udp_config(&self) -> Result<Option<UdpConfig>, String> {
        if !self.udp_on {
            return Ok(None);
        }
        if self.udp_host.trim().is_empty() {
            return Err("UDP report host is required".to_string());
        }
        Ok(Some(UdpConfig {
            host: self.udp_host.trim().to_string(),
            port: parse_port(&self.udp_port)?,
        }))
    }

    pub(super) fn settings_validation_error(&self, tab: SettingsTab) -> Option<String> {
        match tab {
            SettingsTab::Frequency => parse_frequency_range(&self.nfa, &self.nfb).err(),
            SettingsTab::Output => self.udp_config().err(),
            SettingsTab::Compare if self.udp_in_on => {
                if self.udp_in_host.trim().is_empty() {
                    Some("UDP listen host is required".to_owned())
                } else {
                    parse_port(&self.udp_in_port).err()
                }
            }
            _ => None,
        }
    }

    pub(super) fn invalid_monitor_settings_page(&self) -> Option<SettingsTab> {
        [SettingsTab::Frequency, SettingsTab::Output]
            .into_iter()
            .find(|&tab| self.settings_validation_error(tab).is_some())
    }
}

fn parse_frequency_range(low: &str, high: &str) -> Result<(f64, f64), String> {
    let parse = |value: &str| {
        value
            .trim()
            .parse::<f64>()
            .map_err(|_| "Frequency range must contain numbers".to_owned())
    };
    let (low, high) = (parse(low)?, parse(high)?);
    ft8rs::stream::validation::validate_frequency_range(low, high)?;
    Ok((low, high))
}

pub(super) fn parse_port(value: &str) -> Result<u16, String> {
    value
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|&port| port != 0)
        .ok_or_else(|| "UDP port must be an integer from 1 to 65535".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_numeric_settings_without_silent_defaults() {
        assert_eq!(parse_frequency_range("200", "3000"), Ok((200.0, 3000.0)));
        for (low, high) in [
            ("NaN", "3000"),
            ("200", "inf"),
            ("bad", "3000"),
            ("3000", "200"),
            ("200", "200"),
            ("-1", "3000"),
            ("200", "5001"),
        ] {
            assert!(parse_frequency_range(low, high).is_err());
        }
        assert_eq!(parse_port(" 2237 "), Ok(2237));
        for value in ["", "0", "65536", "-1", "abc"] {
            assert!(parse_port(value).is_err());
        }
    }
}
