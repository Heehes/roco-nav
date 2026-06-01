use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// 路线中的一个点位（大地图原图像素坐标）。
#[derive(Debug, Clone, Copy)]
pub struct RoutePoint {
    pub x: f32,
    pub y: f32,
    pub label: Option<&'static str>,
}

/// 一条由多个点位组成的路线（对应一个 JSON 文件）。
#[derive(Debug, Clone)]
pub struct Route {
    pub name: String,
    pub points: Vec<RoutePoint>,
    /// 是否为有序路线（按顺序连线）vs 散点集合
    pub is_path: bool,
    /// 文件中声明的默认偏移/缩放（可被 UI 覆盖）
    pub default_offset: [f32; 2],
    pub default_scale: [f32; 2],
}

/// 外部路线文件（兼容多种格式）。
#[derive(Debug, Deserialize)]
struct ExternalRouteFile {
    name: Option<String>,
    #[serde(default)]
    offset_x: f32,
    #[serde(default)]
    offset_y: f32,
    #[serde(default = "default_scale")]
    scale_x: f32,
    #[serde(default = "default_scale")]
    scale_y: f32,
    /// 有序路径点（路线格式）
    #[serde(default)]
    points: Vec<ExternalPoint>,
    /// 散点集合（地点类格式）
    #[serde(default)]
    items: Vec<ExternalPoint>,
}

fn default_scale() -> f32 {
    1.0
}

#[derive(Debug, Deserialize)]
struct ExternalPoint {
    x: f32,
    y: f32,
    #[serde(default)]
    label: Option<String>,
}

/// 从 `res/routes/` 目录加载所有 JSON 路线文件。
pub fn load_dir(dir: impl AsRef<Path>) -> Result<Vec<Route>> {
    let dir = dir.as_ref();
    if !dir.is_dir() {
        anyhow::bail!("路线目录不存在: {}", dir.display());
    }

    let mut routes = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("无法读取路线目录: {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "json"))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        match load_single_file(&path) {
            Ok(route) => routes.push(route),
            Err(e) => eprintln!("跳过路线文件 {}: {e}", path.display()),
        }
    }

    Ok(routes)
}

fn load_single_file(path: &Path) -> Result<Route> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取: {}", path.display()))?;
    let file: ExternalRouteFile =
        serde_json::from_str(&text).with_context(|| format!("JSON 解析失败: {}", path.display()))?;

    let name = file.name.unwrap_or_else(|| {
        path.file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });

    // 优先使用 points（有序路线），否则使用 items（散点）
    let (raw_points, is_path) = if !file.points.is_empty() {
        (file.points, true)
    } else {
        (file.items, false)
    };

    let points: Vec<RoutePoint> = raw_points
        .into_iter()
        .map(|item| {
            let label_str = item.label.map(|s| -> &'static str { Box::leak(s.into_boxed_str()) });
            RoutePoint {
                x: item.x,
                y: item.y,
                label: label_str,
            }
        })
        .collect();

    Ok(Route {
        name,
        points,
        is_path,
        default_offset: [file.offset_x, file.offset_y],
        default_scale: [file.scale_x, file.scale_y],
    })
}
