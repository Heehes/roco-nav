//! 定位方案：带遮罩的归一化互相关（ZNCC 模板匹配）。
//!
//! 关键事实（经实验验证）：游戏小地图是**北朝上、与大地图同一套美术**的局部裁切，
//! 玩家图标只是叠加在正中央、随朝向旋转的小箭头。因此小地图 → 大地图只是
//! 「缩放 + 平移」，没有旋转。这种「在大图里找一块同源小图」的问题，
//! 模板匹配（ZNCC）远比跨域 SIFT 特征匹配稳定可靠。
//!
//! - 遮罩：只用小地图的圆环有效区（去掉圆形黑边与正中心的玩家箭头）。
//! - 全局定位：把大地图降采样成粗图，在若干候选尺度上滑动模板找峰值，
//!   再在全分辨率局部窗口里精修，得到精确世界坐标并锁定尺度。
//! - 跟踪：已知上一帧坐标时，只在其邻域窗口里用锁定尺度做 ZNCC（粗→精两级），
//!   极快且不跟丢；分数过低判定跟丢，交由上层自动重新全局定位。

use std::sync::Arc;

use image::{imageops, GrayImage, RgbImage};
use imageops::FilterType;

use crate::config::{LocatingConfig, MatchingConfig, TrackingConfig};

/// 一次定位结果（世界坐标 = 大地图原图像素坐标）。
#[derive(Debug, Clone, Copy)]
pub struct WorldPos {
    pub x: f32,
    pub y: f32,
    /// 匹配置信度 = ZNCC 峰值分数（-1~1，越接近 1 越好）。
    pub score: f32,
    /// 尺度（大地图像素 / 小地图像素）。
    pub scale: f32,
}

/// 侧栏调试信息。
#[derive(Debug, Clone)]
pub struct LocateDebug {
    pub mode: &'static str,
    pub score: f32,
    pub scale: f32,
    pub ok: bool,
}

impl Default for LocateDebug {
    fn default() -> Self {
        Self {
            mode: "全局定位",
            score: 0.0,
            scale: 0.0,
            ok: false,
        }
    }
}

impl LocateDebug {
    pub fn brief(&self) -> String {
        format!(
            "mode={} 结果={} 分数={:.3} 尺度={:.3}",
            self.mode,
            if self.ok { "成功" } else { "失败" },
            self.score,
            self.scale
        )
    }
}

/// 一个准备好的小地图模板（圆环遮罩内的去均值像素）。
struct Template {
    /// (x, y, 去均值后的像素值)，x/y 为模板内坐标 [0, diam)。
    pts: Vec<(u32, u32, f32)>,
    /// 去均值模板的 L2 范数。
    norm: f32,
    diam: u32,
}

/// 命中：世界坐标 + 分数。
#[derive(Clone, Copy)]
struct Hit {
    score: f32,
    x: f32,
    y: f32,
}

pub struct MapLocator {
    /// 全分辨率大地图灰度图（局部精修 / 跟踪时裁窗用）。
    map_gray: Arc<GrayImage>,
    map_w: u32,
    map_h: u32,
    /// 全局搜索用的粗图（灰度，f32 扁平存储）。
    coarse: Vec<f32>,
    coarse_w: u32,
    coarse_h: u32,
    /// 世界像素 / 粗图像素。
    coarse_fx: f32,
    coarse_fy: f32,
    matching: MatchingConfig,
    locating: LocatingConfig,
    tracking: TrackingConfig,
    /// 全局定位成功后锁定的尺度（0 表示尚未锁定，用配置猜测值）。
    locked_scale: f32,
    last_debug: LocateDebug,
}

impl MapLocator {
    pub fn new(
        big_map: &RgbImage,
        matching: &MatchingConfig,
        locating: &LocatingConfig,
        tracking: &TrackingConfig,
    ) -> Self {
        let map_gray = Arc::new(imageops::grayscale(big_map));
        let (map_w, map_h) = (map_gray.width(), map_gray.height());

        let coarse_w = locating.coarse_width.clamp(200, map_w);
        let coarse_h = ((map_h as u64 * coarse_w as u64) / map_w as u64).max(1) as u32;
        let coarse_img = imageops::resize(map_gray.as_ref(), coarse_w, coarse_h, FilterType::Triangle);
        let coarse: Vec<f32> = coarse_img.pixels().map(|p| p.0[0] as f32).collect();

        println!(
            "[定位] 大地图 {}x{}  粗图 {}x{}（全局搜索用）",
            map_w, map_h, coarse_w, coarse_h
        );

        Self {
            map_gray,
            map_w,
            map_h,
            coarse,
            coarse_w,
            coarse_h,
            coarse_fx: map_w as f32 / coarse_w as f32,
            coarse_fy: map_h as f32 / coarse_h as f32,
            matching: matching.clone(),
            locating: locating.clone(),
            tracking: tracking.clone(),
            locked_scale: 0.0,
            last_debug: LocateDebug::default(),
        }
    }

    pub fn big_map_gray(&self) -> &Arc<GrayImage> {
        &self.map_gray
    }

    pub fn last_debug(&self) -> &LocateDebug {
        &self.last_debug
    }

    /// 当前锁定/猜测的尺度（供 UI 覆盖圆与裁图显示）。
    pub fn scale_hint(&self) -> f32 {
        if self.locked_scale > 0.0 {
            self.locked_scale
        } else {
            self.matching.scale
        }
    }

    /// 手动定位：直接锁定尺度（用配置值），后续按跟踪精修。
    pub fn lock_scale_for_manual(&mut self) {
        self.locked_scale = self.matching.scale;
    }

    pub fn clear_lock(&mut self) {
        self.locked_scale = 0.0;
    }

    /// 全局定位：在整张大地图上找小地图位置（首帧 / 重定位 / 跟丢自愈时调用）。
    pub fn locate_global(&mut self, minimap: &RgbImage) -> Option<WorldPos> {
        let mm = imageops::grayscale(minimap);
        let (s_lo, s_hi) = self.scale_range();
        let steps = self.locating.scale_steps.max(1);

        // 1) 粗图上逐尺度滑动找峰值。
        let mut best: Option<(f32, f32, f32, f32)> = None; // (score, scale, wx, wy)
        for i in 0..steps {
            let s = if steps == 1 {
                self.matching.scale
            } else {
                s_lo + (s_hi - s_lo) * i as f32 / (steps - 1) as f32
            };
            let diam = (mm.width() as f32 * s / self.coarse_fx).round().max(8.0) as u32;
            if diam >= self.coarse_w || diam >= self.coarse_h {
                continue;
            }
            let tmpl = self.build_template(&mm, diam);
            if let Some((score, bx, by)) =
                best_in_image(&self.coarse, self.coarse_w, self.coarse_h, &tmpl)
            {
                if best.map_or(true, |b| score > b.0) {
                    let wx = (bx as f32 + (diam as f32 - 1.0) * 0.5) * self.coarse_fx;
                    let wy = (by as f32 + (diam as f32 - 1.0) * 0.5) * self.coarse_fy;
                    best = Some((score, s, wx, wy));
                }
            }
        }

        let (cscore, cscale, cwx, cwy) = best?;

        // 2) 全分辨率局部精修（同时微调尺度），得到精确坐标。
        let (score, scale, wx, wy) = self
            .refine(&mm, cscale, cwx, cwy)
            .unwrap_or((cscore, cscale, cwx, cwy));

        self.last_debug = LocateDebug {
            mode: "全局定位",
            score,
            scale,
            ok: score >= self.matching.min_score,
        };

        if score < self.matching.min_score {
            return None;
        }

        self.locked_scale = scale;
        Some(WorldPos { x: wx, y: wy, score, scale })
    }

    /// 跟踪：已知上一帧坐标，在邻域内用锁定尺度做 ZNCC（粗→精两级）。
    pub fn track(&mut self, minimap: &RgbImage, last: (f32, f32)) -> Option<WorldPos> {
        let mm = imageops::grayscale(minimap);
        let scale = self.scale_hint();

        // 粗：大窗口、小模板，快速锁定大致位置。
        let coarse = self.ncc_search(
            &mm,
            scale,
            last.0,
            last.1,
            self.tracking.search_radius,
            self.tracking.coarse_diam,
        );
        let coarse = match coarse {
            Some(h) => h,
            None => {
                self.last_debug = LocateDebug { mode: "跟踪", score: 0.0, scale, ok: false };
                return None;
            }
        };

        // 精：小窗口、大模板，精修到亚像素。窗口取粗级量化误差的两倍。
        let fine_win = (mm.width() as f32 * scale / self.tracking.coarse_diam as f32) * 2.0;
        let fine = self
            .ncc_search(&mm, scale, coarse.x, coarse.y, fine_win.max(4.0), self.tracking.fine_diam)
            .unwrap_or(coarse);

        self.last_debug = LocateDebug {
            mode: "跟踪",
            score: fine.score,
            scale,
            ok: fine.score >= self.matching.min_score,
        };

        if fine.score < self.matching.min_score {
            return None;
        }
        Some(WorldPos { x: fine.x, y: fine.y, score: fine.score, scale })
    }

    // ── 内部实现 ───────────────────────────────────────────────

    fn scale_range(&self) -> (f32, f32) {
        let s = self.matching.scale.max(0.1);
        let t = self.matching.scale_search.clamp(0.0, 0.9);
        if t <= 0.0 {
            (s, s)
        } else {
            (s * (1.0 - t), s * (1.0 + t))
        }
    }

    /// 把小地图重采样到 `diam` 像素并建立圆环遮罩模板。
    fn build_template(&self, mm: &GrayImage, diam: u32) -> Template {
        let t = imageops::resize(mm, diam, diam, FilterType::Triangle);
        let c = (diam as f32 - 1.0) * 0.5;
        let r_out = (diam as f32 * 0.5) * self.matching.inner_ratio;
        let r_in = (diam as f32 * 0.5) * self.matching.center_exclude_ratio;
        let (r_out2, r_in2) = (r_out * r_out, r_in * r_in);

        let mut pts: Vec<(u32, u32, f32)> = Vec::new();
        let mut sum = 0.0f32;
        for (x, y, p) in t.enumerate_pixels() {
            let dx = x as f32 - c;
            let dy = y as f32 - c;
            let d2 = dx * dx + dy * dy;
            if d2 < r_in2 || d2 > r_out2 {
                continue;
            }
            pts.push((x, y, p.0[0] as f32));
            sum += p.0[0] as f32;
        }
        let mean = sum / pts.len().max(1) as f32;
        let mut norm = 0.0f32;
        for p in pts.iter_mut() {
            p.2 -= mean;
            norm += p.2 * p.2;
        }
        Template { pts, norm: norm.sqrt().max(1e-6), diam }
    }

    /// 全局粗峰值附近，在全分辨率上局部精修（顺带在邻近尺度里挑最优）。
    fn refine(&self, mm: &GrayImage, scale: f32, cx: f32, cy: f32) -> Option<(f32, f32, f32, f32)> {
        let search = (self.coarse_fx * 2.5).max(12.0);
        let diam = self.locating.refine_diam;
        let mut best: Option<(f32, f32, f32, f32)> = None;
        for k in [-0.03f32, 0.0, 0.03] {
            let s = scale * (1.0 + k);
            if let Some(h) = self.ncc_search(mm, s, cx, cy, search, diam) {
                if best.map_or(true, |b| h.score > b.0) {
                    best = Some((h.score, s, h.x, h.y));
                }
            }
        }
        best
    }

    /// 在以世界坐标 (cx,cy) 为中心、±search_world 的窗口里，用直径 match_diam 的
    /// 模板（世界尺度 scale）做带遮罩 ZNCC 搜索，返回亚像素世界坐标命中。
    fn ncc_search(
        &self,
        mm: &GrayImage,
        scale: f32,
        cx: f32,
        cy: f32,
        search_world: f32,
        match_diam: u32,
    ) -> Option<Hit> {
        let w_diam = mm.width() as f32 * scale; // 模板对应的世界像素直径
        let half = w_diam * 0.5;

        let rx0 = (cx - search_world - half).floor().max(0.0);
        let ry0 = (cy - search_world - half).floor().max(0.0);
        let rx1 = (cx + search_world + half).ceil().min(self.map_w as f32);
        let ry1 = (cy + search_world + half).ceil().min(self.map_h as f32);
        let crop_w = (rx1 - rx0) as u32;
        let crop_h = (ry1 - ry0) as u32;
        if (crop_w as f32) < w_diam || (crop_h as f32) < w_diam {
            return None;
        }

        let f = w_diam / match_diam as f32; // 世界像素 / 匹配像素
        let tw = ((crop_w as f32) / f).round() as u32;
        let th = ((crop_h as f32) / f).round() as u32;
        if tw <= match_diam || th <= match_diam {
            return None;
        }

        let crop = imageops::crop_imm(self.map_gray.as_ref(), rx0 as u32, ry0 as u32, crop_w, crop_h)
            .to_image();
        let target_img = imageops::resize(&crop, tw, th, FilterType::Triangle);
        let target: Vec<f32> = target_img.pixels().map(|p| p.0[0] as f32).collect();

        let tmpl = self.build_template(mm, match_diam);
        let (score, bx, by) = best_in_image(&target, tw, th, &tmpl)?;

        // 亚像素：对峰值邻域分数做抛物线插值。
        let d = match_diam as usize;
        let (tw_u, th_u) = (tw as usize, th as usize);
        let s0 = score;
        let sxm = if bx > 0 { zncc_at(&target, tw_u, &tmpl, bx - 1, by) } else { s0 };
        let sxp = if (bx as usize) + 1 + d <= tw_u { zncc_at(&target, tw_u, &tmpl, bx + 1, by) } else { s0 };
        let sym = if by > 0 { zncc_at(&target, tw_u, &tmpl, bx, by - 1) } else { s0 };
        let syp = if (by as usize) + 1 + d <= th_u { zncc_at(&target, tw_u, &tmpl, bx, by + 1) } else { s0 };
        let dx = parabolic(sxm, s0, sxp);
        let dy = parabolic(sym, s0, syp);

        let fx = crop_w as f32 / tw as f32;
        let fy = crop_h as f32 / th as f32;
        let tcx = bx as f32 + dx + (match_diam as f32 - 1.0) * 0.5;
        let tcy = by as f32 + dy + (match_diam as f32 - 1.0) * 0.5;
        Some(Hit {
            score,
            x: rx0 + tcx * fx,
            y: ry0 + tcy * fy,
        })
    }
}

/// 在 target 图里滑动模板找 ZNCC 最大值，返回 (分数, 左上角 x, 左上角 y)。
fn best_in_image(target: &[f32], tw: u32, th: u32, tmpl: &Template) -> Option<(f32, u32, u32)> {
    let d = tmpl.diam;
    if tw < d || th < d || tmpl.pts.len() < 8 {
        return None;
    }
    let (tw_u, th_u) = (tw as usize, th as usize);
    let d_u = d as usize;
    let mut best = (f32::MIN, 0u32, 0u32);
    for y in 0..=(th_u - d_u) {
        for x in 0..=(tw_u - d_u) {
            let s = zncc_at(target, tw_u, tmpl, x as u32, y as u32);
            if s > best.0 {
                best = (s, x as u32, y as u32);
            }
        }
    }
    if best.0 == f32::MIN {
        None
    } else {
        Some(best)
    }
}

/// 模板左上角置于 (x,y) 时的带遮罩 ZNCC 分数。
#[inline]
fn zncc_at(target: &[f32], tw: usize, tmpl: &Template, x: u32, y: u32) -> f32 {
    let n = tmpl.pts.len() as f32;
    let (mut si, mut sii, mut num) = (0.0f32, 0.0f32, 0.0f32);
    let base = y as usize * tw + x as usize;
    for &(dx, dy, tc) in &tmpl.pts {
        let iv = target[base + dy as usize * tw + dx as usize];
        si += iv;
        sii += iv * iv;
        num += tc * iv;
    }
    let var = sii - si * si / n;
    if var <= 1e-3 {
        return f32::MIN;
    }
    num / (tmpl.norm * var.sqrt())
}

/// 三点抛物线插值，返回峰值相对中心的亚像素偏移（钳制到 ±1）。
#[inline]
fn parabolic(sm: f32, s0: f32, sp: f32) -> f32 {
    let denom = sm - 2.0 * s0 + sp;
    if denom.abs() < 1e-6 {
        0.0
    } else {
        (0.5 * (sm - sp) / denom).clamp(-1.0, 1.0)
    }
}
