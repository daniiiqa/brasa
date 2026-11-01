//! brasa: el OSD de discada. Al subir o bajar el volumen, el micrófono, el
//! brillo o la luz del teclado sale una tarjeta con el look del rice (como
//! los avisos de arre): ícono, barra con los glifos del look y porcentaje. La
//! barra se desliza al valor nuevo; la entrada y la salida son la animación
//! de layers del look (Hyprland), así que combina sola.
//!
//!   brasa <tipo> <nivel|muted> [título]
//!       tipo: volume | mic | brightness | kbd
//!       nivel: 0..150 (el volumen pasa de 100 con boost)
//!   brasa daemon     el que dibuja (lo arranca solo el primer aviso)
//!
//! Dónde sale: ~/.config/brasa/config.json → "position" como en arre
//! (bottom-center si no dice). No agarra clics.

mod card;
mod osd;

use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

fn socket() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from).join("brasa.sock")
}

/// Lo que pide un aviso: qué, a cuánto (None: silenciado) y con qué título.
pub struct Show {
    pub kind: Kind,
    pub level: Option<u32>,
    pub title: String,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Volume,
    Mic,
    Brightness,
    Kbd,
}

impl Kind {
    fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "volume" | "volumen" => Kind::Volume,
            "mic" => Kind::Mic,
            "brightness" | "brillo" => Kind::Brightness,
            "kbd" | "teclado" => Kind::Kbd,
            _ => return None,
        })
    }
    fn title(self) -> &'static str {
        match self {
            Kind::Volume => "Volume",
            Kind::Mic => "Mic",
            Kind::Brightness => "Brightness",
            Kind::Kbd => "Keyboard",
        }
    }
}

impl Show {
    /// "tipo\tnivel\ttítulo", lo que viaja por el socket.
    fn parse(msg: &str) -> Option<Show> {
        let mut it = msg.splitn(3, '\t');
        let kind = Kind::parse(it.next()?)?;
        let level = match it.next()? {
            "muted" => None,
            n => Some(n.trim_end_matches('%').parse().ok()?),
        };
        let title = it.next().filter(|t| !t.is_empty()).unwrap_or(kind.title()).to_string();
        Some(Show { kind, level, title })
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("daemon") => match osd::run() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("brasa: {e}");
                1
            }
        },
        Some(_) if args.len() >= 2 => send(&args),
        _ => {
            eprintln!("uso: brasa volume|mic|brightness|kbd <nivel|muted> [título]  |  brasa daemon");
            2
        }
    };
    std::process::exit(code);
}

/// Mandarle el aviso al daemon; si no está, arrancarlo y volver a mandar.
fn send(args: &[String]) -> i32 {
    let msg = args[..args.len().min(3)].join("\t");
    if Show::parse(&msg).is_none() {
        eprintln!("brasa: no entiendo '{}'", args.join(" "));
        return 2;
    }
    let Ok(sock) = UnixDatagram::unbound() else { return 1 };
    if sock.send_to(msg.as_bytes(), socket()).is_ok() {
        return 0;
    }
    let Ok(exe) = std::env::current_exe() else { return 1 };
    let mut cmd = Command::new(exe);
    cmd.arg("daemon").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    if cmd.spawn().is_err() {
        return 1;
    }
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(20));
        if sock.send_to(msg.as_bytes(), socket()).is_ok() {
            return 0;
        }
    }
    1
}
