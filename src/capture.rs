use anyhow::{anyhow, Result};
use image::RgbImage;
use std::ffi::c_void;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT, TRUE};
use windows::Win32::Graphics::Gdi::{
    BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
    HGDIOBJ, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, IsWindow, IsWindowVisible, GetClientRect,
};

/// 枚举所有可见且有标题的顶层窗口，返回 (句柄, 标题)。
pub fn list_windows() -> Vec<(HWND, String)> {
    let mut windows: Vec<(HWND, String)> = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(enum_proc),
            LPARAM(&mut windows as *mut Vec<(HWND, String)> as isize),
        );
    }
    windows
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let windows = &mut *(lparam.0 as *mut Vec<(HWND, String)>);
    if IsWindowVisible(hwnd).as_bool() {
        let len = GetWindowTextLengthW(hwnd);
        if len > 0 {
            let mut buf = vec![0u16; (len + 1) as usize];
            let n = GetWindowTextW(hwnd, &mut buf);
            if n > 0 {
                let title = String::from_utf16_lossy(&buf[..n as usize]);
                windows.push((hwnd, title));
            }
        }
    }
    TRUE
}

/// 已定位的游戏窗口，封装客户区几何与截屏能力。
pub struct WindowCapture {
    hwnd: HWND,
}

impl WindowCapture {
    /// 通过窗口标题“包含匹配”（忽略大小写）定位窗口。
    pub fn find(title: &str) -> Result<Self> {
        let needle = title.trim().to_lowercase();
        let windows = list_windows();
        let hit = windows
            .iter()
            .find(|(_, t)| t.to_lowercase().contains(&needle));
        match hit {
            Some((hwnd, _)) => Ok(Self { hwnd: *hwnd }),
            None => {
                let candidates = windows
                    .iter()
                    .map(|(_, t)| t.as_str())
                    .collect::<Vec<_>>()
                    .join(" | ");
                Err(anyhow!("未找到包含\"{title}\"的窗口。当前可见窗口: {candidates}"))
            }
        }
    }

    /// 客户区尺寸 (宽, 高)。
    pub fn client_size(&self) -> Result<(i32, i32)> {
        let mut rect = RECT::default();
        unsafe { GetClientRect(self.hwnd, &mut rect) }?;
        Ok((rect.right - rect.left, rect.bottom - rect.top))
    }

    /// 客户区左上角在屏幕中的坐标。
    pub fn client_origin(&self) -> Result<(i32, i32)> {
        let mut p = POINT { x: 0, y: 0 };
        let ok = unsafe { ClientToScreen(self.hwnd, &mut p) };
        if !ok.as_bool() {
            return Err(anyhow!("ClientToScreen 失败"));
        }
        Ok((p.x, p.y))
    }

    /// 窗口是否仍然有效。
    pub fn is_alive(&self) -> bool {
        unsafe { IsWindow(self.hwnd) }.as_bool()
    }

    /// 截取整个客户区（用于一次性的圆形小地图检测）。
    pub fn capture_client(&self) -> Result<RgbImage> {
        let (cw, ch) = self.client_size()?;
        let (ox, oy) = self.client_origin()?;
        capture_screen_region(ox, oy, cw, ch)
    }

    /// 按已检测到的圆（客户区坐标）截取小地图，截方块并施加圆遮罩。
    pub fn capture_circle(&self, circle: &MinimapCircle) -> Result<(RgbImage, MinimapRegion)> {
        let (cw, ch) = self.client_size()?;
        let (ox, oy) = self.client_origin()?;
        let size = circle.r * 2;
        if size <= 0 {
            return Err(anyhow!("圆半径无效"));
        }
        let sx = ox + circle.cx - circle.r;
        let sy = oy + circle.cy - circle.r;
        let mut img = capture_screen_region(sx, sy, size, size)?;
        apply_circular_mask(&mut img);
        Ok((
            img,
            MinimapRegion {
                client_w: cw,
                client_h: ch,
                x: circle.cx - circle.r,
                y: circle.cy - circle.r,
                size,
            },
        ))
    }
}

/// 圆形小地图在客户区中的位置（一次性检测的结果）。
#[derive(Debug, Clone, Copy)]
pub struct MinimapCircle {
    pub cx: i32,
    pub cy: i32,
    pub r: i32,
}

/// 将正方形图像中内接圆之外的像素置黑，得到圆形小地图。
fn apply_circular_mask(img: &mut RgbImage) {
    let w = img.width() as f32;
    let h = img.height() as f32;
    let cx = (w - 1.0) * 0.5;
    let cy = (h - 1.0) * 0.5;
    let radius = (w.min(h)) * 0.5;
    let r2 = radius * radius;
    for (x, y, px) in img.enumerate_pixels_mut() {
        let dx = x as f32 - cx;
        let dy = y as f32 - cy;
        if dx * dx + dy * dy > r2 {
            *px = image::Rgb([0, 0, 0]);
        }
    }
}

/// 小地图截取区域信息（屏幕坐标），用于调试显示。
#[derive(Debug, Clone, Copy)]
pub struct MinimapRegion {
    pub client_w: i32,
    pub client_h: i32,
    pub x: i32,
    pub y: i32,
    pub size: i32,
}

/// 从屏幕指定区域抓取像素，返回 RGB 图像。
fn capture_screen_region(x: i32, y: i32, w: i32, h: i32) -> Result<RgbImage> {
    unsafe {
        let screen_dc = GetDC(HWND::default());
        if screen_dc.is_invalid() {
            return Err(anyhow!("GetDC 失败"));
        }
        let mem_dc = CreateCompatibleDC(screen_dc);
        let bitmap = CreateCompatibleBitmap(screen_dc, w, h);
        let old = SelectObject(mem_dc, HGDIOBJ(bitmap.0));

        let blt = BitBlt(mem_dc, 0, 0, w, h, screen_dc, x, y, SRCCOPY);

        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // 负值表示自上而下
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                ..Default::default()
            },
            ..Default::default()
        };

        let mut buf = vec![0u8; (w * h * 4) as usize];
        let lines = GetDIBits(
            mem_dc,
            bitmap,
            0,
            h as u32,
            Some(buf.as_mut_ptr() as *mut c_void),
            &mut info,
            DIB_RGB_COLORS,
        );

        // 清理 GDI 资源
        SelectObject(mem_dc, old);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(mem_dc);
        ReleaseDC(HWND::default(), screen_dc);

        blt.map_err(|_| anyhow!("BitBlt 失败"))?;
        if lines == 0 {
            return Err(anyhow!("GetDIBits 失败"));
        }

        // BGRA -> RGB
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for px in buf.chunks_exact(4) {
            rgb.push(px[2]);
            rgb.push(px[1]);
            rgb.push(px[0]);
        }
        RgbImage::from_raw(w as u32, h as u32, rgb).ok_or_else(|| anyhow!("图像缓冲构建失败"))
    }
}
