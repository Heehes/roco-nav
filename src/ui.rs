use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use eframe::egui;
use egui::{Color32, Pos2, Rect, Stroke, Vec2};
use image::{imageops::FilterType, RgbImage};

use crate::config::Config;
use crate::resource::{Resource, ResourceKind};
use crate::route::Route;
use crate::state::SharedState;

/// 启动叠加窗口（阻塞直到关闭）。
pub fn run(
    cfg: Config,
    big_map: Arc<RgbImage>,
    routes: Vec<Route>,
    resources: Vec<Resource>,
    shared: SharedState,
) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([820.0, 560.0])
            .with_min_inner_size([520.0, 360.0])
            .with_always_on_top()
            .with_title("Roco 地图导航"),
        ..Default::default()
    };

    eframe::run_native(
        "roco-nav",
        options,
        Box::new(move |cc| {
            Ok(Box::new(OverlayApp::new(
                cc, cfg, big_map, routes, resources, shared,
            )))
        }),
    )
    .map_err(|e| anyhow!("叠加窗口启动失败: {e}"))
}

struct OverlayApp {
    cfg: Config,
    shared: SharedState,
    routes: Vec<Route>,
    /// 物资点位
    resources: Vec<Resource>,
    /// 每种物资是否显示（与 ResourceKind::ALL 顺序对应）
    show_resource: [bool; 3],
    /// 每条路线各点位是否已到达
    reached: Vec<Vec<bool>>,
    selected: usize,
    /// 缩小后的大地图纹理（显示用）
    map_tex: egui::TextureHandle,
    /// 显示纹理像素 / 世界像素
    display_scale: f32,
    map_world: Vec2,
    /// 相机：视图中心对应的世界坐标
    cam_focus: Vec2,
    /// 相机：屏幕像素 / 世界像素
    cam_zoom: f32,
    /// 是否跟踪玩家（开启时视图中心锁定玩家）
    follow_player: bool,
    /// 相机是否已完成首次初始化
    cam_inited: bool,
    minimap_tex: Option<egui::TextureHandle>,
    last_minimap_seq: u64,
    tracking_patch_tex: Option<egui::TextureHandle>,
    last_tracking_patch_seq: u64,
    /// 右键点击时记录的世界坐标，跨帧持久化供菜单按钮使用
    context_menu_world: Option<egui::Vec2>,
    /// 游戏窗口上的透明箭头叠加层
    game_overlay: Option<crate::overlay::GameOverlay>,
    /// 路线点位变换：偏移 X
    route_offset_x: f32,
    /// 路线点位变换：偏移 Y
    route_offset_y: f32,
    /// 路线点位变换：缩放 X
    route_scale_x: f32,
    /// 路线点位变换：缩放 Y
    route_scale_y: f32,
    /// 是否显示点位标签
    show_point_labels: bool,
    /// 每条路线的开始点位标签（该标签之前的点位自动标记已到达）
    start_labels: Vec<String>,
    /// 平滑后的显示朝向（弧度）：用于路线/箭头转向缓动
    disp_heading: Option<f32>,
}

impl OverlayApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        cfg: Config,
        big_map: Arc<RgbImage>,
        routes: Vec<Route>,
        resources: Vec<Resource>,
        shared: SharedState,
    ) -> Self {
        install_cjk_font(&cc.egui_ctx);

        let map_world = Vec2::new(big_map.width() as f32, big_map.height() as f32);
        let max_side = big_map.width().max(big_map.height());
        let scale = if max_side > cfg.display.max_texture {
            cfg.display.max_texture as f32 / max_side as f32
        } else {
            1.0
        };
        let tw = (big_map.width() as f32 * scale).round().max(1.0) as u32;
        let th = (big_map.height() as f32 * scale).round().max(1.0) as u32;
        let small = image::imageops::resize(big_map.as_ref(), tw, th, FilterType::Triangle);
        let map_tex = cc.egui_ctx.load_texture(
            "big_map",
            to_color_image(&small),
            egui::TextureOptions::LINEAR,
        );

        let reached = routes.iter().map(|r| vec![false; r.points.len()]).collect();
        let start_labels = vec![String::new(); routes.len()];
        let display_scale = tw as f32 / map_world.x;
        let init_zoom = display_scale * cfg.display.view_zoom;

        let (def_offset, def_scale) = routes
            .first()
            .map(|r| (r.default_offset, r.default_scale))
            .unwrap_or(([0.0, 0.0], [1.0, 1.0]));

        Self {
            cfg,
            shared,
            routes,
            resources,
            show_resource: [false; 3],
            reached,
            selected: 0,
            map_tex,
            display_scale,
            map_world,
            cam_focus: map_world * 0.5,
            cam_zoom: init_zoom,
            follow_player: true,
            cam_inited: false,
            minimap_tex: None,
            last_minimap_seq: u64::MAX,
            tracking_patch_tex: None,
            last_tracking_patch_seq: u64::MAX,
            context_menu_world: None,
            game_overlay: crate::overlay::GameOverlay::create(),
            route_offset_x: def_offset[0],
            route_offset_y: def_offset[1],
            route_scale_x: def_scale[0],
            route_scale_y: def_scale[1],
            show_point_labels: true,
            start_labels,
            disp_heading: None,
        }
    }
}

impl eframe::App for OverlayApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 读取共享状态快照
        let (status, locate_debug, player, heading_rad, debug, minimap, tracking_patch, game_rect) = {
            let s = self.shared.lock().unwrap();
            let minimap = if s.minimap_seq != self.last_minimap_seq {
                self.last_minimap_seq = s.minimap_seq;
                s.minimap.clone()
            } else {
                None
            };
            let tracking_patch = if s.tracking_patch_seq != self.last_tracking_patch_seq {
                self.last_tracking_patch_seq = s.tracking_patch_seq;
                s.tracking_patch.clone()
            } else {
                None
            };
            (
                s.status.clone(),
                s.locate_debug.clone(),
                s.player,
                s.heading_rad,
                s.debug,
                minimap,
                tracking_patch,
                s.game_rect,
            )
        };

        // 重建小地图调试纹理
        if let Some(mm) = minimap {
            self.minimap_tex = Some(ctx.load_texture(
                "minimap",
                to_color_image(&mm),
                egui::TextureOptions::NEAREST,
            ));
        }
        if let Some(tp) = tracking_patch {
            self.tracking_patch_tex = Some(ctx.load_texture(
                "tracking_patch",
                to_color_image(&tp),
                egui::TextureOptions::NEAREST,
            ));
        }

        // 标记已到达点位（变换后坐标）
        if let Some(p) = player {
            if let (Some(route), Some(reached)) =
                (self.routes.get(self.selected), self.reached.get_mut(self.selected))
            {
                let rr = self.cfg.route.reach_radius;
                for (i, pt) in route.points.iter().enumerate() {
                    let tx = pt.x * self.route_scale_x + self.route_offset_x;
                    let ty = pt.y * self.route_scale_y + self.route_offset_y;
                    if ((tx - p.x).powi(2) + (ty - p.y).powi(2)).sqrt() <= rr {
                        reached[i] = true;
                    }
                }
            }
        }

        // 朝向平滑（带角度环绕）：让路线/箭头转向有缓动、不跳变。
        let dt = ctx.input(|i| i.stable_dt).clamp(1e-4, 0.1);
        let tau = (self.cfg.nav.turn_smooth_ms / 1000.0).max(0.0);
        if let Some(target) = heading_rad {
            self.disp_heading = Some(match self.disp_heading {
                Some(cur) if tau > 1e-4 => {
                    let k = 1.0 - (-dt / tau).exp();
                    wrap_angle(cur + wrap_angle(target - cur) * k)
                }
                _ => target,
            });
        }
        let disp_heading = self.disp_heading.or(heading_rad);

        self.side_panel(ctx, &status, &locate_debug, player, debug);
        self.map_panel(ctx, player, disp_heading, debug);
        self.game_nav_overlay(player, disp_heading, game_rect);

        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

impl OverlayApp {
    fn side_panel(
        &mut self,
        ctx: &egui::Context,
        status: &str,
        locate_debug: &str,
        player: Option<crate::state::PlayerPos>,
        debug: Option<crate::state::MinimapDebug>,
    ) {
        egui::SidePanel::left("side")
            .resizable(true)
            .default_width(220.0)
            .show(ctx, |ui| {
                ui.heading("Roco 地图导航");
                ui.separator();

                ui.label(format!("状态: {status}"));
                match player {
                    Some(p) => {
                        ui.label(format!("坐标: ({:.0}, {:.0})", p.x, p.y));
                        ui.label(format!("匹配度: {:.3}", p.score));
                    }
                    None => {
                        ui.label("坐标: --");
                    }
                };
                ui.separator();

                ui.label("路线:");
                if self.routes.is_empty() {
                    ui.colored_label(Color32::LIGHT_RED, "未加载到路线 (res/routes.json)");
                } else {
                    let cur = self
                        .routes
                        .get(self.selected)
                        .map(|r| r.name.clone())
                        .unwrap_or_default();
                    let prev_selected = self.selected;
                    egui::ComboBox::from_id_salt("route_sel")
                        .selected_text(cur)
                        .show_ui(ui, |ui| {
                            for (i, r) in self.routes.iter().enumerate() {
                                ui.selectable_value(&mut self.selected, i, &r.name);
                            }
                        });
                    if self.selected != prev_selected {
                        if let Some(route) = self.routes.get(self.selected) {
                            self.route_offset_x = route.default_offset[0];
                            self.route_offset_y = route.default_offset[1];
                            self.route_scale_x = route.default_scale[0];
                            self.route_scale_y = route.default_scale[1];
                        }
                        self.apply_start_label();
                    }
                    ui.horizontal(|ui| {
                        ui.label("开始点位:");
                        if let Some(label) = self.start_labels.get_mut(self.selected) {
                            let resp = ui.text_edit_singleline(label);
                            if resp.changed() {
                                self.apply_start_label();
                            }
                        }
                    });
                    if ui.button("重置当前路线进度").clicked() {
                        if let Some(reached) = self.reached.get_mut(self.selected) {
                            reached.iter_mut().for_each(|v| *v = false);
                        }
                        if let Some(label) = self.start_labels.get_mut(self.selected) {
                            label.clear();
                        }
                    }
                }

                // 路线点位变换调整（临时功能）
                ui.separator();
                ui.heading("点位变换");
                ui.label("偏移 X:");
                ui.add(egui::DragValue::new(&mut self.route_offset_x).speed(1.0));
                ui.label("偏移 Y:");
                ui.add(egui::DragValue::new(&mut self.route_offset_y).speed(1.0));
                ui.label("缩放 X:");
                ui.add(
                    egui::DragValue::new(&mut self.route_scale_x)
                        .speed(0.01)
                        .range(0.01..=10.0),
                );
                ui.label("缩放 Y:");
                ui.add(
                    egui::DragValue::new(&mut self.route_scale_y)
                        .speed(0.01)
                        .range(0.01..=10.0),
                );
                if ui.button("重置变换").clicked() {
                    let (def_offset, def_scale) = self
                        .routes
                        .get(self.selected)
                        .map(|r| (r.default_offset, r.default_scale))
                        .unwrap_or(([0.0, 0.0], [1.0, 1.0]));
                    self.route_offset_x = def_offset[0];
                    self.route_offset_y = def_offset[1];
                    self.route_scale_x = def_scale[0];
                    self.route_scale_y = def_scale[1];
                }
                ui.checkbox(&mut self.show_point_labels, "显示标签");
                if let Some(route) = self.routes.get(self.selected) {
                    ui.label(format!("当前路线: {} 个点位", route.points.len()));
                }

                if self.cfg.debug.enabled {
                    ui.separator();
                    ui.heading("调试信息");
                    if let Some(d) = debug {
                        ui.label(format!("客户区: {} x {}", d.client_w, d.client_h));
                        ui.label(format!("小地图屏幕位置: ({}, {})", d.region_x, d.region_y));
                        ui.label(format!("小地图边长: {} px", d.region_size));
                    } else {
                        ui.label("等待首帧截图...");
                    }
                    if !locate_debug.is_empty() {
                        ui.separator();
                        ui.label("定位诊断:");
                        ui.monospace(locate_debug);
                    }
                    if let Some(tex) = &self.minimap_tex {
                        ui.label("截取到的小地图:");
                        let side = 160.0;
                        ui.add(egui::Image::new(egui::load::SizedTexture::new(
                            tex.id(),
                            Vec2::splat(side),
                        )));
                    }
                    if let Some(tex) = &self.tracking_patch_tex {
                        ui.label("跟踪用切图:");
                        let size = tex.size();
                        let aspect = size[0] as f32 / size[1].max(1) as f32;
                        let w = 200.0_f32;
                        let h = w / aspect;
                        ui.add(egui::Image::new(egui::load::SizedTexture::new(
                            tex.id(),
                            Vec2::new(w, h),
                        )));
                    }
                }
            });
    }

    fn map_panel(
        &mut self,
        ctx: &egui::Context,
        player: Option<crate::state::PlayerPos>,
        heading_rad: Option<f32>,
        debug: Option<crate::state::MinimapDebug>,
    ) {
        egui::CentralPanel::default().show(ctx, |ui| {
            // 顶部工具栏
            ui.horizontal_wrapped(|ui| {
                // 开始/停止跟踪 切换按钮
                let is_tracking = self.shared.lock().map_or(false, |s| s.tracking_enabled);
                if is_tracking {
                    if ui
                        .add(
                            egui::Button::new("⏹ 停止跟踪")
                                .fill(egui::Color32::from_rgba_unmultiplied(180, 60, 60, 220)),
                        )
                        .on_hover_text("停止定位与跟踪")
                        .clicked()
                    {
                        if let Ok(mut s) = self.shared.lock() {
                            s.tracking_enabled = false;
                            s.player = None;
                        }
                    }
                } else {
                    if ui
                        .add(
                            egui::Button::new("▶ 开始跟踪")
                                .fill(egui::Color32::from_rgba_unmultiplied(60, 160, 60, 220)),
                        )
                        .on_hover_text("开始全局定位并持续跟踪")
                        .clicked()
                    {
                        if let Ok(mut s) = self.shared.lock() {
                            s.tracking_enabled = true;
                            s.relocalize = true;
                            s.player = None;
                        }
                    }
                }

                // 重新定位按钮
                if ui
                    .add(
                        egui::Button::new("🔄 重新定位")
                            .fill(egui::Color32::from_rgba_unmultiplied(200, 120, 60, 220)),
                    )
                    .on_hover_text("丢弃当前跟踪，重新执行全局 SIFT 定位")
                    .clicked()
                {
                    if let Ok(mut s) = self.shared.lock() {
                        s.relocalize = true;
                        s.tracking_enabled = true;
                        s.player = None;
                    }
                }
                ui.separator();
                // 4.3 / 4.4 定位与跟踪
                if ui.button("📍 定位自己").clicked() {
                    if let Some(p) = player {
                        self.cam_focus = Vec2::new(p.x, p.y);
                        self.follow_player = true;
                    }
                }
                ui.checkbox(&mut self.follow_player, "跟踪自己");
                ui.separator();
                // 4.5 物资多选
                ui.label("物资:");
                for (i, kind) in ResourceKind::ALL.iter().enumerate() {
                    ui.checkbox(&mut self.show_resource[i], kind.label());
                }
            });

            // 地图绘制区域（占据剩余空间，可交互）
            let rect = ui.available_rect_before_wrap();
            let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
            let painter = ui.painter_at(rect);
            let center = rect.center();

            // 右键点击时记录世界坐标（持久化到 struct 字段，供菜单帧使用）
            if response.secondary_clicked() {
                if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                    let world_pos = self.cam_focus + (pos - center) / self.cam_zoom;
                    self.context_menu_world = Some(world_pos);
                }
            }

            // 右键菜单：egui 原生 context_menu 悬浮于光标位置
            response.context_menu(|ui| {
                if ui.button("📍 手动定位").clicked() {
                    if let Some(world) = self.context_menu_world {
                        if let Ok(mut s) = self.shared.lock() {
                            s.player = Some(crate::state::PlayerPos {
                                x: world.x,
                                y: world.y,
                                score: 1.0,
                                scale: 1.0,
                            });
                            // 通知后台从此坐标开始跟踪，而非继续全图 SIFT
                            s.manual_pos = Some((world.x, world.y));
                            s.status = "手动定位完成，等待跟踪确认...".into();
                        }
                        self.cam_focus = world;
                        self.follow_player = true;
                    }
                    ui.close_menu();
                }
            });

            // 跟踪开启且有玩家时，视图中心锁定玩家
            if self.follow_player {
                if let Some(p) = player {
                    self.cam_focus = Vec2::new(p.x, p.y);
                }
            }

            // 首次初始化相机焦点（无玩家时居中地图）
            if !self.cam_inited {
                self.cam_focus = match player {
                    Some(p) => Vec2::new(p.x, p.y),
                    None => self.map_world * 0.5,
                };
                self.cam_inited = true;
            }

            // 4.2 左键拖动平移（拖动时关闭跟踪，便于自由浏览）
            if response.dragged_by(egui::PointerButton::Primary) {
                let d = response.drag_delta();
                if d != egui::Vec2::ZERO {
                    self.follow_player = false;
                    self.cam_focus -= d / self.cam_zoom;
                }
            }

            // 4.1 滚轮缩放（以光标为中心）
            let scroll = ctx.input(|i| i.raw_scroll_delta.y);
            if scroll != 0.0 && response.hovered() {
                let pointer = ctx
                    .input(|i| i.pointer.hover_pos())
                    .unwrap_or(center);
                // 缩放前光标对应的世界坐标
                let world_before = self.cam_focus + (pointer - center) / self.cam_zoom;
                let factor = (scroll * 0.0015).exp();
                let min_zoom = self.display_scale * 0.1;
                let max_zoom = self.display_scale * 20.0;
                self.cam_zoom = (self.cam_zoom * factor).clamp(min_zoom, max_zoom);
                // 保持光标处世界点不动
                self.cam_focus = world_before - (pointer - center) / self.cam_zoom;
            }

            let z = self.cam_zoom;
            let focus = self.cam_focus;
            let world_to_screen = |w: Vec2| -> Pos2 { center + (w - focus) * z };

            // 绘制地图（整张纹理对应世界 [0,map_world]）
            let map_rect = Rect::from_min_max(
                world_to_screen(Vec2::ZERO),
                world_to_screen(self.map_world),
            );
            painter.image(
                self.map_tex.id(),
                map_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );

            // 绘制路线点位（应用偏移+缩放变换）
            if let (Some(route), Some(reached)) =
                (self.routes.get(self.selected), self.reached.get(self.selected))
            {
                let base = self.cfg.route.color;
                let full = Color32::from_rgba_unmultiplied(base[0], base[1], base[2], base[3]);
                let passed_a = (base[3] as f32 * self.cfg.route.passed_opacity) as u8;
                let passed =
                    Color32::from_rgba_unmultiplied(base[0], base[1], base[2], passed_a);
                let pts = &route.points;

                let transform = |pt: &crate::route::RoutePoint| -> Vec2 {
                    Vec2::new(
                        pt.x * self.route_scale_x + self.route_offset_x,
                        pt.y * self.route_scale_y + self.route_offset_y,
                    )
                };

                // 有序路线：画连线
                if route.is_path {
                    for i in 1..pts.len() {
                        let color = if reached[i - 1] { passed } else { full };
                        painter.line_segment(
                            [
                                world_to_screen(transform(&pts[i - 1])),
                                world_to_screen(transform(&pts[i])),
                            ],
                            Stroke::new(self.cfg.route.line_width, color),
                        );
                    }
                }

                for (i, pt) in pts.iter().enumerate() {
                    let color = if reached[i] { passed } else { full };
                    let screen_pos = world_to_screen(transform(pt));
                    if !rect.contains(screen_pos) {
                        continue;
                    }
                    painter.circle_filled(screen_pos, self.cfg.route.point_radius, color);
                    painter.circle_stroke(
                        screen_pos,
                        self.cfg.route.point_radius + 1.0,
                        Stroke::new(1.0, Color32::WHITE),
                    );

                    if self.show_point_labels {
                        if let Some(label) = pt.label {
                            painter.text(
                                screen_pos + Vec2::new(0.0, -self.cfg.route.point_radius - 8.0),
                                egui::Align2::CENTER_BOTTOM,
                                label,
                                egui::FontId::proportional(10.0),
                                Color32::WHITE,
                            );
                        }
                    }
                }
            }

            // 4.5 绘制物资图标（按多选过滤）
            for res in &self.resources {
                let idx = ResourceKind::ALL.iter().position(|k| *k == res.kind);
                let visible = idx.map_or(false, |i| self.show_resource[i]);
                if !visible {
                    continue;
                }
                let p = world_to_screen(Vec2::new(res.x, res.y));
                if rect.contains(p) {
                    draw_resource_icon(&painter, p, res.kind);
                }
            }

            // 绘制玩家标记（始终在 cam_focus；跟踪时即视图中心）
            if let Some(pl) = player {
                let p = world_to_screen(Vec2::new(pl.x, pl.y));
                painter.circle_filled(p, 5.0, Color32::from_rgb(255, 80, 80));
                painter.circle_stroke(p, 9.0, Stroke::new(2.0, Color32::WHITE));

                if let Some(heading) = heading_rad {
                    let dir = Vec2::new(heading.cos(), heading.sin());
                    let start = p + dir * 9.0;
                    let end = p + dir * 24.0;
                    painter.line_segment(
                        [start, end],
                        Stroke::new(4.0, Color32::from_rgba_unmultiplied(20, 20, 20, 180)),
                    );
                    painter.line_segment(
                        [start, end],
                        Stroke::new(2.2, Color32::from_rgb(255, 210, 60)),
                    );
                }

                // 绘制小地图覆盖圆：半径 = 小地图像素半径 × 尺度
                // 让你直观看到当前尺度下小地图对应大地图多大范围
                if let Some(dbg) = debug {
                    let minimap_r = dbg.region_size as f32 * 0.5;
                    let coverage_r = minimap_r * pl.scale.abs().max(0.01);
                    let screen_r = coverage_r * z;
                    if screen_r > 1.0 {
                        painter.circle_stroke(
                            p,
                            screen_r,
                            Stroke::new(2.0, Color32::from_rgba_unmultiplied(255, 220, 0, 200)),
                        );
                        // 在圆边上显示覆盖半径数值（大地图像素）
                        let label_pos = p + Vec2::new(screen_r + 4.0, 0.0);
                        painter.text(
                            label_pos,
                            egui::Align2::LEFT_CENTER,
                            format!("r={:.0}px  s={:.2}", coverage_r, pl.scale),
                            egui::FontId::proportional(12.0),
                            Color32::from_rgba_unmultiplied(255, 220, 0, 220),
                        );
                    }
                }
            }
        });
    }

    /// 根据开始点位标签，将该标签之前的所有点位标记为已到达
    fn apply_start_label(&mut self) {
        let label_str = match self.start_labels.get(self.selected) {
            Some(s) => s.trim().to_string(),
            None => return,
        };
        let (route, reached) = match (
            self.routes.get(self.selected),
            self.reached.get_mut(self.selected),
        ) {
            (Some(r), Some(v)) => (r, v),
            _ => return,
        };
        if label_str.is_empty() {
            reached.iter_mut().for_each(|v| *v = false);
            return;
        }
        let target_idx = route
            .points
            .iter()
            .position(|pt| pt.label.map_or(false, |l| l == label_str));
        match target_idx {
            Some(idx) => {
                for (i, v) in reached.iter_mut().enumerate() {
                    *v = i < idx;
                }
            }
            None => {
                reached.iter_mut().for_each(|v| *v = false);
            }
        }
    }

    fn next_unreached_target(&self) -> Option<(usize, crate::route::RoutePoint)> {
        let route = self.routes.get(self.selected)?;
        let reached = self.reached.get(self.selected)?;
        for (i, (pt, &r)) in route.points.iter().zip(reached.iter()).enumerate() {
            if !r {
                let mut transformed = *pt;
                transformed.x = pt.x * self.route_scale_x + self.route_offset_x;
                transformed.y = pt.y * self.route_scale_y + self.route_offset_y;
                return Some((i, transformed));
            }
        }
        None
    }

    /// 驱动游戏窗口叠加层显示指向下一个点位的箭头。
    fn game_nav_overlay(
        &self,
        player: Option<crate::state::PlayerPos>,
        heading_rad: Option<f32>,
        game_rect: Option<[i32; 4]>,
    ) {
        let overlay = match &self.game_overlay {
            Some(o) => o,
            None => return,
        };

        let Some([gx, gy, gw, gh]) = game_rect else { return };
        let Some(p) = player else {
            overlay.update(gx, gy, gw, gh, None);
            return;
        };

        let Some((_idx, target)) = self.next_unreached_target() else {
            overlay.update(gx, gy, gw, gh, None);
            return;
        };
        let dx = target.x - p.x;
        let dy = target.y - p.y;
        let distance = (dx * dx + dy * dy).sqrt();
        if distance < 1.0 {
            overlay.update(gx, gy, gw, gh, None);
            return;
        }
        let heading = heading_rad.unwrap_or(-std::f32::consts::FRAC_PI_2);
        let world_angle = dy.atan2(dx);
        let rel_angle = world_angle - heading;
        overlay.update(
            gx,
            gy,
            gw,
            gh,
            Some(crate::overlay::NavContent::Arrow(&crate::overlay::ArrowInfo {
                rel_angle,
                distance,
            })),
        );
    }
}

/// 在屏幕坐标处绘制物资图标。
fn draw_resource_icon(painter: &egui::Painter, p: Pos2, kind: ResourceKind) {
    let r = 7.0;
    match kind {
        // 矿物：青色菱形
        ResourceKind::Mineral => {
            let c = Color32::from_rgb(80, 220, 220);
            let pts = vec![
                p + Vec2::new(0.0, -r),
                p + Vec2::new(r, 0.0),
                p + Vec2::new(0.0, r),
                p + Vec2::new(-r, 0.0),
            ];
            painter.add(egui::Shape::convex_polygon(
                pts,
                c,
                Stroke::new(1.5, Color32::WHITE),
            ));
        }
        // 花朵：洋红色圆点
        ResourceKind::Flower => {
            let c = Color32::from_rgb(255, 105, 180);
            painter.circle_filled(p, r, c);
            painter.circle_stroke(p, r, Stroke::new(1.5, Color32::WHITE));
            painter.circle_filled(p, r * 0.4, Color32::from_rgb(255, 235, 120));
        }
        // 星星：黄色五角星
        ResourceKind::Star => {
            let c = Color32::from_rgb(255, 215, 0);
            let mut pts = Vec::with_capacity(10);
            for i in 0..10 {
                let ang = std::f32::consts::PI * (i as f32) / 5.0
                    - std::f32::consts::FRAC_PI_2;
                let rad = if i % 2 == 0 { r } else { r * 0.45 };
                pts.push(p + Vec2::new(ang.cos() * rad, ang.sin() * rad));
            }
            painter.add(egui::Shape::convex_polygon(
                pts,
                c,
                Stroke::new(1.0, Color32::from_rgb(180, 140, 0)),
            ));
        }
    }
}

/// 把角度归一化到 (-π, π]，便于做最短路径的角度插值。
fn wrap_angle(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let mut x = a % tau;
    if x <= -std::f32::consts::PI {
        x += tau;
    } else if x > std::f32::consts::PI {
        x -= tau;
    }
    x
}

/// RgbImage -> egui ColorImage。
fn to_color_image(img: &RgbImage) -> egui::ColorImage {
    let size = [img.width() as usize, img.height() as usize];
    let mut px = Vec::with_capacity(size[0] * size[1] * 4);
    for p in img.pixels() {
        let [r, g, b] = p.0;
        px.extend_from_slice(&[r, g, b, 255]);
    }
    egui::ColorImage::from_rgba_unmultiplied(size, &px)
}

/// 安装系统中文字体，避免中文显示为方块。
fn install_cjk_font(ctx: &egui::Context) {
    let candidates = [
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "C:\\Windows\\Fonts\\simsun.ttc",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("cjk".to_owned(), egui::FontData::from_owned(bytes));
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "cjk".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("cjk".to_owned());
            ctx.set_fonts(fonts);
            return;
        }
    }
}
