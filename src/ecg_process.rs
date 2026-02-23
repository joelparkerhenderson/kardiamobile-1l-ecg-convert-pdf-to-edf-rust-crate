use anyhow::{anyhow, Result};
use std::collections::HashMap;

use crate::pdf_extract::{DrawingPath, Point};

/// Extract the baseline y-coordinates for each row from horizontal grid lines.
///
/// The 1-lead PDF displays the single lead across multiple rows on one page.
/// Each row has a horizontal baseline at its center.
pub fn extract_baselines(paths: &[DrawingPath]) -> Result<Vec<f64>> {
    for path in paths {
        let (r, g, b) = path.color;
        // Must be black
        if r != 0.0 || g != 0.0 || b != 0.0 {
            continue;
        }
        // Width ~0.4
        if !(0.35 < path.width && path.width < 0.45) {
            continue;
        }
        // Need at least 4 segments
        if path.segments.len() < 4 {
            continue;
        }

        let mut y_values = Vec::new();
        for (p1, p2) in &path.segments {
            // Horizontal line spanning > 500 units
            if (p1.y - p2.y).abs() < 0.01 && (p2.x - p1.x).abs() > 500.0 {
                y_values.push(p1.y);
            }
        }

        // Only keep baselines within visible page area (y < 760)
        let visible: Vec<f64> = y_values.into_iter().filter(|&y| y < 760.0).collect();
        if visible.len() >= 4 {
            return Ok(visible[..4].to_vec());
        }
    }
    Err(anyhow!("Could not find baseline grid lines in PDF"))
}

/// Extract ECG waveform points grouped by row.
///
/// For a 1-lead PDF, the single lead is displayed across multiple rows,
/// each representing a consecutive time segment.
///
/// Returns: row_index -> list of (x, y) points sorted by x.
pub fn extract_ecg_waveform_rows(
    paths: &[DrawingPath],
    baselines: &[f64],
) -> HashMap<usize, Vec<Point>> {
    let mut rows: HashMap<usize, Vec<Point>> = HashMap::new();
    for i in 0..baselines.len() {
        rows.insert(i, Vec::new());
    }

    for path in paths {
        let (r, g, b) = path.color;
        // Must be black
        if r != 0.0 || g != 0.0 || b != 0.0 {
            continue;
        }
        // Width ~0.4
        if !(0.35 < path.width && path.width < 0.45) {
            continue;
        }
        // ECG paths have many segments
        if path.segments.len() < 40 {
            continue;
        }

        // Extract points from line segments, deduplicating adjacent shared endpoints
        let mut points: Vec<Point> = Vec::new();
        for (p1, p2) in &path.segments {
            if points.is_empty()
                || (points.last().unwrap().x - p1.x).abs() > 0.001
                || (points.last().unwrap().y - p1.y).abs() > 0.001
            {
                points.push(*p1);
            }
            points.push(*p2);
        }

        if points.is_empty() {
            continue;
        }

        // Determine which row by y-center proximity to baselines
        let y_sum: f64 = points.iter().map(|p| p.y).sum();
        let y_center = y_sum / points.len() as f64;

        let mut min_dist = f64::INFINITY;
        let mut best_row = 0usize;
        for (ri, bl) in baselines.iter().enumerate() {
            let dist = (y_center - bl).abs();
            if dist < min_dist {
                min_dist = dist;
                best_row = ri;
            }
        }

        if min_dist < 80.0 {
            rows.entry(best_row).or_default().extend(points);
        }
    }

    // Sort each row's points by x-coordinate
    for points in rows.values_mut() {
        points.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());
    }

    rows
}

/// Convert (x, y) points to voltage values in millivolts.
///
/// In the top-left coordinate system, y increases downward,
/// so voltage = (baseline - y) / scale.
pub fn points_to_voltage(points: &[Point], baseline_y: f64, cal_pt_per_mv: f64) -> Vec<f64> {
    points
        .iter()
        .map(|p| (baseline_y - p.y) / cal_pt_per_mv)
        .collect()
}

/// Process all rows: deduplicate, convert to voltages, concatenate.
pub fn concatenate_to_signal(
    rows: &HashMap<usize, Vec<Point>>,
    baselines: &[f64],
    cal_pt_per_mv: f64,
) -> Result<Vec<f64>> {
    let mut all_voltages = Vec::new();

    for ri in 0..baselines.len() {
        let points = rows.get(&ri).ok_or_else(|| anyhow!("Missing row {}", ri))?;
        if points.is_empty() {
            eprintln!("Row {}: no data", ri);
            continue;
        }

        // Remove duplicate x-coordinates (boundary points between segments)
        let mut deduped = vec![points[0]];
        for i in 1..points.len() {
            if (points[i].x - deduped.last().unwrap().x).abs() > 0.01 {
                deduped.push(points[i]);
            }
        }

        let voltages = points_to_voltage(&deduped, baselines[ri], cal_pt_per_mv);
        let min_v = voltages.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_v = voltages.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

        println!(
            "Row {}: {} samples, x:[{:.1}-{:.1}], range [{:.3}, {:.3}] mV",
            ri,
            voltages.len(),
            deduped.first().unwrap().x,
            deduped.last().unwrap().x,
            min_v,
            max_v
        );

        all_voltages.extend(voltages);
    }

    Ok(all_voltages)
}
