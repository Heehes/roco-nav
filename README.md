# Roco 地图导航

基于小地图截图，在大地图上实时定位玩家位置，并提供路线导航、物资显示、游戏内方向箭头叠加。

## 环境要求

| 场景 | 要求 |
|------|------|
| 直接运行 exe | Windows 10/11，无需安装 Rust |
| 源码编译 | [Rust](https://rustup.rs/)（stable），Windows |

## 目录结构

程序运行时，**exe 必须与以下文件同目录**：

```
roco-nav/
├── roco-nav.exe      # 主程序
├── config.toml       # 配置文件
└── res/
    ├── map-use.jpg   # 大地图（必需）
    ├── jt.png        # 玩家朝向模板（必需）
    ├── routes/       # 路线 JSON 目录
    └── resources.json
```

## 运行

### 方式一：直接运行打包好的 exe

1. 解压 `dist/roco-nav.zip`
2. 先启动游戏《洛克王国：世界》
3. 双击 `roco-nav.exe`
4. 在导航窗口点击 **「开始跟踪」**

### 方式二：源码运行（开发调试）

在项目根目录执行：

```powershell
cargo run --release
```

首次编译较慢，之后会快很多。

## 常用操作

- **开始跟踪**：首次会自动全局定位，成功后持续跟踪
- **重新定位**：丢弃当前位置，重新全局搜索
- **手动定位**：在地图上右键 →「手动定位」，从该点进入跟踪
- **定位自己**：视图中心跳到当前玩家位置
- **游戏内箭头**：仅显示指向下一目标点位的方向箭头

## 离线自检（校准尺度）

定位不准时，可用一张小地图截图离线验证：

```powershell
# 源码运行
cargo run --release -- selftest

# 或指定图片
cargo run --release -- selftest debug_minimap.png

# 已打包 exe
.\roco-nav.exe selftest
.\roco-nav.exe selftest debug_minimap.png
```

成功后会输出坐标、分数、尺度，并生成：

- `selftest_minimap.png` — 输入的小地图
- `selftest_mapcrop.png` — 大地图匹配处裁切

两者地形应一致。若尺度不对，修改 `config.toml` 中 `[matching].scale` 后重试。

## 打包成 exe

### 1. 编译 release

```powershell
cargo build --release
```

产物：`target\release\roco-nav.exe`

### 2. 收集发布文件并压缩

在项目根目录执行：

```powershell
$d = "dist\roco-nav"
Remove-Item -Recurse -Force $d -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $d | Out-Null
Copy-Item target\release\roco-nav.exe $d
Copy-Item config.toml $d
Copy-Item -Recurse res "$d\res"
Compress-Archive -Path "$d\*" -DestinationPath dist\roco-nav.zip -Force
Write-Host "完成: dist\roco-nav.zip"
```

解压 `dist\roco-nav.zip` 即可分发，约 10 MB（不含未使用的 `res/map.jpg`）。

## 配置说明

主要参数在 `config.toml`：

| 配置项 | 说明 |
|--------|------|
| `[window].title` | 游戏窗口标题（用于自动找窗） |
| `[matching].scale` | 小地图→大地图尺度，不确定时用 `selftest` 校准 |
| `[matching].min_score` | 匹配分数阈值，低于此值判定跟丢（默认 0.45） |
| `[matching].ema_alpha` | 坐标平滑，越大越跟手，越小越稳 |
| `[tracking].search_radius` | 跟踪搜索半径（世界像素），移动快时可适当加大 |
| `[nav].turn_smooth_ms` | 游戏内方向箭头转向平滑时间常数 |
| `[capture].big_map_path` | 大地图路径 |

## 常见问题

**找不到游戏窗口**

- 确认游戏已启动，且 `config.toml` 里 `[window].title` 与窗口标题一致（默认「洛克王国：世界」）

**定位分数低 / 跟丢**

- 运行 `selftest` 检查尺度 `scale` 是否正确
- 适当降低 `[matching].min_score` 或增大 `[tracking].search_radius`
- 确保 `res/map-use.jpg` 与当前游戏区域对应

**exe 闪退**

- 确认 exe 同目录下有 `config.toml` 和 `res/` 文件夹
- 在命令行运行 `roco-nav.exe` 查看报错信息
