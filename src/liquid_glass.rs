//! 液态玻璃皮肤：抓取岛体身后的桌面，做高斯模糊、边缘折射、色散与雾度。
//!
//! 观感借鉴 NotchPeninsula 液态玻璃 2 的处理步骤（抓屏 → 可分离高斯 → 边缘折射 /
//! 色散 / 雾度 / 高光），算法在 Rust 中重写。抓屏失败或连续耗时过长时，自动退回
//! 现有深色毛玻璃的表现，不另设开关。

use eframe::egui::{Color32, ColorImage, Pos2, Rect, TextureHandle, TextureOptions};
use std::time::Instant;

/// 抓屏间隔（秒）。
const CAPTURE_GAP: f64 = 0.25;
/// 单次合成超过这个时间（毫秒）记一次「太慢」。
const SLOW_MS: u128 = 40;
/// 连续失败或连续过慢多少次后回退。
const FAIL_LIMIT: u32 = 3;
/// 回退后隔这么久再试一次（秒）。
const RETRY_GAP: f64 = 30.0;
/// 模糊与折射需要的额外边距（逻辑像素）。
pub const PAD: f32 = 36.0;
/// 合成图最长边上限，避免大岛把 CPU 吃满。
const MAX_EDGE: u32 = 480;
/// 默认模糊半径（逻辑像素）。
const BLUR_RADIUS: f32 = 7.0;
/// 边缘折射最大位移（逻辑像素）。
const REFRACT_PX: f32 = 14.0;
/// 折射带宽度（逻辑像素）。
const LENS_DEPTH: f32 = 22.0;
/// 色散强度。
const DISPERSION: f32 = 0.04;
/// 边缘高光。
const RIM: f32 = 0.18;
/// 雾度（掺白）。
const FROST: f32 = 0.10;
/// 饱和度。
const SATURATION: f32 = 1.22;
/// 明度。
const BRIGHTNESS: f32 = 1.02;

#[derive(Clone, Debug)]
pub struct ShapeSpec {
    pub logical: Rect,
    pub flare: f32,
    pub flare_h: f32,
    pub radius: f32,
    pub pill: bool,
    pub dock_left: bool,
    pub dock_right: bool,
}

#[derive(Clone, Debug)]
pub struct CaptureRequest {
    pub hwnd: isize,
    pub origin: Pos2,
    pub ppp: f32,
    pub shape: ShapeSpec,
}

/// 液态玻璃引擎：持有最近一帧合成图，决定是否回退。
pub struct Engine {
    texture: Option<TextureHandle>,
    image: Option<ColorImage>,
    image_rect: Rect,
    fallback: bool,
    fail_streak: u32,
    slow_streak: u32,
    last_try: f64,
    last_ok: f64,
    last_key: String,
    /// 最近一次给界面看的简短状态（测试与诊断用）。
    pub status: String,
    rx: Option<std::sync::mpsc::Receiver<ComposeResult>>,
    inflight: bool,
    exclude_set: bool,
}

struct ComposeResult {
    image: Result<ColorImage, String>,
    rect: Rect,
    elapsed_ms: u128,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            texture: None,
            image: None,
            image_rect: Rect::NOTHING,
            fallback: false,
            fail_streak: 0,
            slow_streak: 0,
            last_try: -1000.0,
            last_ok: -1000.0,
            last_key: String::new(),
            status: String::new(),
            rx: None,
            inflight: false,
            exclude_set: false,
        }
    }
}

impl Engine {
    pub fn new() -> Self { Self::default() }

    pub fn fallback(&self) -> bool { self.fallback }

    pub fn has_image(&self) -> bool { self.image.is_some() }

    pub fn live(&self) -> bool { !self.fallback && self.image.is_some() }

    pub fn exclude_from_capture(&self) -> bool { self.live() || (!self.fallback && self.inflight) }

    /// 每帧调用：在需要时发起抓屏，并收下后台结果。
    pub fn tick(&mut self, ctx: &eframe::egui::Context, t: f64, req: Option<&CaptureRequest>) {
        self.poll();
        if let Some(image) = &self.image {
            if self.texture.is_none() {
                self.texture = Some(ctx.load_texture("liquid-glass", image.clone(), TextureOptions::LINEAR));
            }
        }
        let Some(req) = req else { return; };
        if self.fallback {
            if t - self.last_try >= RETRY_GAP {
                self.fallback = false;
                self.fail_streak = 0;
                self.slow_streak = 0;
                self.status = "重试液态玻璃".into();
            } else {
                return;
            }
        }
        let key = shape_key(&req.shape);
        let due = t - self.last_try >= CAPTURE_GAP || key != self.last_key;
        if !due || self.inflight { return; }
        self.last_try = t;
        self.last_key = key;
        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        self.inflight = true;
        let request = req.clone();
        std::thread::Builder::new()
            .name("daozai-liquid-glass".into())
            .spawn(move || {
                let start = Instant::now();
                let result = compose_from_request(&request);
                let _ = tx.send(ComposeResult {
                    image: result,
                    rect: request.shape.logical,
                    elapsed_ms: start.elapsed().as_millis(),
                });
            })
            .ok();
    }

    fn poll(&mut self) {
        let Some(rx) = &self.rx else { return; };
        match rx.try_recv() {
            Ok(result) => {
                self.inflight = false;
                self.rx = None;
                match result.image {
                    Ok(image) => {
                        self.fail_streak = 0;
                        self.last_ok = self.last_try;
                        if result.elapsed_ms > SLOW_MS {
                            self.slow_streak = self.slow_streak.saturating_add(1);
                            if self.slow_streak >= FAIL_LIMIT {
                                self.enter_fallback(format!("合成偏慢（{} ms），已改用深色毛玻璃", result.elapsed_ms));
                                return;
                            }
                        } else {
                            self.slow_streak = 0;
                        }
                        self.image_rect = result.rect;
                        if let Some(tex) = &mut self.texture {
                            tex.set(image.clone(), TextureOptions::LINEAR);
                        }
                        self.image = Some(image);
                        self.status = format!("液态玻璃就绪（{} ms）", result.elapsed_ms);
                    }
                    Err(why) => {
                        self.fail_streak = self.fail_streak.saturating_add(1);
                        self.status = why.clone();
                        if self.image.is_none() || self.fail_streak >= FAIL_LIMIT {
                            self.enter_fallback(why);
                        }
                    }
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.inflight = false;
                self.rx = None;
                self.fail_streak = self.fail_streak.saturating_add(1);
                if self.image.is_none() || self.fail_streak >= FAIL_LIMIT {
                    self.enter_fallback("抓屏线程已结束".into());
                }
            }
        }
    }

    fn enter_fallback(&mut self, why: String) {
        self.fallback = true;
        self.inflight = false;
        self.image = None;
        self.texture = None;
        self.status = format!("已回退深色毛玻璃：{why}");
    }

    /// 在岛体网格上取样合成图。坐标与 `shape.logical` 一致。
    pub fn color_at(&self, x: f32, y: f32) -> Color32 {
        let Some(image) = &self.image else {
            return Color32::from_rgba_unmultiplied(0x1C, 0x1C, 0x1E, 215);
        };
        let r = self.image_rect;
        if r.width() < 1.0 || r.height() < 1.0 { return Color32::TRANSPARENT; }
        let u = ((x - r.left()) / r.width()).clamp(0.0, 1.0);
        let v = ((y - r.top()) / r.height()).clamp(0.0, 1.0);
        let w = image.width() as f32;
        let h = image.height() as f32;
        sample_image(image, u * (w - 1.0).max(0.0), v * (h - 1.0).max(0.0))
    }
}

fn shape_key(s: &ShapeSpec) -> String {
    format!(
        "{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{},{},{}",
        s.logical.min.x, s.logical.min.y, s.logical.width(), s.logical.height(),
        s.flare, s.flare_h, s.radius, s.pill as u8, s.dock_left as u8, s.dock_right as u8
    )
}

fn compose_from_request(req: &CaptureRequest) -> Result<ColorImage, String> {
    let ppp = req.ppp.max(0.5);
    let logical = req.shape.logical;
    if !logical.is_finite() || logical.width() < 2.0 || logical.height() < 2.0 {
        return Err("岛体尺寸无效".into());
    }
    let pad = PAD;
    let capture = Rect::from_min_max(
        Pos2::new(logical.left() - pad, logical.top() - pad),
        Pos2::new(logical.right() + pad, logical.bottom() + pad),
    );
    // 屏幕物理像素：窗口逻辑原点 + 区域。
    let sx = ((req.origin.x + capture.left()) * ppp).round() as i32;
    let sy = ((req.origin.y + capture.top()) * ppp).round() as i32;
    let sw = (capture.width() * ppp).round().max(2.0) as i32;
    let sh = (capture.height() * ppp).round().max(2.0) as i32;

    let mut raw = capture_desktop(req.hwnd, sx, sy, sw, sh)?;
    if raw.width as i32 != sw || raw.height as i32 != sh {
        return Err("抓屏尺寸不匹配".into());
    }
    // 物理像素上的模糊半径。
    let radius = (BLUR_RADIUS * ppp).round().max(1.0) as usize;
    let sigma = (radius as f32) * 0.45 + 0.6;
    blur_bgra(&mut raw, radius, sigma);

    let out_w = ((logical.width() * ppp).round() as u32).clamp(2, MAX_EDGE);
    let out_h = ((logical.height() * ppp).round() as u32).clamp(2, MAX_EDGE);
    // 保持纵横比，不超出上限。
    let scale = (out_w as f32 / (logical.width() * ppp)).min(out_h as f32 / (logical.height() * ppp)).min(1.0);
    let gw = ((logical.width() * ppp) * scale).round().max(2.0) as u32;
    let gh = ((logical.height() * ppp) * scale).round().max(2.0) as u32;

    Ok(refract_compose(&raw, &req.shape, capture, gw, gh, ppp))
}

/// 抓屏得到的 BGRA 缓冲（行优先，每像素 4 字节）。
pub struct RgbaBuf {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>, // BGRA
}

impl RgbaBuf {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, data: vec![0; width as usize * height as usize * 4] }
    }
}

fn capture_desktop(hwnd: isize, x: i32, y: i32, w: i32, h: i32) -> Result<RgbaBuf, String> {
    #[cfg(windows)]
    {
        if let Ok(buf) = capture_dxgi(hwnd, x, y, w, h) {
            return Ok(buf);
        }
        capture_gdi(x, y, w, h)
    }
    #[cfg(not(windows))]
    {
        let _ = (hwnd, x, y, w, h);
        Err("当前系统无法抓取桌面".into())
    }
}

#[cfg(windows)]
fn capture_gdi(x: i32, y: i32, w: i32, h: i32) -> Result<RgbaBuf, String> {
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::Foundation::HWND;
    unsafe {
        let hdc = GetDC(HWND::default());
        if hdc.is_invalid() { return Err("无法打开屏幕设备上下文".into()); }
        struct Release(windows::Win32::Graphics::Gdi::HDC);
        impl Drop for Release { fn drop(&mut self) { unsafe { let _ = ReleaseDC(HWND::default(), self.0); } } }
        let _guard = Release(hdc);
        let mem = CreateCompatibleDC(hdc);
        if mem.is_invalid() { return Err("无法创建兼容设备上下文".into()); }
        struct Dc(windows::Win32::Graphics::Gdi::HDC);
        impl Drop for Dc { fn drop(&mut self) { unsafe { let _ = DeleteDC(self.0); } } }
        let _mem = Dc(mem);
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        if bmp.is_invalid() { return Err("无法创建抓屏位图".into()); }
        struct Bmp(windows::Win32::Graphics::Gdi::HBITMAP);
        impl Drop for Bmp { fn drop(&mut self) { unsafe { let _ = DeleteObject(HGDIOBJ(self.0.0)); } } }
        let _bmp = Bmp(bmp);
        let old = SelectObject(mem, HGDIOBJ(bmp.0));
        let ok = BitBlt(mem, 0, 0, w, h, hdc, x, y, SRCCOPY);
        SelectObject(mem, old);
        if ok.is_err() { return Err("抓取桌面失败".into()); }

        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = RgbaBuf::new(w as u32, h as u32);
        let lines = GetDIBits(mem, bmp, 0, h as u32, Some(buf.data.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
        if lines == 0 { return Err("读取抓屏像素失败".into()); }
        Ok(buf)
    }
}

#[cfg(windows)]
fn capture_dxgi(_hwnd: isize, x: i32, y: i32, w: i32, h: i32) -> Result<RgbaBuf, String> {
    use windows::core::Interface;
    use windows::Win32::Graphics::Direct3D::*;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::Win32::Graphics::Dxgi::*;
    use windows::Win32::Foundation::HMODULE;

    unsafe {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        ).map_err(|e| format!("创建 Direct3D 设备失败：{e}"))?;
        let device = device.ok_or_else(|| "Direct3D 设备为空".to_string())?;
        let context = context.ok_or_else(|| "Direct3D 上下文为空".to_string())?;
        let dxgi: IDXGIDevice = device.cast().map_err(|e| format!("DXGI 设备失败：{e}"))?;
        let adapter = dxgi.GetAdapter().map_err(|e| format!("无法取得显卡适配器：{e}"))?;

        // 在各输出中找包含抓取区域中心的那一块。
        let cx = x + w / 2;
        let cy = y + h / 2;
        let mut output_index = 0u32;
        let mut found: Option<(IDXGIOutput1, i32, i32, i32, i32)> = None;
        loop {
            let output = match adapter.EnumOutputs(output_index) {
                Ok(o) => o,
                Err(_) => break,
            };
            let desc = output.GetDesc().map_err(|e| format!("读取屏幕信息失败：{e}"))?;
            let r = desc.DesktopCoordinates;
            if cx >= r.left && cx < r.right && cy >= r.top && cy < r.bottom {
                let out1: IDXGIOutput1 = output.cast().map_err(|e| format!("屏幕输出接口失败：{e}"))?;
                found = Some((out1, r.left, r.top, r.right - r.left, r.bottom - r.top));
                break;
            }
            output_index += 1;
        }
        let (output, ox, oy, ow, oh) = found.ok_or_else(|| "找不到对应屏幕".to_string())?;
        let dup = output.DuplicateOutput(&device).map_err(|_| "本机桌面复制不可用".to_string())?;

        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        match dup.AcquireNextFrame(80, &mut frame_info, &mut resource) {
            Ok(()) => {}
            Err(e) => {
                let code = e.code();
                if code == DXGI_ERROR_WAIT_TIMEOUT {
                    return Err("桌面复制暂时没有新帧".into());
                }
                if code == DXGI_ERROR_ACCESS_LOST || code == DXGI_ERROR_ACCESS_DENIED {
                    return Err("桌面复制被系统占用".into());
                }
                return Err(format!("桌面复制失败：{e}"));
            }
        }
        struct ReleaseFrame<'a>(&'a IDXGIOutputDuplication);
        impl Drop for ReleaseFrame<'_> { fn drop(&mut self) { unsafe { let _ = self.0.ReleaseFrame(); } } }
        let _release = ReleaseFrame(&dup);
        let resource = resource.ok_or_else(|| "桌面复制帧为空".to_string())?;
        let src: ID3D11Texture2D = resource.cast().map_err(|e| format!("复制帧转换失败：{e}"))?;

        // 裁到抓取矩形，并限制在该输出范围内。
        let left = (x - ox).clamp(0, ow - 1);
        let top = (y - oy).clamp(0, oh - 1);
        let right = (x - ox + w).clamp(left + 1, ow);
        let bottom = (y - oy + h).clamp(top + 1, oh);
        let cw = (right - left) as u32;
        let ch = (bottom - top) as u32;

        let desc = D3D11_TEXTURE2D_DESC {
            Width: cw,
            Height: ch,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        device.CreateTexture2D(&desc, None, Some(&mut staging)).map_err(|e| format!("创建暂存纹理失败：{e}"))?;
        let staging = staging.ok_or_else(|| "暂存纹理为空".to_string())?;
        let box_ = D3D11_BOX {
            left: left as u32,
            top: top as u32,
            front: 0,
            right: right as u32,
            bottom: bottom as u32,
            back: 1,
        };
        context.CopySubresourceRegion(&staging, 0, 0, 0, 0, &src, 0, Some(&box_));

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).map_err(|e| format!("读取复制帧失败：{e}"))?;
        let mut buf = RgbaBuf::new(w as u32, h as u32);
        let src_pitch = mapped.RowPitch as usize;
        let dst_pitch = (w as usize) * 4;
        let src_ptr = mapped.pData as *const u8;
        // 若裁剪小于请求（靠近屏幕边缘），先填黑再拷可见部分。
        let copy_w = cw as usize;
        let copy_h = ch as usize;
        let dst_ox = (left - (x - ox)).max(0) as usize;
        let dst_oy = (top - (y - oy)).max(0) as usize;
        for row in 0..copy_h {
            let src_row = std::slice::from_raw_parts(src_ptr.add(row * src_pitch), copy_w * 4);
            let dst_row = (dst_oy + row) * dst_pitch + dst_ox * 4;
            if dst_row + copy_w * 4 <= buf.data.len() {
                buf.data[dst_row..dst_row + copy_w * 4].copy_from_slice(src_row);
            }
        }
        context.Unmap(&staging, 0);
        // 若实际裁出尺寸与请求一致，直接返回；否则已按偏移写入。
        let _ = (ow, oh);
        Ok(buf)
    }
}

/// 可分离高斯模糊（就地，BGRA）。`weights` 为半径到 0 的半核。
pub fn gaussian_weights(sigma: f32, radius: usize) -> Vec<f32> {
    let mut w = vec![0.0; radius + 1];
    let coeff = 1.0 / (2.0 * sigma.max(0.01) * sigma.max(0.01));
    w[0] = 1.0;
    let mut sum = w[0];
    for i in 1..=radius {
        let x = i as f32;
        w[i] = (-x * x * coeff).exp();
        sum += 2.0 * w[i];
    }
    for v in &mut w { *v /= sum; }
    w
}

pub fn blur_bgra(buf: &mut RgbaBuf, radius: usize, sigma: f32) {
    if radius == 0 || buf.width < 2 || buf.height < 2 { return; }
    let weights = gaussian_weights(sigma, radius);
    let w = buf.width as usize;
    let h = buf.height as usize;
    let mut tmp = vec![0u8; buf.data.len()];
    // 水平
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 4];
            for (i, &wt) in weights.iter().enumerate() {
                let samples = if i == 0 { [x] } else { /* both sides */ [0; 1] };
                let _ = samples;
                let apply = |xx: i32, wt: f32, acc: &mut [f32; 4]| {
                    let xx = xx.clamp(0, w as i32 - 1) as usize;
                    let o = (y * w + xx) * 4;
                    acc[0] += buf.data[o] as f32 * wt;
                    acc[1] += buf.data[o + 1] as f32 * wt;
                    acc[2] += buf.data[o + 2] as f32 * wt;
                    acc[3] += buf.data[o + 3] as f32 * wt;
                };
                if i == 0 {
                    apply(x as i32, wt, &mut acc);
                } else {
                    apply(x as i32 - i as i32, wt, &mut acc);
                    apply(x as i32 + i as i32, wt, &mut acc);
                }
            }
            let o = (y * w + x) * 4;
            tmp[o] = acc[0].round().clamp(0.0, 255.0) as u8;
            tmp[o + 1] = acc[1].round().clamp(0.0, 255.0) as u8;
            tmp[o + 2] = acc[2].round().clamp(0.0, 255.0) as u8;
            tmp[o + 3] = acc[3].round().clamp(0.0, 255.0) as u8;
        }
    }
    // 垂直
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 4];
            for (i, &wt) in weights.iter().enumerate() {
                let apply = |yy: i32, wt: f32, acc: &mut [f32; 4]| {
                    let yy = yy.clamp(0, h as i32 - 1) as usize;
                    let o = (yy * w + x) * 4;
                    acc[0] += tmp[o] as f32 * wt;
                    acc[1] += tmp[o + 1] as f32 * wt;
                    acc[2] += tmp[o + 2] as f32 * wt;
                    acc[3] += tmp[o + 3] as f32 * wt;
                };
                if i == 0 {
                    apply(y as i32, wt, &mut acc);
                } else {
                    apply(y as i32 - i as i32, wt, &mut acc);
                    apply(y as i32 + i as i32, wt, &mut acc);
                }
            }
            let o = (y * w + x) * 4;
            buf.data[o] = acc[0].round().clamp(0.0, 255.0) as u8;
            buf.data[o + 1] = acc[1].round().clamp(0.0, 255.0) as u8;
            buf.data[o + 2] = acc[2].round().clamp(0.0, 255.0) as u8;
            buf.data[o + 3] = acc[3].round().clamp(0.0, 255.0) as u8;
        }
    }
}

fn bilinear_bgra(buf: &RgbaBuf, x: f32, y: f32) -> [f32; 3] {
    let w = buf.width as i32;
    let h = buf.height as i32;
    if w <= 0 || h <= 0 { return [0.0; 3]; }
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let pix = |xx: i32, yy: i32| {
        let o = ((yy as usize) * (w as usize) + xx as usize) * 4;
        [buf.data[o] as f32, buf.data[o + 1] as f32, buf.data[o + 2] as f32] // B,G,R
    };
    let a = pix(x0, y0);
    let b = pix(x1, y0);
    let c = pix(x0, y1);
    let d = pix(x1, y1);
    let mut out = [0.0; 3];
    for i in 0..3 {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bot = c[i] + (d[i] - c[i]) * fx;
        out[i] = top + (bot - top) * fy;
    }
    out
}

fn saturate_brightness(bgr: [f32; 3], sat: f32, bright: f32) -> [f32; 3] {
    let gray = bgr[0] * 0.114 + bgr[1] * 0.587 + bgr[2] * 0.299;
    [
        ((gray + (bgr[0] - gray) * sat) * bright).clamp(0.0, 255.0),
        ((gray + (bgr[1] - gray) * sat) * bright).clamp(0.0, 255.0),
        ((gray + (bgr[2] - gray) * sat) * bright).clamp(0.0, 255.0),
    ]
}

/// 圆角矩形 / 胶囊的有符号距离（正值在内部），以及指向外侧的法线。
pub fn sdf_pill(px: f32, py: f32, w: f32, h: f32, radius: f32) -> (f32, f32, f32) {
    let cx = w * 0.5;
    let cy = h * 0.5;
    let r = radius.min(w * 0.5).min(h * 0.5).max(0.0);
    let hx = (w * 0.5 - r).max(0.0);
    let hy = (h * 0.5 - r).max(0.0);
    let dx = (px - cx).abs() - hx;
    let dy = (py - cy).abs() - hy;
    let ox = dx.max(0.0);
    let oy = dy.max(0.0);
    let outside = (ox * ox + oy * oy).sqrt();
    let inside = dx.max(dy).min(0.0);
    let dist_out = outside + inside; // 负值在内（到圆角矩形外沿）
    let inner = r - dist_out; // 正值在内，到边缘的距离
    // 法线：从中心指向外
    let mut nx = px - cx;
    let mut ny = py - cy;
    let len = (nx * nx + ny * ny).sqrt();
    if len > 0.0001 { nx /= len; ny /= len; } else { nx = 0.0; ny = -1.0; }
    (inner, nx, ny)
}

/// 刘海：顶部贴边的圆角矩形，两侧有肩部（flare）。用圆角盒近似，肩部把上沿加宽。
pub fn sdf_notch(px: f32, py: f32, w: f32, h: f32, radius: f32, flare: f32, flare_h: f32) -> (f32, f32, f32) {
    // 主体：下沿圆角、上沿贴边（顶部半径视为 0，左右下为 radius）。
    // 肩部：在顶部 flare_h 高度内，宽度向外扩展 flare。
    let t = if flare_h > 0.5 { (1.0 - (py / flare_h).clamp(0.0, 1.0)).powf(1.6) } else { 0.0 };
    let extra = flare * t;
    let left = -extra;
    let right = w + extra;
    let ww = right - left;
    let px2 = px - left;
    // 顶部不圆角：把盒子向上延伸再裁切 y>=0。
    let (d, nx, ny) = sdf_pill(px2, py + radius, ww, h + radius, radius);
    // 贴顶：若 py 接近 0，距离还要考虑上沿。
    let dist_top = py;
    let dist = if py < radius { d.min(dist_top + h) } else { d };
    let _ = dist_top;
    (dist, nx, ny)
}

fn edge_of(shape: &ShapeSpec, px: f32, py: f32) -> (f32, f32, f32) {
    let w = shape.logical.width();
    let h = shape.logical.height();
    if shape.pill {
        sdf_pill(px, py, w, h, shape.radius)
    } else {
        sdf_notch(px, py, w, h, shape.radius, shape.flare, shape.flare_h)
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    if b <= a { return if x >= b { 1.0 } else { 0.0 }; }
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn refract_compose(
    blurred: &RgbaBuf,
    shape: &ShapeSpec,
    capture: Rect,
    gw: u32,
    gh: u32,
    ppp: f32,
) -> ColorImage {
    let mut pixels = vec![Color32::TRANSPARENT; gw as usize * gh as usize];
    let logical = shape.logical;
    let refract = REFRACT_PX * ppp;
    let lens = LENS_DEPTH * ppp;
    for y in 0..gh {
        for x in 0..gw {
            let lx = (x as f32 + 0.5) / gw as f32 * logical.width();
            let ly = (y as f32 + 0.5) / gh as f32 * logical.height();
            let (dist, nx, ny) = edge_of(shape, lx, ly);
            if dist < -2.0 * ppp {
                continue; // 完全在外
            }
            let alpha = smoothstep(-1.0 * ppp, 0.6 * ppp, dist);
            if alpha <= 0.002 { continue; }
            let m = 1.0 - smoothstep(0.0, lens, dist.max(0.0)); // 1 在边缘
            let shift = refract * m * m;
            // 取样点：逻辑坐标 → 抓取缓冲像素。
            let world_x = logical.left() + lx;
            let world_y = logical.top() + ly;
            let base_x = (world_x - capture.left()) / capture.width() * (blurred.width as f32 - 1.0);
            let base_y = (world_y - capture.top()) / capture.height() * (blurred.height as f32 - 1.0);
            // 法线位移按抓取缓冲像素计。
            let sx = nx * shift * (blurred.width as f32 / capture.width());
            let sy = ny * shift * (blurred.height as f32 / capture.height());
            let disp = 1.0 + DISPERSION;
            let b = bilinear_bgra(blurred, base_x + sx * (2.0 - disp), base_y + sy * (2.0 - disp)); // B 位移略小
            let g = bilinear_bgra(blurred, base_x + sx, base_y + sy);
            let r = bilinear_bgra(blurred, base_x + sx * disp, base_y + sy * disp);
            // 组装为 B,G,R 通道各自取样
            let mut bgr = [b[0], g[1], r[2]];
            bgr = saturate_brightness(bgr, SATURATION, BRIGHTNESS);
            // 雾度
            let frost = FROST * (0.45 + 0.55 * m);
            for c in &mut bgr { *c = *c * (1.0 - frost) + 255.0 * frost; }
            // 高光：左上光，边缘更亮
            let facing = (0.55 - 0.45 * nx + 0.35 * (-ny)).clamp(0.0, 1.0);
            let rim = RIM * m * facing * 255.0;
            bgr[0] = (bgr[0] + rim).clamp(0.0, 255.0);
            bgr[1] = (bgr[1] + rim).clamp(0.0, 255.0);
            bgr[2] = (bgr[2] + rim * 1.05).clamp(0.0, 255.0);
            let a = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
            // Color32 是 sRGBA，bgr 是 B,G,R
            pixels[(y * gw + x) as usize] = Color32::from_rgba_unmultiplied(
                bgr[2].round().clamp(0.0, 255.0) as u8,
                bgr[1].round().clamp(0.0, 255.0) as u8,
                bgr[0].round().clamp(0.0, 255.0) as u8,
                a,
            );
        }
    }
    ColorImage { size: [gw as usize, gh as usize], pixels }
}

fn sample_image(image: &ColorImage, x: f32, y: f32) -> Color32 {
    let w = image.width();
    let h = image.height();
    if w == 0 || h == 0 { return Color32::TRANSPARENT; }
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let c = |xx: usize, yy: usize| image.pixels[yy * w + xx].to_srgba_unmultiplied();
    let a = c(x0, y0);
    let b = c(x1, y0);
    let d = c(x0, y1);
    let e = c(x1, y1);
    let lerp = |p: [u8; 4], q: [u8; 4], t: f32| -> [u8; 4] {
        std::array::from_fn(|i| (p[i] as f32 + (q[i] as f32 - p[i] as f32) * t).round().clamp(0.0, 255.0) as u8)
    };
    let top = lerp(a, b, fx);
    let bot = lerp(d, e, fx);
    let out = lerp(top, bot, fy);
    Color32::from_rgba_unmultiplied(out[0], out[1], out[2], out[3])
}


/// 把岛体窗口排除出系统抓屏（DXGI 桌面复制会看到身后的桌面）。
pub fn apply_exclusion(hwnd: isize, on: bool) {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WINDOW_DISPLAY_AFFINITY};
        if hwnd == 0 { return; }
        let aff = WINDOW_DISPLAY_AFFINITY(if on { 0x11 } else { 0 }); // WDA_EXCLUDEFROMCAPTURE / WDA_NONE
        unsafe { let _ = SetWindowDisplayAffinity(HWND(hwnd as *mut _), aff); }
    }
    #[cfg(not(windows))]
    { let _ = (hwnd, on); }
}

/// 给原生层用：当前是否应把窗口排除出系统抓屏。
pub fn should_exclude(engine: &Engine, liquid: bool) -> bool {
    liquid && !engine.fallback()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaussian_weights_are_normalized_and_peak_at_center() {
        let w = gaussian_weights(2.0, 6);
        let sum = w[0] + 2.0 * w.iter().skip(1).sum::<f32>();
        assert!((sum - 1.0).abs() < 1e-5, "{sum}");
        assert!(w[0] > w[1] && w[1] > w[5]);
    }

    #[test]
    fn blur_softens_a_hard_edge() {
        let mut buf = RgbaBuf::new(16, 8);
        for y in 0..8 {
            for x in 0..16 {
                let o = (y * 16 + x) * 4;
                let v = if x < 8 { 0 } else { 255 };
                buf.data[o] = v; buf.data[o+1] = v; buf.data[o+2] = v; buf.data[o+3] = 255;
            }
        }
        blur_bgra(&mut buf, 3, 1.6);
        let mid = (4 * 16 + 8) * 4;
        let g = buf.data[mid + 1];
        assert!(g > 20 && g < 240, "边缘应变软，实际 {g}");
    }

    #[test]
    fn pill_sdf_is_positive_inside_and_negative_outside() {
        let (d, _, _) = sdf_pill(40.0, 15.0, 80.0, 30.0, 15.0);
        assert!(d > 5.0, "中心应远离边缘：{d}");
        let (d, _, _) = sdf_pill(-4.0, 15.0, 80.0, 30.0, 15.0);
        assert!(d < 0.0, "外侧应为负：{d}");
    }

    #[test]
    fn compose_keeps_center_tinted_and_edge_brighter() {
        let mut buf = RgbaBuf::new(80, 50);
        for px in buf.data.chunks_mut(4) {
            px[0] = 40; px[1] = 80; px[2] = 160; px[3] = 255; // BGRA 偏蓝
        }
        let shape = ShapeSpec {
            logical: Rect::from_min_size(Pos2::new(10.0, 10.0), eframe::egui::vec2(40.0, 20.0)),
            flare: 8.0, flare_h: 6.0, radius: 10.0, pill: true,
            dock_left: false, dock_right: false,
        };
        let capture = Rect::from_min_size(Pos2::new(0.0, 0.0), eframe::egui::vec2(60.0, 40.0));
        let img = refract_compose(&buf, &shape, capture, 40, 20, 1.0);
        let center = img.pixels[(10 * 40 + 20) as usize].to_srgba_unmultiplied();
        assert!(center[3] > 200, "中心应不透明：{center:?}");
        assert!(center[2] > 30, "应保留背景色倾向：{center:?}");
        // 边缘像素有高光，一般比正中心更亮
        let edge = img.pixels[(10 * 40 + 2) as usize].to_srgba_unmultiplied();
        let luma = |c: [u8;4]| c[0] as i32 + c[1] as i32 + c[2] as i32;
        if edge[3] > 80 {
            assert!(luma(edge) + 5 >= luma(center) - 80, "边缘应有高光倾向 center={center:?} edge={edge:?}");
        }
    }

    #[test]
    fn engine_starts_without_fallback_until_failures() {
        let e = Engine::new();
        assert!(!e.fallback());
        assert!(!e.live());
    }
}
