use crate::stream::session::StreamDecodeConfig;

#[derive(Clone, Debug)]
pub struct SoundcardDecodeOptions {
    pub device: Option<String>,
    pub config: StreamDecodeConfig,
}

pub fn open_soundcard_stream(options: SoundcardDecodeOptions) -> Result<(), String> {
    let device = options.device.as_deref().unwrap_or("default");
    Err(format!(
        "soundcard input is not implemented yet (requested device: {device})"
    ))
}
