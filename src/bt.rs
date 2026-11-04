//! El volumen que llega por bluetooth: cuando el teléfono (u otro aparato)
//! manda su audio a la laptop, sus botones de volumen cambian el volumen de
//! *su* stream (bluez_input…), no el de la bocina: Volume.sh no se entera y
//! no salía el aviso. Aquí se escucha a PipeWire (`pactl subscribe`, como
//! vato y empalme) y al cambiar uno de esos streams sale el OSD con su nombre.

use crate::{Kind, Show};
use serde_json::Value;
use smithay_client_toolkit::reexports::calloop::channel::Sender;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Los streams de bluetooth: índice → (nombre, nivel o None si silenciado).
fn streams() -> HashMap<u64, (String, Option<u32>)> {
    let out = Command::new("pactl").args(["-f", "json", "list", "sink-inputs"]).stderr(Stdio::null()).output();
    let list: Value = out.ok().and_then(|o| serde_json::from_slice(&o.stdout).ok()).unwrap_or(Value::Null);
    let mut map = HashMap::new();
    for s in list.as_array().into_iter().flatten() {
        let props = &s["properties"];
        if !props["node.name"].as_str().is_some_and(|n| n.starts_with("bluez_input")) {
            continue;
        }
        let Some(i) = s["index"].as_u64() else { continue };
        // el nivel: el más alto de los canales ("98%")
        let level = s["volume"]
            .as_object()
            .into_iter()
            .flat_map(|o| o.values())
            .filter_map(|c| c["value_percent"].as_str()?.trim_end_matches('%').parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        let muted = s["mute"].as_bool().unwrap_or(false);
        // "Daniel's S21 (codec aptX)" → "Daniel's S21"
        let name = props["media.name"].as_str().or(props["application.name"].as_str()).unwrap_or("Bluetooth");
        let name = name.split(" (codec").next().unwrap_or(name).trim().to_string();
        map.insert(i, (name, (!muted && level > 0).then_some(level)));
    }
    map
}

/// Escuchar para siempre (si PipeWire se reinicia, `pactl subscribe` muere:
/// volver a empezar). Lo que ya estaba al conectar no avisa; solo los cambios.
pub fn watch(tx: Sender<Show>) {
    std::thread::spawn(move || {
        loop {
            let mut last = streams();
            let child = Command::new("pactl").arg("subscribe").stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
            if let Ok(mut child) = child {
                if let Some(out) = child.stdout.take() {
                    for line in BufReader::new(out).lines().map_while(Result::ok) {
                        if !line.contains("sink-input") {
                            continue;
                        }
                        let now = streams();
                        for (i, (name, level)) in &now {
                            // uno nuevo (recién conectado) no avisa: solo si cambió
                            if last.get(i).is_some_and(|(_, before)| before != level)
                                && tx.send(Show { kind: Kind::Volume, level: *level, title: name.clone() }).is_err()
                            {
                                let _ = child.kill();
                                return;
                            }
                        }
                        last = now;
                    }
                }
                let _ = child.kill();
                let _ = child.wait();
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

/// El nombre del aparato de una ruta de BlueZ (…/dev_8C_DE_…/fd0 → "Daniel's S21").
fn alias(path: &str) -> String {
    let dev = path.rsplit_once('/').map_or(path, |(d, _)| d);
    Command::new("busctl")
        .args(["--system", "get-property", "org.bluez", dev, "org.bluez.Device1", "Alias"])
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().strip_prefix("s ").map(|n| n.trim_matches('"').replace("\\'", "'")))
        .unwrap_or_else(|| "Bluetooth".into())
}

/// El volumen absoluto (AVRCP): los botones del teléfono no tocan el stream
/// de PipeWire, le avisan a BlueZ (MediaTransport1.Volume, de 0 a 127). Se
/// escucha con dbus-monitor; cada cambio es un aviso.
pub fn watch_avrcp(tx: Sender<Show>) {
    std::thread::spawn(move || {
        loop {
            let child = Command::new("dbus-monitor")
                .args(["--system", "type='signal',sender='org.bluez',interface='org.freedesktop.DBus.Properties',member='PropertiesChanged'"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            if let Ok(mut child) = child {
                if let Some(out) = child.stdout.take() {
                    // la ruta de la señal y si lo que sigue es "Volume"
                    let (mut path, mut volume_next) = (String::new(), false);
                    for line in BufReader::new(out).lines().map_while(Result::ok) {
                        let l = line.trim();
                        if l.starts_with("signal ") {
                            path = l.split("path=").nth(1).and_then(|p| p.split(';').next()).unwrap_or("").to_string();
                            volume_next = false;
                        } else if l == "string \"Volume\"" {
                            volume_next = true;
                        } else if volume_next && let Some(v) = l.strip_prefix("variant").map(str::trim).and_then(|v| v.strip_prefix("uint16 ")) {
                            volume_next = false;
                            let Ok(v) = v.trim().parse::<u32>() else { continue };
                            let level = (v * 100 + 63) / 127;
                            let show = Show { kind: Kind::Volume, level: (level > 0).then_some(level), title: alias(&path) };
                            if tx.send(show).is_err() {
                                let _ = child.kill();
                                return;
                            }
                        }
                    }
                }
                let _ = child.kill();
                let _ = child.wait();
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}
