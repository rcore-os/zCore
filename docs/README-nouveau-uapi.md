# uAPI compatible con nouveau en la GPU NVIDIA — estado

Este documento mapea, ioctl por ioctl, la superficie de `nouveau.ko`
(`include/uapi/drm/nouveau_drm.h` del kernel de Linux) contra lo que
implementa `NvidiaGpu::ioctl` en Eclipse OS. Es el mismo ejercicio que
[`README-drm.md`](README-drm.md) ya hace para la UAPI genérica de DRM/KMS,
aplicado a la superficie *driver-specific* de nouveau.

## Por qué existe esto

Mesa's OpenGL/Vulkan acelerado para NVIDIA (`nouveau_dri.so`, NVK) es
espacio de usuario Linux normal, ya compilado para x86_64 por Alpine, y ya
corre sin modificar bajo la capa de syscalls de Eclipse — igual que Xorg,
labwc o busybox. Lo que le falta es que el **lado de kernel** hable el
mismo protocolo de ioctls que `nouveau.ko`: un contrato público y estable,
a diferencia del protocolo privado y sin documentar entre `nvidia.ko` y el
driver cerrado de NVIDIA. Ver la discusión completa en el hilo que originó
este trabajo.

## Alcance de este hito — leer antes de extender

Esto se escribió en una sandbox **sin GPU NVIDIA, sin `/dev/kvm` y sin QEMU
instalado**. Nada de lo de abajo se ha ejecutado contra hardware real.
Para que eso siga siendo honesto, cada operación es una de estas tres
categorías:

- **(a) Reusa un punto de entrada ya ejercitado en hardware real, tal
  cual** — `nvidia_rm_sys::rm_init::step16`/`step17`, las mismas llamadas
  que hacen `/proc/gpustep16`/`gpustep17`.
- **(b) Es contabilidad pura, sin acceso a hardware/registros** — la tabla
  de handles GEM, el allocador de bitmap de VRAM (`NvidiaVramAllocator`,
  que ya existía como código muerto — nunca se llamaba).
- **(c) Se rechaza explícitamente con `EOPNOTSUPP`/`ENOSYS` y una línea de
  log — nunca se simula un éxito falso.**

**Cómo activarlo**: opt-in estricto, igual que `drm.atomic`. Arranca con
`nvidia.nouveau_uapi` en la `cmdline` (`zCore/rboot.conf`):

```ini
cmdline=LOG=warn:nvidia.nouveau_uapi:ROOTPROC=/bin/busybox?sh
```

Sin el flag, `NvidiaGpu::ioctl` se comporta exactamente igual que antes
(`ENOSYS` para cualquier ioctl no reconocido) — cero cambio de
comportamiento por defecto.

## Cobertura de ioctls

Leyenda: ✅ implementado (real, sin hardware nuevo sin probar) · 🟡 parcial ·
❌ rechazado explícitamente (`EOPNOTSUPP`/`ENOSYS`, nunca simulado).

| ioctl | Estado | Notas |
|---|---|---|
| `DRM_IOCTL_NOUVEAU_GETPARAM` | ✅ | `PCI_VENDOR`/`PCI_DEVICE`/`FB_SIZE`/`VRAM_BAR_SIZE` reales; `CHIPSET_ID` es el mínimo del rango `PMC_BOOT0` de la arquitectura ya identificada por PCI ID (aproximado — no relee `PMC_BOOT0` en vivo, ver más abajo); `VRAM_USED` siempre 0 (el allocador no lleva contador) |
| `DRM_IOCTL_NOUVEAU_SETPARAM` | ❌ | deprecated incluso en Linux; no implementado |
| `DRM_IOCTL_NOUVEAU_CHANNEL_ALLOC` | 🟡 | Reusa la escalera `step16`+`step17` ya existente. **Un solo canal en todo el sistema** — un segundo `CHANNEL_ALLOC` sin `CHANNEL_FREE` antes devuelve `EBUSY`. Los campos legacy (`fb_ctxdma_handle`, `subchan[]`) se ignoran: no aplican a Turing+ |
| `DRM_IOCTL_NOUVEAU_CHANNEL_FREE` | 🟡 | Limpia solo la contabilidad de Eclipse — `nvidia-rm-sys` no tiene un punto de desmontaje real para la escalera `step16`/`step17` (su propio doc la llama "idempotente", pensada para construirse una vez por arranque). Un `CHANNEL_ALLOC` posterior reutiliza la misma asignación cacheada, no crea una nueva |
| `DRM_IOCTL_NOUVEAU_NVIF` | ❌ | no implementado |
| `DRM_IOCTL_NOUVEAU_SVM_INIT` / `SVM_BIND` | ❌ | memoria unificada CPU/GPU — fuera de alcance de este hito |
| `DRM_IOCTL_NOUVEAU_VM_INIT` | 🟡 | Exige un canal ya asignado (`CHANNEL_ALLOC` primero, igual que en Linux real). Devuelve un rango `kernel_managed` vacío (0/0) — **placeholder honesto**: no hay un `VM_BIND` real todavía que necesite reservar nada |
| `DRM_IOCTL_NOUVEAU_VM_BIND` | ❌ | Necesita un path general de *binding* de VA de GPU en `nvidia-rm-sys` que hoy no existe. Devuelve `EOPNOTSUPP` con log claro, nunca simulado |
| `DRM_IOCTL_NOUVEAU_EXEC` | ❌ | `step18`/`step19` (los únicos puntos de envío de comandos que existen) cada uno somete **un kernel fijo, escrito a mano** — no un *pushbuffer* arbitrario construido por Mesa. Generalizar eso es trabajo real de seguimiento, no algo para fingir aquí. Devuelve `EOPNOTSUPP` |
| `DRM_IOCTL_NOUVEAU_GET_ZCULL_INFO` | ❌ | no implementado |
| `DRM_IOCTL_NOUVEAU_GEM_NEW` | 🟡 | Solo `NOUVEAU_GEM_DOMAIN_VRAM` (memoria de sistema/GART: `EOPNOTSUPP`). Reserva real vía `NvidiaVramAllocator::alloc` (antes código muerto). `offset` (VA de GPU) siempre 0: sin `VM_BIND` no hay *binding* que reportar. `map_handle` siempre 0: el `mmap()` de CPU de estos objetos necesitaría un registro cruzado en la tabla de handles de `linux-object`, que este driver (una capa más abajo) no puede alcanzar — ver "Huecos conocidos" |
| `DRM_IOCTL_NOUVEAU_GEM_PUSHBUF` | ❌ | ruta legacy pre-`VM_BIND`, no aplica al modelo que se está siguiendo aquí |
| `DRM_IOCTL_NOUVEAU_GEM_CPU_PREP` / `CPU_FINI` | ✅ | No-op real (no simulado): valida que el handle existe y ya está. Como `EXEC` no existe todavía, nada somete trabajo de GPU que toque un buffer `GEM_NEW` — no hay nada que esperar de verdad |
| `DRM_IOCTL_NOUVEAU_GEM_INFO` | 🟡 | Mismas limitaciones que `GEM_NEW` (`offset`/`map_handle` en 0) |

## Huecos conocidos y qué se necesita para cerrarlos

- **`VM_BIND`/`EXEC` genéricos**: `nvidia-rm-sys` necesita una función nueva
  que generalice el patrón de `step17` (mapear un buffer en `hVas`) y
  `step18`/`step19` (construir un *pushbuffer* y tocar el timbre) para
  aceptar contenido arbitrario en vez de un caso fijo. Es la pieza de
  mayor riesgo — necesita probarse contra hardware real, iterativamente,
  exactamente como se construyeron `step16`-`step19`.
- **`mmap()` de objetos `GEM_NEW`**: necesita un camino para registrar un
  buffer asignado dentro de `drivers` en la tabla de handles que
  `linux-object/src/fs/devfs/drm_scheme.rs::DrmDev::get_vmo` consulta —
  hoy esa tabla solo conoce los buffers que crea el `CREATE_DUMB` genérico.
  `drivers` es una capa más abajo que `linux-object`, así que esto es un
  cambio de API entre capas, no un one-liner.
  - Nota lateral, no bloqueante: `NvidiaVramAllocator::new` calcula
    `base_phys` a partir del puntero de framebuffer *mapeado* del driver
    (`fb_vaddr`), no de una dirección física re-derivada de forma
    independiente. Es preexistente (no se tocó su lógica, solo se activó
    una función que antes era código muerto) — vale la pena confirmar en
    hardware real que las direcciones que devuelve `GEM_NEW` caen dentro
    de la apertura VRAM real antes de confiar en ellas para nada más.
- **`CHIPSET_ID` real**: hoy es el mínimo del rango `PMC_BOOT0` de la
  arquitectura ya identificada por PCI ID, no una lectura en vivo de
  `PMC_BOOT0`. Deliberado: cualquier lectura de registro nueva en el
  camino de un ioctl que Mesa puede llamar en cualquier momento del boot
  necesita la misma cautela que ya se documenta en `DrmScheme::debug_dump`
  ("un BAR0 access temprano puede colgar algunas GPUs"). Si hace falta el
  valor exacto, debe leerse solo cuando ya se sepa que es seguro (GPU ya
  atacada por el RM), no incondicionalmente desde `GETPARAM`.
- **Más de un canal**: `CHANNEL_ALLOC` depende de `step16`/`step17`, que
  son funciones sin parámetros que construyen siempre la misma escalera
  fija — no aceptan "otro cliente, otro TSG". Soportar un segundo canal
  real requiere generalizar esas funciones en `nvidia-rm-sys`, igual que
  `VM_BIND`/`EXEC`.

## Qué probar primero en hardware real

Con `nvidia.nouveau_uapi` activo y la GPU ya atacada al RM (`/proc/gpustep5`
`;6;8;9`, o `gpustep14` en la GPU de consola):

1. `GETPARAM` con `PCI_VENDOR`/`PCI_DEVICE`/`FB_SIZE` — sin acceso a
   hardware, debería funcionar siempre; si falla, el bug está en el
   despacho de ioctls, no en RM.
2. `CHANNEL_ALLOC` — reusa `step16`+`step17`; si esos dos ya funcionan por
   `/proc/gpustepN`, esto solo debería envolver el mismo resultado en la
   forma de nouveau. Confirmar que `hVas`/`hNotifier` en el log coinciden
   con lo que ya reportan esos endpoints.
3. `GEM_NEW` con `DOMAIN_VRAM` y un tamaño pequeño (p. ej. 4096) — pura
   contabilidad de bitmap; confirmar con `/proc/gpudbg` (u otra lectura
   independiente de VRAM) que la dirección física devuelta realmente cae
   dentro de la apertura BAR1 de esta GPU (ver la nota sobre
   `NvidiaVramAllocator` arriba).
4. `VM_INIT` después de `CHANNEL_ALLOC` — debería aceptar y devolver
   0/0; antes de `CHANNEL_ALLOC` debería fallar con `EINVAL`.

`VM_BIND` y `EXEC` deberían fallar siempre con `EOPNOTSUPP` en este hito —
si un cliente real de Mesa llega tan lejos, es la señal de que vale la pena
invertir en generalizar `step17`/`step18`/`step19`.

## Mapa de archivos

| Archivo | Rol |
|---|---|
| `drivers/src/display/nouveau_uapi.rs` | números de ioctl, structs (layout C exacto de `nouveau_drm.h`), flag opt-in |
| `drivers/src/display/nvidia.rs` (`NvidiaGpu::ioctl` → `nouveau_ioctl`) | despacho real; reusa `nvidia_rm_sys::rm_init::step16`/`step17` y `NvidiaVramAllocator` |
| `kernel-hal/src/drivers.rs` (`set_nouveau_uapi_enabled`) | puente para que `zCore` active el flag sin depender directamente de `zcore-drivers` |
| `zCore/src/main.rs` | lee `nvidia.nouveau_uapi` de la cmdline |
