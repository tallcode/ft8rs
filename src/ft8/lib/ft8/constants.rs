//! FT8 sync/tone constants shared by WSJT-X FT8 routines.
//!
//! Sources:
//! - `wsjtx/lib/ft8/ft8b.f90`
//! - `wsjtx/lib/ft8/sync8.f90`
//! - `wsjtx/lib/ft8/ft8_a7.f90`

/// 7-symbol Costas array for sync.
pub const COSTAS: [u8; 7] = [3, 1, 4, 0, 6, 5, 2];

/// 8-tone Gray mapping.
pub const GRAY_MAP: [u8; 8] = [0, 1, 3, 2, 5, 6, 4, 7];
