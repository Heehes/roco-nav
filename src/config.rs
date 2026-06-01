use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub window: WindowConfig,
    pub matching: MatchingConfig,
    #[serde(default)]
    pub locating: LocatingConfig,
    #[serde(default)]
    pub tracking: TrackingConfig,
    pub capture: CaptureConfig,
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub route: RouteConfig,
    #[serde(default)]
    pub nav: NavConfig,
    #[serde(default)]
    pub resource: ResourceConfig,
    #[serde(default)]
    pub debug: DebugConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WindowConfig {
    pub title: String,
}

/// ZNCC 模板匹配定位参数（全局/跟踪共用的基础参数）。
#[derive(Debug, Clone, Deserialize)]
pub struct MatchingConfig {
    /// 小地图有效外圆半径比例（<=1，去掉圆形黑边）。
    #[serde(default = "d_inner_ratio")]
    pub inner_ratio: f32,
    /// 小地图中心排除半径比例（去掉正中央随朝向旋转的玩家箭头）。
    #[serde(default = "d_center_exclude_ratio")]
    pub center_exclude_ratio: f32,
    /// 尺度猜测（大地图像素 / 小地图像素）。全局定位以此为中心搜索并最终锁定。
    #[serde(default = "d_scale")]
    pub scale: f32,
    /// 全局定位时围绕 scale 的相对搜索范围（±比例，0 表示固定尺度不搜索）。
    #[serde(default = "d_scale_search")]
    pub scale_search: f32,
    /// 接受阈值：ZNCC 分数低于此值判定为「没匹配上 / 跟丢」。
    #[serde(default = "d_min_score")]
    pub min_score: f32,
    /// 输出坐标 EMA 平滑系数（0~1）：新值权重，越小越平滑、越跟手则调大。
    #[serde(default = "d_ema_alpha")]
    pub ema_alpha: f32,
}

/// 全局定位专属参数（首次 / 重定位 / 跟丢自愈时使用）。
#[derive(Debug, Clone, Deserialize)]
pub struct LocatingConfig {
    /// 全局搜索用粗图的宽度（像素）。越大越准但越慢。
    #[serde(default = "d_coarse_width")]
    pub coarse_width: u32,
    /// 全局搜索测试的候选尺度数量（在 scale 搜索范围内均匀取样）。
    #[serde(default = "d_scale_steps")]
    pub scale_steps: u32,
    /// 全分辨率精修时的匹配模板直径（像素），越大越精细越慢。
    #[serde(default = "d_refine_diam")]
    pub refine_diam: u32,
    /// 未锁定位置时的全局定位轮询间隔（毫秒）。
    #[serde(default = "d_locate_interval_ms")]
    pub interval_ms: u64,
}

impl Default for LocatingConfig {
    fn default() -> Self {
        Self {
            coarse_width: d_coarse_width(),
            scale_steps: d_scale_steps(),
            refine_diam: d_refine_diam(),
            interval_ms: d_locate_interval_ms(),
        }
    }
}

/// 跟踪模式专属参数（已有位置时使用）。
#[derive(Debug, Clone, Deserialize)]
pub struct TrackingConfig {
    /// 围绕上一帧坐标的搜索半径（世界像素）：相邻两帧位移上限。
    #[serde(default = "d_search_radius")]
    pub search_radius: f32,
    /// 粗级匹配模板直径（像素）：大窗口快速锁定，越小越快。
    #[serde(default = "d_track_coarse_diam")]
    pub coarse_diam: u32,
    /// 精级匹配模板直径（像素）：小窗口精修到亚像素，越大越精细。
    #[serde(default = "d_track_fine_diam")]
    pub fine_diam: u32,
    /// 跟踪轮询间隔（毫秒）。
    #[serde(default = "d_track_interval_ms")]
    pub interval_ms: u64,
    /// 连续多少帧分数过低后判定跟丢、自动转入全局重定位。
    #[serde(default = "d_lost_patience")]
    pub lost_patience: u32,
}

impl Default for TrackingConfig {
    fn default() -> Self {
        Self {
            search_radius: d_search_radius(),
            coarse_diam: d_track_coarse_diam(),
            fine_diam: d_track_fine_diam(),
            interval_ms: d_track_interval_ms(),
            lost_patience: d_lost_patience(),
        }
    }
}

fn d_inner_ratio() -> f32 {
    0.92
}
fn d_center_exclude_ratio() -> f32 {
    0.22
}
fn d_scale() -> f32 {
    1.45
}
fn d_scale_search() -> f32 {
    0.18
}
fn d_min_score() -> f32 {
    0.45
}
fn d_ema_alpha() -> f32 {
    0.5
}
fn d_coarse_width() -> u32 {
    640
}
fn d_scale_steps() -> u32 {
    13
}
fn d_refine_diam() -> u32 {
    110
}
fn d_locate_interval_ms() -> u64 {
    700
}
fn d_search_radius() -> f32 {
    150.0
}
fn d_track_coarse_diam() -> u32 {
    56
}
fn d_track_fine_diam() -> u32 {
    120
}
fn d_track_interval_ms() -> u64 {
    40
}
fn d_lost_patience() -> u32 {
    4
}

#[derive(Debug, Clone, Deserialize)]
pub struct CaptureConfig {
    pub big_map_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DisplayConfig {
    /// 显示用大地图最大边长（超出则缩小，规避 GPU 纹理上限）
    pub max_texture: u32,
    /// 叠加窗口中地图的显示缩放
    pub view_zoom: f32,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            max_texture: 2048,
            view_zoom: 1.0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RouteConfig {
    /// 路线文件目录
    #[serde(default = "d_route_dir")]
    pub dir: String,
    pub line_width: f32,
    pub point_radius: f32,
    /// 距离点位多少（大地图像素）算到达
    pub reach_radius: f32,
    /// 路线颜色 RGBA（0~255）
    pub color: [u8; 4],
    /// 已走过部分的不透明度系数（0~1）
    pub passed_opacity: f32,
}

fn d_route_dir() -> String {
    "res/routes".into()
}

impl Default for RouteConfig {
    fn default() -> Self {
        Self {
            dir: d_route_dir(),
            line_width: 1.5,
            point_radius: 3.0,
            reach_radius: 10.0,
            color: [220, 50, 50, 255],
            passed_opacity: 0.12,
        }
    }
}

/// 导航箭头参数。
#[derive(Debug, Clone, Deserialize)]
pub struct NavConfig {
    /// 转向/箭头平滑时间常数（毫秒）。越大越平滑跟手越慢，0 表示不平滑。
    #[serde(default = "d_nav_turn_smooth_ms")]
    pub turn_smooth_ms: f32,
}
fn d_nav_turn_smooth_ms() -> f32 {
    120.0
}

impl Default for NavConfig {
    fn default() -> Self {
        Self {
            turn_smooth_ms: d_nav_turn_smooth_ms(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResourceConfig {
    pub file: String,
}

impl Default for ResourceConfig {
    fn default() -> Self {
        Self {
            file: "res/resources.json".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DebugConfig {
    pub enabled: bool,
    /// 是否把每次截取的小地图保存到磁盘
    pub save_minimap: bool,
    pub save_path: String,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            save_minimap: false,
            save_path: "debug_minimap.png".into(),
        }
    }
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("无法读取配置文件: {}", path.display()))?;
        let cfg: Config = toml::from_str(&text).context("配置文件格式错误")?;
        Ok(cfg)
    }
}
