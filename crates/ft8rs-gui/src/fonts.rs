//! System fonts resolved by family name through Fontique, independently for Latin
//! and Chinese, and validated
//! before use. No additional font files are bundled or downloaded by the application.

use std::collections::BTreeSet;
use std::sync::{Arc, LazyLock};

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use fontique::{
    Collection, CollectionOptions, FallbackKey, GenericFamily, QueryFamily, QueryStatus, Script,
    SourceCache, SourceCacheOptions,
};

use crate::i18n::{Language, Locale};

const LATIN_SAMPLE: &str = "Callsign Grid RX WSJT-X JTDX Hybrid DX 0123456789 +-<>?";
fn chinese_sample() -> &'static str {
    static SAMPLE: LazyLock<String> = LazyLock::new(|| {
        include_str!("i18n.rs")
            .chars()
            .filter(|c| !c.is_ascii() && c.is_alphabetic())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    });
    &SAMPLE
}

// Like CSS font-family: two installed families per platform, then a generic family.
// Keep the decode table's Latin fonts monospaced; all named candidates are sans.
#[cfg(target_os = "macos")]
const LATIN: &[QueryFamily<'static>] = &[
    QueryFamily::Named("Monaco"),
    QueryFamily::Named("Menlo"),
    QueryFamily::Generic(GenericFamily::Monospace),
];
#[cfg(target_os = "windows")]
const LATIN: &[QueryFamily<'static>] = &[
    QueryFamily::Named("Consolas"),
    QueryFamily::Named("Cascadia Mono"),
    QueryFamily::Generic(GenericFamily::Monospace),
];
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const LATIN: &[QueryFamily<'static>] = &[
    QueryFamily::Named("DejaVu Sans Mono"),
    QueryFamily::Named("Noto Sans Mono"),
    QueryFamily::Generic(GenericFamily::Monospace),
];

#[cfg(target_os = "macos")]
const CHINESE: &[QueryFamily<'static>] = &[
    QueryFamily::Named("Hiragino Sans GB"),
    QueryFamily::Named("Heiti SC"),
    QueryFamily::Generic(GenericFamily::SansSerif),
];
#[cfg(target_os = "windows")]
const CHINESE: &[QueryFamily<'static>] = &[
    QueryFamily::Named("Microsoft YaHei"),
    QueryFamily::Named("DengXian"),
    QueryFamily::Generic(GenericFamily::SansSerif),
];
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const CHINESE: &[QueryFamily<'static>] = &[
    QueryFamily::Named("Noto Sans CJK SC"),
    QueryFamily::Named("WenQuanYi Micro Hei"),
    QueryFamily::Generic(GenericFamily::SansSerif),
];

pub const CHINESE_UNAVAILABLE: &str =
    "No usable Chinese font was found. Using English. Install a Chinese font such as Microsoft YaHei or Noto Sans CJK SC, then restart.";

#[derive(Clone, Copy, Debug)]
pub struct FontSupport {
    pub chinese: bool,
}

impl FontSupport {
    pub fn locale(self, requested: Locale) -> Locale {
        if requested == Locale::Chinese && !self.chinese {
            Locale::English
        } else {
            requested
        }
    }

    pub fn language_label(self, language: Language, locale: Locale) -> &'static str {
        if language == Language::Chinese && !self.chinese {
            "Chinese"
        } else {
            language.label(locale)
        }
    }
}

pub fn install(ctx: &egui::Context) -> FontSupport {
    // Fontique owns CoreText / DirectWrite / Fontconfig discovery, file formats,
    // font collection indices and weight matching. Default attributes request
    // normal style and weight, just like CSS font-weight: normal.
    let mut collection = Collection::new(CollectionOptions::default());
    let mut cache = SourceCache::new(SourceCacheOptions::default());
    let (definitions, support) = select_fonts(&mut collection, &mut cache);
    // Only install validated results; unchecked providers could reintroduce a
    // font that advertises characters but cannot be drawn by this renderer.
    ctx.set_font_providers(Vec::new());
    ctx.set_fonts(definitions);
    support
}

fn select_fonts(
    collection: &mut Collection,
    cache: &mut SourceCache,
) -> (FontDefinitions, FontSupport) {
    let mut definitions = FontDefinitions::default();
    if let Some(data) = find_font(collection, cache, LATIN, LATIN_SAMPLE, false) {
        insert(&mut definitions, "system-latin", data, true);
    } else {
        log::info!("No usable system monospace font; using egui's built-in Latin fallback");
    }
    let chinese = if let Some(data) = find_font(collection, cache, CHINESE, chinese_sample(), true)
    {
        insert(&mut definitions, "system-chinese", data, false);
        true
    } else {
        log::warn!("{CHINESE_UNAVAILABLE}");
        false
    };
    (definitions, FontSupport { chinese })
}

fn insert(definitions: &mut FontDefinitions, name: &str, data: FontData, primary: bool) {
    definitions
        .font_data
        .insert(name.to_owned(), Arc::new(data));
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let chain = definitions.families.entry(family).or_default();
        if primary {
            chain.insert(0, name.to_owned());
        } else {
            chain.push(name.to_owned());
        }
    }
}

fn find_font(
    collection: &mut Collection,
    cache: &mut SourceCache,
    families: &[QueryFamily<'_>],
    sample: &str,
    chinese: bool,
) -> Option<FontData> {
    for family in families {
        if let QueryFamily::Named(name) = family {
            if collection.family_id(name).is_none() {
                log::info!("GUI font family unavailable to the renderer: {name}");
            }
        }
    }
    let mut query = collection.query(cache);
    query.set_families(families.iter().copied());
    if chinese {
        // Use the same Simplified Chinese locale key as egui's system provider:
        // zh-SG avoids Fontique 0.11's cached, locale-less default Han fallback.
        let locale = fontique::Language::parse("zh-SG").expect("valid locale");
        query.set_fallbacks(FallbackKey::new(
            Script::from_bytes(*b"Hani"),
            Some(&locale),
        ));
    }
    let mut selected = None;
    let mut rejected = Vec::new();
    query.matches_with(|font| {
        let (blob, _) = font.blob.clone().into_raw_parts();
        let mut data = FontData::from_blob(blob, font.index);
        for (tag, value) in font.synthesis.variation_settings() {
            data.tweak.coords.push(tag.to_be_bytes(), *value);
        }
        if chinese {
            // Keep coverage anti-aliasing, but grid-fit small CJK stems in both
            // axes instead of preserving unhinted horizontal outlines.
            data.tweak.hinting = Some(true);
            data.tweak.hinting_target =
                egui::epaint::text::HintingTarget::Smooth(egui::epaint::text::SmoothHinting {
                    light: false,
                    symmetric_rendering: false,
                    preserve_linear_metrics: false,
                });
            data.tweak.subpixel_binning = Some(false);
        }
        balance_line_gap(&mut data);
        match validate_font(&data, sample) {
            Ok(()) => {
                selected = Some((font.family.0, data));
                QueryStatus::Stop
            }
            Err(reason) => {
                rejected.push((font.family.0, font.index, reason));
                QueryStatus::Continue
            }
        }
    });
    drop(query);
    for (family, index, reason) in rejected {
        let name = collection.family_name(family).unwrap_or("unknown");
        log::warn!("Skipping system font {name} (face {index}): {reason}");
    }
    selected.map(|(family, mut data)| {
        let name = collection.family_name(family).unwrap_or("unknown");
        if chinese && name.eq_ignore_ascii_case("Hiragino Sans GB") {
            // A small optical correction after balancing the leading. Keep it
            // specific to this face; other system CJK families have different ink.
            data.tweak.y_offset_factor -= 0.02;
        }
        log::info!(
            "GUI font selected by family name: {name} (face {}; {})",
            data.index,
            font_description(&data)
        );
        data
    })
}

/// egui includes the face's leading in its height, but places glyphs at the
/// ascender without half-leading above them. In mixed runs it then centers the
/// *heights* of fallback faces. A face with large leading (e.g. Hiragino Sans GB:
/// 500 / 1000 em) consequently sits high relative to Latin. Split the leading
/// equally above and below the ink, for every selected system face. Use an em
/// factor so this remains consistent across text styles and display scales.
fn balance_line_gap(data: &mut FontData) {
    if let Ok(face) = ttf_parser::Face::parse(data.bytes(), data.index) {
        data.tweak.y_offset_factor =
            0.5 * f32::from(face.line_gap()) / f32::from(face.units_per_em());
    }
}

fn validate_font(data: &FontData, sample: &str) -> Result<(), &'static str> {
    let face =
        ttf_parser::Face::parse(data.bytes(), data.index).map_err(|_| "invalid font data")?;
    let tables = face.tables();
    if tables.glyf.is_none() && tables.cff.is_none() && tables.cff2.is_none() {
        return Err("no supported glyph outline table");
    }
    if has_serifs(&face) {
        return Err("serif fonts are excluded from GUI fallback");
    }
    if !sample
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|c| face.glyph_index(c).is_some())
    {
        return Err("required characters are missing");
    }
    if !renders(data, sample) {
        return Err("glyphs could not be rasterized");
    }
    Ok(())
}

/// Reject serif faces returned by script fallback as well as named candidates.
/// OpenType OS/2 sFamilyClass and PANOSE are advisory; unknown metadata is left
/// to the system's sans-serif classification rather than guessed from filenames.
fn has_serifs(face: &ttf_parser::Face<'_>) -> bool {
    let Some(os2) = face.raw_face().table(ttf_parser::Tag::from_bytes(b"OS/2")) else {
        return false;
    };
    let family_class = os2.get(30).copied().unwrap_or(0);
    if matches!(family_class, 1..=5 | 7) {
        return true;
    }
    // PANOSE Latin Text family, serif styles 2 through 10 (sans starts at 11).
    os2.get(32) == Some(&2) && os2.get(33).is_some_and(|style| (2..=10).contains(style))
}

fn font_description(data: &FontData) -> String {
    let Ok(face) = ttf_parser::Face::parse(data.bytes(), data.index) else {
        return "invalid font".to_owned();
    };
    let postscript = face
        .names()
        .into_iter()
        .find_map(|name| {
            if name.name_id != ttf_parser::name_id::POST_SCRIPT_NAME {
                return None;
            }
            name.to_string()
                .or_else(|| std::str::from_utf8(name.name).ok().map(str::to_owned))
        })
        .unwrap_or_else(|| "unnamed face".to_owned());
    format!(
        "{postscript}, weight {}, variation {:?}",
        face.weight().to_number(),
        data.tweak.coords
    )
}

/// Metadata is insufficient: a font may advertise glyphs yet draw nothing.
fn renders(data: &FontData, sample: &str) -> bool {
    let mut definitions = FontDefinitions::empty();
    insert(&mut definitions, "probe", data.clone(), true);
    let ctx = egui::Context::default();
    ctx.set_fonts(definitions);
    let mut visible = false;
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
        let galley = ui.painter().layout_no_wrap(
            sample.to_owned(),
            egui::FontId::monospace(14.0),
            egui::Color32::BLACK,
        );
        visible = galley
            .rows
            .iter()
            .flat_map(|row| &row.glyphs)
            .filter(|glyph| !glyph.chr.is_whitespace())
            .all(|glyph| glyph.uv_rect.size.min_elem() > 0.0);
    });
    visible
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated_collection() -> (Collection, SourceCache) {
        (
            Collection::new(CollectionOptions {
                system_fonts: false,
                shared: false,
            }),
            SourceCache::new(SourceCacheOptions::default()),
        )
    }

    #[test]
    fn names_resolve_registered_fonts_and_skip_unusable_preferred_families() {
        let defaults = FontDefinitions::default();
        let good = defaults.font_data["Hack"].bytes().to_vec();
        let mut serif = good.clone();
        let table_count = u16::from_be_bytes([serif[4], serif[5]]) as usize;
        for index in 0..table_count {
            let record = 12 + index * 16;
            if &serif[record..record + 4] == b"OS/2" {
                let offset =
                    u32::from_be_bytes(serif[record + 8..record + 12].try_into().unwrap()) as usize;
                serif[offset + 30..offset + 32].copy_from_slice(&0x0300u16.to_be_bytes());
            }
        }
        assert_eq!(
            validate_font(&FontData::from_owned(serif.clone()), LATIN_SAMPLE),
            Err("serif fonts are excluded from GUI fallback")
        );
        let mut metrics_only = good.clone();
        let table_count = u16::from_be_bytes([metrics_only[4], metrics_only[5]]) as usize;
        for index in 0..table_count {
            let offset = 12 + index * 16;
            if &metrics_only[offset..offset + 4] == b"glyf" {
                metrics_only[offset..offset + 4].copy_from_slice(b"test");
            }
        }
        assert_eq!(
            validate_font(&FontData::from_owned(metrics_only.clone()), LATIN_SAMPLE),
            Err("no supported glyph outline table")
        );
        let (mut collection, mut cache) = isolated_collection();
        for (name, bytes) in [
            ("Serif preferred font", serif),
            ("Unusable preferred font", metrics_only),
            ("User installed fallback", good.clone()),
        ] {
            collection.register_fonts(
                fontique::Blob::new(Arc::new(bytes)),
                Some(fontique::FontInfoOverride {
                    family_name: Some(name),
                    ..Default::default()
                }),
            );
        }
        let data = find_font(
            &mut collection,
            &mut cache,
            &[
                QueryFamily::Named("Missing font"),
                QueryFamily::Named("Serif preferred font"),
                QueryFamily::Named("Unusable preferred font"),
                QueryFamily::Named("User installed fallback"),
            ],
            LATIN_SAMPLE,
            false,
        )
        .expect("the usable named fallback should be selected");
        assert_eq!(data.bytes(), good);
        assert!(renders(&data, LATIN_SAMPLE));
    }

    #[test]
    fn missing_or_corrupt_system_fonts_leave_a_readable_english_ui() {
        for bytes in [None, Some(b"invalid font file".to_vec())] {
            let (mut collection, mut cache) = isolated_collection();
            if let Some(bytes) = bytes {
                assert!(collection
                    .register_fonts(fontique::Blob::new(Arc::new(bytes)), None)
                    .is_empty());
            }
            let (definitions, support) = select_fonts(&mut collection, &mut cache);
            assert!(!support.chinese);
            assert_eq!(support.locale(Locale::Chinese), Locale::English);
            assert_eq!(
                support.language_label(Language::Chinese, Locale::English),
                "Chinese"
            );
            let ctx = egui::Context::default();
            ctx.set_fonts(definitions);
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                let galley = ui.painter().layout_no_wrap(
                    LATIN_SAMPLE.to_owned(),
                    egui::FontId::monospace(14.0),
                    egui::Color32::BLACK,
                );
                assert!(galley
                    .rows
                    .iter()
                    .flat_map(|row| &row.glyphs)
                    .filter(|glyph| !glyph.chr.is_whitespace())
                    .all(|glyph| glyph.uv_rect.size.min_elem() > 0.0));
            });
        }
    }

    #[test]
    fn installed_fonts_center_chinese_and_latin_in_the_same_line() {
        let mut collection = Collection::new(CollectionOptions::default());
        let mut cache = SourceCache::new(SourceCacheOptions::default());
        let (definitions, support) = select_fonts(&mut collection, &mut cache);
        let ctx = egui::Context::default();
        ctx.set_fonts(definitions);
        for scale in [1.0, 1.25, 1.5, 2.0] {
            ctx.set_pixels_per_point(scale);
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                for size in [11.0, 13.0, 14.0, 17.0] {
                    let font = egui::FontId::monospace(size);
                    let latin = ui.painter().layout_no_wrap(
                        "WSJT-X".to_owned(), font.clone(), egui::Color32::BLACK,
                    );
                    assert!((latin.mesh_bounds.center().y - latin.size().y * 0.5).abs() <= 1.01,
                        "Latin ink off center at {scale}x / {size}pt: {:?}", latin.mesh_bounds);
                    if !support.chinese { continue; }
                    let chinese = ui.painter().layout_no_wrap(
                        "电台中田国目".to_owned(), font.clone(), egui::Color32::BLACK,
                    );
                    assert!((chinese.mesh_bounds.center().y - chinese.size().y * 0.5).abs() <= 1.01,
                        "Chinese ink off center at {scale}x / {size}pt: {:?}", chinese.mesh_bounds);
                    let mixed = ui.painter().layout_no_wrap(
                        "仅 WSJT-X".to_owned(), font, egui::Color32::BLACK,
                    );
                    let mut chinese_bounds = egui::Rect::NOTHING;
                    let mut latin_bounds = egui::Rect::NOTHING;
                    for glyph in &mixed.rows[0].glyphs {
                        if glyph.chr.is_whitespace() { continue; }
                        let start = glyph.first_vertex as usize;
                        let bounds = mixed.rows[0].visuals.mesh.vertices[start..start + 4]
                            .iter()
                            .fold(egui::Rect::NOTHING, |mut rect, vertex| {
                                rect.extend_with(vertex.pos);
                                rect
                            });
                        if glyph.chr.is_ascii() {
                            latin_bounds = latin_bounds.union(bounds);
                        } else {
                            chinese_bounds = chinese_bounds.union(bounds);
                        }
                    }
                    assert!((chinese_bounds.center().y - latin_bounds.center().y).abs() <= 1.01,
                        "Mixed text misaligned at {scale}x / {size}pt: CJK={chinese_bounds:?}, Latin={latin_bounds:?}");
                }
            });
        }
    }

    #[test]
    fn installed_fonts_produce_visible_glyphs() {
        let ctx = egui::Context::default();
        let mut collection = Collection::new(CollectionOptions::default());
        let mut cache = SourceCache::new(SourceCacheOptions::default());
        let (definitions, support) = select_fonts(&mut collection, &mut cache);
        if let Some(data) = definitions.font_data.get("system-chinese") {
            eprintln!("Selected Chinese face: {}", font_description(data));
        }
        ctx.set_fonts(definitions);
        for scale in [1.0, 1.25, 1.5, 2.0] {
            ctx.set_pixels_per_point(scale);
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                let sample = if support.chinese {
                    chinese_sample()
                } else {
                    LATIN_SAMPLE
                };
                for size in [11.0, 13.0, 14.0, 17.0] {
                    let galley = ui.painter().layout_no_wrap(
                        sample.to_owned(),
                        egui::FontId::monospace(size),
                        egui::Color32::BLACK,
                    );
                    assert!(
                        galley
                            .rows
                            .iter()
                            .flat_map(|row| &row.glyphs)
                            .filter(|glyph| !glyph.chr.is_whitespace())
                            .all(|glyph| glyph.uv_rect.size.min_elem() > 0.0),
                        "glyph bitmap missing at {scale}x / {size}pt"
                    );
                    let atlas = ui.fonts_mut(|fonts| fonts.image());
                    for glyph in galley
                        .rows
                        .iter()
                        .flat_map(|row| &row.glyphs)
                        .filter(|glyph| !glyph.chr.is_whitespace())
                    {
                        let min = glyph.uv_rect.min;
                        let max = glyph.uv_rect.max;
                        assert!(
                            (min[1]..max[1]).any(|y| (min[0]..max[0]).any(|x| {
                                atlas.pixels[y as usize * atlas.size[0] + x as usize].a() > 0
                            })),
                            "empty bitmap for {:?} at {scale}x / {size}pt",
                            glyph.chr
                        );
                    }
                }
            });
        }
    }
}
