use super::*;

fn insert_decode_row(
    rows: &mut Vec<Row>,
    display: &mut Vec<DisplayItem>,
    incoming: Row,
    tolerance: f64,
) {
    DecodeBatch::new(rows, display, tolerance, DisplayFilters::NONE).insert(incoming);
}

mod comparison;
mod context;
mod display;
