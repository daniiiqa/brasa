# brasa

El OSD de discada: al subir o bajar el volumen, el micrófono, el brillo o la
luz del teclado sale una tarjeta con el look del rice (como los avisos de
[arre](https://github.com/daniiiqa/arre)): ícono, barra con los glifos del
look y porcentaje. La barra se desliza al valor nuevo; la entrada y la salida
son la animación de layers del look en Hyprland.

```
brasa volume|mic|brightness|kbd <nivel|muted> [título]
brasa daemon      el que dibuja (lo arranca solo el primer aviso)
```

El nivel va de 0 a 150 (el volumen con boost sale en rojo). Dónde sale:
`~/.config/brasa/config.json` → `"position"` como en arre (`bottom-center` si
no dice). No agarra clics.

```
cargo install --git https://github.com/daniiiqa/brasa --root ~/.local --locked
```

MIT.
