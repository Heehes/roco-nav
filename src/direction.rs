use std::path::Path;

use anyhow::{anyhow, Context, Result};
use image::{RgbImage, RgbaImage};

const ALPHA_MIN: u8 = 24;
const SEARCH_RADIUS: i32 = 4;
const COARSE_STEP_DEG: f32 = 10.0;
const MEDIUM_STEP_DEG: f32 = 2.0;
const FINE_STEP_DEG: f32 = 0.5;
const MEDIUM_WINDOW_DEG: f32 = 10.0;
const FINE_WINDOW_DEG: f32 = 2.0;
const MATCH_SCORE_MAX: f32 = 0.14;
const MATCH_PIXEL_MAX_ERR: f32 = 0.018;
const MATCH_FRACTION_MIN: f32 = 0.45;

#[derive(Debug, Clone, Copy)]
pub struct DirectionMatch {
    pub heading_rad: f32,
    pub score: f32,
}

#[derive(Debug, Clone, Copy)]
struct TemplatePoint {
    dx: f32,
    dy: f32,
    rgb: [f32; 3],
    weight: f32,
}

#[derive(Debug, Clone, Copy)]
struct CandidateMatch {
    angle_deg: f32,
    score: f32,
    match_fraction: f32,
}

pub struct DirectionDetector {
    points: Vec<TemplatePoint>,
    base_heading_rad: f32,
}

impl DirectionDetector {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_ref = path.as_ref();
        let template = image::open(path_ref)
            .with_context(|| format!("无法加载方向模板: {}", path_ref.display()))?
            .to_rgba8();
        let points = collect_template_points(&template);
        if points.len() < 32 {
            return Err(anyhow!(
                "方向模板有效像素过少: {}",
                path_ref.display()
            ));
        }

        Ok(Self {
            base_heading_rad: estimate_template_heading(&template),
            points,
        })
    }

    pub fn detect(&self, frame: &RgbImage) -> Option<DirectionMatch> {
        if self.points.is_empty() {
            return None;
        }

        let cx = (frame.width() as f32 - 1.0) * 0.5;
        let cy = (frame.height() as f32 - 1.0) * 0.5;
        let mut best: Option<CandidateMatch> = None;

        for oy in -SEARCH_RADIUS..=SEARCH_RADIUS {
            for ox in -SEARCH_RADIUS..=SEARCH_RADIUS {
                let anchor_x = cx + ox as f32;
                let anchor_y = cy + oy as f32;

                let coarse = self.search_angles(
                    frame,
                    anchor_x,
                    anchor_y,
                    -180.0,
                    180.0,
                    COARSE_STEP_DEG,
                );
                let medium = self.search_angles(
                    frame,
                    anchor_x,
                    anchor_y,
                    coarse.angle_deg - MEDIUM_WINDOW_DEG,
                    coarse.angle_deg + MEDIUM_WINDOW_DEG,
                    MEDIUM_STEP_DEG,
                );
                let fine = self.search_angles(
                    frame,
                    anchor_x,
                    anchor_y,
                    medium.angle_deg - FINE_WINDOW_DEG,
                    medium.angle_deg + FINE_WINDOW_DEG,
                    FINE_STEP_DEG,
                );

                if best.map_or(true, |curr| fine.score < curr.score) {
                    best = Some(fine);
                }
            }
        }

        let best = best?;
        if best.score > MATCH_SCORE_MAX || best.match_fraction < MATCH_FRACTION_MIN {
            return None;
        }

        Some(DirectionMatch {
            heading_rad: wrap_angle(self.base_heading_rad + best.angle_deg.to_radians()),
            score: (1.0 - best.score).clamp(0.0, 1.0),
        })
    }

    fn search_angles(
        &self,
        frame: &RgbImage,
        anchor_x: f32,
        anchor_y: f32,
        start_deg: f32,
        end_deg: f32,
        step_deg: f32,
    ) -> CandidateMatch {
        let mut angle_deg = start_deg;
        let mut best = CandidateMatch {
            angle_deg: start_deg,
            score: f32::INFINITY,
            match_fraction: 0.0,
        };

        while angle_deg <= end_deg + step_deg * 0.25 {
            let (score, match_fraction) =
                self.score_at(frame, anchor_x, anchor_y, angle_deg.to_radians());
            if score < best.score {
                best = CandidateMatch {
                    angle_deg,
                    score,
                    match_fraction,
                };
            }
            angle_deg += step_deg;
        }

        best
    }

    fn score_at(&self, frame: &RgbImage, anchor_x: f32, anchor_y: f32, angle_rad: f32) -> (f32, f32) {
        let (sin_theta, cos_theta) = angle_rad.sin_cos();
        let mut total_weight = 0.0;
        let mut total_error = 0.0;
        let mut matched_weight = 0.0;

        for point in &self.points {
            let x = anchor_x + cos_theta * point.dx - sin_theta * point.dy;
            let y = anchor_y + sin_theta * point.dx + cos_theta * point.dy;
            let sample = match sample_rgb(frame, x, y) {
                Some(rgb) => rgb,
                None => {
                    total_weight += point.weight;
                    total_error += point.weight;
                    continue;
                }
            };

            let dr = sample[0] - point.rgb[0];
            let dg = sample[1] - point.rgb[1];
            let db = sample[2] - point.rgb[2];
            let err = (dr * dr + dg * dg + db * db) / (255.0 * 255.0 * 3.0);
            total_weight += point.weight;
            total_error += point.weight * err;
            if err <= MATCH_PIXEL_MAX_ERR {
                matched_weight += point.weight;
            }
        }

        if total_weight > 0.0 {
            (total_error / total_weight, matched_weight / total_weight)
        } else {
            (f32::INFINITY, 0.0)
        }
    }
}

fn collect_template_points(template: &RgbaImage) -> Vec<TemplatePoint> {
    let cx = (template.width() as f32 - 1.0) * 0.5;
    let cy = (template.height() as f32 - 1.0) * 0.5;
    let mut points = Vec::new();

    for (x, y, px) in template.enumerate_pixels() {
        let alpha = px[3];
        if alpha < ALPHA_MIN {
            continue;
        }
        let sat = rgb_saturation(px[0], px[1], px[2]);
        let alpha_w = alpha as f32 / 255.0;
        let weight = alpha_w * (0.35 + sat * 0.65);
        points.push(TemplatePoint {
            dx: x as f32 - cx,
            dy: y as f32 - cy,
            rgb: [px[0] as f32, px[1] as f32, px[2] as f32],
            weight,
        });
    }

    points
}

fn estimate_template_heading(template: &RgbaImage) -> f32 {
    let cx = (template.width() as f32 - 1.0) * 0.5;
    let cy = (template.height() as f32 - 1.0) * 0.5;
    let mut centroid_x = 0.0;
    let mut centroid_y = 0.0;
    let mut total_weight = 0.0;
    let mut samples: Vec<(f32, f32)> = Vec::new();

    for (x, y, px) in template.enumerate_pixels() {
        let alpha = px[3];
        if alpha < 96 {
            continue;
        }
        let rx = x as f32 - cx;
        let ry = y as f32 - cy;
        let weight = alpha as f32 / 255.0;
        centroid_x += rx * weight;
        centroid_y += ry * weight;
        total_weight += weight;
        samples.push((rx, ry));
    }

    if total_weight <= 0.0 || samples.is_empty() {
        return 0.0;
    }

    centroid_x /= total_weight;
    centroid_y /= total_weight;

    let mut best = (1.0f32, 0.0f32);
    let mut best_d2 = -1.0f32;
    for (x, y) in samples {
        let dx = x - centroid_x;
        let dy = y - centroid_y;
        let d2 = dx * dx + dy * dy;
        if d2 > best_d2 {
            best = (dx, dy);
            best_d2 = d2;
        }
    }

    best.1.atan2(best.0)
}

fn rgb_saturation(r: u8, g: u8, b: u8) -> f32 {
    let max_c = r.max(g).max(b) as f32;
    let min_c = r.min(g).min(b) as f32;
    if max_c <= 1.0 {
        0.0
    } else {
        (max_c - min_c) / max_c
    }
}

fn sample_rgb(img: &RgbImage, x: f32, y: f32) -> Option<[f32; 3]> {
    if x < 0.0 || y < 0.0 {
        return None;
    }

    let max_x = img.width().saturating_sub(1) as f32;
    let max_y = img.height().saturating_sub(1) as f32;
    if x > max_x || y > max_y {
        return None;
    }

    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(img.width().saturating_sub(1));
    let y1 = (y0 + 1).min(img.height().saturating_sub(1));
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;

    let p00 = img.get_pixel(x0, y0);
    let p10 = img.get_pixel(x1, y0);
    let p01 = img.get_pixel(x0, y1);
    let p11 = img.get_pixel(x1, y1);
    let mut out = [0.0; 3];

    for i in 0..3 {
        let top = p00[i] as f32 * (1.0 - tx) + p10[i] as f32 * tx;
        let bottom = p01[i] as f32 * (1.0 - tx) + p11[i] as f32 * tx;
        out[i] = top * (1.0 - ty) + bottom * ty;
    }

    Some(out)
}

fn wrap_angle(angle_rad: f32) -> f32 {
    let mut wrapped = angle_rad;
    let two_pi = std::f32::consts::TAU;
    while wrapped <= -std::f32::consts::PI {
        wrapped += two_pi;
    }
    while wrapped > std::f32::consts::PI {
        wrapped -= two_pi;
    }
    wrapped
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn detects_rotated_template_with_small_offset() {
        let detector = DirectionDetector::load("res/jt.png").unwrap();
        let template = image::open("res/jt.png").unwrap().to_rgba8();
        let frame = render_template(&template, 1.2, 3.0, -2.0);
        let found = detector.detect(&frame).unwrap();
        let expected = wrap_angle(detector.base_heading_rad + 1.2);
        assert!(
            angle_diff(found.heading_rad, expected) < 0.08,
            "found={} expected={}",
            found.heading_rad,
            expected
        );
        assert!(found.score > 0.85, "score={}", found.score);
    }

    #[test]
    fn rejects_blank_frame() {
        let detector = DirectionDetector::load("res/jt.png").unwrap();
        let frame = RgbImage::from_pixel(96, 96, Rgb([120, 160, 120]));
        assert!(detector.detect(&frame).is_none());
    }

    fn render_template(template: &RgbaImage, angle_rad: f32, dx: f32, dy: f32) -> RgbImage {
        let mut out = RgbImage::from_pixel(96, 96, Rgb([120, 160, 120]));
        let (sin_theta, cos_theta) = angle_rad.sin_cos();
        let dst_cx = (out.width() as f32 - 1.0) * 0.5 + dx;
        let dst_cy = (out.height() as f32 - 1.0) * 0.5 + dy;
        let src_cx = (template.width() as f32 - 1.0) * 0.5;
        let src_cy = (template.height() as f32 - 1.0) * 0.5;

        for y in 0..out.height() {
            for x in 0..out.width() {
                let rx = x as f32 - dst_cx;
                let ry = y as f32 - dst_cy;
                let sx = cos_theta * rx + sin_theta * ry + src_cx;
                let sy = -sin_theta * rx + cos_theta * ry + src_cy;
                let Some(src) = sample_rgba_nearest(template, sx, sy) else {
                    continue;
                };
                let alpha = src[3] as f32 / 255.0;
                if alpha <= 0.0 {
                    continue;
                }
                let dst = out.get_pixel_mut(x, y);
                for i in 0..3 {
                    dst[i] =
                        (src[i] as f32 * alpha + dst[i] as f32 * (1.0 - alpha)).round() as u8;
                }
            }
        }

        out
    }

    fn sample_rgba_nearest(img: &RgbaImage, x: f32, y: f32) -> Option<[u8; 4]> {
        let xi = x.round() as i32;
        let yi = y.round() as i32;
        if xi < 0 || yi < 0 || xi >= img.width() as i32 || yi >= img.height() as i32 {
            return None;
        }
        Some(img.get_pixel(xi as u32, yi as u32).0)
    }

    fn angle_diff(a: f32, b: f32) -> f32 {
        wrap_angle(a - b).abs()
    }
}