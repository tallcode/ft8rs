//! Shared presentation/CLI frequency input rules; no decoder thresholds change.

pub fn validate_frequency_range(low: f64, high: f64) -> Result<(), String> {
    if low.is_finite() && high.is_finite() && 0.0 <= low && low < high && high <= 5000.0 {
        Ok(())
    } else {
        Err("Frequency range must satisfy 0 ≤ Low < High ≤ 5000 Hz".into())
    }
}

/// Zero or an out-of-band frequency disables QSO focus. Malformed/non-finite
/// values and frequencies outside the supported audio range are rejected.
pub fn normalize_rx_frequency(rx: f64, low: f64, high: f64) -> Result<f64, String> {
    validate_frequency_range(low, high)?;
    if !rx.is_finite() || !(0.0..=5000.0).contains(&rx) {
        return Err("RX frequency must be finite and between 0 and 5000 Hz".into());
    }
    Ok(if rx >= low && rx <= high { rx } else { 0.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_nonfinite_and_unsupported_ranges_and_focus() {
        for (low, high) in [
            (5000.0, 3000.0),
            (200.0, 100.0),
            (f64::NAN, 3000.0),
            (0.0, f64::INFINITY),
            (-1.0, 3000.0),
            (0.0, 5001.0),
        ] {
            assert!(validate_frequency_range(low, high).is_err());
        }
        for rx in [f64::NAN, f64::INFINITY, -1.0, 5001.0] {
            assert!(normalize_rx_frequency(rx, 200.0, 3000.0).is_err());
        }
        for rx in [0.0, 100.0, 4000.0] {
            assert_eq!(normalize_rx_frequency(rx, 200.0, 3000.0).unwrap(), 0.0);
        }
        assert_eq!(
            normalize_rx_frequency(1500.0, 200.0, 3000.0).unwrap(),
            1500.0
        );
    }
}
