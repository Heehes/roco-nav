use std::sync::Arc;
use std::thread::{self, sleep, JoinHandle};
use std::time::{Duration, Instant};

use image::{imageops, GrayImage, RgbImage};

use crate::capture::{MinimapCircle, WindowCapture};
use crate::config::Config;
use crate::direction::DirectionDetector;
use crate::locator::MapLocator;
use crate::state::{MinimapDebug, PlayerPos, SharedState};

/// 启动后台工作线程：定位窗口 -> 持续截取小地图 -> ZNCC 匹配 -> 写入共享状态。
pub fn spawn(cfg: Config, big_map: Arc<RgbImage>, shared: SharedState) -> JoinHandle<()> {
    thread::spawn(move || run_loop(cfg, big_map, shared))
}

fn run_loop(cfg: Config, big_map: Arc<RgbImage>, shared: SharedState) {
    println!("==== 当前可见窗口列表（用于核对窗口标题）====");
    for (_, title) in crate::capture::list_windows() {
        println!("  - {title}");
    }
    println!("=============================================");

    set_status(&shared, "正在构建定位用大地图...");
    let mut locator = MapLocator::new(big_map.as_ref(), &cfg.matching, &cfg.locating, &cfg.tracking);

    let direction_detector = match DirectionDetector::load("res/jt.png") {
        Ok(detector) => {
            println!("[方向] 已加载方向模板: res/jt.png");
            Some(detector)
        }
        Err(e) => {
            eprintln!("[方向] 模板加载失败，已跳过方向识别: {e}");
            None
        }
    };

    let capture = loop {
        match WindowCapture::find(&cfg.window.title) {
            Ok(c) => break c,
            Err(e) => {
                set_status(&shared, &format!("{e}"));
                sleep(Duration::from_secs(1));
            }
        }
    };
    set_status(&shared, &format!("已定位窗口: {}", cfg.window.title));

    let circle: MinimapCircle = loop {
        match capture.capture_client() {
            Ok(client) => {
                let c = crate::circle::detect(&client);
                println!(
                    "[圆检测] 小地图圆：客户区={}x{}  圆心=({},{})  半径={}  直径={}px",
                    client.width(),
                    client.height(),
                    c.cx,
                    c.cy,
                    c.r,
                    c.r * 2
                );
                break c;
            }
            Err(e) => {
                eprintln!("[圆检测] 截取客户区失败({e})，1s 后重试");
                sleep(Duration::from_secs(1));
            }
        }
    };

    let alpha = cfg.matching.ema_alpha.clamp(0.05, 1.0);
    let locate_interval = Duration::from_millis(cfg.locating.interval_ms);
    let track_interval = Duration::from_millis(cfg.tracking.interval_ms);
    let lost_patience = cfg.tracking.lost_patience.max(1);

    let mut last: Option<(f32, f32)> = None;
    let mut last_scale = locator.scale_hint();
    let mut last_score = 0.0f32;
    let mut lost_count = 0u32;
    let mut last_print = Instant::now();

    loop {
        if !capture.is_alive() {
            set_status(&shared, "窗口已关闭");
            break;
        }

        // 读取 UI 控制标志。
        let tracking_enabled = {
            let mut s = shared.lock().unwrap();
            if s.relocalize {
                s.relocalize = false;
                s.manual_pos = None;
                last = None;
                lost_count = 0;
                locator.clear_lock();
                s.status = "正在全局定位...".into();
                s.tracking_enabled = true;
            } else if let Some((mx, my)) = s.manual_pos.take() {
                last = Some((mx, my));
                lost_count = 0;
                locator.lock_scale_for_manual();
                last_scale = locator.scale_hint();
                s.status = "手动定位完成，开始跟踪...".into();
                s.tracking_enabled = true;
            }
            s.tracking_enabled
        };

        if !tracking_enabled {
            last = None;
            lost_count = 0;
            let mut s = shared.lock().unwrap();
            s.player = None;
            s.status = "点击「开始跟踪」开始定位".into();
            drop(s);
            sleep(locate_interval);
            continue;
        }

        let game_origin = capture.client_origin().unwrap_or((0, 0));

        let (frame, region) = match capture.capture_circle(&circle) {
            Ok(v) => v,
            Err(e) => {
                set_status(&shared, &format!("截图失败: {e}"));
                sleep(track_interval);
                continue;
            }
        };
        if cfg.debug.save_minimap {
            let _ = frame.save(&cfg.debug.save_path);
        }

        let heading_rad = direction_detector
            .as_ref()
            .and_then(|d| d.detect(&frame))
            .map(|hit| hit.heading_rad);

        // 全局定位 or 跟踪。
        let prev = last;
        let tracking = prev.is_some();
        let result = match prev {
            None => locator.locate_global(&frame),
            Some(p) => locator.track(&frame, p),
        };

        match result {
            Some(p) => {
                let smoothed = match prev {
                    Some((lx, ly)) => (alpha * p.x + (1.0 - alpha) * lx, alpha * p.y + (1.0 - alpha) * ly),
                    None => (p.x, p.y),
                };
                last = Some(smoothed);
                last_scale = p.scale;
                last_score = p.score;
                lost_count = 0;
            }
            None => {
                if tracking {
                    lost_count += 1;
                    if lost_count >= lost_patience {
                        last = None; // 连续跟丢 -> 下一帧转入全局重定位
                        lost_count = 0;
                    }
                }
            }
        }

        // 节流打印诊断。
        if last_print.elapsed() >= Duration::from_millis(500) {
            println!("[定位] {}", locator.last_debug().brief());
            last_print = Instant::now();
        }

        // 当前已知位置处裁一块大地图用于调试显示。
        let tracking_crop = last.map(|(x, y)| {
            crop_display(
                locator.big_map_gray(),
                x,
                y,
                frame.width() as f32 * 0.5 * last_scale.max(0.1),
            )
        });

        let mut s = shared.lock().unwrap();
        if let Some(crop) = tracking_crop {
            s.tracking_patch = Some(crop);
            s.tracking_patch_seq = s.tracking_patch_seq.wrapping_add(1);
        }
        s.minimap = Some(frame);
        s.minimap_seq = s.minimap_seq.wrapping_add(1);
        s.heading_rad = heading_rad;
        s.game_rect = Some([game_origin.0, game_origin.1, region.client_w, region.client_h]);
        s.debug = Some(MinimapDebug {
            client_w: region.client_w,
            client_h: region.client_h,
            region_x: region.x,
            region_y: region.y,
            region_size: region.size,
        });
        s.locate_debug = locator.last_debug().brief();

        match last {
            Some((x, y)) => {
                s.player = Some(PlayerPos { x, y, score: last_score, scale: last_scale });
                s.status = if result.is_some() {
                    format!("跟踪中  分数 {:.3}", last_score)
                } else {
                    "跟踪不稳，正在重试...".into()
                };
            }
            None => {
                s.player = None;
                s.status = "全局定位中...".into();
            }
        }
        drop(s);

        // 全局模式按较长间隔轮询；跟踪模式按较短间隔（ZNCC 很快）。
        sleep(if last.is_none() { locate_interval } else { track_interval });
    }
}

/// 以世界坐标 (cx,cy) 为中心、半径 r（世界像素）裁剪大地图灰度图，用于调试显示。
fn crop_display(gray: &GrayImage, cx: f32, cy: f32, r: f32) -> RgbImage {
    let r = r.max(8.0) as u32;
    let (map_w, map_h) = (gray.width(), gray.height());
    let x0 = (cx as i32 - r as i32).max(0) as u32;
    let y0 = (cy as i32 - r as i32).max(0) as u32;
    let x1 = (cx as u32 + r).min(map_w);
    let y1 = (cy as u32 + r).min(map_h);
    if x1 <= x0 || y1 <= y0 {
        return RgbImage::new(1, 1);
    }
    let patch = imageops::crop_imm(gray, x0, y0, x1 - x0, y1 - y0).to_image();
    image::DynamicImage::ImageLuma8(patch).to_rgb8()
}

fn set_status(shared: &SharedState, msg: &str) {
    if let Ok(mut s) = shared.lock() {
        s.status = msg.to_string();
    }
}
