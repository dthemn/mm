//! Receipt photo preparation: cropping, deskewing and light flattening.
//! Floats are fine here, because this code handles pixels and never money.

#[derive(Clone, Debug, PartialEq)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

const MAX_SIDE: usize = 2400;
const MAX_SKEW: f32 = 8.0;
const SKEW_STEP: f32 = 0.5;

impl Gray {
    pub fn new(w: usize, h: usize, value: u8) -> Self {
        Self { w, h, px: vec![value; w * h] }
    }

    pub fn from_rgba(rgba: &[u8], w: usize, h: usize) -> Option<Self> {
        if w == 0 || h == 0 || rgba.len() != w.checked_mul(h)?.checked_mul(4)? {
            return None;
        }
        let px = rgba.as_chunks::<4>().0.iter().map(|p| ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8).collect();
        Some(Self { w, h, px })
    }

    pub fn at(&self, x: usize, y: usize) -> u8 {
        self.px[y * self.w + x]
    }

    /// Bilinear sampling, with box averaging when shrinking a lot.
    pub fn resized(&self, w: usize, h: usize) -> Self {
        let (w, h) = (w.max(1), h.max(1));
        let (sx, sy) = (self.w as f32 / w as f32, self.h as f32 / h as f32);
        if sx >= 2.0 || sy >= 2.0 {
            return self.area_resized(w, h);
        }
        let mut out = Self::new(w, h, 0);
        for y in 0..h {
            let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (self.h - 1) as f32);
            let (y0, ty) = (fy as usize, fy.fract());
            let y1 = (y0 + 1).min(self.h - 1);
            for x in 0..w {
                let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (self.w - 1) as f32);
                let (x0, tx) = (fx as usize, fx.fract());
                let x1 = (x0 + 1).min(self.w - 1);
                let top = self.at(x0, y0) as f32 * (1.0 - tx) + self.at(x1, y0) as f32 * tx;
                let bot = self.at(x0, y1) as f32 * (1.0 - tx) + self.at(x1, y1) as f32 * tx;
                out.px[y * w + x] = (top * (1.0 - ty) + bot * ty + 0.5) as u8;
            }
        }
        out
    }

    fn area_resized(&self, w: usize, h: usize) -> Self {
        let mut out = Self::new(w, h, 0);
        for y in 0..h {
            let (y0, y1) = (y * self.h / h, ((y + 1) * self.h / h).max(y * self.h / h + 1).min(self.h));
            for x in 0..w {
                let (x0, x1) = (x * self.w / w, ((x + 1) * self.w / w).max(x * self.w / w + 1).min(self.w));
                let mut sum = 0u32;
                for yy in y0..y1 {
                    for xx in x0..x1 {
                        sum += self.at(xx, yy) as u32;
                    }
                }
                out.px[y * w + x] = (sum / ((x1 - x0) * (y1 - y0)) as u32) as u8;
            }
        }
        out
    }

    pub fn crop(&self, x: usize, y: usize, w: usize, h: usize) -> Self {
        let (x, y) = (x.min(self.w - 1), y.min(self.h - 1));
        let (w, h) = (w.clamp(1, self.w - x), h.clamp(1, self.h - y));
        let mut px = Vec::with_capacity(w * h);
        for row in y..y + h {
            px.extend_from_slice(&self.px[row * self.w + x..row * self.w + x + w]);
        }
        Self { w, h, px }
    }

    pub fn rotated(&self, degrees: f32, fill: u8) -> Self {
        if degrees == 0.0 {
            return self.clone();
        }
        let (s, c) = degrees.to_radians().sin_cos();
        let (cx, cy) = (self.w as f32 / 2.0, self.h as f32 / 2.0);
        let mut out = Self::new(self.w, self.h, fill);
        for y in 0..self.h {
            let dy = y as f32 - cy;
            for x in 0..self.w {
                let dx = x as f32 - cx;
                let (sx, sy) = (c * dx - s * dy + cx, s * dx + c * dy + cy);
                if sx >= 0.0 && sy >= 0.0 && sx < (self.w - 1) as f32 && sy < (self.h - 1) as f32 {
                    let (x0, y0) = (sx as usize, sy as usize);
                    let (tx, ty) = (sx.fract(), sy.fract());
                    let top = self.at(x0, y0) as f32 * (1.0 - tx) + self.at(x0 + 1, y0) as f32 * tx;
                    let bot = self.at(x0, y0 + 1) as f32 * (1.0 - tx) + self.at(x0 + 1, y0 + 1) as f32 * tx;
                    out.px[y * self.w + x] = (top * (1.0 - ty) + bot * ty) as u8;
                }
            }
        }
        out
    }

    /// Box blur repeated three times, which is close to a Gaussian and independent of the radius.
    fn blurred(&self, radius: usize) -> Vec<f32> {
        let mut cur: Vec<f32> = self.px.iter().map(|p| *p as f32).collect();
        for _ in 0..3 {
            cur = box_pass(&cur, self.w, self.h, radius, true);
            cur = box_pass(&cur, self.w, self.h, radius, false);
        }
        cur
    }
}

fn box_pass(src: &[f32], w: usize, h: usize, r: usize, horizontal: bool) -> Vec<f32> {
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let mut out = vec![0f32; src.len()];
    let idx = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
    let span = (2 * r + 1) as f32;
    for line in 0..lines {
        let mut sum = 0f32;
        for i in 0..=2 * r {
            sum += src[idx(line, i.saturating_sub(r).min(len - 1))];
        }
        for i in 0..len {
            out[idx(line, i)] = sum / span;
            sum += src[idx(line, (i + r + 1).min(len - 1))] - src[idx(line, i.saturating_sub(r))];
        }
    }
    out
}

fn otsu(px: &[u8]) -> u8 {
    let mut hist = [0u64; 256];
    for p in px {
        hist[*p as usize] += 1;
    }
    let total = px.len() as f64;
    let sum: f64 = hist.iter().enumerate().map(|(i, n)| i as f64 * *n as f64).sum();
    let (mut bg, mut sum_bg, mut best, mut at) = (0f64, 0f64, -1f64, 128u8);
    for (i, n) in hist.iter().enumerate() {
        bg += *n as f64;
        if bg == 0.0 {
            continue;
        }
        let fg = total - bg;
        if fg == 0.0 {
            break;
        }
        sum_bg += i as f64 * *n as f64;
        let between = bg * fg * (sum_bg / bg - (sum - sum_bg) / fg).powi(2);
        if between > best {
            (best, at) = (between, i as u8);
        }
    }
    at
}

/// Box of the largest bright region, on a small copy. None if nothing looks like paper.
fn paper_box(g: &Gray) -> Option<(usize, usize, usize, usize)> {
    let scale = (g.w.max(g.h) / 320).max(1);
    let small = g.resized(g.w / scale, g.h / scale);
    let thr = otsu(&small.px);
    let mut label = vec![false; small.px.len()];
    let (mut best, mut best_box) = (0usize, None);
    let mut stack = vec![];
    for start in 0..small.px.len() {
        if label[start] || small.px[start] <= thr {
            continue;
        }
        label[start] = true;
        stack.push(start);
        let (mut n, mut x0, mut y0, mut x1, mut y1) = (0, usize::MAX, usize::MAX, 0, 0);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % small.w, i / small.w);
            n += 1;
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            let mut push = |j: usize| {
                if !label[j] && small.px[j] > thr {
                    label[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 { push(i - 1); }
            if x + 1 < small.w { push(i + 1); }
            if y > 0 { push(i - small.w); }
            if y + 1 < small.h { push(i + small.w); }
        }
        if n > best {
            best = n;
            best_box = Some((x0, y0, x1, y1));
        }
    }
    let (x0, y0, x1, y1) = best_box?;
    if best * 100 < small.px.len() * 8 {
        return None;
    }
    let m = 4 * scale;
    let (x, y) = ((x0 * scale).saturating_sub(m), (y0 * scale).saturating_sub(m));
    Some((x, y, ((x1 + 1) * scale + 2 * m).saturating_sub(x), ((y1 + 1) * scale + 2 * m).saturating_sub(y)))
}

/// Skew in degrees, found from dark strokes so that the background cannot mislead it.
fn skew_of(g: &Gray) -> f32 {
    let scale = (g.w.max(g.h) / 800).max(1);
    let small = g.resized(g.w / scale, g.h / scale);
    let bg = small.blurred(8);
    let mut ink = Gray::new(small.w, small.h, 0);
    let pad = small.w.min(small.h) / 12;
    for y in pad..small.h.saturating_sub(pad) {
        for x in pad..small.w.saturating_sub(pad) {
            if (small.at(x, y) as f32) < bg[y * small.w + x] * 0.82 {
                ink.px[y * small.w + x] = 255;
            }
        }
    }
    let (mut best, mut at) = (-1f64, 0f32);
    let steps = (2.0 * MAX_SKEW / SKEW_STEP) as i32;
    for s in 0..=steps {
        let angle = -MAX_SKEW + s as f32 * SKEW_STEP;
        let r = ink.rotated(angle, 0);
        let rows: Vec<f64> = r.px.chunks_exact(r.w).map(|row| row.iter().map(|p| *p as f64).sum()).collect();
        let mean = rows.iter().sum::<f64>() / rows.len() as f64;
        let score: f64 = rows.iter().map(|v| (v - mean).powi(2)).sum();
        if score > best {
            (best, at) = (score, angle);
        }
    }
    at
}

/// Divides by the local background and stretches contrast, keeping gray levels:
/// a hard threshold destroys the strokes of faint thermal print.
fn flatten(g: &Gray) -> Gray {
    let radius = (g.w.max(g.h) / 60).clamp(8, 40);
    let bg = g.blurred(radius);
    let ratio: Vec<f32> = g.px.iter().zip(&bg).map(|(p, b)| *p as f32 / (b + 1.0)).collect();
    let mut hist = [0u32; 512];
    for r in &ratio {
        hist[((r * 256.0) as usize).min(511)] += 1;
    }
    let pick = |share: f32| {
        let want = (ratio.len() as f32 * share) as u32;
        let mut seen = 0;
        for (i, n) in hist.iter().enumerate() {
            seen += n;
            if seen >= want {
                return i as f32 / 256.0;
            }
        }
        1.0
    };
    let (lo, hi) = (pick(0.015), pick(0.60));
    let span = (hi - lo).max(1e-3);
    let px = ratio.iter().map(|r| (((r - lo) / span).clamp(0.0, 1.0) * 255.0) as u8).collect();
    Gray { w: g.w, h: g.h, px }
}

/// Light text on a dark background, as in a screenshot of an e-receipt in dark mode: most of the picture is dark,
/// and no bright area is big enough to be paper (a white barcode panel is not).
fn light_on_dark(g: &Gray, paper: Option<(usize, usize, usize, usize)>) -> bool {
    let thr = otsu(&g.px);
    let dark = g.px.iter().filter(|p| **p <= thr).count();
    let paper_share = paper.map_or(0.0, |(_, _, w, h)| (w * h) as f64 / (g.w * g.h) as f64);
    dark * 10 > g.px.len() * 6 && paper_share < 0.3
}

/// The full preparation of a photo for recognition.
pub fn prepare(mut g: Gray) -> Gray {
    let side = g.w.max(g.h);
    if side > MAX_SIDE {
        g = g.resized(g.w * MAX_SIDE / side, g.h * MAX_SIDE / side);
    }
    let paper = paper_box(&g);
    if light_on_dark(&g, paper) {
        // A screenshot of an e-receipt in a dark app: the whole picture is the receipt, with inverted colors.
        g.px.iter_mut().for_each(|p| *p = 255 - *p);
    } else if let Some((x, y, w, h)) = paper {
        g = g.crop(x, y, w, h);
    }
    let angle = skew_of(&g);
    let fill = otsu(&g.px).saturating_add(60);
    flatten(&g.rotated(angle, fill))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bright receipt with dark text rows on a dark table.
    fn scene(angle: f32) -> Gray {
        let mut paper = Gray::new(400, 700, 40);
        for y in 60..640 {
            for x in 80..320 {
                paper.px[y * 400 + x] = if y % 24 < 6 && (x / 14) % 3 != 0 { 30 } else { 235 };
            }
        }
        paper.rotated(angle, 40)
    }

    fn row_contrast(g: &Gray) -> f64 {
        let rows: Vec<f64> = g.px.chunks_exact(g.w).map(|r| r.iter().map(|p| *p as f64).sum::<f64>() / g.w as f64).collect();
        let mean = rows.iter().sum::<f64>() / rows.len() as f64;
        rows.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / rows.len() as f64
    }

    /// Light text rows on a dark screen, with a white barcode panel at the bottom.
    fn dark_screenshot() -> Gray {
        let mut g = Gray::new(400, 800, 22);
        for y in 40..600 {
            for x in 30..370 {
                if y % 30 < 8 && (x / 12) % 3 != 0 {
                    g.px[y * 400 + x] = 230;
                }
            }
        }
        for y in 660..760 {
            for x in 30..370 {
                g.px[y * 400 + x] = if (x / 5) % 2 == 0 { 250 } else { 10 };
            }
        }
        g
    }

    #[test]
    fn a_dark_screenshot_is_inverted_not_cropped_to_its_barcode() {
        let g = dark_screenshot();
        assert!(light_on_dark(&g, paper_box(&g)));
        let p = prepare(g);
        assert_eq!((p.w, p.h), (400, 800), "the whole screen is kept");
        let mean = p.px.iter().map(|x| *x as f64).sum::<f64>() / p.px.len() as f64;
        assert!(mean > 150.0, "the background becomes light: {mean}");
    }

    #[test]
    fn a_receipt_on_a_dark_table_is_not_inverted() {
        let g = scene(0.0);
        assert!(!light_on_dark(&g, paper_box(&g)));
    }

    #[test]
    fn rgba_size_is_checked() {
        assert!(Gray::from_rgba(&[0; 7], 1, 2).is_none());
        assert!(Gray::from_rgba(&[], 0, 0).is_none());
        assert_eq!(Gray::from_rgba(&[255, 255, 255, 255], 1, 1).unwrap().px, vec![255]);
    }

    #[test]
    fn crop_finds_the_paper() {
        let (x, y, w, h) = paper_box(&scene(0.0)).unwrap();
        assert!(x <= 80 && x + 40 >= 80, "left {x}");
        assert!(y <= 60 && y + 40 >= 60, "top {y}");
        assert!(w < 320 && h < 660, "{w}x{h}");
    }

    #[test]
    fn an_empty_scene_is_not_cropped() {
        assert_eq!(paper_box(&Gray::new(100, 100, 50)), None);
    }

    #[test]
    fn skew_is_found_with_the_correct_sign() {
        for a in [-5.0f32, -2.0, 3.0, 6.0] {
            let got = skew_of(&scene(a).crop(60, 40, 280, 620));
            assert!((got + a).abs() <= 1.5, "rotated {a}, found {got}");
        }
    }

    #[test]
    fn deskewing_restores_the_rows() {
        let tilted = scene(5.0).crop(60, 40, 280, 620);
        let fixed = tilted.rotated(skew_of(&tilted), 200);
        assert!(row_contrast(&fixed) > row_contrast(&tilted) * 2.0);
    }

    #[test]
    fn flatten_removes_uneven_light() {
        let mut g = Gray::new(200, 200, 0);
        for y in 0..200 {
            for x in 0..200 {
                let light = 120 + x / 2;
                g.px[y * 200 + x] = if y % 20 < 4 { (light / 3) as u8 } else { light as u8 };
            }
        }
        let f = flatten(&g);
        assert!(f.at(10, 10) > 200 && f.at(190, 10) > 200, "paper turns white on both sides");
        assert!(f.at(10, 1) < 90 && f.at(190, 1) < 90, "text stays dark on both sides");
    }

    #[test]
    fn faint_print_survives_as_gray() {
        let mut g = Gray::new(200, 200, 215);
        for x in 40..160 {
            for y in 90..96 {
                g.px[y * 200 + x] = 165;
            }
        }
        assert!(flatten(&g).at(100, 92) < 150);
    }

    #[test]
    fn large_photos_are_limited() {
        let out = prepare(Gray::new(3000, 4000, 128));
        assert!(out.w.max(out.h) <= MAX_SIDE);
    }

    #[test]
    fn resize_handles_tiny_results() {
        assert_eq!(Gray::new(10, 10, 9).resized(0, 0).px, vec![9]);
    }
}
