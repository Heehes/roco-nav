//! 一次性的圆形小地图检测（开机时执行一次，性能不敏感）。
//!
//! 思路：截取整张客户区 -> 灰度降采样 -> Sobel 边缘 -> 在右上角区域内做
//! Hough 圆投票（沿梯度方向累加圆心）-> 取峰值得到圆心与半径。
//! 半径范围按客户区宽度比例自适应，无需任何手工配置。
//! 若检测置信度过低，则回退到右上角的几何默认圆。

use image::{imageops, RgbImage};
use imageproc::gradients::{horizontal_sobel, vertical_sobel};

use crate::capture::MinimapCircle;

// 小地图在画面中的经验比例（相对客户区宽度）。
const R_FRAC_MIN: f32 = 0.028;
const R_FRAC_MAX: f32 = 0.060;
const R_FRAC_DEFAULT: f32 = 0.043;
// 圆心搜索限制在右上角区域。
const ROI_X_FRAC: f32 = 0.60; // x >= 0.60 * 宽
const ROI_Y_FRAC: f32 = 0.50; // y <= 0.50 * 高

/// 检测客户区中的圆形小地图，失败回退到右上角几何默认圆。
pub fn detect(client: &RgbImage) -> MinimapCircle {
    match try_hough(client) {
        Some(c) => {
            println!("[圆检测] 成功: 圆心=({}, {}) 半径={}", c.cx, c.cy, c.r);
            c
        }
        None => {
            let c = fallback(client.width() as i32, client.height() as i32);
            println!(
                "[圆检测] 检测置信度低，回退到右上角默认圆: 圆心=({}, {}) 半径={}",
                c.cx, c.cy, c.r
            );
            c
        }
    }
}

/// 右上角几何默认圆（按比例推导）。
fn fallback(cw: i32, ch: i32) -> MinimapCircle {
    let r = (R_FRAC_DEFAULT * cw as f32).round() as i32;
    let right_margin = (0.014 * cw as f32).round() as i32;
    let top_margin = (0.043 * ch as f32).round() as i32;
    MinimapCircle {
        cx: cw - right_margin - r,
        cy: top_margin + r,
        r,
    }
}

fn try_hough(client: &RgbImage) -> Option<MinimapCircle> {
    let cw = client.width() as f32;
    let ch = client.height() as f32;
    if cw < 64.0 || ch < 64.0 {
        return None;
    }

    // 降采样到约 800 宽以提速。
    let target_w = 800.0;
    let d = if cw > target_w { target_w / cw } else { 1.0 };
    let sw = (cw * d).round().max(8.0) as u32;
    let sh = (ch * d).round().max(8.0) as u32;
    let small = imageops::resize(client, sw, sh, imageops::FilterType::Triangle);
    let gray = imageops::grayscale(&small);
    let gx = horizontal_sobel(&gray);
    let gy = vertical_sobel(&gray);

    // 半径范围（降采样坐标）。
    let r_min = (R_FRAC_MIN * cw * d).max(4.0);
    let r_max = (R_FRAC_MAX * cw * d).max(r_min + 1.0);

    // 圆心搜索窗口：右上角区域。
    let wx0 = (ROI_X_FRAC * sw as f32).floor().max(0.0) as i32;
    let wy0 = 0i32;
    let wx1 = (sw as f32 - 1.0) as i32;
    let wy1 = (ROI_Y_FRAC * sh as f32).ceil().min(sh as f32 - 1.0) as i32;
    if wx1 <= wx0 || wy1 <= wy0 {
        return None;
    }
    let ww = (wx1 - wx0 + 1) as usize;
    let wh = (wy1 - wy0 + 1) as usize;
    let mut acc = vec![0u32; ww * wh];

    let mag_thr = 80.0f32;
    let r_steps = ((r_max - r_min).ceil() as i32).max(1);

    for y in 0..sh as i32 {
        for x in 0..sw as i32 {
            let gxv = gx.get_pixel(x as u32, y as u32).0[0] as f32;
            let gyv = gy.get_pixel(x as u32, y as u32).0[0] as f32;
            let mag = (gxv * gxv + gyv * gyv).sqrt();
            if mag < mag_thr {
                continue;
            }
            let nx = gxv / mag;
            let ny = gyv / mag;
            for step in 0..=r_steps {
                let r = r_min + step as f32;
                if r > r_max {
                    break;
                }
                for sign in [1.0f32, -1.0] {
                    let cx = x as f32 + sign * r * nx;
                    let cy = y as f32 + sign * r * ny;
                    let ix = cx.round() as i32;
                    let iy = cy.round() as i32;
                    if ix >= wx0 && ix <= wx1 && iy >= wy0 && iy <= wy1 {
                        let ai = (iy - wy0) as usize * ww + (ix - wx0) as usize;
                        acc[ai] += 1;
                    }
                }
            }
        }
    }

    // 找累加器峰值。
    let mut best_i = 0usize;
    let mut best_v = 0u32;
    for (i, &v) in acc.iter().enumerate() {
        if v > best_v {
            best_v = v;
            best_i = i;
        }
    }
    if best_v == 0 {
        return None;
    }
    let bcx = (best_i % ww) as i32 + wx0;
    let bcy = (best_i / ww) as i32 + wy0;

    // 估计半径：对该圆心做径向边缘的距离直方图。
    let nbins = r_steps as usize + 1;
    let mut rhist = vec![0u32; nbins];
    for y in 0..sh as i32 {
        for x in 0..sw as i32 {
            let gxv = gx.get_pixel(x as u32, y as u32).0[0] as f32;
            let gyv = gy.get_pixel(x as u32, y as u32).0[0] as f32;
            let mag = (gxv * gxv + gyv * gyv).sqrt();
            if mag < mag_thr {
                continue;
            }
            let dx = (x - bcx) as f32;
            let dy = (y - bcy) as f32;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist < r_min || dist > r_max {
                continue;
            }
            let radial = (dx * gxv + dy * gyv).abs() / (dist * mag + 1e-6);
            if radial < 0.7 {
                continue;
            }
            let bin = (dist - r_min).round() as usize;
            if bin < nbins {
                rhist[bin] += 1;
            }
        }
    }
    let mut best_bin = 0usize;
    let mut best_bin_v = 0u32;
    for (i, &v) in rhist.iter().enumerate() {
        if v > best_bin_v {
            best_bin_v = v;
            best_bin = i;
        }
    }
    let r_small = r_min + best_bin as f32;

    // 置信度：峰值投票相对于理论圆周长。
    let circumference = 2.0 * std::f32::consts::PI * r_small;
    let conf = best_v as f32 / circumference.max(1.0);
    if conf < 0.12 || best_bin_v == 0 {
        return None;
    }

    // 映射回全尺寸客户区坐标。
    let inv = 1.0 / d;
    Some(MinimapCircle {
        cx: (bcx as f32 * inv).round() as i32,
        cy: (bcy as f32 * inv).round() as i32,
        r: (r_small * inv).round() as i32,
    })
}
