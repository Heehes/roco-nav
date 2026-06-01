mod app;
mod capture;
mod circle;
mod config;
mod direction;
mod locator;
mod overlay;
mod resource;
mod route;
mod state;
mod ui;

use std::env;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use config::Config;
use locator::MapLocator;

enum LaunchMode {
    Run,
    SelfTest(String),
    Help,
}

fn usage_text() -> &'static str {
    "用法:\n  roco-nav                 启动导航叠加窗口\n  roco-nav selftest [图片]  用一张小地图截图离线验证定位并保存匹配处裁切（默认 debug_minimap.png）\n"
}

fn parse_launch_mode() -> Result<LaunchMode> {
    let mut args = env::args().skip(1);
    let Some(cmd) = args.next() else {
        return Ok(LaunchMode::Run);
    };
    match cmd.as_str() {
        "selftest" => Ok(LaunchMode::SelfTest(
            args.next().unwrap_or_else(|| "debug_minimap.png".into()),
        )),
        "run" => Ok(LaunchMode::Run),
        "-h" | "--help" | "help" => Ok(LaunchMode::Help),
        _ => Err(anyhow!("未知命令: {cmd}\n\n{}", usage_text())),
    }
}

fn main() -> Result<()> {
    let mode = parse_launch_mode()?;
    if let LaunchMode::Help = mode {
        print!("{}", usage_text());
        return Ok(());
    }

    let cfg = Config::load("config.toml")?;

    if let LaunchMode::SelfTest(path) = mode {
        return run_selftest(&cfg, &path);
    }

    let big_map = Arc::new(
        image::open(&cfg.capture.big_map_path)
            .with_context(|| format!("无法加载大地图: {}", cfg.capture.big_map_path))?
            .to_rgb8(),
    );
    println!("大地图已加载: {}x{}", big_map.width(), big_map.height());

    let routes = match route::load_dir(&cfg.route.dir) {
        Ok(r) => {
            println!("已加载 {} 条路线文件", r.len());
            for route in &r {
                println!("  - {} ({} 个点位)", route.name, route.points.len());
            }
            r
        }
        Err(e) => {
            eprintln!("路线加载失败: {e}");
            Vec::new()
        }
    };

    let resources = match resource::load(&cfg.resource.file) {
        Ok(r) => {
            println!("已加载 {} 个物资点位", r.len());
            r
        }
        Err(e) => {
            eprintln!("物资加载失败: {e}");
            Vec::new()
        }
    };

    let shared = state::new_shared();
    app::spawn(cfg.clone(), big_map.clone(), shared.clone());

    ui::run(cfg, big_map, routes, resources, shared)
}

/// 离线自检：用一张小地图截图跑全局定位，打印结果并保存匹配处大地图裁切，
/// 方便肉眼核对定位是否准确、以及校准 [matching].scale。
fn run_selftest(cfg: &Config, minimap_path: &str) -> Result<()> {
    let big_map = image::open(&cfg.capture.big_map_path)
        .with_context(|| format!("无法加载大地图: {}", cfg.capture.big_map_path))?
        .to_rgb8();
    let minimap = image::open(minimap_path)
        .with_context(|| format!("无法加载小地图截图: {minimap_path}"))?
        .to_rgb8();
    println!(
        "大地图 {}x{}  小地图 {}x{}",
        big_map.width(),
        big_map.height(),
        minimap.width(),
        minimap.height()
    );

    let mut locator =
        MapLocator::new(&big_map, &cfg.matching, &cfg.locating, &cfg.tracking);

    let started = std::time::Instant::now();
    let pos = locator.locate_global(&minimap);
    let elapsed = started.elapsed();

    match pos {
        Some(p) => {
            println!(
                "[自检] 定位成功  坐标=({:.1}, {:.1})  分数={:.4}  尺度={:.3}  用时={:?}",
                p.x, p.y, p.score, p.scale, elapsed
            );
            // 保存匹配处大地图裁切，与小地图并排核对。
            let half = (minimap.width() as f32 * p.scale * 0.5) as i32;
            let x0 = (p.x as i32 - half).clamp(0, big_map.width() as i32 - 1) as u32;
            let y0 = (p.y as i32 - half).clamp(0, big_map.height() as i32 - 1) as u32;
            let cw = ((half as u32 * 2).min(big_map.width() - x0)).max(1);
            let ch = ((half as u32 * 2).min(big_map.height() - y0)).max(1);
            let crop = image::imageops::crop_imm(&big_map, x0, y0, cw, ch).to_image();
            let crop = image::imageops::resize(&crop, 144, 144, image::imageops::FilterType::Triangle);
            crop.save("selftest_mapcrop.png")?;
            minimap.save("selftest_minimap.png")?;
            println!("[自检] 已保存 selftest_minimap.png 与 selftest_mapcrop.png，请肉眼核对二者是否一致");
        }
        None => {
            println!(
                "[自检] 定位失败（分数低于 min_score={:.2}）。诊断: {}  用时={:?}",
                cfg.matching.min_score,
                locator.last_debug().brief(),
                elapsed
            );
        }
    }
    Ok(())
}
