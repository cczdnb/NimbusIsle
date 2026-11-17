//! 常驻 CPU / 内存。大约每秒在后台采样一次，界面只读最近一次结果。
use eframe::egui::Color32;
use std::sync::Mutex;
use std::time::Duration;

/// 占用分档。颜色表示紧不紧张，CPU / RAM 这两个字负责区分是哪一项。
/// 处理器和内存的分界不同：内存常年占用一半，54% 仍算监控。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MeterBand {
    /// 低于阈值，沿用白色。
    Watch,
    /// 偏高。
    Busy,
    /// 已经很高。
    Severe,
}

/// 和芯片上 `{value:.0}%` 用同一种取整，避免数字已经进位、颜色还停在上一档。
pub(crate) fn meter_band(cpu: bool, value: f32) -> MeterBand {
    let Some(shown) = shown_percent(value) else { return MeterBand::Watch };
    let (busy, severe) = if cpu { (60, 85) } else { (80, 90) };
    if shown >= severe {
        MeterBand::Severe
    } else if shown >= busy {
        MeterBand::Busy
    } else {
        MeterBand::Watch
    }
}

fn shown_percent(value: f32) -> Option<i32> {
    if !value.is_finite() { return None; }
    format!("{:.0}", value.clamp(0.0, 100.0)).parse().ok()
}

pub(crate) struct MeterColors {
    pub tag: Color32,
    pub track: Color32,
    pub bar: Color32,
}

/// 标签底色和进度条同色。监控最淡，普通浅一档，严重再浓一档。避开电量环的绿。
pub(crate) fn meter_colors(band: MeterBand) -> MeterColors {
    match band {
        MeterBand::Watch => MeterColors {
            tag: Color32::from_white_alpha(32),
            track: Color32::from_white_alpha(46),
            bar: crate::theme::WHITE,
        },
        MeterBand::Busy => MeterColors {
            tag: Color32::from_rgba_unmultiplied(crate::theme::YELLOW.r(), crate::theme::YELLOW.g(), crate::theme::YELLOW.b(), 96),
            track: Color32::from_rgba_unmultiplied(crate::theme::YELLOW.r(), crate::theme::YELLOW.g(), crate::theme::YELLOW.b(), 64),
            bar: crate::theme::YELLOW,
        },
        MeterBand::Severe => MeterColors {
            tag: Color32::from_rgba_unmultiplied(crate::theme::RED.r(), crate::theme::RED.g(), crate::theme::RED.b(), 168),
            track: Color32::from_rgba_unmultiplied(crate::theme::RED.r(), crate::theme::RED.g(), crate::theme::RED.b(), 80),
            bar: crate::theme::RED,
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    pub cpu: f32,
    pub mem: f32,
}

static LATEST: Mutex<Option<Reading>> = Mutex::new(None);

pub fn current() -> Option<Reading> {
    ensure();
    LATEST.lock().ok().and_then(|slot| *slot)
}

fn ensure() {
    use std::sync::Once;
    static START: Once = Once::new();
    START.call_once(|| {
        std::thread::spawn(|| {
            let mut previous = None;
            loop {
                if let Some(reading) = sample(&mut previous) {
                    if let Ok(mut slot) = LATEST.lock() { *slot = Some(reading); }
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    });
}

/// 用两次系统时间的差值算占用。内核时间含空闲，忙时 = 内核 + 用户 − 空闲。
pub(crate) fn cpu_percent(previous: (u64, u64), current: (u64, u64)) -> Option<f32> {
    let idle = current.0.saturating_sub(previous.0);
    let total = current.1.saturating_sub(previous.1);
    if total == 0 { return None; }
    let busy = total.saturating_sub(idle);
    Some((100.0 * busy as f32 / total as f32).clamp(0.0, 100.0))
}

fn sample(previous: &mut Option<(u64, u64)>) -> Option<Reading> {
    #[cfg(windows)]
    {
        let times = system_times()?;
        let cpu = previous.and_then(|before| cpu_percent(before, times));
        *previous = Some(times);
        let mem = memory_load()?;
        cpu.map(|cpu| Reading { cpu, mem })
    }
    #[cfg(not(windows))]
    {
        let _ = previous;
        None
    }
}

#[cfg(windows)]
fn system_times() -> Option<(u64, u64)> {
    use windows::Win32::System::Threading::GetSystemTimes;
    unsafe {
        let mut idle = windows::Win32::Foundation::FILETIME::default();
        let mut kernel = windows::Win32::Foundation::FILETIME::default();
        let mut user = windows::Win32::Foundation::FILETIME::default();
        GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).ok()?;
        let idle = file_time(idle);
        let total = file_time(kernel).saturating_add(file_time(user));
        Some((idle, total))
    }
}

#[cfg(windows)]
fn file_time(time: windows::Win32::Foundation::FILETIME) -> u64 {
    ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64
}

#[cfg(windows)]
fn memory_load() -> Option<f32> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    unsafe {
        let mut status = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        GlobalMemoryStatusEx(&mut status).ok()?;
        Some((status.dwMemoryLoad as f32).clamp(0.0, 100.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_percent_uses_the_delta_and_ignores_a_zero_span() {
        assert_eq!(cpu_percent((0, 0), (0, 0)), None);
        assert!((cpu_percent((10, 110), (20, 210)).unwrap() - 90.0).abs() < 0.01);
        assert!((cpu_percent((0, 0), (50, 100)).unwrap() - 50.0).abs() < 0.01);
        assert_eq!(cpu_percent((0, 0), (200, 100)).unwrap(), 0.0, "空闲超过总时间时不超过 0");
    }

    #[test]
    fn meter_band_splits_cpu_and_memory_and_follows_the_printed_percent() {
        assert_eq!(meter_band(true, 0.0), MeterBand::Watch);
        assert_eq!(meter_band(true, 59.0), MeterBand::Watch);
        assert_eq!(meter_band(true, 60.0), MeterBand::Busy);
        assert_eq!(meter_band(true, 84.0), MeterBand::Busy);
        assert_eq!(meter_band(true, 85.0), MeterBand::Severe);
        assert_eq!(meter_band(true, 100.0), MeterBand::Severe);
        assert_eq!(meter_band(false, 54.0), MeterBand::Watch);
        assert_eq!(meter_band(false, 79.0), MeterBand::Watch);
        assert_eq!(meter_band(false, 80.0), MeterBand::Busy);
        assert_eq!(meter_band(false, 89.0), MeterBand::Busy);
        assert_eq!(meter_band(false, 90.0), MeterBand::Severe);
        assert_eq!(meter_band(true, f32::NAN), MeterBand::Watch);
        assert_eq!(meter_band(false, f32::INFINITY), MeterBand::Watch);
        for (cpu, value) in [(true, 59.6f32), (true, 84.5), (false, 79.5), (false, 89.5)] {
            let shown: i32 = format!("{value:.0}").parse().unwrap();
            let (busy, severe) = if cpu { (60, 85) } else { (80, 90) };
            let expect = if shown >= severe { MeterBand::Severe } else if shown >= busy { MeterBand::Busy } else { MeterBand::Watch };
            assert_eq!(meter_band(cpu, value), expect, "{value} 显示成 {shown}");
        }
        let calm = meter_colors(MeterBand::Watch);
        let busy = meter_colors(MeterBand::Busy);
        let severe = meter_colors(MeterBand::Severe);
        assert!(busy.tag.a() > calm.tag.a() && severe.tag.a() > busy.tag.a(), "标签底色逐档变浓");
        assert_ne!(busy.bar, severe.bar);
        assert_ne!(calm.bar, busy.bar);
    }
}
