//! Presentation-only language preference. Protocol messages and decode rows stay unchanged.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    Auto,
    English,
    Chinese,
}

impl Language {
    pub const ALL: [Self; 3] = [Self::Auto, Self::English, Self::Chinese];

    pub fn parse(value: &str) -> Self {
        match value {
            "en" => Self::English,
            "zh" => Self::Chinese,
            _ => Self::Auto,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::English => "en",
            Self::Chinese => "zh",
        }
    }

    pub fn label(self, locale: Locale) -> &'static str {
        match self {
            Self::Auto => locale.text("Automatic (System)"),
            Self::English => "English",
            Self::Chinese => "中文",
        }
    }

    pub fn resolve_system(self) -> Locale {
        if self == Self::Auto {
            self.resolve(sys_locale::get_locale().as_deref())
        } else {
            self.resolve(None)
        }
    }

    fn resolve(self, system_locale: Option<&str>) -> Locale {
        match self {
            Self::English => Locale::English,
            Self::Chinese => Locale::Chinese,
            Self::Auto => {
                let language = system_locale
                    .unwrap_or("")
                    .split(['-', '_', '.', '@'])
                    .next()
                    .unwrap_or("");
                if language.eq_ignore_ascii_case("zh") {
                    Locale::Chinese
                } else {
                    Locale::English
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locale {
    English,
    Chinese,
}

impl Locale {
    /// Unknown text (e.g. an operating-system error) remains available verbatim.
    pub fn text(self, english: &str) -> &str {
        if self == Self::English {
            return english;
        }
        match english {
            "About" => "关于",
            "About ft8.rs" => "关于 ft8.rs",
            "Settings" => "设置",
            "Settings…" => "设置…",
            "Profile" => "解码方案",
            "Display" => "显示",
            "Language" => "语言",
            "Quit" => "退出",
            "Automatic (System)" => "自动（跟随系统）",
            "General" => "通用",
            "My Station" => "本站信息",
            "Audio" => "音频",
            "Decode" => "解码",
            "Frequency" => "频率",
            "Output" => "输出",
            "Compare" => "对比",
            "Advanced" => "高级",
            "Callsign" => "呼号",
            "Grid" => "网格",
            "RX" => "接收频率",
            "Drag to adjust the receive frequency, or click to type." => "拖动调整接收频率，或点击输入。",
            "Stop monitoring" => "停止监听",
            "Start monitoring" => "开始监听",
            "Input device" => "输入设备",
            "Default" => "系统默认",
            "Channel" => "声道",
            "Mono" => "单声道",
            "Left" => "左声道",
            "Right" => "右声道",
            "Mix (average)" => "混合（取平均）",
            "Input level" => "输入电平",
            " (near clipping)" => "（接近削波）",
            "silent (−inf dBFS)" => "静音（−inf dBFS）",
            "— (start monitoring)" => "—（请开始监听）",
            "Mono requests one channel; if unavailable, uses channel 1." => "单声道优先请求一个声道；不可用时使用声道 1。",
            "Mix averages all channels and can cancel opposite-phase audio." => "混合模式取所有声道的平均值，反相信号可能互相抵消。",
            "Controls both JTDX paths in Hybrid." => "控制 Hybrid 中的两条 JTDX 路径。",
            "Exp. decoder" => "实验解码器",
            "Adds weak stable-signal and interference recovery. Results carry exp; processing may take about 10 seconds." => "增加稳定弱信号及干扰恢复。结果以 exp 标记；处理可能需要约 10 秒。",
            "Low (Hz)" => "下限（Hz）",
            "High (Hz)" => "上限（Hz）",
            "UDP reports" => "UDP 上报",
            "Host" => "主机",
            "Port" => "端口",
            "Listen to UDP" => "监听 UDP",
            "Match ± Hz" => "匹配容差（± Hz）",
            "Merges WSJT-X decodes with the local ones to compare sensitivity." => "合并 WSJT-X 与本地解码结果以对比灵敏度。",
            "Color shows the source:" => "颜色表示来源：",
            "only WSJT-X" => "仅 WSJT-X",
            "only ft8.rs" => "仅 ft8.rs",
            "both decoders" => "两者均解出",
            "Hide <...> calls" => "隐藏 <...> 呼号",
            "Hide contest messages" => "隐藏竞赛消息",
            "Hide telemetry" => "隐藏遥测",
            "Decoder changes take effect at the next slot." => "解码设置将在下一个时隙生效。",
            "DX information" => "DX 信息",
            "No data yet" => "暂无数据",
            "Target" => "目标",
            "Focus frequencies (Hz)" => "重点频率（Hz）",
            "Target TX slot" => "对方发射时隙",
            "Even" => "偶数",
            "Odd" => "奇数",
            " (user)" => "（手动设置）",
            " (decoded)" => "（解码获得）",
            "Switch DX target" => "切换 DX 目标",
            "This clears the collected DX information." => "这会清除已收集的 DX 信息。",
            "Cancel" => "取消",
            "Switch" => "切换",
            "Audio discontinuity reports" => "音频不连续报告",
            "monitoring continues" => "监听继续",
            "Frequency range must contain numbers" => "频率范围必须为数字",
            "Frequency range must satisfy 0 ≤ Low < High ≤ 5000 Hz" => "频率范围必须满足 0 ≤ 下限 < 上限 ≤ 5000 Hz",
            "RX frequency must be a number" => "接收频率必须为数字",
            "RX frequency must be finite and between 0 and 5000 Hz" => "接收频率必须为 0 至 5000 Hz 之间的有限数值",
            "UDP port must be an integer from 1 to 65535" => "UDP 端口必须为 1 至 65535 的整数",
            "UDP report host is required" => "请填写 UDP 上报主机",
            "UDP listen host is required" => "请填写 UDP 监听主机",
            "DX requires a target callsign." => "DX 解码方案需要目标呼号。",
            _ => english,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_follows_system_language_with_an_english_fallback() {
        for tag in [
            "zh",
            "zh-CN",
            "zh-Hans-CN",
            "zh-Hant-TW",
            "zh_HK.UTF-8",
            "ZH-cn",
        ] {
            assert_eq!(Language::Auto.resolve(Some(tag)), Locale::Chinese, "{tag}");
        }
        for tag in [
            None,
            Some(""),
            Some("en-US"),
            Some("ja-JP"),
            Some("C"),
            Some("zhx"),
        ] {
            assert_eq!(Language::Auto.resolve(tag), Locale::English);
            assert_eq!(Language::Chinese.resolve(tag), Locale::Chinese);
        }
        assert_eq!(Language::English.resolve(Some("zh-CN")), Locale::English);
    }

    #[test]
    fn persisted_choices_round_trip_and_old_settings_default_to_auto() {
        for language in Language::ALL {
            assert_eq!(Language::parse(language.as_str()), language);
        }
        for value in ["", "unknown", "auto"] {
            assert_eq!(Language::parse(value), Language::Auto);
        }
    }
}
