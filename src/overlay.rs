use std::ffi::c_void;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub struct ArrowInfo {
    pub rel_angle: f32,
    pub distance: f32,
}

/// 游戏窗口叠加层要绘制的导航内容。
pub enum NavContent<'a> {
    Arrow(&'a ArrowInfo),
}

/// Win32 分层窗口（WS_EX_LAYERED），实现逐像素透明 + 穿透点击的游戏叠加箭头。
pub struct GameOverlay {
    hwnd: HWND,
}

impl GameOverlay {
    pub fn create() -> Option<Self> {
        unsafe {
            let class_name: Vec<u16> = "RocoOverlay\0".encode_utf16().collect();
            let pcwstr = windows::core::PCWSTR(class_name.as_ptr());

            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(overlay_wndproc),
                lpszClassName: pcwstr,
                ..Default::default()
            };
            RegisterClassExW(&wc);

            let style = WS_EX_LAYERED
                | WS_EX_TRANSPARENT
                | WS_EX_TOPMOST
                | WS_EX_TOOLWINDOW
                | WS_EX_NOACTIVATE;

            let hwnd = CreateWindowExW(
                style,
                pcwstr,
                windows::core::PCWSTR::null(),
                WS_POPUP,
                0, 0, 1, 1,
                HWND::default(),
                HMENU::default(),
                HINSTANCE::default(),
                None,
            )
            .ok()?;

            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            Some(Self { hwnd })
        }
    }

    /// 更新叠加层位置、大小及导航内容。content 为 None 时绘制空帧（全透明）。
    pub fn update(&self, sx: i32, sy: i32, w: i32, h: i32, content: Option<NavContent>) {
        if w <= 0 || h <= 0 {
            return;
        }

        unsafe {
            let screen_dc = GetDC(HWND::default());
            let mem_dc = CreateCompatibleDC(screen_dc);

            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    ..Default::default()
                },
                ..Default::default()
            };

            let mut bits: *mut c_void = std::ptr::null_mut();
            let Ok(dib) =
                CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            else {
                let _ = DeleteDC(mem_dc);
                ReleaseDC(HWND::default(), screen_dc);
                return;
            };
            let old = SelectObject(mem_dc, HGDIOBJ(dib.0));

            let buf =
                std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
            buf.fill(0);

            match content {
                Some(NavContent::Arrow(a)) => render_arrow(buf, w, h, a),
                None => {}
            }

            let pt_dst = POINT { x: sx, y: sy };
            let sz = SIZE { cx: w, cy: h };
            let pt_src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: 0,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: 1, // AC_SRC_ALPHA
            };

            let _ = UpdateLayeredWindow(
                self.hwnd,
                screen_dc,
                Some(&pt_dst),
                Some(&sz),
                mem_dc,
                Some(&pt_src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );

            SelectObject(mem_dc, old);
            let _ = DeleteObject(HGDIOBJ(dib.0));
            let _ = DeleteDC(mem_dc);
            ReleaseDC(HWND::default(), screen_dc);
        }
    }
}

impl Drop for GameOverlay {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

unsafe extern "system" fn overlay_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

// ── 软件光栅化 ──────────────────────────────────────────────

fn render_arrow(buf: &mut [u8], w: i32, h: i32, arrow: &ArrowInfo) {
    let cx = w as f32 / 2.0;
    let cy = h as f32 / 2.0;

    let arr_dx = arrow.rel_angle.sin();
    let arr_dy = -arrow.rel_angle.cos();

    // 地图距离 [100, 500] 像素 → 屏幕线长 [最短, 半屏]
    const MAP_DIST_MIN: f32 = 100.0;
    const MAP_DIST_MAX: f32 = 500.0;

    let min_dim = (w as f32).min(h as f32);
    let max_len = min_dim * 0.5;
    let min_len = max_len * (MAP_DIST_MIN / MAP_DIST_MAX);
    let d = arrow.distance.clamp(MAP_DIST_MIN, MAP_DIST_MAX);
    let t = (d - MAP_DIST_MIN) / (MAP_DIST_MAX - MAP_DIST_MIN);
    let len = min_len + t * (max_len - min_len);

    let start_off = 25.0;
    let sx = cx + arr_dx * start_off;
    let sy = cy + arr_dy * start_off;
    let ex = cx + arr_dx * (start_off + len);
    let ey = cy + arr_dy * (start_off + len);

    let outline = premul(0, 0, 0, 180);
    let green = premul(0, 255, 120, 230);

    // 轴线：先粗描边再细主色
    draw_thick_line(buf, w, h, sx, sy, ex, ey, 6.0, outline);
    draw_thick_line(buf, w, h, sx, sy, ex, ey, 3.0, green);

    // 箭头三角
    let head_len = 20.0;
    let head_hw = 12.0;
    let bx = ex - arr_dx * head_len;
    let by = ey - arr_dy * head_len;
    let px = -arr_dy;
    let py = arr_dx;

    let out_pts = [
        (ex + arr_dx * 4.0, ey + arr_dy * 4.0),
        (bx + px * (head_hw + 3.0), by + py * (head_hw + 3.0)),
        (bx - px * (head_hw + 3.0), by - py * (head_hw + 3.0)),
    ];
    fill_triangle(buf, w, h, out_pts, outline);

    let in_pts = [
        (ex, ey),
        (bx + px * head_hw, by + py * head_hw),
        (bx - px * head_hw, by - py * head_hw),
    ];
    fill_triangle(buf, w, h, in_pts, green);
}


/// 实心圆（alpha-over 合成，颜色须为预乘 BGRA）。
fn fill_circle(buf: &mut [u8], w: i32, h: i32, cx: f32, cy: f32, r: f32, color: [u8; 4]) {
    if r < 0.5 {
        return;
    }
    let x0 = ((cx - r).floor() as i32).max(0);
    let x1 = ((cx + r).ceil() as i32).min(w - 1);
    let y0 = ((cy - r).floor() as i32).max(0);
    let y1 = ((cy + r).ceil() as i32).min(h - 1);
    let r2 = r * r;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            if dx * dx + dy * dy <= r2 {
                put_pixel(buf, w, h, x, y, color);
            }
        }
    }
}

/// BGRA 预乘 alpha（UpdateLayeredWindow 要求预乘格式）。
fn premul(r: u8, g: u8, b: u8, a: u8) -> [u8; 4] {
    let af = a as f32 / 255.0;
    [
        (b as f32 * af) as u8,
        (g as f32 * af) as u8,
        (r as f32 * af) as u8,
        a,
    ]
}

/// 逐像素 alpha-over 合成（均为预乘）。
fn put_pixel(buf: &mut [u8], w: i32, h: i32, x: i32, y: i32, c: [u8; 4]) {
    if x < 0 || y < 0 || x >= w || y >= h {
        return;
    }
    let idx = ((y * w + x) * 4) as usize;
    if idx + 3 >= buf.len() {
        return;
    }
    let sa = c[3] as u16;
    let inv = 255 - sa;
    for i in 0..4 {
        buf[idx + i] = ((c[i] as u16 * 255 + buf[idx + i] as u16 * inv) / 255) as u8;
    }
}

/// 用旋转矩形模拟粗线条。
fn draw_thick_line(
    buf: &mut [u8],
    w: i32,
    h: i32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    thickness: f32,
    color: [u8; 4],
) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.5 {
        return;
    }
    let nx = -dy / len;
    let ny = dx / len;
    let ht = thickness / 2.0;
    let c = [
        (x0 + nx * ht, y0 + ny * ht),
        (x0 - nx * ht, y0 - ny * ht),
        (x1 - nx * ht, y1 - ny * ht),
        (x1 + nx * ht, y1 + ny * ht),
    ];
    fill_triangle(buf, w, h, [c[0], c[1], c[2]], color);
    fill_triangle(buf, w, h, [c[0], c[2], c[3]], color);
}

/// 扫描线填充三角形。
fn fill_triangle(
    buf: &mut [u8],
    w: i32,
    h: i32,
    pts: [(f32, f32); 3],
    color: [u8; 4],
) {
    let min_y = (pts[0].1.min(pts[1].1).min(pts[2].1).floor() as i32).max(0);
    let max_y = (pts[0].1.max(pts[1].1).max(pts[2].1).ceil() as i32).min(h - 1);

    for y in min_y..=max_y {
        let yf = y as f32 + 0.5;
        let mut xs = [f32::MAX; 2];
        let mut cnt = 0usize;
        for i in 0..3 {
            let j = (i + 1) % 3;
            let (ax, ay) = pts[i];
            let (bx, by) = pts[j];
            if (ay <= yf && by > yf) || (by <= yf && ay > yf) {
                let t = (yf - ay) / (by - ay);
                if cnt < 2 {
                    xs[cnt] = ax + t * (bx - ax);
                    cnt += 1;
                }
            }
        }
        if cnt == 2 {
            let (l, r) = if xs[0] < xs[1] { (xs[0], xs[1]) } else { (xs[1], xs[0]) };
            let x_start = (l.floor() as i32).max(0);
            let x_end = (r.ceil() as i32).min(w - 1);
            for x in x_start..=x_end {
                put_pixel(buf, w, h, x, y, color);
            }
        }
    }
}
