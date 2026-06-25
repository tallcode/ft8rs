//! Program identity, copyright, license, and third-party attribution.
//!
//! This is the single source of truth shared by the CLI (`ft8rs license`) and
//! the GUI About dialog, so the credits never drift between front-ends. The
//! per-binary version string is injected separately (`env!("FT8RS_VERSION")`),
//! since it lives in each binary crate, not here.

/// Human-facing program name.
pub const NAME: &str = "ft8.rs";

/// One-line description.
pub const DESCRIPTION: &str = "A streaming FT8 decoder.";

/// Copyright holder line for this project.
pub const COPYRIGHT: &str = "Copyright (C) 2026 Tao Xu (BG5ATV)";

/// License shown to users (SPDX identifier in Cargo.toml is `GPL-3.0-only`).
pub const LICENSE: &str = "GPL-3.0";

/// Project homepage.
pub const HOMEPAGE: &str = "https://github.com/tallcode/ft8rs";

/// The standard GPL "interactive program" short notice (no warranty plus the
/// redistribution terms), shown by the CLI subcommand and the About dialog.
// One logical line (no hard line breaks) so the GUI wraps it naturally; the
// trailing backslashes join the source lines without inserting newlines.
pub const WARRANTY_NOTICE: &str = "\
This program comes with ABSOLUTELY NO WARRANTY. This is free software, and you \
are welcome to redistribute it under the terms of version 3 of the GNU General \
Public License. See the bundled LICENSE file for the full terms.";

/// Where the full license text lives.
pub const LICENSE_URL: &str = "https://www.gnu.org/licenses/gpl-3.0.html";

/// WSJT-X's license requires this exact notice be displayed prominently in any
/// derivative work (verbatim from WSJT-X `mainwindow.cpp`,
/// `on_actionCopyright_Notice_triggered`). ft8.rs is such a derivative.
pub const WSJTX_COPYRIGHT_NOTICE: &str = "\
The algorithms, source code, look-and-feel of WSJT-X and related programs, and \
protocol specifications for the modes FSK441, FST4, FT8, JT4, JT6M, JT9, JT65, \
JTMS, QRA64, Q65, MSK144 are Copyright (C) 2001-2026 by one or more of the \
following authors: Joseph Taylor, K1JT; Bill Somerville, G4WJS; Steven Franke, \
K9AN; Nico Palermo, IV3NWV; Greg Beam, KI7MT; Michael Black, W9MDB; Edson \
Pereira, PY2SDR; Philip Karn, KA9Q; Uwe Risse, DG2YCB; Brian Moran, N9ADG; \
Roger Rehr, W3SZ; John Nelson, G4KLA; Charlie Suckling, DL3WDG; Terrell Deppe, \
KJ5HST; and other members of the WSJT Development Group.";

/// An upstream work this project derives from. The FT8 decoder is a Rust port
/// of WSJT-X / JTDX, so those works' GPL terms flow through to this one.
pub struct Attribution {
    /// Project name.
    pub name: &'static str,
    /// What this project takes from it.
    pub detail: &'static str,
    /// Upstream copyright line.
    pub copyright: &'static str,
    /// Upstream license.
    pub license: &'static str,
    /// Upstream homepage.
    pub url: &'static str,
}

/// GPL works the decoder is derived from (the reason ft8.rs is GPL-licensed).
pub const ATTRIBUTIONS: &[Attribution] = &[
    Attribution {
        name: "WSJT-X",
        detail: "FT8 protocol and the original decoder, ported here from Fortran.",
        copyright: "Copyright (C) 2001-2026 Joe Taylor (K1JT), Bill Somerville (G4WJS), Steven Franke (K9AN), Nico Palermo (IV3NWV), and the WSJT Development Group",
        license: "GPL-3.0",
        url: "https://wsjt.sourceforge.io/",
    },
    Attribution {
        name: "JTDX",
        detail: "Deep-decode improvements (a WSJT-X derivative), also ported here.",
        copyright: "Copyright (C) 2016-2022 Igor Chernikov (UA3DJY) and Arvo Järve (ES1JA)",
        license: "GPL-3.0",
        url: "https://www.jtdx.tech/",
    },
];

/// Other (permissively licensed) third-party crates, credited for goodwill.
pub const LIBRARIES: &[(&str, &str)] = &[
    ("RustFFT", "FFT engine"),
    ("egui / eframe", "user interface"),
    ("cpal", "audio capture"),
];

/// Render the full copyright / license / attribution block as plain text, with
/// the given version string. Used by the CLI `license` subcommand.
pub fn notice(version: &str) -> String {
    let mut out = String::new();

    // Header: who, what, license, where.
    out.push_str(&format!("{NAME} {version} — {DESCRIPTION}\n"));
    out.push_str(&format!("{COPYRIGHT}\n"));
    out.push_str(&format!("License:  {LICENSE}  (see LICENSE, or {LICENSE_URL})\n"));
    out.push_str(&format!("Homepage: {HOMEPAGE}\n\n"));
    out.push_str(WARRANTY_NOTICE);
    out.push('\n');

    // Why it is GPL: it is a port of WSJT-X / JTDX.
    out.push_str(
        "\nft8.rs's FT8 decoder is a Rust port of the WSJT-X and JTDX decoders, so it\n\
         is distributed under the same GNU GPL terms as the works it derives from:\n",
    );
    for a in ATTRIBUTIONS {
        out.push_str(&format!("\n  {} — {}\n", a.name, a.detail));
        out.push_str(&format!("    {}\n", a.copyright));
        out.push_str(&format!("    {} · {}\n", a.license, a.url));
    }

    let libs: Vec<&str> = LIBRARIES.iter().map(|(name, _)| *name).collect();
    out.push_str(&format!("\nAlso built with: {}.\n", libs.join(", ")));

    // The WSJT-X copyright notice is the longest block, so it comes last.
    out.push_str("\nWSJT-X copyright notice:\n\n  \"");
    out.push_str(WSJTX_COPYRIGHT_NOTICE);
    out.push_str("\"\n");

    out
}
