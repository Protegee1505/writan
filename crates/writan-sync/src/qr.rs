//! The pairing QR (ADR-0007 §4): a payload text from
//! [`pair_payload`](crate::server::pair_payload) drawn as a module matrix or a
//! small SVG, so both shells show the same code.
//!
//! A QR is never secret material here: it carries a single-use code that
//! grants blob access only.

use crate::server::ServerError;

/// A QR code as a square module grid; `true` is a dark module, row by row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrMatrix {
    width: usize,
    modules: Vec<bool>,
}

impl QrMatrix {
    /// Modules per side.
    pub fn width(&self) -> usize {
        self.width
    }

    /// The module at (x, y); `false` outside the matrix.
    pub fn at(&self, x: usize, y: usize) -> bool {
        x < self.width && y < self.width && self.modules[y * self.width + x]
    }

    /// Every module, row by row.
    pub fn modules(&self) -> &[bool] {
        &self.modules
    }
}

/// Encode `text` as a QR code with error correction M.
pub fn qr_matrix(text: &str) -> Result<QrMatrix, ServerError> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M)
        .map_err(|error| ServerError::Refused(format!("could not build a QR code: {error}")))?;
    let width = code.width();
    let modules = code
        .to_colors()
        .into_iter()
        .map(|color| color == qrcode::Color::Dark)
        .collect();
    Ok(QrMatrix { width, modules })
}

/// The same QR as a standalone SVG picture: one rectangle per dark module,
/// `scale` pixels per module, with the four-module quiet zone (white, on
/// purpose — a QR needs the margin).
pub fn qr_svg(text: &str, scale: u32) -> Result<String, ServerError> {
    let matrix = qr_matrix(text)?;
    let quiet = 4;
    let side = (matrix.width() + quiet * 2) as u32 * scale;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{side}\" height=\"{side}\" \
         viewBox=\"0 0 {side} {side}\" shape-rendering=\"crispEdges\">\
         <rect width=\"{side}\" height=\"{side}\" fill=\"#ffffff\"/><g fill=\"#000000\">"
    );
    for (index, dark) in matrix.modules().iter().enumerate() {
        if !*dark {
            continue;
        }
        let x = (index % matrix.width() + quiet) as u32 * scale;
        let y = (index / matrix.width() + quiet) as u32 * scale;
        svg.push_str(&format!(
            "<rect x=\"{x}\" y=\"{y}\" width=\"{scale}\" height=\"{scale}\"/>"
        ));
    }
    svg.push_str("</g></svg>");
    Ok(svg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_payload_becomes_a_square_matrix() {
        let text = crate::server::pair_payload(
            "https://notes.example.com/",
            "DjqT6IkcukpA65-9NoH504L5iCS7tEBRcf3ICW-emcw",
            "ada@example.com",
        );
        let matrix = qr_matrix(&text).expect("encode");
        assert!(matrix.width() >= 21, "at least version 1");
        assert_eq!(matrix.modules().len(), matrix.width() * matrix.width());
        // Every QR code has finder patterns in three corners.
        assert!(matrix.at(0, 0));
        assert!(matrix.at(matrix.width() - 1, 0));
        assert!(matrix.at(0, matrix.width() - 1));
        assert!(matrix.at(matrix.width() - 1, matrix.width() - 1));
        assert!(!matrix.at(matrix.width(), 0), "outside the matrix");
    }

    #[test]
    fn the_svg_carries_the_quiet_zone_and_modules() {
        let svg = qr_svg("hello world", 4).expect("svg");
        assert!(svg.starts_with("<svg"), "{svg}");
        assert!(svg.ends_with("</svg>"));
        // 11 bytes fit version 1 (21 modules); 21 + 2 x 4 quiet modules make
        // 29 modules at 4 px each.
        assert!(svg.contains("width=\"116\""), "sized for the quiet zone");
        assert!(svg.contains("fill=\"#ffffff\""), "white quiet zone");
        assert!(svg.contains("<rect x=\"16\" y=\"16\""), "quiet zone offset");
    }
}
