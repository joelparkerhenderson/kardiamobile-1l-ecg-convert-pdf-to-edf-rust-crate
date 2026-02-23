use anyhow::Result;
use std::fs::File;
use std::io::Write;

/// Write a space-padded ASCII field of exact width.
fn write_field(file: &mut File, value: &str, width: usize) -> Result<()> {
    let mut buf = value.as_bytes().to_vec();
    buf.resize(width, b' '); // right-pad with spaces
    buf.truncate(width); // ensure exact width
    file.write_all(&buf)?;
    Ok(())
}

/// Convert a physical voltage value to a 16-bit digital value.
fn voltage_to_digital(voltage: f64, phys_min: f64, phys_max: f64) -> i16 {
    let dig_min: f64 = -32768.0;
    let dig_max: f64 = 32767.0;
    let scaled = dig_min + (voltage - phys_min) / (phys_max - phys_min) * (dig_max - dig_min);
    scaled.round().clamp(-32768.0, 32767.0) as i16
}

/// Build EDF+ TAL (Time-stamped Annotation List) bytes for a data record.
fn make_annotation_bytes(onset_seconds: usize, annotation_samples: usize) -> Vec<u8> {
    let tal = format!("+{}\x14\x14", onset_seconds);
    let mut bytes = tal.into_bytes();
    let total_bytes = annotation_samples * 2;
    bytes.resize(total_bytes, 0); // null-pad to fill annotation channel
    bytes
}

/// Write the ECG signal as an EDF+ file.
pub fn write_edf(path: &str, signal: &[f64], sample_rate: usize) -> Result<()> {
    let record_duration: usize = 1; // 1 second per data record
    let samples_per_record = sample_rate * record_duration;
    let n_records = (signal.len() + samples_per_record - 1) / samples_per_record;
    let n_signals: usize = 2; // EKG + Annotations
    let annotation_samples: usize = 57; // matches pyedflib default
    let header_bytes = 256 + n_signals * 256;

    // Compute physical range with margin
    let phys_min = signal.iter().cloned().fold(f64::INFINITY, f64::min) - 0.1;
    let phys_max = signal.iter().cloned().fold(f64::NEG_INFINITY, f64::max) + 0.1;

    let mut file = File::create(path)?;

    // === Main header (256 bytes) ===
    write_field(&mut file, "0", 8)?; // version
    write_field(&mut file, "X M 04-MAY-1970 Joel_Henderson", 80)?; // patient ID (EDF+)
    write_field(
        &mut file,
        "Startdate 13-FEB-2026 X X KardiaMobile_1L",
        80,
    )?; // recording ID
    write_field(&mut file, "13.02.26", 8)?; // start date
    write_field(&mut file, "22.42.00", 8)?; // start time
    write_field(&mut file, &header_bytes.to_string(), 8)?; // header size
    write_field(&mut file, "EDF+C", 44)?; // reserved (EDF+ continuous)
    write_field(&mut file, &n_records.to_string(), 8)?; // num data records
    write_field(&mut file, &record_duration.to_string(), 8)?; // record duration
    write_field(&mut file, &n_signals.to_string(), 4)?; // num signals

    // === Signal headers (interleaved: all labels, then all transducers, etc.) ===

    // Labels (16 bytes each)
    write_field(&mut file, "EKG I", 16)?;
    write_field(&mut file, "EDF Annotations", 16)?;

    // Transducer type (80 bytes each)
    write_field(&mut file, "KardiaMobile 1L electrode", 80)?;
    write_field(&mut file, "", 80)?;

    // Physical dimension (8 bytes each)
    write_field(&mut file, "mV", 8)?;
    write_field(&mut file, "", 8)?;

    // Physical minimum (8 bytes each)
    write_field(&mut file, &format_edf_num(phys_min), 8)?;
    write_field(&mut file, "-1", 8)?;

    // Physical maximum (8 bytes each)
    write_field(&mut file, &format_edf_num(phys_max), 8)?;
    write_field(&mut file, "1", 8)?;

    // Digital minimum (8 bytes each)
    write_field(&mut file, "-32768", 8)?;
    write_field(&mut file, "-32768", 8)?;

    // Digital maximum (8 bytes each)
    write_field(&mut file, "32767", 8)?;
    write_field(&mut file, "32767", 8)?;

    // Prefiltering (80 bytes each)
    write_field(&mut file, "Enhanced Filter, 50Hz mains", 80)?;
    write_field(&mut file, "", 80)?;

    // Number of samples per data record (8 bytes each)
    write_field(&mut file, &samples_per_record.to_string(), 8)?;
    write_field(&mut file, &annotation_samples.to_string(), 8)?;

    // Reserved (32 bytes each)
    write_field(&mut file, "", 32)?;
    write_field(&mut file, "", 32)?;

    // === Data records ===
    for rec in 0..n_records {
        // ECG samples
        let start = rec * samples_per_record;
        for i in 0..samples_per_record {
            let idx = start + i;
            let phys_val = if idx < signal.len() {
                signal[idx]
            } else {
                0.0
            };
            let dig_val = voltage_to_digital(phys_val, phys_min, phys_max);
            file.write_all(&dig_val.to_le_bytes())?;
        }

        // Annotation samples (TAL)
        let annotation_bytes = make_annotation_bytes(rec * record_duration, annotation_samples);
        file.write_all(&annotation_bytes)?;
    }

    Ok(())
}

/// Format a floating point number for an EDF header field (max 8 chars).
fn format_edf_num(val: f64) -> String {
    // Try full precision, progressively reduce if too long
    for precision in (0..=6).rev() {
        let s = format!("{:.prec$}", val, prec = precision);
        if s.len() <= 8 {
            return s;
        }
    }
    format!("{:.0}", val)
}
