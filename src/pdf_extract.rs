use anyhow::{anyhow, Result};
use lopdf::content::Content;
use lopdf::{Document, Object, ObjectId};

/// A 2D point in top-left-origin coordinates (matching pymupdf convention).
#[derive(Debug, Clone, Copy)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// A drawing path extracted from the PDF content stream.
#[derive(Debug, Clone)]
pub struct DrawingPath {
    /// Line segments as (start, end) pairs.
    pub segments: Vec<(Point, Point)>,
    /// Stroke color as (r, g, b). Black = (0, 0, 0).
    pub color: (f64, f64, f64),
    /// Line width in PDF user units.
    pub width: f64,
}

/// Graphics state tracked during content stream parsing.
#[derive(Clone)]
struct GraphicsState {
    /// Current transformation matrix [a, b, c, d, e, f].
    ctm: [f64; 6],
    /// Stroke color as RGB.
    stroke_color: (f64, f64, f64),
    /// Line width.
    line_width: f64,
}

impl Default for GraphicsState {
    fn default() -> Self {
        Self {
            ctm: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            stroke_color: (0.0, 0.0, 0.0),
            line_width: 1.0,
        }
    }
}

/// Multiply two 2D affine transformation matrices.
/// Each matrix is [a, b, c, d, e, f] representing:
///   | a c e |
///   | b d f |
///   | 0 0 1 |
/// Result = m1 * m2 (m2 applied first, then m1).
fn multiply_ctm(m1: &[f64; 6], m2: &[f64; 6]) -> [f64; 6] {
    [
        m1[0] * m2[0] + m1[2] * m2[1],
        m1[1] * m2[0] + m1[3] * m2[1],
        m1[0] * m2[2] + m1[2] * m2[3],
        m1[1] * m2[2] + m1[3] * m2[3],
        m1[0] * m2[4] + m1[2] * m2[5] + m1[4],
        m1[1] * m2[4] + m1[3] * m2[5] + m1[5],
    ]
}

/// Transform a raw PDF coordinate through the CTM, then flip y to top-left origin.
fn transform_point(x: f64, y: f64, ctm: &[f64; 6], page_height: f64) -> Point {
    let x_pdf = ctm[0] * x + ctm[2] * y + ctm[4];
    let y_pdf = ctm[1] * x + ctm[3] * y + ctm[5];
    Point {
        x: x_pdf,
        y: page_height - y_pdf,
    }
}

/// Extract a numeric value from a lopdf Object.
fn obj_f64(obj: &Object) -> Result<f64> {
    match obj {
        Object::Real(f) => Ok(*f as f64),
        Object::Integer(i) => Ok(*i as f64),
        _ => Err(anyhow!("Expected number, got {:?}", obj)),
    }
}

/// Dereference an Object if it's a Reference, otherwise return as-is.
fn deref<'a>(doc: &'a Document, obj: &'a Object) -> Result<&'a Object> {
    match obj {
        Object::Reference(id) => doc.get_object(*id).map_err(|e| anyhow!("{}", e)),
        _ => Ok(obj),
    }
}

/// Get the page height from the MediaBox (checking page dict, then parent).
pub fn get_page_height(doc: &Document, page_id: ObjectId) -> Result<f64> {
    get_page_height_inner(doc, page_id, 0)
}

fn get_page_height_inner(doc: &Document, obj_id: ObjectId, depth: usize) -> Result<f64> {
    if depth > 10 {
        return Ok(792.0); // default US Letter
    }
    let obj = doc.get_object(obj_id)?;
    let dict = obj.as_dict().map_err(|e| anyhow!("{}", e))?;

    if let Ok(mb) = dict.get(b"MediaBox") {
        let mb = deref(doc, mb)?;
        if let Object::Array(arr) = mb {
            if arr.len() == 4 {
                return obj_f64(&arr[3]);
            }
        }
    }

    // Walk up to parent
    if let Ok(parent_ref) = dict.get(b"Parent") {
        if let Object::Reference(parent_id) = parent_ref {
            return get_page_height_inner(doc, *parent_id, depth + 1);
        }
    }

    Ok(792.0)
}

/// Extract all stroked drawing paths from a PDF page's content stream.
pub fn extract_paths(
    doc: &Document,
    page_id: ObjectId,
    page_height: f64,
) -> Result<Vec<DrawingPath>> {
    let content_bytes = doc.get_page_content(page_id)?;
    let content = Content::decode(&content_bytes).map_err(|e| anyhow!("{}", e))?;

    let mut paths = Vec::new();
    let mut state = GraphicsState::default();
    let mut state_stack: Vec<GraphicsState> = Vec::new();
    let mut current_segments: Vec<(Point, Point)> = Vec::new();
    let mut current_pos = Point { x: 0.0, y: 0.0 };
    let mut subpath_start = Point { x: 0.0, y: 0.0 };

    for op in &content.operations {
        match op.operator.as_str() {
            // Save/restore graphics state
            "q" => {
                state_stack.push(state.clone());
            }
            "Q" => {
                if let Some(s) = state_stack.pop() {
                    state = s;
                }
            }

            // Concat transformation matrix
            "cm" => {
                if op.operands.len() == 6 {
                    let m = [
                        obj_f64(&op.operands[0])?,
                        obj_f64(&op.operands[1])?,
                        obj_f64(&op.operands[2])?,
                        obj_f64(&op.operands[3])?,
                        obj_f64(&op.operands[4])?,
                        obj_f64(&op.operands[5])?,
                    ];
                    state.ctm = multiply_ctm(&state.ctm, &m);
                }
            }

            // Set line width
            "w" => {
                if let Some(w) = op.operands.first() {
                    state.line_width = obj_f64(w)?;
                }
            }

            // Set stroke color (RGB)
            "RG" => {
                if op.operands.len() == 3 {
                    state.stroke_color = (
                        obj_f64(&op.operands[0])?,
                        obj_f64(&op.operands[1])?,
                        obj_f64(&op.operands[2])?,
                    );
                }
            }

            // Set stroke color (grayscale)
            "G" => {
                if let Some(g) = op.operands.first() {
                    let v = obj_f64(g)?;
                    state.stroke_color = (v, v, v);
                }
            }

            // Set stroke color (CMYK)
            "K" => {
                if op.operands.len() == 4 {
                    let c = obj_f64(&op.operands[0])?;
                    let m = obj_f64(&op.operands[1])?;
                    let y = obj_f64(&op.operands[2])?;
                    let k = obj_f64(&op.operands[3])?;
                    state.stroke_color = (
                        (1.0 - c) * (1.0 - k),
                        (1.0 - m) * (1.0 - k),
                        (1.0 - y) * (1.0 - k),
                    );
                }
            }

            // Set stroke color (generic, variable operands)
            "SC" | "SCN" => {
                match op.operands.len() {
                    1 => {
                        let v = obj_f64(&op.operands[0])?;
                        state.stroke_color = (v, v, v);
                    }
                    3 => {
                        state.stroke_color = (
                            obj_f64(&op.operands[0])?,
                            obj_f64(&op.operands[1])?,
                            obj_f64(&op.operands[2])?,
                        );
                    }
                    4 => {
                        let c = obj_f64(&op.operands[0])?;
                        let m = obj_f64(&op.operands[1])?;
                        let y = obj_f64(&op.operands[2])?;
                        let k = obj_f64(&op.operands[3])?;
                        state.stroke_color = (
                            (1.0 - c) * (1.0 - k),
                            (1.0 - m) * (1.0 - k),
                            (1.0 - y) * (1.0 - k),
                        );
                    }
                    _ => {}
                }
            }

            // Moveto
            "m" => {
                if op.operands.len() == 2 {
                    let x = obj_f64(&op.operands[0])?;
                    let y = obj_f64(&op.operands[1])?;
                    let p = transform_point(x, y, &state.ctm, page_height);
                    current_pos = p;
                    subpath_start = p;
                }
            }

            // Lineto
            "l" => {
                if op.operands.len() == 2 {
                    let x = obj_f64(&op.operands[0])?;
                    let y = obj_f64(&op.operands[1])?;
                    let new_pos = transform_point(x, y, &state.ctm, page_height);
                    current_segments.push((current_pos, new_pos));
                    current_pos = new_pos;
                }
            }

            // Close subpath
            "h" => {
                if (current_pos.x - subpath_start.x).abs() > 0.001
                    || (current_pos.y - subpath_start.y).abs() > 0.001
                {
                    current_segments.push((current_pos, subpath_start));
                    current_pos = subpath_start;
                }
            }

            // Rectangle
            "re" => {
                if op.operands.len() == 4 {
                    let rx = obj_f64(&op.operands[0])?;
                    let ry = obj_f64(&op.operands[1])?;
                    let rw = obj_f64(&op.operands[2])?;
                    let rh = obj_f64(&op.operands[3])?;
                    let p1 = transform_point(rx, ry, &state.ctm, page_height);
                    let p2 = transform_point(rx + rw, ry, &state.ctm, page_height);
                    let p3 = transform_point(rx + rw, ry + rh, &state.ctm, page_height);
                    let p4 = transform_point(rx, ry + rh, &state.ctm, page_height);
                    current_segments.push((p1, p2));
                    current_segments.push((p2, p3));
                    current_segments.push((p3, p4));
                    current_segments.push((p4, p1));
                    current_pos = p1;
                    subpath_start = p1;
                }
            }

            // Stroke path
            "S" => {
                emit_path(&mut paths, &mut current_segments, &state);
            }

            // Close and stroke
            "s" => {
                if (current_pos.x - subpath_start.x).abs() > 0.001
                    || (current_pos.y - subpath_start.y).abs() > 0.001
                {
                    current_segments.push((current_pos, subpath_start));
                }
                emit_path(&mut paths, &mut current_segments, &state);
            }

            // Fill operations — discard path
            "f" | "F" | "f*" => {
                current_segments.clear();
            }

            // Fill and stroke
            "B" | "B*" | "b" | "b*" => {
                emit_path(&mut paths, &mut current_segments, &state);
            }

            // End path without painting
            "n" => {
                current_segments.clear();
            }

            _ => {}
        }
    }

    Ok(paths)
}

fn emit_path(
    paths: &mut Vec<DrawingPath>,
    segments: &mut Vec<(Point, Point)>,
    state: &GraphicsState,
) {
    if !segments.is_empty() {
        paths.push(DrawingPath {
            segments: segments.drain(..).collect(),
            color: state.stroke_color,
            width: state.line_width,
        });
    }
}
