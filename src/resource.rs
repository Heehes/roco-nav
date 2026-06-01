use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// 物资类型（目前三种，后续可扩展）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceKind {
    /// 矿物
    Mineral,
    /// 花朵
    Flower,
    /// 星星
    Star,
}

impl ResourceKind {
    /// 全部类型，按显示顺序排列。
    pub const ALL: [ResourceKind; 3] =
        [ResourceKind::Mineral, ResourceKind::Flower, ResourceKind::Star];

    /// 中文名称（用于多选框文字）。
    pub fn label(self) -> &'static str {
        match self {
            ResourceKind::Mineral => "矿物",
            ResourceKind::Flower => "花朵",
            ResourceKind::Star => "星星",
        }
    }
}

/// 单个物资点位（大地图原图像素坐标 + 类型）。
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Resource {
    pub x: f32,
    pub y: f32,
    pub kind: ResourceKind,
}

#[derive(Debug, Clone, Deserialize)]
struct ResourceFile {
    #[serde(default)]
    resources: Vec<Resource>,
}

/// 从 json 文件加载所有物资点位。文件不存在时返回空列表。
pub fn load(path: impl AsRef<Path>) -> Result<Vec<Resource>> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取物资文件: {}", path.display()))?;
    let file: ResourceFile = serde_json::from_str(&text).context("物资文件格式错误")?;
    Ok(file.resources)
}
