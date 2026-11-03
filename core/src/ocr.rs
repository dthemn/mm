//! PP-OCR input and output handling. The model itself runs in a Web Worker.
//! Parameters follow the reference implementation: detection normalized to [-1, 1],
//! recognition at height 48, CTC decoding with the blank at index 0.

use crate::vision::Gray;

const DET_MIN_SIDE: usize = 736;
const DET_MAX_SIDE: usize = 1920;
const BIN_THRESH: f32 = 0.3;
const BOX_THRESH: f32 = 0.5;
const UNCLIP_RATIO: f32 = 1.6;
pub const REC_HEIGHT: usize = 48;
const REC_MAX_WIDTH: usize = 1280;
const REC_MIN_WIDTH: usize = 96;

/// NCHW tensor data with three equal channels.
pub struct Tensor {
    pub data: Vec<f32>,
    pub h: usize,
    pub w: usize,
}

fn tensor(g: &Gray) -> Tensor {
    let n = g.w * g.h;
    let mut data = vec![0f32; n * 3];
    for (i, p) in g.px.iter().enumerate() {
        let v = *p as f32 / 127.5 - 1.0;
        data[i] = v;
        data[n + i] = v;
        data[2 * n + i] = v;
    }
    Tensor { data, h: g.h, w: g.w }
}

fn round32(v: usize) -> usize {
    ((v + 16) / 32 * 32).max(32)
}

pub fn det_input(g: &Gray) -> Tensor {
    let (short, long) = (g.w.min(g.h), g.w.max(g.h));
    let mut ratio = (DET_MIN_SIDE as f32 / short as f32).max(1.0);
    if long as f32 * ratio > DET_MAX_SIDE as f32 {
        ratio = DET_MAX_SIDE as f32 / long as f32;
    }
    let (w, h) = (round32((g.w as f32 * ratio) as usize), round32((g.h as f32 * ratio) as usize));
    tensor(&g.resized(w, h))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

/// Turns the probability map into boxes in the coordinates of an image of size `w`×`h`.
pub fn det_boxes(prob: &[f32], mh: usize, mw: usize, w: usize, h: usize) -> Vec<TextBox> {
    if prob.len() != mh * mw {
        return vec![];
    }
    let mut seen = vec![false; prob.len()];
    let mut out = vec![];
    let mut stack = vec![];
    let (sx, sy) = (w as f32 / mw as f32, h as f32 / mh as f32);
    for start in 0..prob.len() {
        if seen[start] || prob[start] <= BIN_THRESH {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let (mut n, mut sum) = (0usize, 0f32);
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % mw, i / mw);
            n += 1;
            sum += prob[i];
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            let mut push = |j: usize| {
                if !seen[j] && prob[j] > BIN_THRESH {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 { push(i - 1); }
            if x + 1 < mw { push(i + 1); }
            if y > 0 { push(i - mw); }
            if y + 1 < mh { push(i + mw); }
        }
        let (bw, bh) = ((x1 - x0 + 1) as f32, (y1 - y0 + 1) as f32);
        if bw < 4.0 || bh < 3.0 || n < 8 || sum / (n as f32) < BOX_THRESH {
            continue;
        }
        // The network shrinks text regions, so they are grown back by area over perimeter.
        let grow = bw * bh * UNCLIP_RATIO / (2.0 * (bw + bh));
        out.push(TextBox {
            x0: ((x0 as f32 - grow) * sx).max(0.0),
            y0: ((y0 as f32 - grow) * sy).max(0.0),
            x1: ((x1 as f32 + 1.0 + grow) * sx).min(w as f32),
            y1: ((y1 as f32 + 1.0 + grow) * sy).min(h as f32),
        });
    }
    out
}

pub fn rec_input(g: &Gray, b: &TextBox) -> Option<Tensor> {
    let (x, y) = (b.x0 as usize, b.y0 as usize);
    let (w, h) = ((b.x1 - b.x0) as usize, (b.y1 - b.y0) as usize);
    if w < 2 || h < 2 {
        return None;
    }
    let crop = g.crop(x, y, w, h);
    let width = (crop.w * REC_HEIGHT / crop.h).clamp(REC_MIN_WIDTH, REC_MAX_WIDTH);
    Some(tensor(&crop.resized(width, REC_HEIGHT)))
}

/// `logits` holds T rows of C classes. Class 0 is the CTC blank, then come `dict` entries.
/// Returns the text and the mean confidence of its characters.
pub fn ctc_decode(probs: &[f32], classes: usize, dict: &[String]) -> (String, f32) {
    if classes == 0 {
        return (String::new(), 0.0);
    }
    let (mut text, mut last, mut sum, mut n) = (String::new(), 0usize, 0f32, 0usize);
    for row in probs.chunks_exact(classes) {
        let (mut best, mut at) = (f32::MIN, 0);
        for (i, p) in row.iter().enumerate() {
            if *p > best {
                (best, at) = (*p, i);
            }
        }
        if at != 0 && at != last
            && let Some(ch) = dict.get(at - 1) {
                text.push_str(ch);
                sum += best;
                n += 1;
            }
        last = at;
    }
    (text, if n == 0 { 0.0 } else { sum / n as f32 })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    pub confidence: f32,
}

/// Joins boxes of one printed row, left to right. Rows matter because prices are often
/// detected as a separate column, far from their item name.
/// A recognized box: where it is, what it says, and how sure the model was.
pub type Recognized = (TextBox, String, f32);

pub fn assemble(mut boxes: Vec<Recognized>) -> Vec<Line> {
    boxes.retain(|b| !b.1.trim().is_empty());
    boxes.sort_by(|a, b| (a.0.y0 + a.0.y1).total_cmp(&(b.0.y0 + b.0.y1)));
    let mut rows: Vec<(f32, Vec<Recognized>)> = vec![];
    for b in boxes {
        let cy = (b.0.y0 + b.0.y1) / 2.0;
        let h = b.0.y1 - b.0.y0;
        match rows.iter_mut().find(|r| (r.0 - cy).abs() < 0.5 * h) {
            Some(r) => r.1.push(b),
            None => rows.push((cy, vec![b])),
        }
    }
    rows.into_iter().map(|(_, mut parts)| {
        parts.sort_by(|a, b| a.0.x0.total_cmp(&b.0.x0));
        let confidence = parts.iter().map(|p| p.2).fold(1.0, f32::min);
        Line { text: parts.iter().map(|p| p.1.as_str()).collect::<Vec<_>>().join(" "), confidence }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict() -> Vec<String> {
        ["a", "b", "ł", " "].iter().map(|s| s.to_string()).collect()
    }

    fn row(top: usize) -> Vec<f32> {
        let mut r = vec![0.0; 5];
        r[top] = 0.9;
        r
    }

    #[test]
    fn ctc_merges_repeats_and_keeps_letters_split_by_blank() {
        let seq = [1, 1, 0, 1, 2, 2, 3, 0];
        let probs: Vec<f32> = seq.iter().flat_map(|c| row(*c)).collect();
        let (t, conf) = ctc_decode(&probs, 5, &dict());
        assert_eq!(t, "aabł");
        assert!((conf - 0.9).abs() < 1e-6);
    }

    #[test]
    fn ctc_handles_empty_output() {
        assert_eq!(ctc_decode(&[], 5, &dict()).0, "");
        assert_eq!(ctc_decode(&row(0), 5, &dict()).1, 0.0);
    }

    #[test]
    fn detection_input_is_multiple_of_32_and_normalized() {
        let t = det_input(&Gray::new(1000, 2400, 255));
        assert_eq!((t.w % 32, t.h % 32), (0, 0));
        assert!(t.h <= DET_MAX_SIDE + 16);
        assert_eq!(t.data.len(), 3 * t.w * t.h);
        assert!((t.data[0] - 1.0).abs() < 1e-6);
        assert!((det_input(&Gray::new(64, 64, 0)).data[0] + 1.0).abs() < 1e-6);
    }

    #[test]
    fn boxes_split_lines_and_scale_to_the_image() {
        let (mw, mh) = (100, 40);
        let mut p = vec![0f32; mw * mh];
        for y in 5..9 {
            for x in 10..60 {
                p[y * mw + x] = 0.9;
            }
        }
        for y in 20..24 {
            for x in 10..60 {
                p[y * mw + x] = 0.9;
            }
        }
        let b = det_boxes(&p, mh, mw, 1000, 400);
        assert_eq!(b.len(), 2);
        assert!(b[0].x0 < 100.0 && b[0].x1 > 600.0 && b[0].y1 < b[1].y0);
    }

    #[test]
    fn weak_regions_and_specks_are_ignored() {
        let (mw, mh) = (40, 40);
        let mut p = vec![0f32; mw * mh];
        p[0] = 0.9;
        for y in 10..20 {
            for x in 10..30 {
                p[y * mw + x] = 0.35;
            }
        }
        assert!(det_boxes(&p, mh, mw, 40, 40).is_empty());
        assert!(det_boxes(&p[..5], mh, mw, 40, 40).is_empty());
    }

    #[test]
    fn rec_input_has_fixed_height() {
        let g = Gray::new(800, 200, 200);
        let t = rec_input(&g, &TextBox { x0: 0.0, y0: 0.0, x1: 400.0, y1: 40.0 }).unwrap();
        assert_eq!((t.h, t.w), (REC_HEIGHT, 480));
        assert!(rec_input(&g, &TextBox { x0: 5.0, y0: 5.0, x1: 5.5, y1: 5.5 }).is_none());
    }

    #[test]
    fn columns_join_into_rows_left_to_right() {
        let b = |x0: f32, y0: f32, t: &str, c| (TextBox { x0, y0, x1: x0 + 100.0, y1: y0 + 30.0 }, t.to_string(), c);
        let lines = assemble(vec![b(400.0, 102.0, "5,00 A", 0.9), b(10.0, 100.0, "Mleko", 0.7), b(10.0, 50.0, "PARAGON", 0.95), b(5.0, 150.0, "  ", 0.9)]);
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["PARAGON", "Mleko 5,00 A"]);
        assert!((lines[1].confidence - 0.7).abs() < 1e-6);
    }
}
