mod ecg_process;
mod edf_write;
mod pdf_extract;

use anyhow::{anyhow, Result};

fn main() -> Result<()> {
    let pdf_path = "kardiamobile-1l-ecg.pdf";
    let edf_path = "kardiamobile-1l-ecg.edf";

    // Calibration: 1 mV = 28.346 PDF points (10mm at 2.8346 pt/mm)
    let cal_pt_per_mv = 28.346_f64;
    let sample_rate: usize = 300;

    // Load PDF
    let doc = lopdf::Document::load(pdf_path)?;
    let pages = doc.get_pages();
    let &page_id = pages.get(&2).ok_or_else(|| anyhow!("Page 2 not found"))?;

    // Get page height for coordinate transformation
    let page_height = pdf_extract::get_page_height(&doc, page_id)?;

    // Extract drawing paths from page 2
    let paths = pdf_extract::extract_paths(&doc, page_id, page_height)?;

    // Find baselines
    let baselines = ecg_process::extract_baselines(&paths)?;
    println!(
        "Baselines (PDF y-coordinates): {:?}",
        baselines
            .iter()
            .map(|b| format!("{:.1}", b))
            .collect::<Vec<_>>()
    );

    // Extract waveform rows
    let rows = ecg_process::extract_ecg_waveform_rows(&paths, &baselines);

    // Concatenate all rows into a single voltage signal
    let signal = ecg_process::concatenate_to_signal(&rows, &baselines, cal_pt_per_mv)?;

    let duration_sec = signal.len() as f64 / sample_rate as f64;
    let min_v = signal.iter().cloned().fold(f64::INFINITY, f64::min);
    let max_v = signal.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    println!("\nTotal samples: {}", signal.len());
    println!("Duration: {:.2} seconds", duration_sec);
    println!("Sampling rate: {} Hz", sample_rate);
    println!("Voltage range: [{:.3}, {:.3}] mV", min_v, max_v);

    // Write EDF+ file
    edf_write::write_edf(edf_path, &signal, sample_rate)?;

    let file_size = std::fs::metadata(edf_path)?.len();
    println!("\nEDF file written: {}", edf_path);
    println!("File size: {} bytes", file_size);

    Ok(())
}
