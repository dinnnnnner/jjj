//! Runtime swap control for this application's Windows / Glow renderer.
use eframe::egui;

pub(crate) struct VsyncControl {
    enabled: bool,
    error: Option<String>,
    #[cfg(windows)]
    driver: Option<WglSwapControl>,
}

impl VsyncControl {
    pub(crate) fn unavailable(reason: &str) -> Self {
        Self {
            enabled: false,
            error: Some(reason.into()),
            #[cfg(windows)]
            driver: None,
        }
    }

    pub(crate) fn new(cc: &eframe::CreationContext<'_>) -> Self {
        #[cfg(windows)]
        {
            match WglSwapControl::load(cc) {
                Ok(driver) => match driver.interval() {
                    Ok(interval) => Self {
                        enabled: interval != 0,
                        error: None,
                        driver: Some(driver),
                    },
                    Err(error) => Self::unavailable(&error),
                },
                Err(error) => Self::unavailable(&error),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = cc;
            Self::unavailable("实时垂直同步开关目前支持 Windows OpenGL 后端")
        }
    }

    fn available(&self) -> bool {
        #[cfg(windows)]
        {
            self.driver.is_some()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    fn set_enabled(&mut self, enabled: bool) {
        #[cfg(windows)]
        if let Some(driver) = &self.driver {
            // The driver result, not the requested checkbox value, determines
            // the displayed state. A rejected request must not look successful.
            match driver.set_interval(i32::from(enabled)) {
                Ok(()) => {
                    self.enabled = enabled;
                    self.error = None;
                }
                Err(error) => {
                    if let Ok(interval) = driver.interval() {
                        self.enabled = interval != 0;
                    }
                    self.error = Some(error);
                }
            }
        }
        #[cfg(not(windows))]
        let _ = enabled;
    }

    pub(crate) fn ui(&mut self, ui: &mut egui::Ui) {
        let mut requested = self.enabled;
        let response = ui.add_enabled(
            self.available(),
            egui::Checkbox::new(&mut requested, "垂直同步"),
        ).on_hover_text(self.error.as_deref().unwrap_or(
            "立即切换：开启时按显示器刷新节奏提交画面；关闭时尽快重绘。实际行为仍受驱动和 Windows 影响。",
        )).on_disabled_hover_text(self.error.as_deref().unwrap_or("垂直同步不可用"));
        if response.changed() {
            self.set_enabled(requested);
        }
        if self.available()
            && let Some(error) = &self.error
        {
            ui.colored_label(egui::Color32::YELLOW, "切换失败")
                .on_hover_text(error);
        }
    }
}

#[cfg(windows)]
struct WglSwapControl {
    context: windows_sys::Win32::Graphics::OpenGL::HGLRC,
    set: unsafe extern "system" fn(i32) -> i32,
    get: unsafe extern "system" fn() -> i32,
}

#[cfg(windows)]
impl WglSwapControl {
    fn load(cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        use windows_sys::Win32::Graphics::OpenGL::wglGetCurrentContext;
        if cc.gl.is_none() {
            return Err("当前渲染后端不是 OpenGL，无法实时切换垂直同步".into());
        }
        // SAFETY: WGL's current-context query has no pointer arguments.
        let context = unsafe { wglGetCurrentContext() };
        if context.is_null() {
            return Err("当前不是 Windows WGL 上下文，无法实时切换垂直同步".into());
        }
        let load = cc.get_proc_address.ok_or("OpenGL 未提供扩展函数加载器")?;
        let set = load(c"wglSwapIntervalEXT");
        let get = load(c"wglGetSwapIntervalEXT");
        // WGL can use 1, 2, 3 or -1 as failure sentinels as well as NULL.
        let valid = |ptr: *const std::ffi::c_void| !matches!(ptr as usize, 0..=3 | usize::MAX);
        if !valid(set) || !valid(get) {
            return Err("显卡驱动不支持 WGL_EXT_swap_control，无法实时切换垂直同步".into());
        }
        Ok(Self {
            context,
            // SAFETY: these non-null driver entry points have the exact
            // WGL_EXT_swap_control ABI. eframe owns the GL library/context for
            // the application's lifetime; every call checks the current context.
            set: unsafe {
                std::mem::transmute::<*const std::ffi::c_void, unsafe extern "system" fn(i32) -> i32>(
                    set,
                )
            },
            get: unsafe {
                std::mem::transmute::<*const std::ffi::c_void, unsafe extern "system" fn() -> i32>(
                    get,
                )
            },
        })
    }

    fn check_current(&self) -> Result<(), String> {
        // SAFETY: no pointers are passed; require this application's current context.
        let current = unsafe { windows_sys::Win32::Graphics::OpenGL::wglGetCurrentContext() };
        if current.is_null() || current != self.context {
            return Err("OpenGL 上下文尚未就绪，请再次点击重试".into());
        }
        Ok(())
    }

    fn interval(&self) -> Result<i32, String> {
        self.check_current()?;
        // SAFETY: the captured WGL context is current on this calling thread.
        let interval = unsafe { (self.get)() };
        if interval < 0 {
            return Err("驱动返回了无法识别的垂直同步状态".into());
        }
        Ok(interval)
    }

    fn set_interval(&self, interval: i32) -> Result<(), String> {
        self.check_current()?;
        // SAFETY: context is current; the requested interval is 0 or 1.
        if unsafe { (self.set)(interval) } == 0 {
            return Err(format!(
                "驱动拒绝切换垂直同步：{}",
                std::io::Error::last_os_error()
            ));
        }
        if self.interval()? != interval {
            return Err("驱动未采用请求的垂直同步设置".into());
        }
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use winit::platform::windows::EventLoopBuilderExtWindows;

    #[test]
    #[ignore = "opens a temporary OpenGL window to verify real driver swap control"]
    fn native_vsync_switches_off_on_off() {
        struct Probe {
            control: VsyncControl,
            step: usize,
            result: Arc<Mutex<Vec<Result<bool, String>>>>,
        }
        impl eframe::App for Probe {
            fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
                if self.step >= 3 {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
                let requested = [false, true, false][self.step];
                self.control.set_enabled(requested);
                let result = match &self.control.error {
                    Some(error) => Err(error.clone()),
                    None => Ok(self.control.enabled),
                };
                self.result.lock().unwrap().push(result);
                self.step += 1;
                if self.step == 3 {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    ctx.request_repaint();
                }
            }
        }
        let result = Arc::new(Mutex::new(Vec::new()));
        let captured = result.clone();
        let options = eframe::NativeOptions {
            renderer: eframe::Renderer::Glow,
            vsync: false,
            viewport: egui::ViewportBuilder::default().with_inner_size([320.0, 100.0]),
            event_loop_builder: Some(Box::new(|builder| {
                builder.with_any_thread(true);
            })),
            ..Default::default()
        };
        eframe::run_native(
            "VSync driver verification",
            options,
            Box::new(move |cc| {
                Ok(Box::new(Probe {
                    control: VsyncControl::new(cc),
                    step: 0,
                    result: captured,
                }))
            }),
        )
        .unwrap();
        let observed = result.lock().unwrap();
        eprintln!("WGL swap control results: {observed:?}");
        assert_eq!(&*observed, &[Ok(false), Ok(true), Ok(false)]);
    }
}
