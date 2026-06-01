use std::sync::{Arc, Mutex};

use image::RgbImage;

/// 小地图调试信息（屏幕坐标 / 客户区尺寸）。
#[derive(Debug, Clone, Copy)]
pub struct MinimapDebug {
    pub client_w: i32,
    pub client_h: i32,
    pub region_x: i32,
    pub region_y: i32,
    pub region_size: i32,
}

/// 玩家在大地图（原图）上的像素坐标与置信度。
#[derive(Debug, Clone, Copy)]
pub struct PlayerPos {
    pub x: f32,
    pub y: f32,
    pub score: f32,
    /// RANSAC 估出的尺度（大地图像素 / 小地图像素），用于渲染覆盖圆。
    pub scale: f32,
}

/// 后台线程与 UI 线程之间的共享状态。
#[derive(Default)]
pub struct Shared {
    pub status: String,
    pub locate_debug: String,
    pub player: Option<PlayerPos>,
    /// 最近一次识别到的玩家朝向（弧度；x 轴向右，y 轴向下）。
    pub heading_rad: Option<f32>,
    /// 最近一次截取的小地图（用于调试显示）
    pub minimap: Option<RgbImage>,
    /// 每次更新自增，UI 据此决定是否重建纹理
    pub minimap_seq: u64,
    /// 跟踪用的局部切图（用于调试显示）
    pub tracking_patch: Option<RgbImage>,
    pub tracking_patch_seq: u64,
    pub debug: Option<MinimapDebug>,
    /// UI 请求重新全局定位：后台读到后重置跟踪状态并清零此标志
    pub relocalize: bool,
    /// UI 请求从指定世界坐标开始跟踪（手动定位）：后台读到后直接进入跟踪模式
    pub manual_pos: Option<(f32, f32)>,
    /// 是否启用跟踪（由 UI「开始跟踪/停止跟踪」按钮控制）
    pub tracking_enabled: bool,
    /// 游戏窗口客户区在屏幕上的位置 [screen_x, screen_y, width, height]
    pub game_rect: Option<[i32; 4]>,
}

pub type SharedState = Arc<Mutex<Shared>>;

pub fn new_shared() -> SharedState {
    Arc::new(Mutex::new(Shared {
        status: "初始化...".into(),
        locate_debug: String::new(),
        ..Default::default()
    }))
}
