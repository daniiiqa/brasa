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
