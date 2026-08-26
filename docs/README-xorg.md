# Ejecutar un servidor X (`startx`) en Eclipse OS

Eclipse OS (zCore) expone la capa de consola/terminal virtual que un servidor X
de Linux necesita para tomar el control de la pantalla. Este documento explica
qué soporta el núcleo y cómo configurar el espacio de usuario (p. ej. Alpine)
para que `startx` funcione.

## Qué proporciona el núcleo

El núcleo implementa los dispositivos e `ioctl`s que `Xorg` usa en su rutina
`xf86OpenConsole`:

- **Nodos de consola**: `/dev/tty`, `/dev/tty0` (VT activo), `/dev/console` y
  `/dev/tty1`..`/dev/tty6`.
- **`ioctl`s de VT** (`<linux/vt.h>`): `VT_OPENQRY`, `VT_GETMODE`, `VT_SETMODE`,
  `VT_GETSTATE`, `VT_ACTIVATE`, `VT_WAITACTIVE`, `VT_RELDISP`, `VT_DISALLOCATE`.
- **`ioctl`s de KD** (`<linux/kd.h>`): `KDGETMODE`/`KDSETMODE`
  (`KD_TEXT`/`KD_GRAPHICS`, por VT), `KDGKBMODE`/`KDSKBMODE` (modo de teclado) y
  `KDGKBTYPE`. Cuando X pone el teclado en `K_OFF`/`K_RAW`, el núcleo deja de
  inyectar caracteres "cocidos" en ese TTY; los eventos crudos siguen llegando
  por `/dev/input/event*`.
- **Framebuffer**: `/dev/fb0` con `FBIOGET_VSCREENINFO`/`FBIOGET_FSCREENINFO` y,
  como la resolución es fija, acepta `FBIOPUT_VSCREENINFO`, `FBIOPAN_DISPLAY` y
  `FBIOBLANK` para que el driver `fbdev` de X arranque.
- **Entrada**: `/dev/input/event*` y `/dev/input/mice` (PS/2 y virtio).
- **DRM**: `/dev/dri/card0` (parcial).
- **Cambio de VT**: Ctrl+Alt+F1..F6. El modo KD es por-VT, así que al salir del
  VT gráfico de X se sigue viendo una consola de texto normal.

## Configuración de userspace recomendada

El núcleo no incluye `udev` ni un KMS/DRM completo, así que conviene forzar el
driver **`fbdev`** y declarar la entrada **`evdev`** de forma estática en lugar
de depender del autodescubrimiento de `libinput`/`udev`.

Instala los paquetes (en Alpine):

```sh
apk add xorg-server xf86-video-fbdev xf86-input-evdev xinit mesa-dri-gallium
```

Crea `/etc/X11/xorg.conf.d/10-eclipse.conf` con:

```
Section "ServerFlags"
    Option "AutoAddDevices" "false"   # no hay udev: añadimos la entrada a mano
    Option "DontZap"        "false"
EndSection

Section "Device"
    Identifier "fb"
    Driver     "fbdev"
    Option     "fbdev" "/dev/fb0"
EndSection

Section "Screen"
    Identifier "screen"
    Device     "fb"
EndSection

Section "InputDevice"
    Identifier "keyboard"
    Driver     "evdev"
    Option     "Device" "/dev/input/event0"
    Option     "CoreKeyboard"
EndSection

Section "InputDevice"
    Identifier "mouse"
    Driver     "evdev"
    Option     "Device" "/dev/input/mice"
    Option     "CorePointer"
EndSection

Section "ServerLayout"
    Identifier  "layout"
    Screen      "screen"
    InputDevice "keyboard"
    InputDevice "mouse"
EndSection
```

Ajusta los `event0`/`mice` a los nodos reales que aparezcan en `/dev/input/`.

## Probar

```sh
startx
```

Si quieres ver el registro del servidor para diagnosticar:

```sh
Xorg -verbose 6 :0 vt1 2> /tmp/Xorg.log ; cat /tmp/Xorg.log
```

## Diagnóstico: `startx` no arranca y *no aparece ningún log de X*

Si `startx` termina al instante y no se genera `/tmp/Xorg.log` ni
`/var/log/Xorg.0.log`, el servidor X probablemente **muere antes de llegar a
`main()`**, dentro del cargador dinámico de musl (le falta una biblioteca
compartida o un símbolo). `Xorg` enlaza con muchas más `.so` que una aplicación
de consola, así que basta con que falte una para que aborte sin escribir nada en
su propio log.

El núcleo registra ahora en `dmesg` la cadena de `exec` y los errores del
cargador dinámico. Tras intentar `startx`, mira el log del kernel:

```sh
dmesg | grep -E 'EXECVE|XLOG'
```

- Las líneas `EXECVE[pid] "/ruta" argv=[...]` muestran exactamente qué binarios
  se ejecutan. Si **no** aparece ningún `EXECVE` con `Xorg`/`X`/`Xorg.wrap`, el
  problema está en `xinit`/`startx` (no encuentra el servidor): revisa
  `~/.xinitrc`, `$PATH` y que `/usr/bin/X` apunte al servidor.
- Las líneas `XLOG: Error loading shared library ...` o
  `XLOG: Error relocating ...: symbol not found` nombran la biblioteca o el
  símbolo que falta. Instala el paquete que la aporta.

También puedes ejecutar el servidor a mano para ver el error del enlazador en el
acto (musl lo escribe en `stderr`):

```sh
Xorg -version            # si imprime versión, el enlace dinámico está bien
LD_TRACE_LOADED_OBJECTS=1 Xorg   # lista las .so que necesita y cuáles faltan
```

Si `Xorg -version` falla con `Error loading shared library libfoo.so.N`,
instala el paquete correspondiente (`apk add ...`) y reintenta. Cuando
`Xorg -version` imprime la versión, el problema ya no es el enlazado y conviene
mirar `/tmp/Xorg.log` (sección anterior) para el siguiente fallo.

## Notas y limitaciones

- `VT_OPENQRY` devuelve el VT activo, de modo que X se apropia del terminal
  desde el que se lanzó `startx` (y al salir vuelve a él).
- El reenganche por señales `VT_PROCESS` (relsig/acqsig) se acepta pero no se
  emiten señales: cambiar de VT mientras X corre y volver puede requerir que la
  aplicación repinte.
- La aceleración por GPU no está disponible; usa el renderizado por software de
  Mesa (`llvmpipe`/`softpipe`), por eso se instalan `mesa-dri-gallium` y
  `llvm-libs`.

## Pseudo-terminales (PTY)

Los emuladores de terminal bajo X (xterm, st, rxvt…) necesitan pseudo-terminales
para arrancar una shell. Eclipse OS las expone igual que Linux:

- `/dev/ptmx`: abrir este nodo crea un par maestro/esclavo nuevo y devuelve el
  *maestro*. `ptsname(3)` resuelve el número con `ioctl(TIOCGPTN)` y `unlockpt(3)`
  usa `ioctl(TIOCSPTLCK)` (aceptado; no se exige para abrir el esclavo).
- `/dev/pts/N`: el *esclavo*. El programa que corre en él (la shell) lo ve como
  un terminal real: `tcgetattr`/`tcsetattr`, `TIOCGWINSZ`/`TIOCSWINSZ`,
  grupos de proceso en primer plano (`TIOCGPGRP`/`TIOCSPGRP`) y señales
  (`Ctrl+C`→SIGINT, `SIGWINCH` al redimensionar, `SIGHUP` al cerrar el maestro).

La disciplina de línea (modo canónico/raw, eco, traducción CR/NL) corre en el
lado del esclavo: lo que escribe el emulador en el maestro se procesa y, con eco
activo, vuelve al maestro para que la terminal muestre lo tecleado.
