//! El daemon: escucha el socket y dibuja la tarjeta en una superficie de
//! Wayland (layer-shell, espacio de nombres "brasa"), en la capa de encima de
//! todo y sin agarrar clics. Mientras llegan avisos se queda; 1.5 s después
//! del último se va (la superficie se destruye y Hyprland la anima al irse).

use crate::card::{COLS, card};
use crate::{Kind, Show, socket};
use disco::look::Look;
use disco::paint::Painter;
use smithay_client_toolkit::reexports::calloop::EventLoop;
use smithay_client_toolkit::reexports::calloop::channel::{Event, channel};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::{wl_output, wl_shm, wl_surface};
use smithay_client_toolkit::reexports::client::{Connection, QueueHandle};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData, Region},
    delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use std::os::unix::net::UnixDatagram;
use std::time::{Duration, Instant};

/// Cuánto se queda después del último cambio.
const HOLD: Duration = Duration::from_millis(1500);
/// Lo que tarda la barra en llegar al valor nuevo.
const SLIDE: f32 = 0.14;
/// Separación con la orilla de la pantalla, en pixeles.
const MARGIN: i32 = 60;

/// Lo que está en pantalla.
struct Osd {
    kind: Kind,
    level: Option<u32>,
    title: String,
    /// la barra se desliza de `from` a `to` desde `t0`
    from: f64,
    to: f64,
    t0: Instant,
    until: Instant,
}

impl Osd {
    fn frac(&self) -> f64 {
        let t = (self.t0.elapsed().as_secs_f32() / SLIDE).min(1.0);
        let e = 1.0 - (1.0 - t).powi(3);
        self.from + (self.to - self.from) * e as f64
    }
    fn sliding(&self) -> bool {
        self.t0.elapsed().as_secs_f32() < SLIDE
    }
}

struct App {
    registry: RegistryState,
    output: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    qh: QueueHandle<App>,
    pool: Option<SlotPool>,
    layer: Option<LayerSurface>,
    configured: bool,
    frame_pending: bool,
    scale: i32,
    look: Look,
    painter: Painter,
    osd: Option<Osd>,
}

fn position() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    std::fs::read(format!("{home}/.config/brasa/config.json"))
        .ok()
        .and_then(|d| serde_json::from_slice::<serde_json::Value>(&d).ok())
        .and_then(|v| v["position"].as_str().map(str::to_string))
        .unwrap_or_else(|| "bottom-center".into())
}

impl App {
    fn show(&mut self, s: Show) {
        let to = s.level.map_or(0.0, |l| (l as f64 / 100.0).min(1.0));
        let now = Instant::now();
        let from = match &self.osd {
            Some(o) if o.kind == s.kind => o.frac(),
            _ => to,
        };
        self.osd = Some(Osd { kind: s.kind, level: s.level, title: s.title, from, to, t0: now, until: now + HOLD });
        if self.layer.is_none() {
            // otro look desde la última vez: colores y fuente al día
            if self.look.refresh() {
                self.painter = Painter::new();
            }
            let (w, h) = self.painter.size(COLS, 3);
            let surface = self.compositor.create_surface(&self.qh);
            if let Ok(region) = Region::new(&self.compositor) {
                surface.set_input_region(Some(region.wl_region()));
            }
            let layer = self.layer_shell.create_layer_surface(&self.qh, surface, Layer::Overlay, Some("brasa"), None);
            let pos = position();
            let (vert, side) = pos.split_once('-').unwrap_or(("bottom", "center"));
            let mut anchor = if vert == "top" { Anchor::TOP } else { Anchor::BOTTOM };
            match side {
                "left" => anchor |= Anchor::LEFT,
                "right" => anchor |= Anchor::RIGHT,
                _ => {}
            }
            layer.set_anchor(anchor);
            let m = |on: bool| if on { MARGIN } else { 0 };
            layer.set_margin(m(vert == "top"), m(side == "right"), m(vert != "top"), m(side == "left"));
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            layer.set_size(w, h);
            layer.commit();
            self.layer = Some(layer);
            self.configured = false;
            return;
        }
        if self.configured && !self.frame_pending {
            self.draw();
        }
    }

    fn hide(&mut self) {
        self.osd = None;
        self.layer = None;
        self.pool = None;
        self.configured = false;
        self.frame_pending = false;
    }

    fn draw(&mut self) {
        let (Some(layer), Some(osd)) = (self.layer.clone(), &self.osd) else { return };
        let s = self.scale.max(1);
        let (lw, lh) = self.painter.size(COLS, 3);
        let (w, h) = (lw as i32 * s, lh as i32 * s);
        let Some(mut pix) = tiny_skia::Pixmap::new(w as u32, h as u32) else { return };
        let buf = card(osd.kind, osd.level, &osd.title, osd.frac(), &self.look);
        self.painter.card(&mut pix, &buf, &self.look, 0.0, 0.0, s as f32);
        let sliding = osd.sliding();

        let pool = match &mut self.pool {
            Some(p) => p,
            None => self.pool.insert(SlotPool::new((w * h * 4) as usize, &self.shm).expect("memoria compartida")),
        };
        let Ok((buffer, canvas)) = pool.create_buffer(w, h, w * 4, wl_shm::Format::Argb8888) else { return };
        // tiny-skia: RGBA premultiplicado; Wayland ARGB8888 en little endian: BGRA
        for (dst, src) in canvas.chunks_exact_mut(4).zip(pix.data().chunks_exact(4)) {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
            dst[3] = src[3];
        }
        let surface = layer.wl_surface();
        surface.set_buffer_scale(s);
        surface.damage_buffer(0, 0, w, h);
        // mientras la barra se desliza, un cuadro por refresco
        if sliding {
            surface.frame(&self.qh, FrameCallbackData(surface.clone()));
            self.frame_pending = true;
        }
        if buffer.attach_to(surface).is_ok() {
            layer.commit();
        }
    }
}

pub fn run() -> Result<(), String> {
    // el socket: uno solo a la vez; si ya hay quien escuche, este sobra
    let path = socket();
    if UnixDatagram::unbound().and_then(|s| s.send_to(b"", &path)).is_ok() {
        return Ok(());
    }
    let _ = std::fs::remove_file(&path);
    let sock = UnixDatagram::bind(&path).map_err(|e| format!("socket: {e}"))?;

    let conn = Connection::connect_to_env().map_err(|e| format!("Wayland: {e}"))?;
    let (globals, queue) = registry_queue_init(&conn).map_err(|e| format!("Wayland: {e}"))?;
    let qh = queue.handle();
    let mut event_loop: EventLoop<App> = EventLoop::try_new().map_err(|e| e.to_string())?;
    let handle = event_loop.handle();
    WaylandSource::new(conn.clone(), queue).insert(handle.clone()).map_err(|e| e.to_string())?;

    let (tx, rx) = channel::<Show>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 512];
        while let Ok(n) = sock.recv(&mut buf) {
            if let Some(show) = std::str::from_utf8(&buf[..n]).ok().and_then(Show::parse)
                && tx.send(show).is_err()
            {
                return;
            }
        }
    });
    handle
        .insert_source(rx, |ev, _, app: &mut App| {
            if let Event::Msg(show) = ev {
                app.show(show);
            }
        })
        .map_err(|e| e.to_string())?;

    let mut app = App {
        registry: RegistryState::new(&globals),
        output: OutputState::new(&globals, &qh),
        compositor: CompositorState::bind(&globals, &qh).map_err(|_| "el compositor no tiene wl_compositor")?,
        layer_shell: LayerShell::bind(&globals, &qh).map_err(|_| "el compositor no tiene layer-shell")?,
        shm: Shm::bind(&globals, &qh).map_err(|_| "el compositor no tiene wl_shm")?,
        qh,
        pool: None,
        layer: None,
        configured: false,
        frame_pending: false,
        scale: 1,
        look: Look::load("brasa"),
        painter: Painter::new(),
        osd: None,
    };
    loop {
        event_loop.dispatch(Duration::from_millis(100), &mut app).map_err(|e| e.to_string())?;
        if app.osd.as_ref().is_some_and(|o| Instant::now() >= o.until) {
            app.hide();
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, factor: i32) {
        if factor != self.scale {
            self.scale = factor;
            self.pool = None;
            if self.configured {
                self.draw();
            }
        }
    }
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.frame_pending = false;
        self.draw();
    }
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.hide();
    }
    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface, _: LayerSurfaceConfigure, _: u32) {
        self.configured = true;
        if !self.frame_pending {
            self.draw();
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_registry!(App);

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}

smithay_client_toolkit::delegate_dispatch2!(App);
