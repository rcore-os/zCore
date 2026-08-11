# DRM / KMS en Eclipse OS — conformidad con la UAPI de Linux

Este documento mapea la implementación de DRM (Direct Rendering Manager) de
Eclipse OS contra la documentación del kernel de Linux
([`Documentation/gpu`](https://github.com/torvalds/linux/tree/master/Documentation/gpu)),
y registra qué está implementado, qué es parcial y qué falta.

## Alcance: qué significa "ser compatible"

`Documentation/gpu` tiene dos clases de contenido muy distintas:

- **Contrato con el espacio de usuario (UAPI)** — `drm-uapi.rst`,
  `drm-usage-stats.rst`, `driver-uapi.rst`. Es lo que de verdad determina si el
  software gráfico de Linux (libdrm, Mesa, wlroots, Xorg…) funciona. **Esto es
  lo que Eclipse OS implementa.**
- **Internals del kernel de Linux** — `drm-internals.rst`, `drm-mm.rst`,
  `drm-kms-helpers.rst`, `drm-ras.rst`, etc. Describen estructuras y *helpers*
  internos del DRM de Linux (TTM, `drm_device`, midlayers de KMS…). No son un
  contrato observable desde userspace: una reimplementación desde cero **no
  necesita reproducirlos**, solo ofrecer la misma UAPI por encima.

Eclipse OS no es Linux: no hay *midlayer* DRM ni drivers de GPU completos. En su
lugar implementa la UAPI directamente sobre una ruta **"software KMS"**: cuando
hay un framebuffer (UEFI GOP, virtio-gpu, …) se sintetizan los objetos KMS
mínimos (1 CRTC + 1 connector + 1 encoder + 1 plane primario) y el *scanout* se
hace copiando el *dumb buffer* del cliente al framebuffer (`blit_from`). Esto es
suficiente para compositores Wayland por software (wlroots/labwc con pixman) y
para Xorg con el driver `fbdev`.

## Nodos de dispositivo

| Nodo | major:minor | Estado |
|---|---|---|
| `/dev/dri/card0` | 226:0 | ✅ nodo primario (KMS + dumb buffers) |
| `/dev/dri/renderD128` | 226:128 | ✅ nodo de render (solo ioctls `DRM_RENDER_ALLOW`) |
| `/dev/fb0` | 29:0 | ✅ framebuffer legacy (`fbdev`) |
| `/sys/class/drm/card0` | — | ✅ entradas mínimas en sysfs |

### Render nodes (`drm-uapi.rst` → *Render nodes*)

Igual que en Linux, `renderD128` **solo** acepta los ioctls marcados
`DRM_RENDER_ALLOW` en `drm_ioctl.c` (`VERSION`, `GET_CAP`, `GEM_CLOSE`,
`PRIME_*`, `SYNCOBJ_*`, `SET_CLIENT_NAME` y el rango de comandos del driver);
cualquier ioctl de modeset, *dumb buffers* o master/auth devuelve **EACCES**.
Así, un cliente que sondee el render node ve un nodo de render de verdad, no un
segundo dispositivo KMS.

## Cobertura de la UAPI de DRM (`drm-uapi.rst`)

Leyenda: ✅ implementado · 🟡 parcial / no-op deliberado · ❌ no implementado.

### Genéricos y autenticación

| ioctl | Estado | Notas |
|---|---|---|
| `DRM_IOCTL_VERSION` | ✅ | nombre `zcore`, versión 1.0.0 |
| `DRM_IOCTL_GET_UNIQUE` | 🟡 | `zcore-gpu` (no es un *busid* parseable `pci:…`) |
| `DRM_IOCTL_SET_VERSION` | ✅ | interfaz 1.4, driver 1.0; valida majors como `drm_setversion` (lo usa Xorg/modesetting) |
| `DRM_IOCTL_GET_MAGIC` / `AUTH_MAGIC` | 🟡 | cliente único = master implícito |
| `DRM_IOCTL_SET_MASTER` / `DROP_MASTER` | ✅ | conmuta la consola de texto del kernel (KD_GRAPHICS/KD_TEXT) |
| `DRM_IOCTL_GET_CAP` | ✅ | ver tabla de *caps* |
| `DRM_IOCTL_SET_CLIENT_CAP` | ✅ | `ATOMIC` según `drm.atomic` (ver sección atómico); `WRITEBACK` solo para clientes atómicos (EINVAL como Linux); resto aceptado |
| `DRM_IOCTL_WAIT_VBLANK` | ✅ | vblank sintético ~60 Hz; modo evento encola `DRM_EVENT_VBLANK` |

### GEM / *dumb buffers* / PRIME

| ioctl | Estado | Notas |
|---|---|---|
| `DRM_IOCTL_MODE_CREATE_DUMB` | ✅ | memoria física contigua vía VMO; *pitch* alineado a 64 B |
| `DRM_IOCTL_MODE_MAP_DUMB` | ✅ | *offset* = `handle << 12`; `mmap` mapea el VMO físico |
| `DRM_IOCTL_MODE_DESTROY_DUMB` | ✅ | |
| `DRM_IOCTL_GEM_CLOSE` | ✅ | |
| `DRM_IOCTL_PRIME_HANDLE_TO_FD` / `FD_TO_HANDLE` | ✅ | dma-buf real (fd de proceso); despachado en la capa de syscalls porque necesita la tabla de fds |
| `DRM_IOCTL_GEM_FLINK` / `GEM_OPEN` | ❌ | interfaz legacy insegura; los clientes nuevos deben usar PRIME (igual que recomienda `drm-uapi.rst`) |

### Framebuffers

| ioctl | Estado | Notas |
|---|---|---|
| `DRM_IOCTL_MODE_ADDFB` | ✅ | |
| `DRM_IOCTL_MODE_ADDFB2` | ✅ | usa `handles[0]`/`pitches[0]` |
| `DRM_IOCTL_MODE_RMFB` | ✅ | |
| `DRM_IOCTL_MODE_CLOSEFB` | ✅ | Linux 6.6+: suelta la referencia sin apagar el plano |
| `DRM_IOCTL_MODE_GETFB` | ✅ | devuelve geometría + handle (cliente master único) |
| `DRM_IOCTL_MODE_GETFB2` | ✅ | formato `XR24`, plano 0 |
| `DRM_IOCTL_MODE_DIRTYFB` | ✅ | blitea solo la unión de los `drm_clip_rect` recibidos (sin *clips*, o si excede el límite defensivo, re-escanea todo) |

### KMS (modeset legacy)

| ioctl | Estado | Notas |
|---|---|---|
| `DRM_IOCTL_MODE_GETRESOURCES` | ✅ | 1 CRTC + 1 connector + 1 encoder sintéticos |
| `DRM_IOCTL_MODE_GETCRTC` / `SETCRTC` | ✅ | `SETCRTC` con fb hace *scanout*; `GETCRTC` devuelve el modo actual (`mode_valid=1`) |
| `DRM_IOCTL_MODE_GETENCODER` | ✅ | encoder `VIRTUAL`, `possible_crtcs=1` |
| `DRM_IOCTL_MODE_GETCONNECTOR` | ✅ | 1 modo = resolución nativa (preferido); propiedades estándar (ver abajo) |
| `DRM_IOCTL_MODE_GETPLANERESOURCES` | ✅ | 1 plano primario |
| `DRM_IOCTL_MODE_GETPLANE` | ✅ | formatos `XR24`/`AR24` |
| `DRM_IOCTL_MODE_SETPLANE` | ✅ | equivale a *scanout* del fb (ruta primaria SW) |
| `DRM_IOCTL_MODE_PAGE_FLIP` | ✅ | *scanout* + `DRM_EVENT_FLIP_COMPLETE` con `crtc_id` |
| `DRM_IOCTL_MODE_OBJ_GETPROPERTIES` | ✅ | tabla de propiedades por objeto; las propiedades `DRM_MODE_PROP_ATOMIC` solo se muestran a clientes atómicos (mismo filtrado que Linux) |
| `DRM_IOCTL_MODE_GETPROPERTY` | ✅ | metadatos completos: flags, nombre, rangos/enums/tipo de objeto |
| `DRM_IOCTL_MODE_OBJ_SETPROPERTY` / `SETPROPERTY` | 🟡 | aceptado como no-op (DPMS…; sin estado programable) |
| `DRM_IOCTL_MODE_GETPROPBLOB` | ✅ | EDID + blobs del almacén de propiedades |
| `DRM_IOCTL_MODE_CREATEPROPBLOB` / `DESTROYPROPBLOB` | ✅ | almacén de blobs; destruir un blob del kernel da EACCES (Linux: EPERM) |
| `DRM_IOCTL_MODE_CURSOR` / `CURSOR2` | ✅ | cursor compuesto por el kernel sobre cada frame (`set_cursor_bo`/`move_cursor`) |
| `DRM_IOCTL_MODE_ATOMIC` | ✅ | **opt-in** con `drm.atomic` (ver sección siguiente) |
| `DRM_IOCTL_MODE_GETGAMMA` / `SETGAMMA` | ❌ | sin LUT (`gamma_size=0`) |
| `DRM_IOCTL_MODE_CREATE_LEASE` … | ❌ | *leases* no soportados |
| `DRM_IOCTL_SYNCOBJ_*` | ❌ | `DRM_CAP_SYNCOBJ=0` (honesto); solo relevante con aceleración |

### Propiedades KMS estándar (`drm-kms.rst` → *KMS Properties*)

| Objeto | Propiedad | Tipo | Notas |
|---|---|---|---|
| plane | `type` | enum inmutable | {Overlay, Primary, Cursor} |
| plane | `FB_ID`, `CRTC_ID` | object, atómica | |
| plane | `CRTC_X/Y/W/H`, `SRC_X/Y/W/H` | range, atómicas | `SRC_*` en 16.16 |
| CRTC | `ACTIVE` | range 0–1, atómica | |
| CRTC | `MODE_ID` | blob, atómica | blob de `drm_mode_modeinfo` (68 B) |
| connector | `DPMS` | enum | siempre "On"; escritura aceptada como no-op |
| connector | `link-status` | enum | siempre "Good" |
| connector | `non-desktop` | range inmutable | 0 |
| connector | `EDID` | blob inmutable | EDID real (UEFI/DDC) si existe |
| connector | `CRTC_ID` | object, atómica | |

### Capacidades (`DRM_IOCTL_GET_CAP`)

| Capacidad | Valor | Notas |
|---|---|---|
| `DRM_CAP_DUMB_BUFFER` | 1 | |
| `DRM_CAP_DUMB_PREFERRED_DEPTH` | 24 | scanout XRGB8888 |
| `DRM_CAP_DUMB_PREFER_SHADOW` | 1 | el *present* es un blit CPU sobre PCIe: renderizar a *shadow* y copiar es exactamente lo aconsejable |
| `DRM_CAP_PRIME` | 3 | IMPORT \| EXPORT (dma-buf real) |
| `DRM_CAP_TIMESTAMP_MONOTONIC` | 1 | |
| `DRM_CAP_ASYNC_PAGE_FLIP` | 0 | |
| `DRM_CAP_CURSOR_WIDTH` / `HEIGHT` | 64 | cursor compuesto por el kernel |
| `DRM_CAP_ADDFB2_MODIFIERS` | 1 | |
| `DRM_CAP_PAGE_FLIP_TARGET` | 0 | |
| `DRM_CAP_CRTC_IN_VBLANK_EVENT` | 1 | el evento de flip lleva `crtc_id` |
| `DRM_CAP_SYNCOBJ` / `SYNCOBJ_TIMELINE` | 0 | |
| `DRM_CAP_ATOMIC_ASYNC_PAGE_FLIP` | 0 | |

## KMS atómico (`drm-uapi.rst` → *Atomic Mode Setting*) — opt-in

La ruta atómica completa está implementada para el *pipeline* sintético
(software KMS): negociación con `DRM_CLIENT_CAP_ATOMIC`, propiedades atómicas
en los tres objetos, blobs de modo (`CREATEPROPBLOB`/`MODE_ID`) y
`DRM_IOCTL_MODE_ATOMIC` con la semántica de Linux:

- `TEST_ONLY` valida sin tocar estado (y `TEST_ONLY`+`PAGE_FLIP_EVENT` es
  EINVAL, como en Linux).
- `ALLOW_MODESET` es obligatorio para cambios de `MODE_ID`/`ACTIVE`
  ("requires full modeset").
- `PAGE_FLIP_EVENT` encola un `DRM_EVENT_FLIP_COMPLETE` por CRTC del commit.
- `PAGE_FLIP_ASYNC` se rechaza (las *caps* async son 0).
- Objeto/propiedad desconocidos → ENOENT; valores fuera de rango → EINVAL.
- El commit de un `FB_ID` hace el *scanout* (mismo blit que la ruta legacy).

**Cómo activarlo**: es estrictamente **opt-in** mientras la ruta legacy siga
siendo la probada en hardware real — el mismo despliegue que hizo nouveau con
`nouveau.atomic=1`. Arranca con el flag `drm.atomic` en la `cmdline` del kernel
(`zCore/rboot.conf`):

```ini
cmdline=LOG=warn:drm.atomic:ROOTPROC=/bin/busybox?sh
```

y quita `WLR_DRM_NO_ATOMIC=1` del entorno para que wlroots use la ruta
atómica. Sin el flag, `SET_CLIENT_CAP(ATOMIC)` devuelve **EOPNOTSUPP** (igual
que un driver Linux sin `DRIVER_ATOMIC`) y los compositores caen a la ruta
legacy exactamente como antes.

Limitaciones deliberadas: un solo modo (el nativo del panel), sin plano de
cursor atómico (el cursor legacy sigue compuesto por el kernel), sin
`IN_FENCE_FD`/`OUT_FENCE_PTR` (sin *fences* explícitas) y solo sobre la ruta
software-KMS (con un driver KMS por hardware la negociación atómica se
rechaza).

## Huecos conocidos y justificación

- **`GET_UNIQUE` no devuelve un busid `pci:…`**. libdrm moderno deriva el bus
  del sysfs (`/sys/class/drm/card0/device`), que sí está; solo afectaría a
  clientes legacy que parseen el *busid*.
- **Gamma/CTM/HDR**: sin LUTs (`gamma_size=0`, sin `GAMMA_LUT`/`CTM`); el
  *scanout* software no las aplica.
- **Leases y syncobj**: sin soporte; las *caps* correspondientes se anuncian
  como 0 para que ningún cliente tome esa ruta.
- **`drm-usage-stats.rst` (fdinfo)**. No se exponen estadísticas de
  uso/memoria/engine por `fdinfo`.
- **Render / 3D**. No hay aceleración: se usa el render por software de Mesa
  (`llvmpipe`/`softpipe`).

## Cómo lanzar labwc

El kernel implementa la ruta **legacy-KMS + dumb buffers + scanout por
software** (y, opt-in, la atómica). Para que wlroots/labwc usen la ruta por
defecto (y NO intenten GBM/EGL/GL, que no hay aceleración) hay que forzar el
renderer **pixman** y el KMS legacy por variables de entorno:

```sh
# Gestor de asientos (o arráncalo en /etc/init.d/rcS). Da acceso a
# /dev/dri/card0 y /dev/input/event* sin logind.
seatd -g video &

# Directorio de runtime para el socket de Wayland (modo 0700, del usuario).
export XDG_RUNTIME_DIR=/run/user/0
mkdir -p "$XDG_RUNTIME_DIR" && chmod 0700 "$XDG_RUNTIME_DIR"

# Render por software (sin GBM/EGL) y KMS legacy (no atómico, sin modifiers).
export WLR_RENDERER=pixman
export WLR_DRM_NO_ATOMIC=1     # innecesario sin drm.atomic; quítalo para probar la ruta atómica
export WLR_DRM_NO_MODIFIERS=1

labwc
```

> **Importante**: sin `WLR_RENDERER=pixman`, wlroots intenta primero el renderer
> GLES2 sobre GBM/EGL (Mesa). Como aquí no hay GPU, esa ruta puede **colgarse,
> fallar o funcionar extremadamente lenta**: Mesa cae a `llvmpipe` (render por
> CPU) y cada frame además se vuelve a copiar al framebuffer DRM por software.
> El síntoma típico del fallo de inicialización es que el log del kernel se queda
> en `[drm] VERSION …` y nunca llega al *scanout* (pantalla congelada). Si sí
> arranca con `WLR_RENDERER=gles2`, el compositor seguirá siendo muy lento por
> ese doble trabajo en CPU. Forzar pixman evita esa ruta por completo.

## Diagnóstico

Con `LOG=error`, el kernel registra el avance de la negociación DRM. Una sesión
sana (ruta legacy) imprime, en orden:

```
[drm] VERSION — /dev/dri/card0 opened by userspace (minor=0)
[drm] GET_CAP cap=0x1 -> 1
[drm] SET_CLIENT_CAP cap=2 -> accepted        # UNIVERSAL_PLANES
[drm] SET_CLIENT_CAP ATOMIC -> EOPNOTSUPP ... # sin drm.atomic (forzamos legacy)
[drm] SET_MASTER (minor=0)
[drm] GETRESOURCES: software KMS -> 1 crtc, 1 connector ...
[drm] GETCONNECTOR id=2 connected=true modes=1 ...
[drm] CREATE_DUMB 1920x1080 bpp=32 -> handle=1 ...
[drm] SETCRTC crtc=1 fb=1 ...
[drm] scanout: fb=1 ... -> display ...
```

Con `drm.atomic` y la ruta atómica activa se ve además:

```
[drm] SET_CLIENT_CAP ATOMIC=1 -> accepted
[drm] CREATEPROPBLOB len=68 -> blob=30000     # el MODE_ID del compositor
[drm] ATOMIC objs=3 test_only=true allow_modeset=true ...   # commit de prueba
[drm] ATOMIC objs=3 test_only=false ... fb=Some(1) ...      # modeset real
```

- Si se detiene en `VERSION` (o `minor=128`, el render node) y no aparece
  `GETRESOURCES`, es la inicialización del renderer GL: usa `WLR_RENDERER=pixman`.
- `[drm] render node refused ...` es normal: un cliente sondeó `renderD128`
  con un ioctl de modeset y recibió EACCES, como en Linux.
- Si llega a `SETCRTC`/`scanout` pero no se ve nada, el problema está en el
  *blit* al framebuffer (ver [`drm.rs`](../linux-object/src/fs/devfs/drm.rs)).
- `[drm] UNHANDLED ioctl …` indica un ioctl que labwc pide y aún no manejamos
  (el `drm nr` identifica el `DRM_IOCTL_*`).

La consola de texto del kernel cede a gráficos (`KD_GRAPHICS`) solo en el primer
*scanout* real, no al hacer `SET_MASTER`: así, si labwc se atasca antes de
pintar, el terminal sigue usable y sus logs visibles.

## Cómo probar (Xorg)

- **Xorg**: driver `fbdev` sobre `/dev/fb0`. Ver [`README-xorg.md`](README-xorg.md).

## `fbdev` (`/dev/fb0`, API legacy de framebuffer)

| ioctl | Estado |
|---|---|
| `FBIOGET_VSCREENINFO` / `FBIOGET_FSCREENINFO` | ✅ |
| `FBIOPUT_VSCREENINFO` | 🟡 (resolución fija; devuelve la real) |
| `FBIOPAN_DISPLAY` / `FBIOBLANK` | 🟡 (no-op) |
| `FBIOGETCMAP` / `FBIOPUTCMAP` | 🟡 (no-op; TrueColor) |

## Mapa de archivos

| Archivo | Rol |
|---|---|
| `linux-object/src/fs/devfs/drm_scheme.rs` | dispatch de ioctls de `/dev/dri/card*` (incl. atómico, blobs, filtro de render node) |
| `linux-object/src/fs/devfs/drm.rs` | núcleo DRM: GEM, framebuffers, *scanout*, eventos, KMS sintético, commit atómico |
| `linux-object/src/fs/devfs/fbdev.rs` | `/dev/fb0` (API `fbdev` legacy) |
| `linux-object/src/fs/dmabuf.rs` | dma-buf (PRIME) |
| `drivers/src/scheme/drm.rs` | trait `DrmScheme` para drivers (virtio-gpu, nvidia) |
| `drivers/src/virtio/gpu.rs` | driver virtio-gpu |
| `linux-object/src/fs/sysfs.rs` | `/sys/class/drm/card0` |
